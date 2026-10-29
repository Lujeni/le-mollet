//! Friends: each profile gets a handle, `Name#1234`; a request is sent to a handle, and once accepted both
//! profiles see each other's profile and rides (a coach follows their riders this way). The page sends each ride
//! it keeps; the server keeps the last ones. Handles are all the others ever see: profile ids stay on the server.
//! Kept in the database (src/db.rs), one row per profile.
//! ponytail: a pid is the only credential, like clubs (src/club.rs); in cloud mode the account owns the pid

use crate::db::Db;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Rides kept per profile, newest first.
pub const MAX_RIDES: usize = 100;
/// Friends, and pending requests, per profile.
pub const MAX_FRIENDS: usize = 200;

#[derive(Debug, Default)]
pub struct Friends {
    pub people: HashMap<String, Person>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Person {
    pub name: String,
    /// The number after `#`, 0 to 9999: two profiles with the same name differ by it.
    pub num: u16,
    #[serde(default)]
    pub country: Option<String>,
    /// Profile ids of the friends.
    #[serde(default)]
    pub friends: Vec<String>,
    /// Profile ids that asked, waiting for an answer.
    #[serde(default)]
    pub asked: Vec<String>,
    /// The rides as the page keeps them (summary and trace), newest first.
    #[serde(default)]
    pub rides: Vec<Value>,
}

impl Person {
    pub fn handle(&self) -> String {
        format!("{}#{:04}", self.name, self.num)
    }
}

/// A name for a handle: no `#`, no control characters, 1 to 24 characters.
fn clean(name: &str) -> String {
    let n: String = name.chars().filter(|c| !c.is_control() && *c != '#').take(24).collect();
    if n.trim().is_empty() { "Cycliste".into() } else { n.trim().to_owned() }
}

impl Friends {
    pub fn load(db: &Db) -> Result<Self> {
        Ok(Self { people: db.all("friends")?.into_iter().collect() })
    }

    /// Store these profiles.
    pub fn save(&self, db: &Db, pids: &[&str]) -> Result<()> {
        for pid in pids {
            if let Some(p) = self.people.get(*pid) { db.put("friends", pid, p)? }
        }
        Ok(())
    }

    fn taken(&self, pid: &str, name: &str, num: u16) -> bool {
        self.people.iter().any(|(k, p)| k != pid && p.num == num && p.name.eq_ignore_ascii_case(name))
    }

    /// The profile's entry, made on its first visit; a new name keeps its number unless someone has that handle.
    /// `seed` picks the first number tried.
    pub fn me(&mut self, pid: &str, name: &str, country: Option<String>, seed: u16) -> &Person {
        let name = clean(name);
        let num = self.people.get(pid).map_or(seed % 10_000, |p| p.num);
        let num = (0..10_000).map(|i| (num + i) % 10_000).find(|n| !self.taken(pid, &name, *n)).unwrap_or(num);
        let p = self.people.entry(pid.into()).or_insert_with(|| Person { name: name.clone(), num, country: None, friends: vec![], asked: vec![], rides: vec![] });
        (p.name, p.num, p.country) = (name, num, country);
        p
    }

    /// The profile id behind a handle (`name#1234`, any case, spaces around ignored).
    pub fn find(&self, handle: &str) -> Option<String> {
        let (name, num) = handle.trim().rsplit_once('#')?;
        let num: u16 = num.trim().parse().ok()?;
        self.people.iter().find(|(_, p)| p.num == num && p.name.eq_ignore_ascii_case(name.trim())).map(|(k, _)| k.clone())
    }

    /// Ask a handle to be friends; if they had asked first, it is a yes. Returns the other's profile id.
    pub fn ask(&mut self, pid: &str, handle: &str) -> Result<String, &'static str> {
        let other = self.find(handle).ok_or("no one with this handle")?;
        if other == pid { return Err("that's you") }
        if !self.people.contains_key(pid) { return Err("no such profile") }
        if self.people[pid].friends.contains(&other) { return Err("already friends") }
        if self.people[pid].asked.contains(&other) { return self.accept(pid, handle) }
        let them = self.people.get_mut(&other).unwrap();
        if them.asked.len() + them.friends.len() >= MAX_FRIENDS { return Err("too many friends") }
        if !them.asked.iter().any(|a| a == pid) { them.asked.push(pid.into()) }
        Ok(other)
    }

