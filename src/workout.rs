//! Structured workouts: steps in % of FTP, played back in ERG mode (trainer holds
//! the target) or as a target to follow in demo mode.

use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Step {
    pub secs: u32,
    /// Start and end target in % of FTP (equal for a steady step, different for a ramp).
    pub from: f32,
    pub to: f32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Workout {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub steps: Vec<Step>,
}

/// Where a workout stands at a given time.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Progress {
    pub step: usize,
    pub step_left: f32,
    pub target: f32,
    pub done: bool,
}

const fn steady(min: u32, pct: f32) -> Step {
    Step { secs: min * 60, from: pct, to: pct }
}
const fn ramp(min: u32, from: f32, to: f32) -> Step {
    Step { secs: min * 60, from, to }
}
fn repeat(n: usize, block: &[Step]) -> Vec<Step> {
    block.iter().copied().cycle().take(n * block.len()).collect()
}

impl Workout {
    pub fn duration(&self) -> u32 {
        self.steps.iter().map(|s| s.secs).sum()
    }

    /// Target watts at `t` seconds for a rider with `ftp`.
    pub fn progress(&self, t: f32, ftp: f32) -> Progress {
        let mut start = 0.0;
        for (i, s) in self.steps.iter().enumerate() {
            let end = start + s.secs as f32;
            if t < end {
                let k = (t - start) / s.secs as f32;
                let pct = s.from + (s.to - s.from) * k;
                return Progress { step: i, step_left: end - t, target: (pct / 100.0 * ftp).round(), done: false };
            }
            start = end;
        }
        let last = self.steps.len() - 1;
        Progress { step: last, step_left: 0.0, target: 0.0, done: true }
    }
}

/// Built-in sessions, classic structures expressed in % of FTP.
pub fn builtin() -> Vec<Workout> {
    vec![
        Workout {
            id: "recup",
            name: "Récup active",
            description: "30 min très facile pour faire tourner les jambes.",
            steps: vec![ramp(5, 40.0, 55.0), steady(20, 55.0), ramp(5, 55.0, 40.0)],
        },
        Workout {
            id: "endurance",
            name: "Endurance",
            description: "1 h en zone 2, la base de tout.",
            steps: vec![ramp(10, 45.0, 65.0), steady(40, 68.0), ramp(10, 65.0, 45.0)],
        },
        Workout {
            id: "sweet-spot",
            name: "Sweet spot 3×10",
            description: "3 blocs de 10 min à 90 % : le meilleur rapport effort / progrès.",
            steps: [vec![ramp(10, 45.0, 75.0)], repeat(3, &[steady(10, 90.0), steady(5, 55.0)]), vec![ramp(5, 55.0, 40.0)]].concat(),
        },
        Workout {
            id: "seuil",
            name: "Seuil 2×20",
            description: "2 × 20 min au seuil pour repousser ta FTP.",
            steps: [vec![ramp(15, 45.0, 80.0)], repeat(2, &[steady(20, 98.0), steady(5, 55.0)]), vec![ramp(10, 55.0, 40.0)]].concat(),
        },
        Workout {
            id: "vo2max",
            name: "VO2max 5×3",
            description: "5 × 3 min à 118 % : court, intense, efficace.",
            steps: [vec![ramp(12, 45.0, 85.0)], repeat(5, &[steady(3, 118.0), steady(3, 50.0)]), vec![ramp(8, 55.0, 40.0)]].concat(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_and_targets() {
        let w = builtin();
        let ss = w.iter().find(|w| w.id == "sweet-spot").unwrap();
        assert_eq!(ss.duration(), 60 * 60);
        // Ramp 45 -> 75 % over 10 min: halfway is 60 %.
        assert_eq!(ss.progress(300.0, 250.0).target, 150.0);
        // First 90 % block starts at 10 min.
        let p = ss.progress(600.0 + 30.0, 250.0);
        assert_eq!((p.step, p.target, p.step_left), (1, 225.0, 570.0));
        // Recovery between blocks.
        assert_eq!(ss.progress(60.0 * 21.0, 250.0).target, 138.0);
        assert!(ss.progress(3600.0, 250.0).done);
        assert!(w.iter().all(|w| (30 * 60..=90 * 60).contains(&w.duration())));
    }
}
