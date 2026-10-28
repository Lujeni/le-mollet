//! Fair play: what the server knows of each rider, and how it checks the power a ride reports (docs/security.md).
//! Watts come from a trainer the browser reads over Bluetooth: nothing proves them. So the server keeps the
//! rider's weight and rank itself, checks each second of power for what no human or trainer does, and only
//! lets rides it believes into leaderboards and race results. Kept in `riders.json` next to the courses.

use crate::event::category;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const DAY: u64 = 86_400;
/// Weight moves at most `WEIGHT_STEP` kg a week, so nobody turns 40 kg for a race; free on the first day.
const WEIGHT_STEP: f32 = 2.0;
/// Rank: the best of the declared FTP and the one seen in rides over this many days.
const SEEN_DAYS: u64 = 60;
/// Anaerobic capacity for the W′ check, J. A generous one: only the impossible is flagged.
const W_PRIME: f32 = 25_000.0;
/// A bike, kg.
pub const BIKE_KG: f32 = 8.0;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Riders {
    #[serde(default)]
    pub riders: HashMap<String, Rider>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rider {
    pub weight: f32,
    /// Weight a week ago, and since when: the next change stays within `WEIGHT_STEP` of it.
    pub anchor: f32,
    pub anchor_at: u64,
    pub created: u64,
    /// Declared FTP, W.
    pub ftp: f32,
    /// Strava athlete who owns this profile, in cloud mode: no one else may use it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<u64>,
    /// FTP seen in rides, W, with when: 95 % of a 20 min best, or a race's best effort.
    #[serde(default)]
    pub seen: Vec<(u64, f32)>,
    /// Rides the checks flagged, newest last, for the moderators.
    #[serde(default)]
    pub flags: Vec<Flagged>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Flagged {
    pub at: u64,
    pub track: String,
    pub name: String,
    pub why: Vec<String>,
}

impl Rider {
    pub fn seen_ftp(&self, now: u64) -> f32 {
        self.seen.iter().filter(|s| s.0 + SEEN_DAYS * DAY >= now).map(|s| s.1).fold(0.0, f32::max)
    }

    /// FTP the server goes by: never below what it saw.
    pub fn ftp_now(&self, now: u64) -> f32 {
        self.ftp.max(self.seen_ftp(now))
    }

    pub fn wkg(&self, now: u64) -> f32 {
        self.ftp_now(now) / self.weight
    }

    pub fn rank(&self, now: u64) -> char {
        category(self.wkg(now))
    }
}

impl Riders {
    /// A missing or broken file is an empty one.
    pub fn load(path: &str) -> Self {
        std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &str) -> Result<()> {
        std::fs::write(path, serde_json::to_string(self)?).with_context(|| format!("write {path}"))
    }

    /// Whoever asks with this profile id may use it: anyone at home, its owner only in cloud mode.
    pub fn may_use(&self, pid: &str, athlete: Option<u64>) -> bool {
        self.riders.get(pid).and_then(|r| r.owner).is_none_or(|o| Some(o) == athlete)
    }

    /// Create or update a rider. The weight moves slowly (see `WEIGHT_STEP`); the FTP is free, it can only
    /// raise the rank, never lower it below what rides showed.
    pub fn register(&mut self, pid: &str, owner: Option<u64>, weight: f32, ftp: f32, now: u64) -> Result<&Rider, &'static str> {
        if !(35.0..=150.0).contains(&weight) { return Err("weight: 35 to 150 kg") }
        if !(50.0..=600.0).contains(&ftp) { return Err("ftp: 50 to 600 W") }
        if !self.may_use(pid, owner) { return Err("this profile belongs to another account") }
        let r = self.riders.entry(pid.into()).or_insert_with(|| Rider { weight, anchor: weight, anchor_at: now, created: now, ftp, owner, seen: vec![], flags: vec![] });
        if now >= r.anchor_at + 7 * DAY { (r.anchor, r.anchor_at) = (r.weight, now) }
        if now >= r.created + DAY && (weight - r.anchor).abs() > WEIGHT_STEP + 1e-3 { return Err("weight: at most 2 kg a week") }
        (r.weight, r.ftp) = (weight, ftp);
        if now < r.created + DAY { (r.anchor, r.anchor_at) = (weight, now) }
        r.owner = r.owner.or(owner);
        Ok(r)
    }

    /// A ride is over: what it showed of the rider's FTP, and why it was flagged, if it was.
    pub fn ride_done(&mut self, pid: &str, seen_ftp: Option<f32>, flagged: Option<Flagged>, now: u64) {
        let Some(r) = self.riders.get_mut(pid) else { return };
        if let Some(f) = seen_ftp { r.seen.push((now, f)) }
        r.seen.retain(|s| s.0 + SEEN_DAYS * DAY >= now);
        if let Some(f) = flagged {
            r.flags.push(f);
            if r.flags.len() > 20 { r.flags.remove(0); }
        }
    }
}

/// How far a ride's power is trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Trust {
    /// The demo slider, or no profile: a ride for fun, out of every ranking.
    Demo,
    /// Something no rider or trainer does: kept out of rankings until a moderator looks.
    Flagged,
    /// A trainer, and nothing odd.
    Device,
}

