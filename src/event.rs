//! Scheduled rides: group rides behind a pace leader, and races with categories, a live ranking and results.
//! Riders sign up, wait in the start pen during the last minutes, and all go at the start time. Kept in
//! the database (src/db.rs). Riders are profiles (`pid`), like clubs: no account needed at home.
//! ponytail: a pid is the only credential, fine at home; in cloud mode tie entrants to the signed-in account

use crate::db::Db;
use anyhow::Result;
use serde::{Deserialize, Serialize};

/// The pen opens this long before the start, s.
pub const PEN_S: u64 = 600;
/// An event leaves the list this long after its start, s.
pub const KEEP_S: u64 = 3 * 3600;
/// Scheduled races start on every half hour (`Events::schedule`).
pub const SLOT_S: u64 = 1800;
/// They are listed this long ahead, s.
pub const AHEAD_S: u64 = 3600;
/// Their length, about, m: laps of a loop, or a climb once.
const AUTO_M: f32 = 15_000.0;
/// Scheduled group rides start on the quarter hours between the races (:15, :45), so something starts every
/// 15 minutes; one per pace, behind a leader; each is kept this long.
pub const GROUP_SLOT_S: u64 = 900;
pub const GROUP_PACES: [(f32, &str); 3] = [(1.5, "Balade"), (2.5, "Tempo"), (3.5, "Costaud")];
pub const GROUP_KEEP_S: u64 = 2700;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Together behind a leader at a set W/kg.
    Group,
    /// First across the line after `laps` laps (or at the top of a climb).
    Race,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub kind: Kind,
    pub name: String,
    /// Course or open world id.
    pub track: String,
    /// Start, unix seconds.
    pub start: u64,
    /// Laps of a loop; 1 on a climb.
    pub laps: u32,
    /// Group ride pace, W/kg.
    #[serde(default)]
    pub pace: f32,
    /// Ranks it is open to (codes of `TIERS`), e.g. "O" or "PO"; empty: everyone.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cats: String,
    /// How long after the start it stays, s; 0 means `KEEP_S`.
    #[serde(default, skip_serializing_if = "u64_is_zero")]
    pub keep: u64,
    /// Club tag when it is a club ride.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub club: Option<String>,
    /// Profile that scheduled it, and its name.
    pub by: String,
    pub by_name: String,
    #[serde(default)]
    pub entrants: Vec<Entrant>,
    #[serde(default)]
    pub results: Vec<Finish>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entrant {
    pub pid: String,
    pub name: String,
    pub wkg: f32,
    pub cat: char,
    /// One of the server's riders (src/peloton.rs), not a person.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bot: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finish {
    pub pid: String,
    pub name: String,
    pub cat: char,
    /// Race time, s.
    pub time: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Upcoming,
    /// In the start pen: riders wait at the line.
    Pen,
    Live,
    Over,
}

/// Ranks by FTP in W/kg, like a ladder in a game, strongest first: code, lowest W/kg, name.
/// Riders may race up a rank, never down. The page has the same table (TIERS in web/index.html).
pub const TIERS: [(char, f32, &str); 7] = [
    ('L', 5.0, "Légende"),
    ('M', 4.4, "Maître"),
    ('D', 3.8, "Diamant"),
    ('P', 3.2, "Platine"),
    ('O', 2.5, "Or"),
    ('A', 2.0, "Argent"),
    ('B', 0.0, "Bronze"),
];

/// A rider's rank by FTP in W/kg.
pub fn category(wkg: f32) -> char {
    TIERS.iter().find(|t| wkg >= t.1).map_or('B', |t| t.0)
}

/// 0 for the strongest rank; unknown codes come last.
fn rank(c: char) -> usize {
    TIERS.iter().position(|t| t.0 == c).unwrap_or(TIERS.len())
}

fn u64_is_zero(n: &u64) -> bool {
    *n == 0
}

impl Event {
    pub fn phase(&self, now: u64) -> Phase {
        let keep = if self.keep > 0 { self.keep } else { KEEP_S };
        match now {
            t if t + PEN_S < self.start => Phase::Upcoming,
            t if t < self.start => Phase::Pen,
            t if t < self.start + keep => Phase::Live,
            _ => Phase::Over,
        }
    }

    /// Whether a rider of rank `cat` may sign up: open to it, or to a stronger rank.
    pub fn open_to(&self, cat: char) -> bool {
        self.cats.is_empty() || self.cats.chars().any(|c| rank(c) <= rank(cat))
    }

