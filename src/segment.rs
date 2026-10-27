//! Timed segments (KOM climbs and sprints) along what a rider rides, and their leaderboards.
//!
//! A segment is a window `[d0, d1]` of distance along the rider's course: the whole road of a climb, a sprint
//! and the steepest stretch of a loop, or, in an open world, a named street each time the route takes it.

use crate::course::Course;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Kom,
    Sprint,
}

#[derive(Debug, Clone, Serialize)]
pub struct Window {
    pub name: String,
    pub kind: Kind,
    pub d0: f32,
    pub d1: f32,
}

/// An open world's segment: a street, timed whenever it is ridden end to end (uphill only for a KOM).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Def {
    pub name: String,
    pub kind: Kind,
    pub street: String,
}

/// Sprint length on a loop, and the stretch searched for its KOM, m.
const SPRINT_M: f32 = 400.0;
const KOM_M: f32 = 1500.0;

/// Segments of a course: a climb is one KOM end to end; a loop gets a sprint a quarter of the way and its
/// steepest 1.5 km as a KOM.
pub fn course_windows(c: &Course) -> Vec<Window> {
    let len = c.length();
    if !c.looped {
        return vec![Window { name: format!("KOM {}", c.name), kind: Kind::Kom, d0: 0.0, d1: len }];
    }
    let mut out = vec![];
    if len > SPRINT_M * 3.0 {
        out.push(Window { name: "Sprint de la boucle".into(), kind: Kind::Sprint, d0: len * 0.25, d1: len * 0.25 + SPRINT_M });
    }
    if len > KOM_M * 2.0 {
        let (mut best, mut at) = (f32::MIN, 0.0);
        let mut d = 0.0;
        while d + KOM_M < len {
            let gain = c.ele_at(d + KOM_M) - c.ele_at(d);
            if gain > best { (best, at) = (gain, d) }
            d += 50.0;
        }
        // A KOM that overlaps the sprint would make one window hide the other: keep the sprint.
        let clear = out.iter().all(|w| at + KOM_M < w.d0 || at > w.d1);
        if best > 15.0 && clear {
            out.push(Window { name: "KOM de la boucle".into(), kind: Kind::Kom, d0: at, d1: at + KOM_M });
        }
    }
    out
}

/// One rider's clock on the segments.
#[derive(Debug, Default)]
pub struct Timer {
    running: Option<(usize, f32)>,
    last: Option<f32>,
}

/// Rides further than this in one update are teleports (demo seek): the running segment is void.
const JUMP_M: f32 = 30.0;

impl Timer {
    /// The running segment and since when.
    pub fn running(&self) -> Option<(usize, f32)> {
        self.running
    }

    /// Feed the position `d` and the ride clock `t` after each tick; returns `(window, time)` when one is done.
    pub fn update(&mut self, windows: &[Window], d: f32, t: f32) -> Option<(usize, f32)> {
        let last = self.last.replace(d).unwrap_or(-1.0);
        let step = d - last;
        if !(-JUMP_M..=JUMP_M).contains(&step) {
            self.running = None; // a jump, or a loop wrapping round: nothing to finish across it
            return None;
        }
        if let Some((i, t0)) = self.running {
            let w = &windows[i];
            if d >= w.d1 {
                self.running = None;
                return Some((i, t - t0));
            }
            if d < w.d0 { self.running = None }
        }
        if self.running.is_none() {
            self.running = windows.iter().position(|w| last < w.d0 && d >= w.d0 && d < w.d1).map(|i| (i, t));
        }
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// Profile id (or session) so a rider keeps one line, their best.
    pub who: String,
    pub name: String,
    pub color: String,
    pub time: f32,
    /// Unix seconds.
    pub date: u64,
}

/// Where a new time lands: overall and among today's (UTC day) times.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Rank {
    pub rank: usize,
    pub of: usize,
    pub today: usize,
    pub of_today: usize,
}

/// Best time per rider on each segment, by key (`track:segment`).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Boards(pub HashMap<String, Vec<Entry>>);

/// Riders kept per segment.
const KEEP: usize = 200;

impl Boards {
    pub fn load(path: &str) -> Self {
        std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &str) -> std::io::Result<()> {
        std::fs::write(path, serde_json::to_string(self)?)
    }

