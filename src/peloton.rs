//! The tactical peloton: riders of the server's own in every race, with a profile, a reserve of energy and a
//! plan, like a manager game. A few go in the early break, the bunch lets them dangle then chases, the puncheurs
//! and climbers attack in the last kilometres, the sprinters wait for the last 600 m. They draft like everyone,
//! tire like everyone (W′ balance), and reach the finish like everyone. The race radio tells what they do.
//! Pure logic: the server task in src/web.rs feeds each bot's grade and distance and takes its watts back.

use crate::event::{Entrant, Event, Kind, category};
use serde::{Deserialize, Serialize};

/// Bots in a race, when none were asked.
pub const FIELD: usize = 15;
/// Simulation step of the tests, s (the server's tick).
#[cfg(test)]
const DT: f32 = 0.1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    /// Strong when it climbs, weak on the flat.
    Grimpeur,
    /// A big last 600 m, suffers on climbs.
    Sprinteur,
    /// Steady and strong on the flat: the break's engine.
    Rouleur,
    /// Short steep efforts: the late attacks.
    Puncheur,
}

impl Profile {
    pub fn label(self) -> &'static str {
        match self { Profile::Grimpeur => "grimpeur", Profile::Sprinteur => "sprinteur", Profile::Rouleur => "rouleur", Profile::Puncheur => "puncheur" }
    }

    /// Share of the FTP this rider sustains on a road of `grade` %, relative to a plain rider.
    fn on(self, grade: f32) -> f32 {
        match self {
            Profile::Grimpeur => if grade > 3.0 { 1.08 } else if grade < 1.0 { 0.95 } else { 1.0 },
            Profile::Sprinteur => if grade > 3.0 { 0.9 } else { 1.03 },
            Profile::Rouleur => if grade < 2.0 { 1.05 } else { 0.98 },
            Profile::Puncheur => if grade > 5.0 { 1.1 } else { 1.0 },
        }
    }

    /// Sprint power, times the FTP.
    fn sprint(self) -> f32 {
        match self { Profile::Sprinteur => 2.6, Profile::Puncheur => 2.1, Profile::Rouleur => 1.9, Profile::Grimpeur => 1.7 }
    }

    /// Anaerobic reserve W′, J.
    fn wprime(self) -> f32 {
        match self { Profile::Sprinteur => 26_000.0, Profile::Puncheur => 24_000.0, Profile::Rouleur => 20_000.0, Profile::Grimpeur => 18_000.0 }
    }
}

const FIRST: [&str; 20] = ["Léa", "Nino", "Camille", "Sacha", "Inès", "Louis", "Maëlle", "Tom", "Zoé", "Hugo", "Jade", "Eliott", "Lina", "Gabin", "Anna", "Marius", "Chloé", "Noé", "Romy", "Basile"];
const LAST: [&str; 20] = ["Morel", "Garcia", "Petit", "Roux", "Fontaine", "Lemaire", "Dubois", "Girard", "Blanc", "Perrin", "Leroy", "Marchand", "Renard", "Vidal", "Lambert", "Bonnet", "Caron", "Gauthier", "Aubert", "Masson"];
/// Teams: a name and a jersey colour, in Lego's palette.
pub const TEAMS: [(&str, &str); 4] = [("Brique Rouge", "#c91a09"), ("Bleu Azur", "#0055bf"), ("Vert Sapin", "#237841"), ("Orange Vif", "#fe8a18")];
const COUNTRIES: [&str; 8] = ["FR", "FR", "FR", "BE", "ES", "IT", "NL", "CH"];

/// A small deterministic generator: the same seed gives the same field, so a race's bots are the same for
/// everyone and across restarts.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f32 / (1u64 << 53) as f32
    }
    fn pick<T: Copy>(&mut self, v: &[T]) -> T {
        v[(self.next() * v.len() as f32) as usize % v.len()]
    }
}

fn seed_of(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)) | 1
}

