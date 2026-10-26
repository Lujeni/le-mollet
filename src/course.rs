//! Real-world courses: road geometry from OpenStreetMap, elevation from IGN.
//! Approach and per-course recipes: docs/courses.md.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Point {
    /// Distance from start, m.
    pub d: f32,
    pub lat: f64,
    pub lon: f64,
    /// Smoothed elevation, m.
    pub ele: f32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Course {
    pub name: String,
    pub attribution: String,
    /// `[lat, lon]` waypoints the route was built from, kept for rebuilds.
    pub waypoints: Vec<[f64; 2]>,
    /// Road refs the route sticks to, e.g. "D 211". Kept for rebuilds.
    #[serde(default)]
    pub roads: Vec<String>,
    /// Closed loop: riders go round and round, no finish line.
    #[serde(default)]
    pub looped: bool,
    pub points: Vec<Point>,
}

/// Spacing between course points, m.
pub const STEP_M: f64 = 10.0;
/// Gaussian smoothing sigma in points (5 × 10 m = 50 m). The DEM is exact to the
/// meter, but OSM road lines cut hairpin insides and graze embankments: narrower
/// windows leave fake 15-20 % ramps in every switchback (see docs/courses.md).
const SMOOTH_SIGMA_PTS: f32 = 5.0;
pub(crate) const UA: &str = "le-mollet/0.1 (https://github.com/lujeni/le-mollet)";

impl Course {
    pub fn length(&self) -> f32 {
        self.points.last().map_or(0.0, |p| p.d)
    }

    /// `[lat, lon]` at distance `d`, interpolated.
    pub fn latlon_at(&self, d: f32) -> [f64; 2] {
        let p = &self.points;
        let i = p.partition_point(|q| q.d <= d).clamp(1, p.len() - 1);
        let (a, b) = (p[i - 1], p[i]);
        let t = ((d - a.d) / (b.d - a.d)).clamp(0.0, 1.0) as f64;
        [a.lat + (b.lat - a.lat) * t, a.lon + (b.lon - a.lon) * t]
    }

    pub fn ele_at(&self, d: f32) -> f32 {
        let p = &self.points;
        let i = p.partition_point(|q| q.d <= d).clamp(1, p.len() - 1);
        let (a, b) = (p[i - 1], p[i]);
        let t = ((d - a.d) / (b.d - a.d)).clamp(0.0, 1.0);
        a.ele + (b.ele - a.ele) * t
    }

    /// Road grade in % at distance `d`, over a 10 m baseline.
    pub fn grade_at(&self, d: f32) -> f32 {
        let (a, b) = ((d - 5.0).max(0.0), (d + 5.0).min(self.length()));
        if b <= a { return 0.0 }
        (self.ele_at(b) - self.ele_at(a)) / (b - a) * 100.0
    }

    /// Average grade of each full kilometer, in %.
    pub fn km_grades(&self) -> Vec<f32> {
        (0..(self.length() / 1000.0) as usize)
            .map(|k| (self.ele_at((k + 1) as f32 * 1000.0) - self.ele_at(k as f32 * 1000.0)) / 10.0)
            .collect()
    }

    /// Steepest and most downhill grade over any `window` m stretch (10 m steps), in %.
    pub fn grade_extremes(&self, window: f32) -> (f32, f32) {
        let n = ((self.length() - window) / 10.0).max(0.0) as usize;
        (0..=n)
            .map(|i| (self.ele_at(i as f32 * 10.0 + window) - self.ele_at(i as f32 * 10.0)) / window * 100.0)
            .fold((f32::MIN, f32::MAX), |(hi, lo), g| (hi.max(g), lo.min(g)))
    }

    pub fn elevation_gain(&self) -> f32 {
        self.points.windows(2).map(|w| (w[1].ele - w[0].ele).max(0.0)).sum()
    }

    /// Flat straight road, for workouts: grade is always 0.
    pub fn flat(length_m: f32) -> Self {
        let p = |d| Point { d, lat: 0.0, lon: 0.0, ele: 0.0 };
        Self { name: "Route plate".into(), attribution: String::new(), waypoints: vec![], roads: vec![], looped: false, points: vec![p(0.0), p(length_m)] }
    }

