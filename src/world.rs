//! Procedural open worlds: a closed road loop through generated terrain.
//!
//! Same outputs as a real course (a looped `Course` + a `Terrain`), so physics,
//! trainer control and the 3D view work unchanged. Deterministic for a given seed.

use crate::course::{Course, Point, STEP_M};
use crate::terrain::{self, CELL_M, Terrain};
use serde::Serialize;
use std::f64::consts::TAU;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Relief {
    Flat,
    Hills,
    Mountains,
}

impl Relief {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "flat" => Some(Self::Flat),
            "hills" => Some(Self::Hills),
            "mountains" => Some(Self::Mountains),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Flat => "Plat",
            Self::Hills => "Vallonné",
            Self::Mountains => "Montagneux",
        }
    }

    /// Terrain amplitude around the base altitude, m.
    fn amplitude(self) -> f64 {
        match self {
            Self::Flat => 35.0,
            Self::Hills => 220.0,
            Self::Mountains => 650.0,
        }
    }

    /// Steepest road grade allowed, as a fraction: what makes a relief feel flat, rolling or alpine.
    fn max_grade(self) -> f64 {
        match self {
            Self::Flat => 0.03,
            Self::Hills => 0.08,
            Self::Mountains => 0.12,
        }
    }
}

// ponytail: fake anchor, the terrain format is lat/lon based; any spot works
const LAT0: f64 = 45.0;
const LON0: f64 = 6.0;
const BASE_M: f64 = 700.0;
/// Terrain kept around the loop, m.
const MARGIN_M: f64 = 1000.0;
/// Road elevation smoothing (Gaussian sigma), in 10 m points.
const ROAD_SMOOTH_PTS: f64 = 25.0;

