//! Zombie Halloween: a horde chases the rider along any road. Each wave makes it faster,
//! mini sprints and road bonuses push it back, and the game ends when it catches up.
//! Everything is scaled on the rider's FTP, and the horde rides the same physics on the
//! same grade as the rider, so a climb is no harder to survive than a flat.

use crate::physics::{self, Rider};
use crate::ride::Ride;
use serde::Serialize;

const START_GAP: f32 = 150.0;
/// Banking more lead than this is pointless: the horde is always in sight.
const MAX_GAP: f32 = 400.0;
const WAVE_SECS: f32 = 60.0;
const FIRST_SPRINT: f32 = 45.0;
const SPRINT_EVERY: f32 = 75.0;
const SPRINT_SECS: f32 = 15.0;
/// Seconds at or above the target, within the sprint, to win it.
const SPRINT_HOLD: f32 = 8.0;
const SPRINT_PUSH: f32 = 100.0;
const BONUS_EVERY: f32 = 500.0;
const PUMPKIN_PUSH: f32 = 60.0;
const BOOST_SECS: f32 = 10.0;
const BOOST: f32 = 1.5;
/// Where the horde is thrown back to when the shield saves the rider.
const SHIELD_GAP: f32 = 80.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Bonus {
    /// Garlic-stuffed pumpkin: the horde falls back.
    Pumpkin,
    /// Rider counts as faster against the horde for a few seconds.
    Boost,
    /// Survive one catch.
    Shield,
}
const BONUSES: [Bonus; 4] = [Bonus::Pumpkin, Bonus::Boost, Bonus::Pumpkin, Bonus::Shield];

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Sprint {
    pub target: f32,
    pub left: f32,
    /// Seconds spent at or above the target so far, out of `need`.
    pub held: f32,
    pub need: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Game {
    #[serde(skip)]
    ftp: f32,
    #[serde(skip)]
    horde_v: f32,
    /// Meters between the rider and the horde.
    pub gap: f32,
    pub wave: u32,
    pub horde_kmh: f32,
    pub sprint: Option<Sprint>,
    /// Game clock (riding time) at which the next sprint starts.
    pub next_sprint: f32,
    pub sprints: u32,
    pub sprints_won: u32,
    /// Next bonus on the road, picked up when `ridden` reaches `bonus_at`.
    pub bonus: Bonus,
    pub bonus_at: f32,
    pub picked: u32,
    pub boost_left: f32,
    pub shield: bool,
    pub over: bool,
}

impl Game {
    pub fn new(ftp: f32) -> Self {
        Self {
            ftp,
            horde_v: 0.0,
            gap: START_GAP,
            wave: 1,
            horde_kmh: 0.0,
            sprint: None,
            next_sprint: FIRST_SPRINT,
            sprints: 0,
            sprints_won: 0,
            bonus: BONUSES[0],
            bonus_at: BONUS_EVERY / 2.0,
            picked: 0,
            boost_left: 0.0,
            shield: false,
            over: false,
        }
    }

    /// Horde power for a wave: 50 % of FTP, +8 % per minute, capped at 160 % so it always wins.
    pub fn horde_power(&self, wave: u32) -> f32 {
        self.ftp * (0.5 + 0.08 * (wave - 1) as f32).min(1.6)
    }

    /// Sprint target: 120 % of FTP, +5 % each sprint, capped at 180 %.
    pub fn sprint_target(&self, n: u32) -> f32 {
        (self.ftp * (1.2 + 0.05 * n as f32).min(1.8)).round()
    }

    /// Advance `dt` seconds of riding. The game clock is the ride's riding time.
    pub fn tick(&mut self, ride: &Ride, dt: f32) {
        if self.over {
            return;
        }
        let t = ride.elapsed;
        self.wave = 1 + (t / WAVE_SECS) as u32;
        // ponytail: the horde rides the rider's grade, not its own spot on the road; fine within MAX_GAP
        let horde = Rider { mass_kg: ride.mass, ..Default::default() };
        self.horde_v = physics::step(self.horde_v, self.horde_power(self.wave), ride.grade, ride.ele, &horde, dt);
        self.horde_kmh = self.horde_v * 3.6;
        let mut v = ride.speed_kmh / 3.6;
        if self.boost_left > 0.0 {
            v *= BOOST;
            self.boost_left = (self.boost_left - dt).max(0.0);
        }
        self.gap = (self.gap + (v - self.horde_v) * dt).min(MAX_GAP);

        match self.sprint.as_mut() {
            None if t >= self.next_sprint => {
                self.sprint = Some(Sprint { target: self.sprint_target(self.sprints), left: SPRINT_SECS, held: 0.0, need: SPRINT_HOLD });
                self.sprints += 1;
                self.next_sprint += SPRINT_EVERY;
            }
            Some(s) => {
                if ride.power >= s.target {
                    s.held += dt;
                }
                s.left -= dt;
                if s.held >= s.need {
                    self.sprints_won += 1;
                    self.gap = (self.gap + SPRINT_PUSH).min(MAX_GAP);
                    self.sprint = None;
                } else if s.left <= 0.0 {
                    self.sprint = None;
                }
            }
            None => {}
        }

        if ride.ridden >= self.bonus_at {
            match self.bonus {
                Bonus::Pumpkin => self.gap = (self.gap + PUMPKIN_PUSH).min(MAX_GAP),
                Bonus::Boost => self.boost_left = BOOST_SECS,
                Bonus::Shield => self.shield = true,
            }
            self.picked += 1;
            self.bonus = BONUSES[self.picked as usize % BONUSES.len()];
            self.bonus_at += BONUS_EVERY;
        }

        if self.gap <= 0.0 {
            if self.shield {
                self.shield = false;
                self.gap = SHIELD_GAP;
            } else {
                self.gap = 0.0;
                self.over = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn riding(power: f32, speed_kmh: f32) -> Ride {
        Ride { power, speed_kmh, mass: 83.0, ..Default::default() }
    }

    /// Play `secs` seconds at a steady pace on the flat, moving `ride` along.
    fn play(g: &mut Game, ride: &mut Ride, secs: f32) {
        for _ in 0..(secs * 10.0) as u32 {
            ride.elapsed += 0.1;
            ride.ridden += ride.speed_kmh / 3.6 * 0.1;
            g.tick(ride, 0.1);
        }
    }

    #[test]
    fn waves_ramp_up_and_cap() {
        let g = Game::new(250.0);
        assert_eq!(g.horde_power(1), 125.0);
        assert_eq!(g.horde_power(6), 225.0);
        assert_eq!(g.horde_power(50), 400.0);
        assert_eq!((g.sprint_target(0), g.sprint_target(4), g.sprint_target(40)), (300.0, 350.0, 450.0));
    }

    #[test]
    fn standing_still_gets_caught() {
        let (mut g, mut r) = (Game::new(250.0), riding(0.0, 0.0));
        // The wave 1 horde (125 W) closes 150 m in well under a minute.
        play(&mut g, &mut r, 59.0);
        assert!(g.over && g.gap == 0.0 && g.wave == 1);
        // Game over freezes the game.
        let before = g.clone();
        play(&mut g, &mut r, 10.0);
        assert_eq!((g.wave, g.gap), (before.wave, before.gap));
    }

    #[test]
    fn shield_saves_once() {
        let (mut g, mut r) = (Game::new(250.0), riding(0.0, 0.0));
        g.shield = true;
        g.gap = 0.05;
        r.elapsed = 600.0;
        g.horde_v = 10.0;
        g.tick(&r, 0.1);
        assert!(!g.over && !g.shield && g.gap == SHIELD_GAP);
    }

    #[test]
    fn sprints_and_bonuses_push_the_horde_back() {
        // Fast enough to hold the horde in the first waves, sprint power above every target.
        let (mut g, mut r) = (Game::new(250.0), riding(500.0, 40.0));
        play(&mut g, &mut r, FIRST_SPRINT + 1.0);
        assert_eq!(g.sprint.map(|s| s.target), Some(300.0));
        play(&mut g, &mut r, SPRINT_HOLD);
        assert_eq!((g.sprints, g.sprints_won, g.sprint), (1, 1, None));
        // Missed sprint: power under the target for the whole window.
        r.power = 100.0;
        play(&mut g, &mut r, SPRINT_EVERY - SPRINT_HOLD + SPRINT_SECS);
        assert_eq!((g.sprints, g.sprints_won, g.sprint), (2, 1, None));
        // 40 km/h for ~2 min = ~1.4 km: pumpkin at 250 m, boost at 750 m, pumpkin at 1250 m.
        assert_eq!(g.picked, 3);
        assert_eq!((g.bonus, g.bonus_at), (Bonus::Shield, 1750.0));
        assert!(!g.over && g.gap == MAX_GAP);
    }
}