    pub fn load(path: &str) -> Result<Self> {
        let c: Self = serde_json::from_str(&std::fs::read_to_string(path).with_context(|| format!("read {path}"))?)?;
        if c.points.len() < 2 { bail!("{path}: course needs at least 2 points") }
        Ok(c)
    }

    /// Build a course from `[lat, lon]` waypoints: route along OpenStreetMap roads
    /// (preferring `roads` refs, ignoring one-ways like a closed race road), resample
    /// every 10 m, sample IGN RGE ALTI elevation, smooth.
    pub fn build(name: &str, waypoints: &[[f64; 2]], roads: &[String]) -> Result<Self> {
        if waypoints.len() < 2 { bail!("need at least start and finish waypoints") }
        let line = route_osm(waypoints, roads)?;
        let samples = resample(&line, STEP_M);
        let mut ele = Vec::with_capacity(samples.len());
        // ponytail: 100 points per GET keeps URLs short; ~15 requests for a 14 km climb
        for chunk in samples.chunks(100) {
            let join = |f: fn(&(f64, [f64; 2])) -> f64| chunk.iter().map(|s| format!("{:.6}", f(s))).collect::<Vec<_>>().join("|");
            let url = format!(
                "https://data.geopf.fr/altimetrie/1.0/calcul/alti/rest/elevation.json?lon={}&lat={}&resource=ign_rge_alti_wld&zonly=true",
                join(|s| s.1[1]),
                join(|s| s.1[0])
            );
            let r = request_json(|| ureq::get(&url).set("User-Agent", UA).call())?;
            for z in r["elevations"].as_array().context("IGN: no elevations")? {
                let z = z.as_f64().context("IGN: bad elevation")?;
                // IGN answers -99999 outside its coverage (France + overseas).
                if z < -1000.0 { bail!("IGN has no elevation here; course outside France?") }
                ele.push(z as f32);
            }
        }

        let ele = smooth(&ele, SMOOTH_SIGMA_PTS);
        let round = |x: f64| (x * 1e6).round() / 1e6;
        let points = samples
            .iter()
            .zip(ele)
            .map(|(&(d, [lat, lon]), e)| Point { d: d as f32, lat: round(lat), lon: round(lon), ele: (e * 10.0).round() / 10.0 })
            .collect();
        Ok(Self {
            name: name.into(),
            attribution: "Route © OpenStreetMap contributors (ODbL). Elevation © IGN RGE ALTI (Licence Ouverte Etalab 2.0).".into(),
            waypoints: waypoints.to_vec(),
            roads: roads.to_vec(),
            looped: false,
            points,
        })
    }
}

const DRIVABLE: &[&str] = &[
    "trunk", "primary", "secondary", "tertiary", "unclassified", "residential", "living_street", "road",
    "trunk_link", "primary_link", "secondary_link", "tertiary_link",
];
/// Cost multiplier for roads outside the requested refs: used only to bridge gaps.
const OFF_ROAD_PENALTY: f64 = 10.0;

type Graph = HashMap<i64, Vec<(i64, f64)>>;