    /// Record a time (kept only if it beats the rider's own) and say where this time ranks.
    pub fn record(&mut self, key: &str, e: Entry) -> Rank {
        let list = self.0.entry(key.to_owned()).or_default();
        let day = e.date / 86_400;
        let time = e.time;
        match list.iter_mut().find(|o| o.who == e.who) {
            Some(o) if o.time <= e.time => {} // keep the best
            Some(o) => *o = e,
            None => list.push(e),
        }
        list.sort_by(|a, b| a.time.total_cmp(&b.time));
        list.truncate(KEEP);
        let today: Vec<&Entry> = list.iter().filter(|o| o.date / 86_400 == day).collect();
        Rank {
            rank: list.iter().filter(|o| o.time < time).count() + 1,
            of: list.len(),
            today: today.iter().filter(|o| o.time < time).count() + 1,
            of_today: today.len().max(1),
        }
    }

    /// Wipe a segment's times (moderation). Returns whether it had any.
    pub fn reset(&mut self, key: &str) -> bool {
        self.0.remove(key).is_some()
    }

    /// The fastest `n` overall and today, for the menus.
    pub fn top(&self, key: &str, n: usize, now: u64) -> (Vec<Entry>, Vec<Entry>) {
        let list = self.0.get(key).map(Vec::as_slice).unwrap_or_default();
        let today = list.iter().filter(|o| o.date / 86_400 == now / 86_400).take(n).cloned().collect();
        (list.iter().take(n).cloned().collect(), today)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::course::Point;

    fn course(len: f32, looped: bool, ele: impl Fn(f32) -> f32) -> Course {
        let points = (0..=(len / 10.0) as usize).map(|i| { let d = i as f32 * 10.0; Point { d, lat: 0.0, lon: 0.0, ele: ele(d) } }).collect();
        Course { name: "Col".into(), attribution: String::new(), waypoints: vec![], roads: vec![], looped, points }
    }

    #[test]
    fn climbs_and_loops_get_their_segments() {
        let w = course_windows(&course(5000.0, false, |d| d * 0.08));
        assert_eq!((w.len(), w[0].kind, w[0].d1), (1, Kind::Kom, 5000.0));
        // A loop with a hill between 6 and 7.5 km.
        let w = course_windows(&course(10_000.0, true, |d| if (6000.0..7500.0).contains(&d) { (d - 6000.0) * 0.06 } else if d >= 7500.0 { 90.0 - (d - 7500.0) * 0.036 } else { 0.0 }));
        assert_eq!(w.iter().map(|w| w.kind).collect::<Vec<_>>(), vec![Kind::Sprint, Kind::Kom]);
        assert!((w[1].d0 - 6000.0).abs() <= 50.0, "kom at {}", w[1].d0);
    }

    #[test]
    fn timer_times_a_window_and_voids_jumps() {
        let w = vec![Window { name: "S".into(), kind: Kind::Sprint, d0: 100.0, d1: 200.0 }];
        let mut t = Timer::default();
        let mut done = None;
        for k in 0..300 {
            // 10 m/s, 1 s per update
            if let Some(r) = t.update(&w, k as f32 * 10.0, k as f32) { done = Some(r) }
        }
        assert_eq!(done, Some((0, 10.0)));
        // Teleported into the middle: not a start; teleported out of a running one: void.
        let mut t = Timer::default();
        assert_eq!(t.update(&w, 0.0, 0.0), None);
        assert_eq!(t.update(&w, 150.0, 1.0), None);
        assert!(t.running().is_none());
        t.update(&w, 95.0, 2.0);
        t.update(&w, 105.0, 3.0);
        assert!(t.running().is_some());
        t.update(&w, 500.0, 4.0);
        assert!(t.running().is_none());
    }

    #[test]
    fn boards_keep_each_riders_best() {
        let mut b = Boards::default();
        let e = |who: &str, time: f32| Entry { who: who.into(), name: who.into(), color: "#fff".into(), time, date: 86_400 * 10 };
        assert_eq!(b.record("k", e("a", 60.0)).rank, 1);
        assert_eq!(b.record("k", e("b", 50.0)), Rank { rank: 1, of: 2, today: 1, of_today: 2 });
        assert_eq!(b.record("k", e("a", 70.0)).rank, 3); // slower than both kept times
        assert_eq!(b.0["k"].iter().find(|o| o.who == "a").unwrap().time, 60.0); // best kept
        assert_eq!(b.record("k", e("a", 40.0)).rank, 1);
        assert_eq!(b.top("k", 1, 86_400 * 10).0[0].who, "a");
    }
}
