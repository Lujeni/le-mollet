//! Clubs: riders gather under a name and a short tag shown before their names, and the server counts each
//! member's kilometres this week. Members are profiles (their `pid`), so clubs work without accounts, on one
//! PC or a LAN. One club per profile. Kept in the database (src/db.rs).
//! ponytail: a pid is the only credential, fine at home; in cloud mode tie members to the signed-in account

use crate::db::Db;
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Clubs {
    #[serde(default)]
    pub clubs: Vec<Club>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Club {
    /// Invite code, also the club's id.
    pub code: String,
    pub name: String,
    /// 2 to 4 capitals or digits, shown as `[TAG]` before members' names.
    pub tag: String,
    pub color: String,
    /// Members, the founder first; the founder's role passes to the next one when they leave.
    pub members: Vec<Member>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Member {
    pub pid: String,
    pub name: String,
    /// The week `km` counts (see `week`).
    pub week: u64,
    pub km: f32,
}

/// Weeks since the epoch, starting on Mondays (1970-01-01 was a Thursday).
pub fn week(unix_secs: u64) -> u64 {
    (unix_secs / 86_400 + 3) / 7
}

impl Clubs {
    pub fn load(db: &Db) -> Result<Self> {
        Ok(Self { clubs: db.all("clubs")?.into_iter().map(|(_, c)| c).collect() })
    }

    pub fn save(&self, db: &Db) -> Result<()> {
        db.replace("clubs", self.clubs.iter().map(|c| (c.code.clone(), c)))
    }

    pub fn of(&self, pid: &str) -> Option<&Club> {
        self.clubs.iter().find(|c| c.members.iter().any(|m| m.pid == pid))
    }

    /// Found a club; the founder leaves their current one.
    pub fn create(&mut self, pid: &str, me: &str, name: &str, tag: &str, color: &str, code: String) -> Result<&Club, &'static str> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 30 {
            return Err("name: 1 to 30 characters");
        }
        if !(2..=4).contains(&tag.len()) || !tag.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()) {
            return Err("tag: 2 to 4 capitals or digits");
        }
        if self.clubs.iter().any(|c| c.tag == tag) {
            return Err("tag taken");
        }
        self.leave(pid);
        let member = Member { pid: pid.into(), name: me.into(), week: 0, km: 0.0 };
        self.clubs.push(Club { code, name: name.into(), tag: tag.into(), color: color.into(), members: vec![member] });
        Ok(self.clubs.last().unwrap())
    }

    pub fn join(&mut self, code: &str, pid: &str, me: &str) -> Option<&Club> {
        let i = self.clubs.iter().position(|c| c.code.eq_ignore_ascii_case(code))?;
        if !self.clubs[i].members.iter().any(|m| m.pid == pid) {
            self.leave(pid);
            let i = self.clubs.iter().position(|c| c.code.eq_ignore_ascii_case(code))?; // leaving may drop an empty club
            self.clubs[i].members.push(Member { pid: pid.into(), name: me.into(), week: 0, km: 0.0 });
        }
        self.clubs.iter().find(|c| c.code.eq_ignore_ascii_case(code))
    }

    /// Leave one's club; the last one out closes it.
    pub fn leave(&mut self, pid: &str) {
        for c in &mut self.clubs {
            c.members.retain(|m| m.pid != pid)
        }
        self.clubs.retain(|c| !c.members.is_empty());
    }

    /// A ride of `km` ended this `week`: counts for the rider's club, if any. Returns whether it counted.
    /// `name` is the name ridden under, maybe with the club's tag before it.
    pub fn add_km(&mut self, pid: &str, name: &str, km: f32, week: u64) -> bool {
        let Some(c) = self.clubs.iter_mut().find(|c| c.members.iter().any(|m| m.pid == pid)) else { return false };
        let name = name.strip_prefix(&format!("[{}] ", c.tag)).unwrap_or(name);
        let m = c.members.iter_mut().find(|m| m.pid == pid).unwrap();
        if m.week != week {
            (m.week, m.km) = (week, 0.0)
        }
        m.km += km;
        m.name = name.into(); // the profile may have been renamed
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clubs_found_join_count_and_close() {
        let mut c = Clubs::default();
        assert!(c.create("a", "Ana", "Les Mollets", "lm", "#c91a09", "AAA111".into()).is_err()); // lowercase tag
        c.create("a", "Ana", "Les Mollets", "LM", "#c91a09", "AAA111".into()).unwrap();
        assert!(c.create("x", "Xi", "Autre", "LM", "#000000", "BBB222".into()).is_err()); // tag taken
        assert_eq!(c.join("aaa111", "b", "Bob").unwrap().members.len(), 2);
        assert_eq!(c.join("aaa111", "b", "Bob").unwrap().members.len(), 2); // joining twice changes nothing
        assert!(c.join("nope", "b", "Bob").is_none());

        // Kilometres count per week and start over the next one.
        let w = week(1_760_000_000);
        assert!(c.add_km("b", "Bob", 20.0, w) && c.add_km("b", "[LM] Bobby", 5.0, w));
        assert!(!c.add_km("z", "Zed", 5.0, w));
        let bob = |c: &Clubs| c.of("b").unwrap().members[1].clone();
        assert_eq!((bob(&c).km, bob(&c).name.as_str()), (25.0, "Bobby"));
        c.add_km("b", "Bobby", 3.0, w + 1);
        assert_eq!(bob(&c).km, 3.0);

        // The founder leaves: Bob runs it; Bob leaves: the club closes.
        c.leave("a");
        assert_eq!(c.of("b").unwrap().members[0].pid, "b");
        c.leave("b");
        assert!(c.clubs.is_empty() && c.of("b").is_none());
    }

    #[test]
    fn weeks_start_on_monday() {
        // 2026-10-25 is a Sunday, 2026-10-26 a Monday (noon UTC).
        assert_eq!(week(1_792_929_600) + 1, week(1_793_016_000));
    }
}
