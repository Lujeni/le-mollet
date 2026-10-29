//! What the server keeps across restarts, in one SQLite file (`le-mollet.db`, next to where it runs): riders,
//! events, clubs, leaderboards, accounts, roles, sign-in sessions. Nothing to install or configure, at home
//! as on a server. Each kind of record is a collection of JSON documents by id: a change writes its own row, in
//! a transaction, so a crash never loses the rest. The live state of a ride (where everyone is on the road)
//! stays in memory. JSON files from older versions are imported on the first start and kept as `.json.bak`.
//! ponytail: documents, not columns; split a collection into real columns when a query needs to look inside

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use std::path::Path;
use std::sync::Mutex;

pub struct Db(Mutex<Connection>);

/// A database call, timed and counted (src/metrics.rs). Run as blocking work: on an async worker, tokio hands
/// the other tasks to another thread meanwhile; outside a runtime (the CLI, tests) it just runs.
fn timed<T>(op: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
    let r = tokio::task::block_in_place(|| crate::metrics::time("le_mollet_db_seconds", &[("op", op)], f));
    if r.is_err() {
        crate::metrics::inc("le_mollet_db_errors_total", &[("op", op)])
    }
    r
}

/// Older JSON files: collection, file, the field holding the records, the field naming each one (else map keys).
const IMPORT: [(&str, &str, Option<&str>, Option<&str>); 5] = [
    ("boards", "leaderboards.json", None, None),
    ("clubs", "clubs.json", Some("clubs"), Some("code")),
    ("events", "events.json", Some("events"), Some("id")),
    ("riders", "riders.json", Some("riders"), None),
    ("roles", "roles.json", Some("grants"), None),
];

impl Db {
    /// Open (or create) the database; JSON files found in `dirs` fill collections still empty.
    pub fn open(path: &str, dirs: &[&str]) -> Result<Self> {
        let c = Connection::open(path).with_context(|| format!("open {path}"))?;
        // WAL: readers never wait for the writer; the CLI (`le-mollet role`) can write while the server runs.
        c.pragma_update(None, "journal_mode", "WAL")?;
        c.pragma_update(None, "synchronous", "NORMAL")?;
        c.busy_timeout(std::time::Duration::from_secs(5))?;
        c.execute_batch("CREATE TABLE IF NOT EXISTS docs (coll TEXT NOT NULL, id TEXT NOT NULL, data TEXT NOT NULL, PRIMARY KEY (coll, id)) WITHOUT ROWID")?;
        // Password hashes and sign-in sessions live here: for the server's user only.
        #[cfg(unix)]
        std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
        let db = Self(Mutex::new(c));
        for dir in dirs {
            db.import(dir)?
        }
        Ok(db)
    }

    /// In memory, for tests.
    pub fn memory() -> Self {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE docs (coll TEXT NOT NULL, id TEXT NOT NULL, data TEXT NOT NULL, PRIMARY KEY (coll, id)) WITHOUT ROWID").unwrap();
        Self(Mutex::new(c))
    }

    fn import(&self, dir: &str) -> Result<()> {
        for (coll, file, field, key) in IMPORT {
            let path = Path::new(dir).join(file);
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            if !self.is_empty(coll)? {
                continue;
            }
            let mut v: serde_json::Value = serde_json::from_str(&text).with_context(|| format!("{}: not JSON, left as is", path.display()))?;
            let v = match field {
                Some(f) => v[f].take(),
                None => v,
            };
            let rows: Vec<(String, serde_json::Value)> = match (v, key) {
                (serde_json::Value::Array(a), Some(k)) => a.into_iter().map(|x| (x[k].as_str().unwrap_or_default().to_owned(), x)).collect(),
                (serde_json::Value::Object(o), _) => o.into_iter().collect(),
                _ => vec![],
            };
            self.replace(coll, rows.iter().map(|(id, x)| (id.clone(), x)))?;
            std::fs::rename(&path, path.with_extension("json.bak"))?;
            eprintln!("imported {} ({} records) into the database", path.display(), rows.len());
        }
        Ok(())
    }

    pub fn is_empty(&self, coll: &str) -> Result<bool> {
        let c = self.0.lock().unwrap();
        Ok(c.query_row("SELECT 1 FROM docs WHERE coll = ?1 LIMIT 1", [coll], |_| Ok(())).optional()?.is_none())
    }

    /// Every record of a collection, by id. A record that no longer reads is skipped, with a word in the log.
    pub fn all<T: DeserializeOwned>(&self, coll: &str) -> Result<Vec<(String, T)>> {
        timed("all", || self.all_(coll))
    }