/// Road geometry through `waypoints` from OpenStreetMap (Overpass API).
fn route_osm(waypoints: &[[f64; 2]], roads: &[String]) -> Result<Vec<[f64; 2]>> {
    // ~3 km margin: climbs wander away from the start-finish box (Ventoux goes via
    // Chalet Reynard, east of the summit). Add vias if a road strays further.
    const PAD: f64 = 0.03;
    let fold = |f: fn(f64, f64) -> f64, i: usize, init: f64| waypoints.iter().map(|w| w[i]).fold(init, f);
    let (s, n) = (fold(f64::min, 0, 90.0) - PAD, fold(f64::max, 0, -90.0) + PAD);
    let (w, e) = (fold(f64::min, 1, 180.0) - PAD, fold(f64::max, 1, -180.0) + PAD);
    let query = format!("[out:json][timeout:90];way({s},{w},{n},{e})[highway~\"^({})$\"];out body geom;", DRIVABLE.join("|"));
    let r = request_json(|| ureq::post("https://overpass-api.de/api/interpreter").set("User-Agent", UA).send_form(&[("data", &query)]))?;

    let (mut graph, mut on_road, mut coords) = (Graph::new(), HashSet::new(), HashMap::new());
    for way in r["elements"].as_array().context("Overpass: no elements")? {
        let wanted = roads.is_empty()
            || way["tags"]["ref"].as_str().is_some_and(|r| r.split(';').any(|r| roads.iter().any(|x| x == r.trim())));
        let ids = way["nodes"].as_array().context("Overpass: way without nodes")?;
        let geom = way["geometry"].as_array().context("Overpass: way without geometry")?;
        let pts: Vec<(i64, [f64; 2])> = ids
            .iter()
            .zip(geom)
            .filter_map(|(id, g)| Some((id.as_i64()?, [g["lat"].as_f64()?, g["lon"].as_f64()?])))
            .collect();
        for p in pts.windows(2) {
            let ((a, pa), (b, pb)) = (p[0], p[1]);
            let cost = haversine_m(pa, pb) * if wanted { 1.0 } else { OFF_ROAD_PENALTY };
            graph.entry(a).or_default().push((b, cost));
            graph.entry(b).or_default().push((a, cost));
            coords.extend([(a, pa), (b, pb)]);
            if wanted { on_road.extend([a, b]) }
        }
    }
    if on_road.is_empty() { bail!("no road matching {roads:?} in the area") }

    let snap = |wp: [f64; 2]| *on_road.iter().min_by(|a, b| haversine_m(coords[a], wp).total_cmp(&haversine_m(coords[b], wp))).unwrap();
    let mut path: Vec<i64> = vec![];
    for leg in waypoints.windows(2) {
        let p = shortest_path(&graph, snap(leg[0]), snap(leg[1])).with_context(|| format!("no road between {:?} and {:?}", leg[0], leg[1]))?;
        path.extend(p.into_iter().skip(usize::from(!path.is_empty())));
    }
    Ok(path.iter().map(|id| coords[id]).collect())
}

/// Dijkstra; costs rounded to millimeters.
fn shortest_path(g: &Graph, from: i64, to: i64) -> Option<Vec<i64>> {
    let (mut dist, mut prev) = (HashMap::from([(from, 0u64)]), HashMap::new());
    let mut heap = BinaryHeap::from([Reverse((0u64, from))]);
    while let Some(Reverse((d, u))) = heap.pop() {
        if u == to {
            let mut p = vec![to];
            while let Some(&q) = prev.get(p.last().unwrap()) { p.push(q) }
            p.reverse();
            return Some(p);
        }
        if d > dist[&u] { continue }
        for &(v, w) in g.get(&u).into_iter().flatten() {
            let nd = d + (w * 1000.0) as u64;
            if dist.get(&v).is_none_or(|&x| nd < x) {
                dist.insert(v, nd);
                prev.insert(v, u);
                heap.push(Reverse((nd, v)));
            }
        }
    }
    None
}

/// Run a JSON request, backing off on 429/5xx gateway errors (IGN and Overpass rate-limit and time out).
pub(crate) fn request_json(send: impl Fn() -> Result<ureq::Response, ureq::Error>) -> Result<serde_json::Value> {
    for attempt in 1..=5 {
        match send() {
            Ok(r) => return Ok(r.into_json()?),
            Err(ureq::Error::Status(429 | 502 | 503 | 504, _)) => std::thread::sleep(std::time::Duration::from_secs(2 * attempt)),
            Err(e) => return Err(e.into()),
        }
    }
    bail!("still rate limited after 5 attempts")
}

pub(crate) fn haversine_m(a: [f64; 2], b: [f64; 2]) -> f64 {
    let (la1, la2) = (a[0].to_radians(), b[0].to_radians());
    let (dla, dlo) = (la2 - la1, (b[1] - a[1]).to_radians());
    let h = (dla / 2.0).sin().powi(2) + la1.cos() * la2.cos() * (dlo / 2.0).sin().powi(2);
    2.0 * 6_371_000.0 * h.sqrt().asin()
}

