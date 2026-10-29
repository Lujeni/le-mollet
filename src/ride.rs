//! One rider progressing along a course.

use crate::course::Course;
use crate::physics::{self, Rider};
use serde::Serialize;

/// Road bonuses, like Zwift's power-ups: one is picked up under an arch every `POWERUP_EVERY` metres of course,
/// kept until the rider uses it, then lasts a few seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PowerUp {
    /// 8 kg lighter: climbs faster.
    Feather,
    /// A quarter less drag: faster on the flat.
    Aero,
    /// Half again as much draft behind another rider.
    Draft,
}

pub const POWERUP_EVERY: f32 = 1000.0;
const POWERUPS: [PowerUp; 4] = [PowerUp::Feather, PowerUp::Aero, PowerUp::Draft, PowerUp::Aero];

impl PowerUp {
    pub fn secs(self) -> f32 {
        match self {
            PowerUp::Draft => 30.0,
            _ => 15.0,
        }
    }
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Ride {
    pub power: f32,
    pub cadence: Option<f32>,
    /// Heart rate from a strap, bpm.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hr: Option<f32>,
    /// Share of air drag saved by the rider ahead (0 alone, 0.3 on a wheel).
    pub draft: f32,
    pub speed_kmh: f32,
    pub distance: f32,
    pub elapsed: f32,
    pub grade: f32,
    pub ele: f32,
    pub mass: f32,
    pub finished: bool,
    /// Ride totals, counted while the clock runs. `ridden` excludes demo teleports,
    /// unlike `distance` which is the position on the course.
    pub ridden: f32,
    /// Completed laps on a looped course.
    pub laps: u32,
    pub energy_kj: f32,
    pub max_power: f32,
    pub climbed: f32,
    /// Road bonuses, when this ride has them (not in workouts nor against the zombies).
    #[serde(skip)]
    pub powerups: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub held: Option<PowerUp>,
    /// The bonus in use and its seconds left.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<(PowerUp, f32)>,
    /// Bonus arches passed so far (`distance / POWERUP_EVERY` at the last tick).
    #[serde(skip)]
    pub arch: u32,
}

impl Ride {
    pub fn new(mass_kg: f32) -> Self {
        Self { mass: mass_kg, ..Default::default() }
    }

    /// Use the held bonus, if any and none is running.
    pub fn use_powerup(&mut self) {
        if self.active.is_none()
            && let Some(p) = self.held.take()
        {
            self.active = Some((p, p.secs()));
        }
    }