    /// Race distance on a course of `lap_m` m: laps of a loop, or the climb once.
    pub fn distance(&self, lap_m: f32, looped: bool) -> f32 {
        if looped { lap_m * self.laps.max(1) as f32 } else { lap_m }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Events {
    #[serde(default)]
    pub events: Vec<Event>,
}

impl Events {
    pub fn load(db: &Db) -> Result<Self> {
        let mut events: Vec<Event> = db.all("events")?.into_iter().map(|(_, e)| e).collect();
        events.sort_by_key(|e| e.start);
        Ok(Self { events })
    }

    pub fn save(&self, db: &Db) -> Result<()> {
        db.replace("events", self.events.iter().map(|e| (e.id.clone(), e)))
    }

    /// Not over yet, soonest first; older events are dropped.
    pub fn current(&mut self, now: u64) -> Vec<Event> {
        self.events.retain(|e| e.phase(now) != Phase::Over);
        self.events.sort_by_key(|e| e.start);
        self.events.clone()
    }

    pub fn get(&self, id: &str) -> Option<&Event> {
        self.events.iter().find(|e| e.id == id)
    }

    /// Sign up (again: name and W/kg updated). Not once the race is on.
    pub fn join(&mut self, id: &str, pid: &str, name: &str, wkg: f32, now: u64) -> Result<&Event, &'static str> {
        let e = self.events.iter_mut().find(|e| e.id == id).ok_or("no such event")?;
        if e.kind == Kind::Race && e.phase(now) != Phase::Upcoming && e.phase(now) != Phase::Pen { return Err("the race has started") }
        if !e.open_to(category(wkg)) { return Err("not your category") }
        e.entrants.retain(|x| x.pid != pid);
        e.entrants.push(Entrant { pid: pid.into(), name: name.into(), wkg, cat: category(wkg), bot: false });
        Ok(e)
    }

    /// A race per rank on every half hour of the next hour, and a group ride per pace on the quarter hours
    /// between (loops first: a bunch rides round and round), so something always starts within 15 minutes; on
    /// `tracks` in turn (id, name, lap m, looped). Returns whether it added any.
    /// ponytail: an organizer deleting a scheduled race sees it come back; keep a list of skipped ids if that bothers
    pub fn schedule(&mut self, now: u64, tracks: &[(String, String, f32, bool)]) -> bool {
        if tracks.is_empty() { return false }
        let mut added = false;
        for slot in now / SLOT_S + 1..=(now + AHEAD_S) / SLOT_S {
            let (track, name, lap_m, looped) = &tracks[slot as usize % tracks.len()];
            for (cat, _, tier) in TIERS {
                let id = format!("auto-{slot}-{cat}");
                if self.get(&id).is_some() { continue }
                self.events.push(Event {
                    id,
                    kind: Kind::Race,
                    name: format!("{name} · {tier}"),
                    track: track.clone(),
                    start: slot * SLOT_S,
                    laps: if *looped { (AUTO_M / lap_m).round().max(1.0) as u32 } else { 1 },
                    pace: 0.0,
                    keep: 0,
                    cats: cat.into(),
                    club: None,
                    by: "auto".into(),
                    by_name: "le-mollet".into(),
                    entrants: vec![],
                    results: vec![],
                });
                added = true;
            }
        }
        let loops: Vec<_> = tracks.iter().filter(|t| t.3).collect();
        let rides = if loops.is_empty() { tracks.iter().collect() } else { loops };
        for slot in now / GROUP_SLOT_S + 1..=(now + AHEAD_S) / GROUP_SLOT_S {
            if (slot * GROUP_SLOT_S).is_multiple_of(SLOT_S) { continue } // the races' half hours
            let (track, name, ..) = rides[slot as usize % rides.len()];
            for (i, (pace, label)) in GROUP_PACES.iter().enumerate() {
                let id = format!("auto-g{slot}-{i}");
                if self.get(&id).is_some() { continue }
                self.events.push(Event {
                    id,
                    kind: Kind::Group,
                    name: format!("{label} · {name}"),
                    track: track.clone(),
                    start: slot * GROUP_SLOT_S,
                    laps: 1,
                    pace: *pace,
                    keep: GROUP_KEEP_S,
                    cats: String::new(),
                    club: None,
                    by: "auto".into(),
                    by_name: "le-mollet".into(),
                    entrants: vec![],
                    results: vec![],
                });
                added = true;
            }
        }
        added
    }

    pub fn leave(&mut self, id: &str, pid: &str) {
        if let Some(e) = self.events.iter_mut().find(|e| e.id == id) { e.entrants.retain(|x| x.pid != pid) }
    }

    /// A rider crossed the finish line: their place overall and in their category (1-based), once.
    pub fn finish(&mut self, id: &str, pid: &str, name: &str, time: f32) -> Option<(usize, usize)> {
        let e = self.events.iter_mut().find(|e| e.id == id)?;
        let cat = e.entrants.iter().find(|x| x.pid == pid).map_or('B', |x| x.cat);
        if !e.results.iter().any(|r| r.pid == pid) {
            e.results.push(Finish { pid: pid.into(), name: name.into(), cat, time });
            e.results.sort_by(|a, b| a.time.total_cmp(&b.time));
        }
        let overall = e.results.iter().position(|r| r.pid == pid)? + 1;
        let in_cat = e.results.iter().filter(|r| r.cat == cat).position(|r| r.pid == pid)? + 1;
        Some((overall, in_cat))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn race(start: u64) -> Event {
        Event { id: "e1".into(), kind: Kind::Race, name: "Critérium".into(), track: "longchamp".into(), start, laps: 5, pace: 0.0, keep: 0, cats: String::new(), club: None, by: "a".into(), by_name: "Ana".into(), entrants: vec![], results: vec![] }
    }

    #[test]
    fn phases_categories_and_results() {
        let e = race(10_000);
        assert_eq!([e.phase(9_000), e.phase(9_500), e.phase(10_000), e.phase(10_000 + KEEP_S)], [Phase::Upcoming, Phase::Pen, Phase::Live, Phase::Over]);
        assert_eq!(e.distance(3540.0, true), 17_700.0);
        assert_eq!(e.distance(13_600.0, false), 13_600.0);
        assert_eq!([5.2, 4.5, 3.9, 3.5, 2.6, 2.1, 1.9].map(category), ['L', 'M', 'D', 'P', 'O', 'A', 'B']);

        let mut ev = Events { events: vec![race(10_000)] };
        assert_eq!(ev.join("e1", "a", "Ana", 3.4, 9_000).unwrap().entrants[0].cat, 'P');
        ev.join("e1", "b", "Bob", 2.0, 9_000).unwrap();
        ev.join("e1", "c", "Cyd", 3.3, 9_800).unwrap(); // in the pen: still fine
        assert!(ev.join("e1", "d", "Dan", 3.0, 10_100).is_err()); // started
        assert_eq!(ev.get("e1").unwrap().entrants.len(), 3);
        assert_eq!(ev.finish("e1", "b", "Bob", 1500.0), Some((1, 1)));
        assert_eq!(ev.finish("e1", "a", "Ana", 1400.0), Some((1, 1)));
        assert_eq!(ev.finish("e1", "c", "Cyd", 1450.0), Some((2, 2)));
        assert_eq!(ev.finish("e1", "a", "Ana", 1600.0), Some((1, 1))); // once
        assert_eq!(ev.current(10_000 + KEEP_S).len(), 0); // over: dropped
    }

    #[test]
    fn scheduled_races_by_level() {
        let mut ev = Events::default();
        let tracks = [("longchamp".to_string(), "Longchamp".to_string(), 3540.0, true), ("alpe".into(), "Alpe".into(), 13_600.0, false)];
        let now = 100 * SLOT_S + 60;
        assert!(ev.schedule(now, &tracks) && !ev.schedule(now, &tracks)); // once
        assert_eq!(ev.events.len(), 20); // the next two half hours, a race per rank, and two quarter hours, a group ride per pace
        let g = ev.get("auto-g201-1").unwrap(); // :15 past the first half hour, the Tempo ride, on the loop
        assert_eq!((g.kind, g.start, g.track.as_str(), g.pace), (Kind::Group, 201 * GROUP_SLOT_S, "longchamp", 2.5));
        assert_eq!([g.phase(g.start), g.phase(g.start + GROUP_KEEP_S)], [Phase::Live, Phase::Over]); // kept 45 min, not 3 h
        assert!(ev.join("auto-g201-0", "a", "Ana", 4.2, now).is_ok() && ev.join("auto-g201-0", "b", "Bob", 1.2, now).is_ok()); // open to all
        let b = ev.get("auto-101-P").unwrap();
        assert_eq!((b.start, b.track.as_str(), b.laps), (101 * SLOT_S, "alpe", 1));
        assert_eq!(ev.get("auto-102-L").unwrap().laps, 4); // ~15 km of a 3.5 km loop

        // Race up a level, never down.
        assert!(ev.join("auto-101-P", "c", "Cyd", 3.0, now).is_ok()); // Or into Platine
        assert_eq!(ev.join("auto-101-O", "a", "Ana", 4.2, now).unwrap_err(), "not your category"); // Diamant into Or
        assert!(ev.join("auto-101-D", "a", "Ana", 4.2, now).is_ok());
    }
}