    /// Say yes to a request. Returns the other's profile id.
    pub fn accept(&mut self, pid: &str, handle: &str) -> Result<String, &'static str> {
        let other = self.find(handle).ok_or("no one with this handle")?;
        let me = self.people.get_mut(pid).ok_or("no such profile")?;
        if !me.asked.contains(&other) { return Err("no request from them") }
        me.asked.retain(|a| *a != other);
        me.friends.push(other.clone());
        let them = self.people.get_mut(&other).unwrap();
        them.asked.retain(|a| a != pid);
        if !them.friends.iter().any(|f| f == pid) { them.friends.push(pid.into()) }
        Ok(other)
    }

    /// No more friends, or no to a request, or a request taken back: whatever links the two goes.
    pub fn remove(&mut self, pid: &str, handle: &str) -> Result<String, &'static str> {
        let other = self.find(handle).ok_or("no one with this handle")?;
        for (a, b) in [(pid, other.as_str()), (other.as_str(), pid)] {
            if let Some(p) = self.people.get_mut(a) {
                p.friends.retain(|f| f != b);
                p.asked.retain(|f| f != b);
            }
        }
        Ok(other)
    }

    /// A ride the page keeps (an object with an `id`); the same id again replaces it.
    pub fn add_ride(&mut self, pid: &str, ride: Value) -> Result<(), &'static str> {
        let id = ride.get("id").and_then(Value::as_str).ok_or("ride: needs an id")?.to_owned();
        let p = self.people.get_mut(pid).ok_or("no such profile")?;
        p.rides.retain(|r| r.get("id").and_then(Value::as_str) != Some(&id));
        p.rides.insert(0, ride);
        // Newest first by date (a backfill sends older rides after newer ones); ISO dates sort as text.
        p.rides.sort_by(|a, b| b.get("date").and_then(Value::as_str).cmp(&a.get("date").and_then(Value::as_str)));
        p.rides.truncate(MAX_RIDES);
        Ok(())
    }

    pub fn drop_ride(&mut self, pid: &str, id: &str) {
        if let Some(p) = self.people.get_mut(pid) { p.rides.retain(|r| r.get("id").and_then(Value::as_str) != Some(id)) }
    }

    /// A friend's entry, and their profile id; only between friends.
    pub fn view(&self, pid: &str, handle: &str) -> Option<(String, &Person)> {
        let other = self.find(handle)?;
        self.people.get(pid)?.friends.contains(&other).then(|| (other.clone(), &self.people[&other]))
    }

    /// Handles of the profiles this one asked, still waiting.
    pub fn sent(&self, pid: &str) -> Vec<String> {
        self.people.values().filter(|p| p.asked.iter().any(|a| a == pid)).map(Person::handle).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn handles_requests_and_rides() {
        let mut f = Friends::default();
        assert_eq!(f.me("a", "Lujeni", None, 2138).handle(), "Lujeni#2138");
        assert_eq!(f.me("b", "lujeni", None, 2138).handle(), "lujeni#2139"); // same handle taken: the next number
        assert_eq!(f.me("c", "Coach#1", Some("FR".into()), 7).handle(), "Coach1#0007");
        assert_eq!(f.me("a", "Lujeni", None, 1).num, 2138); // a visit keeps the number

        // Ask, see nothing yet, accept, see each other.
        assert_eq!(f.ask("c", "LUJENI#2138 "), Ok("a".into()));
        assert_eq!(f.sent("c"), vec!["Lujeni#2138"]);
        assert!(f.view("c", "Lujeni#2138").is_none());
        assert_eq!(f.ask("c", "Coach1#0007"), Err("that's you"));
        assert_eq!(f.ask("c", "Nobody#0001"), Err("no one with this handle"));
        assert_eq!(f.accept("b", "Coach1#0007"), Err("no request from them"));
        f.accept("a", "Coach1#0007").unwrap();
        assert!(f.view("c", "Lujeni#2138").is_some() && f.view("a", "Coach1#0007").is_some());
        assert!(f.view("b", "Lujeni#2138").is_none());
        assert_eq!(f.ask("a", "Coach1#0007"), Err("already friends"));

        // Asking someone who asked you is a yes.
        f.ask("b", "Coach1#0007").unwrap();
        f.ask("c", "lujeni#2139").unwrap();
        assert!(f.view("b", "Coach1#0007").is_some());

        // Rides: newest first by date, the same id replaced.
        f.add_ride("a", json!({ "id":"r1", "date":"2026-10-01T10:00:00Z" })).unwrap();
        f.add_ride("a", json!({ "id":"r2", "date":"2026-10-03T10:00:00Z" })).unwrap();
        f.add_ride("a", json!({ "id":"r0", "date":"2026-09-01T10:00:00Z" })).unwrap();
        f.add_ride("a", json!({ "id":"r1", "date":"2026-10-01T10:00:00Z", "name":"again" })).unwrap();
        let ids = |f: &Friends| f.view("c", "Lujeni#2138").unwrap().1.rides.iter().map(|r| r["id"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
        assert_eq!(ids(&f), ["r2", "r1", "r0"]);
        assert!(f.add_ride("a", json!({ "date":"x" })).is_err());
        f.drop_ride("a", "r2");
        assert_eq!(ids(&f), ["r1", "r0"]);

        // Removing a friend ends it both ways.
        f.remove("c", "Lujeni#2138").unwrap();
        assert!(f.view("a", "Coach1#0007").is_none() && f.view("c", "Lujeni#2138").is_none());
    }
}