/// Checks a ride's power second by second.
#[derive(Debug)]
pub struct Check {
    /// FTP and weight it is judged against, W and kg.
    cp: f32,
    pub kg: f32,
    /// FTP a race finish showed (`effort_ftp`), set by the ride.
    pub race_ftp: Option<f32>,
    /// Average power of each full second.
    pub secs: Vec<f32>,
    part: (f32, f32),
    demo: bool,
    cadence_seen: bool,
    no_cadence: u32,
    flat: (f32, u32),
    wbal: f32,
    flags: Vec<&'static str>,
}

impl Check {
    /// No profile: a demo ride.
    pub fn new(ftp: Option<f32>, kg: f32) -> Self {
        Self { cp: ftp.unwrap_or(0.0), kg, race_ftp: None, secs: vec![], part: (0.0, 0.0), demo: ftp.is_none(), cadence_seen: false, no_cadence: 0, flat: (0.0, 0), wbal: W_PRIME, flags: vec![] }
    }

    fn flag(&mut self, why: &'static str) {
        if !self.flags.contains(&why) { self.flags.push(why) }
    }

    /// `dt` s of riding at `w` W. `measured`: from a trainer (the server's, or the browser's own), not the slider.
    pub fn tick(&mut self, w: f32, cadence: Option<f32>, measured: bool, dt: f32) {
        if w > 0.0 && !measured { self.demo = true }
        if cadence.is_some_and(|c| c > 0.0) { self.cadence_seen = true }
        self.part = (self.part.0 + w * dt, self.part.1 + dt);
        if self.part.1 + 1e-4 < 1.0 { return }
        let s = self.part.0 / self.part.1;
        self.part = (0.0, 0.0);
        let last = self.secs.last().copied();
        self.secs.push(s);
        // A real trainer reads a few watts off every second; a typed number holds still.
        if last.is_some_and(|l| (s - l).abs() > 1000.0) { self.flag("spike") }
        if s > 50.0 && (s - self.flat.0).abs() < 0.01 { self.flat.1 += 1 } else { self.flat = (s, 1) }
        if self.flat.1 >= 120 { self.flag("flat") }
        if self.cadence_seen && s > 250.0 && cadence.is_none_or(|c| c <= 0.0) { self.no_cadence += 1 } else { self.no_cadence = 0 }
        if self.no_cadence >= 30 { self.flag("no cadence") }
        if s / self.kg > 25.0 { self.flag("1 s over 25 W/kg") }
        // W′ balance (Skiba): above the FTP the reserve drains, below it refills. Far below empty is not human.
        if self.cp > 0.0 {
            self.wbal += if s > self.cp { -(s - self.cp) } else { (W_PRIME - self.wbal) * (self.cp - s) / W_PRIME };
            if self.wbal < -0.5 * W_PRIME { self.flag("beyond W′") }
        }
        if self.secs.len().is_multiple_of(60) {
            if self.best(1200).is_some_and(|b| b / self.kg > 6.5) { self.flag("20 min over 6.5 W/kg") }
            if self.best(300).is_some_and(|b| b / self.kg > 8.0) { self.flag("5 min over 8 W/kg") }
        }
    }

    /// Best average power over `n` s, if the ride lasted that long.
    pub fn best(&self, n: usize) -> Option<f32> {
        if n == 0 || self.secs.len() < n { return None }
        let mut sum: f32 = self.secs[..n].iter().sum();
        let mut best = sum;
        for i in n..self.secs.len() {
            sum += self.secs[i] - self.secs[i - n];
            best = best.max(sum);
        }
        Some(best / n as f32)
    }

    pub fn trust(&self) -> Trust {
        if self.demo { Trust::Demo } else if self.flags.is_empty() { Trust::Device } else { Trust::Flagged }
    }

    pub fn why(&self) -> Vec<String> {
        if self.demo { vec!["demo".into()] } else { self.flags.iter().map(|f| f.to_string()).collect() }
    }