    fn all_<T: DeserializeOwned>(&self, coll: &str) -> Result<Vec<(String, T)>> {
        let c = self.0.lock().unwrap();
        let mut q = c.prepare("SELECT id, data FROM docs WHERE coll = ?1")?;
        let rows = q.query_map([coll], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = vec![];
        for row in rows {
            let (id, data) = row?;
            match serde_json::from_str(&data) {
                Ok(v) => out.push((id, v)),
                Err(e) => eprintln!("database: {coll}/{id} skipped: {e}"),
            }
        }
        Ok(out)
    }

    pub fn get<T: DeserializeOwned>(&self, coll: &str, id: &str) -> Result<Option<T>> {
        timed("get", || {
            let c = self.0.lock().unwrap();
            let data: Option<String> = c.query_row("SELECT data FROM docs WHERE coll = ?1 AND id = ?2", [coll, id], |r| r.get(0)).optional()?;
            Ok(data.map(|d| serde_json::from_str(&d)).transpose()?)
        })
    }

    pub fn put<T: Serialize + ?Sized>(&self, coll: &str, id: &str, v: &T) -> Result<()> {
        timed("put", || {
            let c = self.0.lock().unwrap();
            c.execute("INSERT OR REPLACE INTO docs (coll, id, data) VALUES (?1, ?2, ?3)", params![coll, id, serde_json::to_string(v)?])?;
            Ok(())
        })
    }

    /// Store a new record; false when the id is taken (atomic: two sign-ups with one name cannot both win).
    pub fn insert<T: Serialize + ?Sized>(&self, coll: &str, id: &str, v: &T) -> Result<bool> {
        timed("insert", || {
            let c = self.0.lock().unwrap();
            Ok(c.execute("INSERT OR IGNORE INTO docs (coll, id, data) VALUES (?1, ?2, ?3)", params![coll, id, serde_json::to_string(v)?])? == 1)
        })
    }

    pub fn delete(&self, coll: &str, id: &str) -> Result<()> {
        timed("delete", || {
            self.0.lock().unwrap().execute("DELETE FROM docs WHERE coll = ?1 AND id = ?2", [coll, id])?;
            Ok(())
        })
    }

    /// A whole small collection at once (events, clubs, roles), in one transaction.
    pub fn replace<'a, T: Serialize + 'a>(&self, coll: &str, rows: impl IntoIterator<Item = (String, &'a T)>) -> Result<()> {
        timed("replace", || self.replace_(coll, rows))
    }

    fn replace_<'a, T: Serialize + 'a>(&self, coll: &str, rows: impl IntoIterator<Item = (String, &'a T)>) -> Result<()> {
        let mut c = self.0.lock().unwrap();
        let tx = c.transaction()?;
        tx.execute("DELETE FROM docs WHERE coll = ?1", [coll])?;
        for (id, v) in rows {
            tx.execute("INSERT OR REPLACE INTO docs (coll, id, data) VALUES (?1, ?2, ?3)", params![coll, id, serde_json::to_string(v)?])?;
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_round_trip_and_old_files_are_imported() {
        let dir = std::env::temp_dir().join(format!("le-mollet-db-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("events.json"), r#"{"events":[{"id":"e1","x":1},{"id":"e2","x":2}]}"#).unwrap();
        std::fs::write(dir.join("leaderboards.json"), r#"{"alpe:KOM":[{"t":1}]}"#).unwrap();
        let path = dir.join("t.db");
        let db = Db::open(path.to_str().unwrap(), &[dir.to_str().unwrap()]).unwrap();
        let mut ev: Vec<(String, serde_json::Value)> = db.all("events").unwrap();
        ev.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!((ev.len(), ev[1].1["x"].as_i64()), (2, Some(2)));
        assert!(dir.join("events.json.bak").exists() && !dir.join("events.json").exists());
        assert_eq!(db.get::<serde_json::Value>("boards", "alpe:KOM").unwrap().unwrap()[0]["t"], 1);

        assert!(db.insert("accounts", "x", &1).unwrap() && !db.insert("accounts", "x", &2).unwrap());
        assert_eq!(db.get::<i32>("accounts", "x").unwrap(), Some(1));
        db.put("riders", "a", &5).unwrap();
        db.replace("events", [("e3".to_string(), &7)]).unwrap();
        drop(db);
        let db = Db::open(path.to_str().unwrap(), &[dir.to_str().unwrap()]).unwrap(); // reopened: kept, no re-import
        assert_eq!((db.get::<i32>("riders", "a").unwrap(), db.all::<i32>("events").unwrap()), (Some(5), vec![("e3".into(), 7)]));
        db.delete("riders", "a").unwrap();
        assert!(db.is_empty("riders").unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