fn splitmix(x: &mut u64) -> u64 {
    *x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn unit(x: &mut u64) -> f64 {
    (splitmix(x) >> 11) as f64 / (1u64 << 53) as f64
}

/// Smooth value noise in [0, 1].
struct Noise(u64);

impl Noise {
    fn lattice(&self, i: i64, j: i64) -> f64 {
        let mut s = self.0 ^ (i as u64).wrapping_mul(0x9E37_79B9) ^ (j as u64).wrapping_mul(0x85EB_CA6B).rotate_left(17);
        unit(&mut s)
    }

    fn value(&self, x: f64, y: f64) -> f64 {
        let (i, j) = (x.floor(), y.floor());
        let (fx, fy) = (x - i, y - j);
        let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
        let (i, j) = (i as i64, j as i64);
        let top = self.lattice(i, j) + (self.lattice(i + 1, j) - self.lattice(i, j)) * sx;
        let bottom = self.lattice(i, j + 1) + (self.lattice(i + 1, j + 1) - self.lattice(i, j + 1)) * sx;
        top + (bottom - top) * sy
    }

    /// Fractal sum of octaves, normalized to [0, 1].
    fn fbm(&self, x: f64, y: f64, octaves: u32) -> f64 {
        let (mut sum, mut amp, mut norm, mut f) = (0.0, 1.0, 0.0, 1.0);
        for _ in 0..octaves {
            sum += amp * self.value(x * f, y * f);
            norm += amp;
            amp *= 0.5;
            f *= 2.0;
        }
        sum / norm
    }
}

pub struct World {
    pub course: Course,
    pub terrain: Terrain,
}

/// Generate a loop of about `length_m` through `relief` terrain.
pub fn generate(seed: u64, length_m: f64, relief: Relief, name: &str) -> World {
    let (ground, woods) = (Noise(seed), Noise(seed ^ 0xA5A5_5A5A_DEAD_BEEF));
    let amp = relief.amplitude();
    // Hill size follows the loop size, so a short loop still crosses a few hills.
    let wavelength = (length_m / 3.0).clamp(1200.0, 4000.0);
    let height = |x: f64, z: f64| BASE_M + amp * (ground.fbm(x / wavelength, z / wavelength, 5) - 0.5) * 2.0;

    // Loop shape: a circle whose radius wobbles with a few random harmonics (periodic, so it closes).
    let mut rng = seed;
    let harmonics: Vec<(f64, f64, f64)> = (2..6).map(|k| (k as f64, 0.16 / k as f64 * (0.5 + unit(&mut rng)), unit(&mut rng) * TAU)).collect();
    let shape = |t: f64| 1.0 + harmonics.iter().map(|(k, a, p)| a * (k * t + p).sin()).sum::<f64>();
    let ring = |scale: f64| -> Vec<[f64; 2]> { (0..4000).map(|i| i as f64 / 4000.0 * TAU).map(|t| [scale * shape(t) * t.cos(), scale * shape(t) * t.sin()]).collect() };
    let perimeter = |p: &[[f64; 2]]| (0..p.len()).map(|i| dist(p[i], p[(i + 1) % p.len()])).sum::<f64>();
    let mut scale = length_m / TAU;
    for _ in 0..3 {
        scale *= length_m / perimeter(&ring(scale));
    }
    let line = ring(scale);

    // Resample every 10 m around the closed loop; the last point closes it.
    let mut xz = vec![line[0]];
    let (mut walked, mut next) = (0.0, STEP_M);
    for i in 0..line.len() {
        let (a, b) = (line[i], line[(i + 1) % line.len()]);
        let seg = dist(a, b);
        while next <= walked + seg && next < length_m - 1.0 {
            let t = (next - walked) / seg;
            xz.push([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]);
            next += STEP_M;
        }
        walked += seg;
    }
    let total = walked;

    // Road elevation: terrain under the line, smoothed around the loop, grades capped.
    let raw: Vec<f64> = xz.iter().map(|p| height(p[0], p[1])).collect();
    let mut ele = smooth_circular(&raw, ROAD_SMOOTH_PTS);
    let steepest = (0..ele.len()).map(|i| (ele[(i + 1) % ele.len()] - ele[i]).abs() / STEP_M).fold(0.0, f64::max);
    if steepest > relief.max_grade() {
        let mean = ele.iter().sum::<f64>() / ele.len() as f64;
        let k = relief.max_grade() / steepest;
        for e in &mut ele {
            *e = mean + (*e - mean) * k;
        }
    }

    let (kx, kz) = (LAT0.to_radians().cos() * 111_320.0, 110_540.0);
    let geo = |p: [f64; 2]| (LAT0 - p[1] / kz, LON0 + p[0] / kx); // x east, z south
    let round = |v: f64| (v * 1e6).round() / 1e6;
    let mut points: Vec<Point> = xz
        .iter()
        .zip(&ele)
        .enumerate()
        .map(|(i, (&p, &e))| {
            let (lat, lon) = geo(p);
            Point { d: (i as f64 * STEP_M) as f32, lat: round(lat), lon: round(lon), ele: (e * 10.0).round() as f32 / 10.0 }
        })
        .collect();
    points.push(Point { d: total as f32, ..points[0] });
    let course = Course { name: name.into(), attribution: "Monde généré par le-mollet.".into(), waypoints: vec![], roads: vec![], looped: true, points, ..Default::default() };

    // Terrain grid around the loop.
    let fold = |f: fn(f64, f64) -> f64, i: usize, init: f64| line.iter().map(|p| p[i]).fold(init, f);
    let (min_x, min_z) = (fold(f64::min, 0, f64::MAX) - MARGIN_M, fold(f64::min, 1, f64::MAX) - MARGIN_M);
    let w = ((fold(f64::max, 0, f64::MIN) + MARGIN_M - min_x) / CELL_M).ceil() as usize;
    let h = ((fold(f64::max, 1, f64::MIN) + MARGIN_M - min_z) / CELL_M).ceil() as usize;
    let (north, west) = geo([min_x, min_z]);
    let (south, east) = geo([min_x + w as f64 * CELL_M, min_z + h as f64 * CELL_M]);
    let start = xz[0];
    let (mut heights, mut cover) = (Vec::with_capacity(w * h), Vec::with_capacity(w * h));
    for r in 0..h {
        for c in 0..w {
            let (x, z) = (min_x + (c as f64 + 0.5) * CELL_M, min_z + (r as f64 + 0.5) * CELL_M);
            let e = height(x, z);
            heights.push(e as f32);
            let mut block = (c as u64 / 2) << 32 | (r as u64 / 2);
            block ^= seed;
            cover.push(if e < BASE_M - amp * 0.55 {
                terrain::WATER
            } else if e > BASE_M + amp * 0.6 {
                terrain::ROCK
            } else if dist([x, z], start) < 350.0 && unit(&mut block) < 0.12 {
                terrain::BUILDING // start village, 10 m houses
            } else if woods.fbm(x / 700.0, z / 700.0, 3) > 0.55 {
                terrain::FOREST
            } else {
                terrain::GRASS
            });
        }
    }
    let mut terrain = Terrain { w, h, west, south, east, north, heights, cover };
    terrain.carve_road(&course);
    World { course, terrain }
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Gaussian smoothing on a closed loop (indices wrap).
fn smooth_circular(v: &[f64], sigma: f64) -> Vec<f64> {
    let (n, r) = (v.len() as isize, (3.0 * sigma).ceil() as isize);
    let w: Vec<f64> = (-r..=r).map(|k| (-0.5 * (k as f64 / sigma).powi(2)).exp()).collect();
    let norm: f64 = w.iter().sum();
    (0..n).map(|i| (-r..=r).zip(&w).map(|(k, wk)| v[(i + k).rem_euclid(n) as usize] * wk).sum::<f64>() / norm).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_matches_request() {
        for relief in [Relief::Flat, Relief::Hills, Relief::Mountains] {
            let w = generate(42, 10_000.0, relief, "test");
            let c = &w.course;
            assert!(c.looped);
            assert!((c.length() - 10_000.0).abs() < 100.0, "{relief:?} length {}", c.length());
            let (first, last) = (c.points[0], *c.points.last().unwrap());
            assert_eq!((first.lat, first.lon, first.ele), (last.lat, last.lon, last.ele)); // closed
            let (steep, down) = c.grade_extremes(100.0);
            let cap = relief.max_grade() as f32 * 100.0 + 0.5;
            assert!(steep <= cap && down >= -cap, "{relief:?} grades {steep} {down}");
            assert_eq!(w.terrain.heights.len(), w.terrain.w * w.terrain.h);
            assert!(w.terrain.cover.contains(&terrain::ROAD));
        }
        let hilly = generate(42, 10_000.0, Relief::Hills, "t").course.elevation_gain();
        let flat = generate(42, 10_000.0, Relief::Flat, "t").course.elevation_gain();
        assert!(hilly > flat * 2.0, "hills {hilly} vs flat {flat}");
    }

    #[test]
    fn deterministic_per_seed() {
        let ele = |seed| generate(seed, 5_000.0, Relief::Hills, "t").course.points.iter().map(|p| p.ele).collect::<Vec<_>>();
        assert_eq!(ele(7), ele(7));
        assert_ne!(ele(7), ele(8));
    }
}