/// Sign `n` bots up to a race, as entrants anyone can see in the lobby: their W/kg fits the race's ranks (or
/// the humans' level), their names and teams come from the race id. Does nothing for group rides or a race
/// that has its bots already.
pub fn enroll(e: &mut Event, n: usize) {
    if e.kind != Kind::Race || e.entrants.iter().any(|x| x.bot) { return }
    let mut rng = Rng(seed_of(&e.id));
    // The rank's W/kg band; several ranks: the whole span; none: around the humans, else Or.
    let band = |c: char| crate::event::TIERS.iter().position(|t| t.0 == c).map(|i| (crate::event::TIERS[i].1, if i == 0 { crate::event::TIERS[0].1 + 0.6 } else { crate::event::TIERS[i - 1].1 }));
    let bands: Vec<(f32, f32)> = e.cats.chars().filter_map(band).collect();
    let (lo, hi) = if !bands.is_empty() {
        (bands.iter().map(|b| b.0).fold(f32::MAX, f32::min).max(1.2), bands.iter().map(|b| b.1).fold(0.0, f32::max))
    } else if !e.entrants.is_empty() {
        let m = e.entrants.iter().map(|x| x.wkg).sum::<f32>() / e.entrants.len() as f32;
        (m * 0.85, m * 1.15)
    } else {
        (2.5, 3.2)
    };
    for i in 0..n {
        let wkg = lo + (hi - lo) * (0.1 + 0.8 * rng.next());
        let name = format!("{} {}", rng.pick(&FIRST), rng.pick(&LAST));
        e.entrants.push(Entrant { pid: format!("bot:{}:{i}", e.id), name, wkg, cat: category(wkg), bot: true });
    }
}

/// One of the server's riders in a race.
#[derive(Debug, Clone)]
pub struct Bot {
    pub pid: String,
    pub name: String,
    pub country: &'static str,
    pub team: usize,
    pub profile: Profile,
    pub mass: f32,
    pub ftp: f32,
    wprime: f32,
    wbal: f32,
    role: Role,
    /// An attack in progress: until this race second.
    attack_until: f32,
    pub finished: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Break,
    Bunch,
}

impl Bot {
    fn from(x: &Entrant, rng: &mut Rng) -> Self {
        let profile = rng.pick(&[Profile::Grimpeur, Profile::Sprinteur, Profile::Rouleur, Profile::Puncheur]);
        let mass = 58.0 + rng.next() * 26.0;
        Self { pid: x.pid.clone(), name: x.name.clone(), country: rng.pick(&COUNTRIES), team: (rng.next() * TEAMS.len() as f32) as usize % TEAMS.len(), profile, mass, ftp: x.wkg * mass, wprime: profile.wprime(), wbal: profile.wprime(), role: Role::Bunch, attack_until: 0.0, finished: None }
    }

    pub fn color(&self) -> &'static str {
        TEAMS[self.team].1
    }

    /// Reserve left, 0 to 1.
    pub fn reserve(&self) -> f32 {
        (self.wbal / self.wprime).clamp(0.0, 1.0)
    }

    /// The watts for the next step, given the plan's intensity (times the FTP) on this grade; the reserve
    /// drains above the FTP and refills below, and an empty reserve brings the rider back to a pace it holds.
    fn power(&mut self, intensity: f32, grade: f32, dt: f32) -> f32 {
        let mut w = self.ftp * intensity * self.profile.on(grade);
        if self.wbal <= 0.0 { w = w.min(self.ftp * 0.9) } else if self.wbal < 0.25 * self.wprime { w = w.min(self.ftp * 1.02) }
        self.wbal += if w > self.ftp { -(w - self.ftp) * dt } else { (self.wprime - self.wbal) * (self.ftp - w) / self.wprime * dt };
        self.wbal = self.wbal.min(self.wprime);
        w
    }
}

/// What a bot sees of the race at a step.
#[derive(Debug, Clone, Copy)]
pub struct View {
    /// Metres ridden by this bot, and the race's length.
    pub ridden: f32,
    pub race_m: f32,
    pub grade: f32,
}

/// A race's bots and plan.
#[derive(Debug)]
pub struct Race {
    pub bots: Vec<Bot>,
    pub race_m: f32,
    /// Race seconds.
    t: f32,
    chasing: bool,
    sprint_called: bool,
    /// Late attacks still allowed in this race: a few, not everyone at once.
    attacks_left: u8,
    /// Radio lines not read yet.
    pub radio: Vec<String>,
    last_gap_said: f32,
}

impl Race {
    pub fn new(e: &Event, race_m: f32) -> Self {
        let mut rng = Rng(seed_of(&e.id) ^ 0x5bd1_e995);
        let mut bots: Vec<Bot> = e.entrants.iter().filter(|x| x.bot).map(|x| Bot::from(x, &mut rng)).collect();
        // The break: three or four, rouleurs and puncheurs first.
        let mut order: Vec<usize> = (0..bots.len()).collect();
        order.sort_by_key(|&i| match bots[i].profile { Profile::Rouleur => 0, Profile::Puncheur => 1, Profile::Grimpeur => 2, Profile::Sprinteur => 3 });
        let n_break = if bots.len() >= 10 { 4 } else { (bots.len() / 3).max(1) };
        for &i in order.iter().take(n_break) { bots[i].role = Role::Break }
        let names: Vec<&str> = bots.iter().filter(|b| b.role == Role::Break).map(|b| b.name.as_str()).collect();
        let radio = vec![format!("{} coureurs au départ. Attention à l'échappée : {} ont l'air décidés.", bots.len() + 1, names.join(", "))];
        Self { bots, race_m, t: 0.0, chasing: false, sprint_called: false, radio, last_gap_said: 0.0, attacks_left: 3 }
    }