/// Points every `step` m along a `[lat, lon]` polyline, with their distance from start.
pub(crate) fn resample(line: &[[f64; 2]], step: f64) -> Vec<(f64, [f64; 2])> {
    let mut out = vec![(0.0, line[0])];
    let (mut walked, mut next) = (0.0, step);
    for w in line.windows(2) {
        let seg = haversine_m(w[0], w[1]);
        while seg > 0.0 && next <= walked + seg {
            let t = (next - walked) / seg;
            out.push((next, [w[0][0] + (w[1][0] - w[0][0]) * t, w[0][1] + (w[1][1] - w[0][1]) * t]));
            next += step;
        }
        walked += seg;
    }
    if walked > out.last().unwrap().0 + 1.0 {
        out.push((walked, *line.last().unwrap()));
    }
    out
}

/// Gaussian smoothing, `sigma` in points; weights renormalized at both ends.
pub(crate) fn smooth(v: &[f32], sigma: f32) -> Vec<f32> {
    let r = (3.0 * sigma).ceil() as isize;
    let w: Vec<f32> = (-r..=r).map(|k| (-0.5 * (k as f32 / sigma).powi(2)).exp()).collect();
    (0..v.len() as isize)
        .map(|i| {
            let (mut sum, mut weight) = (0.0, 0.0);
            for (k, wk) in (-r..=r).zip(&w) {
                if let Some(x) = usize::try_from(i + k).ok().and_then(|j| v.get(j)) {
                    sum += x * wk;
                    weight += wk;
                }
            }
            sum / weight
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_straight_line() {
        // ~1112 m due north
        let pts = resample(&[[45.0, 6.0], [45.005, 6.0], [45.01, 6.0]], 100.0);
        assert_eq!(pts.len(), 13); // 0..=1100 every 100 m + end
        assert!((pts[5].0 - 500.0).abs() < 1e-9);
        assert!((pts.last().unwrap().0 - 1111.95).abs() < 0.1);
    }

    #[test]
    fn shortest_path_prefers_cheap_edges() {
        let mut g = Graph::new();
        for (a, b, w) in [(1, 2, 1.0), (2, 4, 1.0), (1, 3, 0.5), (3, 4, 0.5), (4, 5, 1.0)] {
            g.entry(a).or_default().push((b, w));
            g.entry(b).or_default().push((a, w));
        }
        assert_eq!(shortest_path(&g, 1, 5), Some(vec![1, 3, 4, 5]));
        assert_eq!(shortest_path(&g, 5, 1), Some(vec![5, 4, 3, 1]));
        assert_eq!(shortest_path(&g, 1, 99), None);
    }

    #[test]
    fn smoothing_keeps_ramps_and_removes_spikes() {
        let ramp: Vec<f32> = (0..30).map(|i| i as f32 * 0.8).collect();
        assert!((smooth(&ramp, 3.0)[15] - 12.0).abs() < 1e-4); // symmetric: ramps untouched
        let mut spike = vec![0.0; 31];
        spike[15] = 10.0;
        assert!(smooth(&spike, 3.0)[15] < 1.5);
    }

    #[test]
    fn grade_and_elevation() {
        let points = (0..=10)
            .map(|i| Point { d: i as f32 * 10.0, lat: 0.0, lon: 0.0, ele: i as f32 * 0.8 })
            .collect();
        let c = Course { name: "8%".into(), attribution: String::new(), waypoints: vec![], roads: vec![], looped: false, points };
        assert!((c.grade_at(50.0) - 8.0).abs() < 1e-3);
        assert!((c.ele_at(55.0) - 4.4).abs() < 1e-3);
        assert!((c.elevation_gain() - 8.0).abs() < 1e-3);
        assert_eq!(c.ele_at(-10.0), 0.0);
        let (hi, lo) = c.grade_extremes(50.0);
        assert!((hi - 8.0).abs() < 1e-3 && (lo - 8.0).abs() < 1e-3);
        assert!(c.km_grades().is_empty()); // 100 m course: no full km
        assert_eq!(c.ele_at(1e6), 8.0);
    }
}
