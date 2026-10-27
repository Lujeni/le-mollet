//! Scheduled rides: group rides behind a pace leader, and races with categories, a live ranking and results.
//! Riders sign up, wait in the start pen during the last minutes, and all go at the start time. Kept in
//! `events.json` next to the courses. Riders are profiles (`pid`), like clubs: no account needed at home.
//! ponytail: a pid is the only credential, fine at home; in cloud mode tie entrants to the signed-in account

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// The pen opens this long before the start, s.
pub const PEN_S: u64 = 600;
/// An event leaves the list this long after its start, s.
pub const KEEP_S: u64 = 3 * 3600;

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

/// Race category by FTP in W/kg, like Zwift's: A 4.0+, B 3.2+, C 2.5+, D below.
pub fn category(wkg: f32) -> char {
    match wkg {
        w if w >= 4.0 => 'A',
        w if w >= 3.2 => 'B',
        w if w >= 2.5 => 'C',
        _ => 'D',
    }
}

impl Event {
    pub fn phase(&self, now: u64) -> Phase {
        match now {
            t if t + PEN_S < self.start => Phase::Upcoming,
            t if t < self.start => Phase::Pen,
            t if t < self.start + KEEP_S => Phase::Live,
            _ => Phase::Over,
        }
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
    /// A missing or broken file is an empty one.
    pub fn load(path: &str) -> Self {
        std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &str) -> Result<()> {
        std::fs::write(path, serde_json::to_string(self)?).with_context(|| format!("write {path}"))
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
        e.entrants.retain(|x| x.pid != pid);
        e.entrants.push(Entrant { pid: pid.into(), name: name.into(), wkg, cat: category(wkg) });
        Ok(e)
    }

    pub fn leave(&mut self, id: &str, pid: &str) {
        if let Some(e) = self.events.iter_mut().find(|e| e.id == id) { e.entrants.retain(|x| x.pid != pid) }
    }

    /// A rider crossed the finish line: their place overall and in their category (1-based), once.
    pub fn finish(&mut self, id: &str, pid: &str, name: &str, time: f32) -> Option<(usize, usize)> {
        let e = self.events.iter_mut().find(|e| e.id == id)?;
        let cat = e.entrants.iter().find(|x| x.pid == pid).map_or('D', |x| x.cat);
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
        Event { id: "e1".into(), kind: Kind::Race, name: "Critérium".into(), track: "longchamp".into(), start, laps: 5, pace: 0.0, club: None, by: "a".into(), by_name: "Ana".into(), entrants: vec![], results: vec![] }
    }

    #[test]
    fn phases_categories_and_results() {
        let e = race(10_000);
        assert_eq!([e.phase(9_000), e.phase(9_500), e.phase(10_000), e.phase(10_000 + KEEP_S)], [Phase::Upcoming, Phase::Pen, Phase::Live, Phase::Over]);
        assert_eq!(e.distance(3540.0, true), 17_700.0);
        assert_eq!(e.distance(13_600.0, false), 13_600.0);
        assert_eq!([category(4.2), category(3.5), category(2.6), category(1.9)], ['A', 'B', 'C', 'D']);

        let mut ev = Events { events: vec![race(10_000)] };
        assert_eq!(ev.join("e1", "a", "Ana", 3.4, 9_000).unwrap().entrants[0].cat, 'B');
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
}