    /// Gap from the bunch to the break, m: where the break's last rider is, less where the bunch's first is.
    fn gap(&self, views: &[View]) -> f32 {
        let last_break = self.bots.iter().zip(views).filter(|(b, _)| b.role == Role::Break && b.finished.is_none()).map(|(_, v)| v.ridden).fold(f32::MAX, f32::min);
        let first_bunch = self.bots.iter().zip(views).filter(|(b, _)| b.role == Role::Bunch && b.finished.is_none()).map(|(_, v)| v.ridden).fold(0.0, f32::max);
        if last_break == f32::MAX { 0.0 } else { last_break - first_bunch }
    }

    /// One step: each bot's watts for the next `dt` s. `human_lead` is how far the strongest human is ahead of
    /// the bunch's front (m, negative behind): a human off the front is chased too.
    pub fn step(&mut self, views: &[View], human_lead: f32, dt: f32) -> Vec<f32> {
        self.t += dt;
        let t = self.t;
        let done = views.iter().zip(&self.bots).filter(|(_, b)| b.finished.is_none()).map(|(v, _)| v.ridden / v.race_m.max(1.0)).fold(0.0, f32::max);
        let gap = self.gap(views);
        // The plan of the day, by race fraction.
        if !self.chasing && (gap > 350.0 || human_lead > 60.0 || done > 0.55) && done > 0.15 {
            self.chasing = true;
            self.radio.push(if human_lead > 60.0 { "Le peloton réagit : il chasse derrière toi !".into() } else { format!("Le peloton s'organise, la chasse est lancée à {} km de l'arrivée.", ((1.0 - done) * self.race_m / 1000.0).round()) });
        }
        if gap > 0.0 && (gap - self.last_gap_said).abs() > if self.last_gap_said == 0.0 { 60.0 } else { 120.0 } && done < 0.95 {
            self.last_gap_said = gap;
            self.radio.push(format!("Échappée : {} d'avance.", gap_text(gap, views.iter().map(|v| v.ridden).fold(0.0, f32::max), t)));
        }
        let mut out = Vec::with_capacity(self.bots.len());
        for (i, b) in self.bots.iter_mut().enumerate() {
            let v = views[i];
            let left = v.race_m - v.ridden;
            if b.finished.is_some() { out.push(0.0); continue }
            let frac = v.ridden / v.race_m.max(1.0);
            let mut intensity = match b.role {
                Role::Break if frac < 0.04 => 1.2f32, // the jump
                Role::Break => if self.chasing { 1.02 } else { 0.97 },
                Role::Bunch if frac < 0.04 => 0.85,
                Role::Bunch => if self.chasing { 0.96 } else { 0.78 },
            };
            // Late attacks: puncheurs and climbers go on a rise in the last 3 km, when they have the legs.
            if self.attacks_left > 0 && left < 3000.0 && left > 700.0 && b.attack_until == 0.0 && matches!(b.profile, Profile::Puncheur | Profile::Grimpeur) && v.grade > 1.5 && b.reserve() > 0.5 && seed_of(&b.pid) % 7 == (t as u64 / 5) % 7 {
                b.attack_until = t + 40.0;
                self.attacks_left -= 1;
                self.radio.push(format!("Attaque de {} ({}) à {:.1} km !", b.name, b.profile.label(), left / 1000.0));
            }
            if b.attack_until > t { intensity = intensity.max(1.3) }
            // The sprint: everyone opens up in the last 600 m, the sprinters most.
            if left < 600.0 {
                intensity = intensity.max(b.profile.sprint() * if left < 250.0 { 1.0 } else { 0.7 });
                if !self.sprint_called { self.sprint_called = true; self.radio.push("Sprint lancé !".into()) }
            } else if left < 2000.0 && b.profile == Profile::Sprinteur {
                intensity = intensity.min(0.95); // sit in, save it
            }
            out.push(b.power(intensity, v.grade, dt));
        }
        out
    }

    /// A bot crossed the line at race second `time`.
    pub fn finish(&mut self, i: usize, time: f32) {
        if self.bots[i].finished.is_none() {
            self.bots[i].finished = Some(time);
            if self.bots.iter().filter(|b| b.finished.is_some()).count() == 1 { self.radio.push(format!("{} passe la ligne en premier des coureurs du serveur, {}.", self.bots[i].name, hms(time))) }
        }
    }

