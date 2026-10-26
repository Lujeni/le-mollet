//! One rider progressing along a course.

use crate::course::Course;
use crate::physics::{self, Rider};
use serde::Serialize;

#[derive(Debug, Default, Clone, Serialize)]
pub struct Ride {
    pub power: f32,
    pub cadence: Option<f32>,
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
}

impl Ride {
    pub fn new(mass_kg: f32) -> Self {
        Self { mass: mass_kg, ..Default::default() }
    }

    /// Advance `dt` seconds using the current `power`. Uses the real road grade:
    /// trainer difficulty only changes what the legs feel, not virtual speed.
    pub fn tick(&mut self, course: &Course, dt: f32) {
        if self.finished {
            return;
        }
        self.grade = course.grade_at(self.distance);
        self.ele = course.ele_at(self.distance);
        let rider = Rider { mass_kg: self.mass, ..Default::default() };
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
        let mut c = Course { name: "8%".into(), attribution: String::new(), waypoints: vec![], roads: vec![], looped: false, points };
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
}