    /// Advance `dt` seconds using the current `power`. Uses the real road grade:
    /// trainer difficulty only changes what the legs feel, not virtual speed.
    pub fn tick(&mut self, course: &Course, dt: f32) {
        if self.finished {
            return;
        }
        self.grade = course.grade_at(self.distance);
        self.ele = course.ele_at(self.distance);
        let base = Rider::default();
        let on = |p| self.active.is_some_and(|(a, _)| a == p);
        let draft = if on(PowerUp::Draft) { self.draft * 1.5 } else { self.draft };
        let mass_kg = if on(PowerUp::Feather) { self.mass - 8.0 } else { self.mass };
        let cda = base.cda * (1.0 - draft) * if on(PowerUp::Aero) { 0.75 } else { 1.0 };
        let rider = Rider { mass_kg, cda, ..base };
        let v = physics::step(self.speed_kmh / 3.6, self.power, self.grade, self.ele, &rider, dt);
        let len = course.length();
        let step = if course.looped { v * dt } else { (self.distance + v * dt).min(len) - self.distance };
        self.distance += step;
        self.ridden += step;
        if course.looped && self.distance >= len {
            self.distance -= len;
            self.laps += 1;
        }
        self.climbed += (course.ele_at(self.distance) - self.ele).max(0.0);
        if self.distance > 0.0 {
            self.elapsed += dt;
            self.energy_kj += self.power * dt / 1000.0;
            self.max_power = self.max_power.max(self.power);
        }
        self.speed_kmh = v * 3.6;
        if let Some((p, left)) = self.active {
            self.active = (left > dt).then_some((p, left - dt));
        }
        // An arch every POWERUP_EVERY m: riding under the next one gives a bonus (not a teleport, nor a new lap).
        let arch = (self.distance / POWERUP_EVERY) as u32;
        if self.powerups && arch == self.arch + 1 && self.held.is_none() {
            self.held = Some(POWERUPS[arch as usize % POWERUPS.len()]);
        }
        self.arch = arch;
        self.finished = !course.looped && self.distance >= len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::course::Point;

    #[test]
    fn climbs_to_the_finish() {
        // 1 km at 8 %
        let points = (0..=100).map(|i| Point { d: i as f32 * 10.0, lat: 0.0, lon: 0.0, ele: i as f32 * 0.8 }).collect();
        let mut c = Course { name: "8%".into(), attribution: String::new(), waypoints: vec![], roads: vec![], looped: false, points, ..Default::default() };
        let mut r = Ride::new(78.0);
        r.power = 250.0;
        for _ in 0..10_000 {
            r.tick(&c, 0.1);
        }
        assert!(r.finished);
        assert_eq!(r.distance, 1000.0);
        assert!((r.ridden - 1000.0).abs() < 1e-2);
        // ~13 km/h at 250 W on 8 % for 78 kg: about 4.5 minutes
        assert!((240.0..320.0).contains(&r.elapsed), "elapsed {}", r.elapsed);
        assert!((r.climbed - 80.0).abs() < 0.5, "climbed {}", r.climbed);
        assert!((r.energy_kj - 0.25 * r.elapsed).abs() < 0.1, "energy {}", r.energy_kj);
        assert_eq!(r.max_power, 250.0);
        let t = r.elapsed;
        r.tick(&c, 0.1);
        assert_eq!(r.elapsed, t); // frozen after the finish

        // Same road as a loop: laps count, no finish, ridden keeps growing.
        c.looped = true;
        let mut r = Ride::new(78.0);
        r.power = 400.0;
        for _ in 0..20_000 {
            r.tick(&c, 0.1);
        }
        assert!(!r.finished && r.laps >= 2, "laps {}", r.laps);
        let expected = r.laps as f32 * 1000.0 + r.distance; // f32 sums drift a little over 20k steps
        assert!((r.ridden - expected).abs() < expected * 0.005, "ridden {} vs {expected}", r.ridden);
    }

    #[test]
    fn powerups_are_picked_under_the_arches_and_last() {
        let points = (0..=500).map(|i| Point { d: i as f32 * 10.0, lat: 0.0, lon: 0.0, ele: 0.0 }).collect();
        let c = Course { name: "flat".into(), attribution: String::new(), waypoints: vec![], roads: vec![], looped: false, points, ..Default::default() };
        let mut r = Ride { powerups: true, power: 250.0, ..Ride::new(78.0) };
        while r.distance < 1100.0 {
            r.tick(&c, 0.1)
        }
        assert_eq!((r.held, r.active), (Some(PowerUp::Aero), None)); // arch 1
        let v = r.speed_kmh;
        r.use_powerup();
        assert_eq!(r.held, None);
        for _ in 0..100 {
            r.tick(&c, 0.1)
        }
        assert!(r.speed_kmh > v + 1.0, "aero {v} -> {}", r.speed_kmh);
        for _ in 0..60 {
            r.tick(&c, 0.1)
        }
        assert_eq!(r.active, None); // 15 s gone
        // A teleport over several arches gives nothing; no bonuses at all when the ride has none.
        r.distance = 4500.0;
        r.tick(&c, 0.1);
        assert_eq!(r.held, None);
        let mut w = Ride { power: 250.0, ..Ride::new(78.0) };
        while w.distance < 1100.0 {
            w.tick(&c, 0.1)
        }
        assert_eq!(w.held, None);
    }
}