    /// FTP this ride shows: 95 % of its best 20 min. None for a demo or a short ride.
    pub fn seen_ftp(&self) -> Option<f32> {
        self.best(1200).filter(|_| !self.demo).map(|b| b * 0.95)
    }

    /// FTP an effort of `secs` s shows, for a race shorter than 20 min: 95 % of its best (an underestimate,
    /// on purpose: only clearly too strong riders are moved up).
    pub fn effort_ftp(&self, secs: usize) -> Option<f32> {
        self.best(secs.clamp(300, 1200)).filter(|_| !self.demo).map(|b| b * 0.95)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weight_moves_slowly_and_rank_never_drops_below_rides() {
        let mut rs = Riders::default();
        let t0 = 1_000 * DAY;
        rs.register("a", None, 70.0, 200.0, t0).unwrap();
        rs.register("a", None, 60.0, 200.0, t0 + 3600).unwrap(); // first day: free
        assert_eq!(rs.register("a", None, 55.0, 200.0, t0 + 2 * DAY).unwrap_err(), "weight: at most 2 kg a week");
        rs.register("a", None, 58.0, 200.0, t0 + 2 * DAY).unwrap();
        assert!(rs.register("a", None, 56.5, 200.0, t0 + 3 * DAY).is_err()); // 2 kg steps don't add up within a week
        rs.register("a", None, 56.5, 200.0, t0 + 9 * DAY).unwrap();
        // Sandbagging: declare a low FTP, the rides still count.
        rs.ride_done("a", Some(240.0), None, t0 + 9 * DAY);
        rs.register("a", None, 56.5, 150.0, t0 + 10 * DAY).unwrap();
        let r = &rs.riders["a"];
        assert_eq!((r.ftp_now(t0 + 10 * DAY), r.rank(t0 + 10 * DAY)), (240.0, 'D')); // 4.2 W/kg: Diamant
        assert_eq!(r.ftp_now(t0 + 80 * DAY), 150.0); // two months on, the ride is forgotten

        // Cloud mode: a profile is its owner's.
        rs.register("b", Some(7), 70.0, 200.0, t0).unwrap();
        assert!(rs.may_use("b", Some(7)) && !rs.may_use("b", Some(8)) && !rs.may_use("b", None));
        assert!(rs.register("b", Some(8), 70.0, 200.0, t0).is_err());
    }

    fn ride(watts: impl Fn(usize) -> f32, secs: usize, cad: Option<f32>, ftp: f32) -> Check {
        let mut c = Check::new(Some(ftp), 70.0);
        for i in 0..secs * 10 { c.tick(watts(i / 10), cad, true, 0.1) }
        c
    }

    #[test]
    fn honest_rides_pass_and_impossible_ones_are_flagged() {
        // 30 min around 250 W with the usual wobble, at an FTP of 260 W.
        let honest = ride(|s| 250.0 + (s as f32 * 0.7).sin() * 12.0, 1800, Some(90.0), 260.0);
        assert_eq!(honest.trust(), Trust::Device, "{:?}", honest.why());
        assert!((honest.seen_ftp().unwrap() - 237.5).abs() < 2.0);

        // A number typed in: dead flat.
        assert_eq!(ride(|_| 300.0, 300, Some(90.0), 260.0).why(), ["flat"]);
        // 450 W for 20 min at 70 kg: beyond the reserve of an FTP of 260 W, and over 6 W/kg for long.
        let rocket = ride(|s| 450.0 + (s as f32).sin() * 10.0, 1200, Some(95.0), 260.0);
        assert!(rocket.why().contains(&"beyond W′".to_string()), "{:?}", rocket.why());
        // Watts without legs.
        assert!(ride(|s| 300.0 + (s % 7) as f32, 60, None, 400.0).why().is_empty()); // no cadence sensor: fine
        let mut c = Check::new(Some(400.0), 70.0);
        for i in 0..600 { c.tick(300.0 + (i % 7) as f32, Some(if i < 100 { 90.0 } else { 0.0 }), true, 0.1) }
        assert_eq!(c.why(), ["no cadence"]);
        // The slider, or no profile: demo.
        let mut d = Check::new(Some(260.0), 70.0);
        d.tick(200.0, None, false, 1.0);
        assert_eq!((d.trust(), d.seen_ftp()), (Trust::Demo, None));
        assert_eq!(Check::new(None, 70.0).trust(), Trust::Demo);
        assert_eq!(honest.best(1).map(|b| b > 250.0), Some(true));
    }
}