    pub fn all_done(&self) -> bool {
        self.bots.iter().all(|b| b.finished.is_some())
    }
}

fn hms(t: f32) -> String {
    let t = t as u64;
    if t >= 3600 { format!("{}h{:02}'{:02}\"", t / 3600, t / 60 % 60, t % 60) } else { format!("{}'{:02}\"", t / 60, t % 60) }
}

/// A gap in metres as time at the race's pace so far.
fn gap_text(gap_m: f32, ridden: f32, t: f32) -> String {
    let v = if t > 30.0 { ridden / t } else { 8.0 };
    let s = (gap_m / v.max(3.0)) as u64;
    if s >= 60 { format!("{}'{:02}\"", s / 60, s % 60) } else { format!("{s} s") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::course::Course;
    use crate::ride::Ride;

    fn race(cats: &str) -> Event {
        let mut e = Event { id: "auto-1-P".into(), kind: Kind::Race, name: "Test".into(), track: "flat".into(), start: 0, laps: 1, pace: 0.0, keep: 0, cats: cats.into(), club: None, by: "auto".into(), by_name: "le-mollet".into(), entrants: vec![], results: vec![] };
        enroll(&mut e, FIELD);
        e
    }

    #[test]
    fn a_field_fits_the_rank_and_is_the_same_every_time() {
        let e = race("P");
        assert_eq!(e.entrants.len(), FIELD);
        assert!(e.entrants.iter().all(|x| x.bot && x.cat == 'P' && (3.2..3.8).contains(&x.wkg)), "{:?}", e.entrants.iter().map(|x| x.wkg).collect::<Vec<_>>());
        assert_eq!(race("P").entrants[3].name, e.entrants[3].name);
        let mut again = e.clone();
        enroll(&mut again, FIELD); // not twice
        assert_eq!(again.entrants.len(), FIELD);
        let open = race("");
        assert!(open.entrants.iter().all(|x| (2.5..3.2).contains(&x.wkg)));
    }

    #[test]
    fn a_race_unfolds_and_everyone_finishes() {
        // 10 km flat: a break, a chase, a sprint, all within the reserve, all over the line.
        let e = race("O");
        let course = Course::flat(10_000.0);
        let mut r = Race::new(&e, 10_000.0);
        let mut rides: Vec<Ride> = r.bots.iter().map(|b| Ride::new(b.mass + 8.0)).collect();
        let (mut t, mut max_gap, mut sprinter_max) = (0.0f32, 0.0f32, 0.0f32);
        while !r.all_done() && t < 1800.0 {
            let views: Vec<View> = rides.iter().map(|rd| View { ridden: rd.ridden, race_m: 10_000.0, grade: 0.0 }).collect();
            let powers = r.step(&views, -100.0, DT);
            t += DT;
            // A wheel to follow within 25 m: the draft the server computes for everyone.
            let ahead: Vec<f32> = rides.iter().map(|r| r.distance).collect();
            for (i, rd) in rides.iter_mut().enumerate() {
                rd.draft = if ahead.iter().any(|&d| d > rd.distance && d - rd.distance <= 25.0) { 0.3 } else { 0.0 };
                rd.power = powers[i];
                rd.tick(&course, DT);
                if rd.ridden >= 10_000.0 { r.finish(i, t) }
                if r.bots[i].profile == Profile::Sprinteur && rd.ridden > 9_800.0 { sprinter_max = sprinter_max.max(rd.power / r.bots[i].ftp) }
            }
            max_gap = max_gap.max(r.gap(&views));
            assert!(r.bots.iter().all(|b| b.wbal >= -1.0), "a bot went past its reserve");
        }
        assert!(r.all_done(), "{} finished in 30 min", r.bots.iter().filter(|b| b.finished.is_some()).count());
        assert!(max_gap > 100.0, "the break got {max_gap:.0} m");
        assert!(r.chasing, "nobody chased");
        assert!(sprinter_max > 1.8, "the sprinters opened up at {sprinter_max:.2}x FTP");
        assert!(r.radio.iter().any(|l| l.starts_with("Sprint")) && r.radio.iter().any(|l| l.starts_with("Échappée")), "{:?}", r.radio);
        let times: Vec<f32> = r.bots.iter().filter_map(|b| b.finished).collect();
        let (lo, hi) = (times.iter().cloned().fold(f32::MAX, f32::min), times.iter().cloned().fold(0.0, f32::max));
        assert!(lo > 700.0 && hi < 1500.0 && hi - lo < 360.0, "times {lo:.0}..{hi:.0} s");
    }
}
