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

#[derive(Debug, Default, Serialize, Deserialize)]
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
    /// OpenStreetMap route relation a loop was built from (`build_ring`), kept for rebuilds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation: Option<u64>,
    /// ISO 3166 country code (FR, ES, IT...), from OpenStreetMap at the start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    /// Terrain cell size, m, when finer than the default 5 m (short loops get small bricks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell_m: Option<f64>,
    /// Decor read from the aerial photo and OpenStreetMap (`le-mollet decor`, see docs/courses.md).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub props: Vec<crate::city::Prop>,
    pub points: Vec<Point>,
}

/// Spacing between course points, m.
pub const STEP_M: f64 = 10.0;
/// Gaussian smoothing sigma in points (5 × 10 m = 50 m). The DEM is exact to the
/// meter, but OSM road lines cut hairpin insides and graze embankments: narrower
/// windows leave fake 15-20 % ramps in every switchback (see docs/courses.md).
const SMOOTH_SIGMA_PTS: f32 = 5.0;
/// Outside France the elevation model is ~30 m, not 1 m (src/dem.rs): stacked hairpins borrow each other's
/// height, so the smoothing is twice as wide (100 m).
const SMOOTH_SIGMA_DEM_PTS: f32 = 10.0;
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

    /// Cap the climb at `max` % per 10 m step, keeping the total: what a step has above the cap goes to the
    /// nearest steps that have room, shared by their room, within a window that widens 100 m at a time.
    /// For the ~30 m elevation outside France (src/dem.rs), which squeezes a short ramp or a hairpin stack
    /// into a fake wall; `max` is the climb's published steepest ramp (see docs/courses.md).
    pub fn cap_grade(&mut self, max: f32) {
        let p = &mut self.points;
        let mut d: Vec<f32> = p.windows(2).map(|w| w[1].ele - w[0].ele).collect();
        let cap: Vec<f32> = p.windows(2).map(|w| (w[1].d - w[0].d) * max / 100.0).collect();
        for i in 0..d.len() {
            let mut extra = d[i] - cap[i];
            if extra <= 0.0 { continue }
            d[i] = cap[i];
            for r in (10..=d.len()).step_by(10) {
                let near = i.saturating_sub(r)..(i + r + 1).min(d.len());
                let room: f32 = near.clone().map(|j| (cap[j] - d[j]).max(0.0)).sum();
                if room <= 0.0 { continue }
                let share = (extra / room).min(1.0);
                for j in near { d[j] += (cap[j] - d[j]).max(0.0) * share }
                extra -= room * share;
                if extra <= 1e-4 { break }
            }
        }
        for i in 0..d.len() { p[i + 1].ele = p[i].ele + d[i] }
    }

    pub fn elevation_gain(&self) -> f32 {
        self.points.windows(2).map(|w| (w[1].ele - w[0].ele).max(0.0)).sum()
    }

    /// Flat straight road, for workouts: grade is always 0.
    pub fn flat(length_m: f32) -> Self {
        let p = |d| Point { d, lat: 0.0, lon: 0.0, ele: 0.0 };
        Self { name: "Route plate".into(), attribution: String::new(), waypoints: vec![], roads: vec![], looped: false, points: vec![p(0.0), p(length_m)], ..Default::default() }
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
        let (ele, dem) = elevations(&samples)?;
        let ele = smooth(&ele, if dem { SMOOTH_SIGMA_DEM_PTS } else { SMOOTH_SIGMA_PTS });
        Ok(Self { waypoints: waypoints.to_vec(), roads: roads.to_vec(), country: country_at(waypoints[0]), ..Self::from_samples(name, &samples, ele, dem) })
    }

    /// Build a closed loop from an OpenStreetMap route relation (a cycling ring): its ways chained in order,
    /// ridden the way its one-way members go, from the point nearest `start`. Elevation is smoothed
    /// round the loop, so the start is not a seam.
    pub fn build_ring(name: &str, relation: u64, start: Option<[f64; 2]>) -> Result<Self> {
        let query = format!("[out:json][timeout:90];rel({relation});out geom;way(r);out tags;");
        let r = request_json(|| ureq::post("https://overpass-api.de/api/interpreter").set("User-Agent", UA).send_form(&[("data", &query)]))?;
        let els = r["elements"].as_array().context("Overpass: no elements")?;
        let rel = els.iter().find(|e| e["type"] == "relation").with_context(|| format!("no relation {relation}"))?;
        let oneway: HashSet<i64> = els.iter().filter(|e| e["type"] == "way" && e["tags"]["oneway"] == "yes").filter_map(|e| e["id"].as_i64()).collect();
        let ways: Vec<(i64, Vec<[f64; 2]>)> = rel["members"].as_array().into_iter().flatten()
            .filter(|m| m["type"] == "way")
            .map(|m| (m["ref"].as_i64().unwrap_or(0), m["geometry"].as_array().into_iter().flatten().filter_map(|p| Some([p["lat"].as_f64()?, p["lon"].as_f64()?])).collect::<Vec<_>>()))
            .filter(|(_, g)| g.len() >= 2)
            .collect();
        let ring = chain_ring(&ways, &oneway).with_context(|| format!("relation {relation}"))?;
        let ring = match start {
            Some(s) => {
                let i = (0..ring.len() - 1).min_by(|&a, &b| haversine_m(ring[a], s).total_cmp(&haversine_m(ring[b], s))).unwrap();
                ring[i..].iter().chain(&ring[1..=i]).copied().collect()
            }
            None => ring,
        };
        let samples = resample(&ring, STEP_M);
        let (mut ele, dem) = elevations(&samples)?;
        // The last sample is the start again: smooth the loop without it, then close it.
        ele.pop();
        let mut ele = smooth_loop(&ele, if dem { SMOOTH_SIGMA_DEM_PTS } else { SMOOTH_SIGMA_PTS });
        ele.push(ele[0]);
        Ok(Self { waypoints: vec![ring[0]], relation: Some(relation), looped: true, country: country_at(ring[0]), ..Self::from_samples(name, &samples, ele, dem) })
    }

    /// `dem`: some elevations came from the Terrain Tiles (outside France), not IGN.
    fn from_samples(name: &str, samples: &[(f64, [f64; 2])], ele: Vec<f32>, dem: bool) -> Self {
        let round = |x: f64| (x * 1e6).round() / 1e6;
        let points = samples
            .iter()
            .zip(ele)
            .map(|(&(d, [lat, lon]), e)| Point { d: d as f32, lat: round(lat), lon: round(lon), ele: (e * 10.0).round() / 10.0 })
            .collect();
        Self {
            name: name.into(),
            attribution: if dem { format!("Route © OpenStreetMap contributors (ODbL). {}", crate::dem::ATTRIBUTION) }
                else { "Route © OpenStreetMap contributors (ODbL). Elevation © IGN RGE ALTI (Licence Ouverte Etalab 2.0).".into() },
            points,
            ..Default::default()
        }
    }
}

/// Ways of a route relation, in order, into one closed line (last point = first), oriented like most of its
/// one-way members. Fails on a gap over 15 m between consecutive ways or between the end and the start.
fn chain_ring(ways: &[(i64, Vec<[f64; 2]>)], oneway: &HashSet<i64>) -> Result<Vec<[f64; 2]>> {
    const GAP_M: f64 = 15.0;
    if ways.len() < 2 { bail!("a ring needs at least 2 ways") }
    let mut ring: Vec<[f64; 2]> = vec![];
    let (mut with, mut against) = (0, 0);
    for (k, (id, g)) in ways.iter().enumerate() {
        // Orient each way to start where the line so far ends; the first one toward the second way.
        let flip = match ring.last() {
            Some(&end) => haversine_m(end, g[g.len() - 1]) < haversine_m(end, g[0]),
            None => {
                let next = &ways[1].1;
                let near = |p| haversine_m(p, next[0]).min(haversine_m(p, next[next.len() - 1]));
                near(g[0]) < near(g[g.len() - 1])
            }
        };
        let g: Vec<[f64; 2]> = if flip { g.iter().rev().copied().collect() } else { g.clone() };
        if let Some(&end) = ring.last() && haversine_m(end, g[0]) > GAP_M {
            bail!("gap of {:.0} m before way {id} (member {k})", haversine_m(end, g[0]));
        }
        if oneway.contains(id) { if flip { against += 1 } else { with += 1 } }
        ring.extend(g.into_iter().skip(usize::from(!ring.is_empty())));
    }
    let (first, last) = (ring[0], ring[ring.len() - 1]);
    if haversine_m(first, last) > GAP_M { bail!("not a closed ring: {:.0} m from the end back to the start", haversine_m(first, last)) }
    ring.pop();
    ring.push(first);
    if against > with { ring.reverse() }
    Ok(ring)
}

/// Elevation at each sample: IGN RGE ALTI (1 m) in France, the Terrain Tiles elsewhere (src/dem.rs).
/// Also tells whether any came from the tiles.
fn elevations(samples: &[(f64, [f64; 2])]) -> Result<(Vec<f32>, bool)> {
    let mut ele = Vec::with_capacity(samples.len());
    let mut tiles: Option<crate::dem::Tiles> = None;
    // ponytail: 100 points per GET keeps URLs short; ~15 requests for a 14 km climb
    for chunk in samples.chunks(100) {
        let join = |f: fn(&(f64, [f64; 2])) -> f64| chunk.iter().map(|s| format!("{:.6}", f(s))).collect::<Vec<_>>().join("|");
        let url = format!(
            "https://data.geopf.fr/altimetrie/1.0/calcul/alti/rest/elevation.json?lon={}&lat={}&resource=ign_rge_alti_wld&zonly=true",
            join(|s| s.1[1]),
            join(|s| s.1[0])
        );
        let r = request_json(|| ureq::get(&url).set("User-Agent", UA).call())?;
        for (z, s) in r["elevations"].as_array().context("IGN: no elevations")?.iter().zip(chunk) {
            let z = z.as_f64().context("IGN: bad elevation")?;
            // IGN answers -99999 outside its coverage (France + overseas): the tiles take over there.
            ele.push(if z < -1000.0 { tiles.get_or_insert_with(Default::default).ele(s.1[0], s.1[1])? } else { z as f32 });
        }
    }
    Ok((ele, tiles.is_some()))
}

/// ISO 3166 code of the country at a point, from OpenStreetMap's admin boundaries; None if unknown.
fn country_at([lat, lon]: [f64; 2]) -> Option<String> {
    let query = format!("[out:json][timeout:60];is_in({lat},{lon})->.a;area.a[admin_level=2][\"ISO3166-1\"];out tags;");
    let r = request_json(|| ureq::post("https://overpass-api.de/api/interpreter").set("User-Agent", UA).send_form(&[("data", &query)])).ok()?;
    r["elements"].as_array()?.iter().find_map(|e| e["tags"]["ISO3166-1"].as_str().map(str::to_owned))
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

/// Gaussian smoothing round a loop: both ends borrow from each other.
pub(crate) fn smooth_loop(v: &[f32], sigma: f32) -> Vec<f32> {
    let r = ((3.0 * sigma).ceil() as usize).min(v.len());
    let padded: Vec<f32> = v[v.len() - r..].iter().chain(v).chain(&v[..r]).copied().collect();
    smooth(&padded, sigma)[r..r + v.len()].to_vec()
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
    fn ring_chains_ways_and_follows_one_ways() {
        // A triangle in three ways, the middle one drawn backwards, the last one one-way against the order.
        let (a, b, c) = ([48.0, 2.0], [48.0, 2.01], [48.01, 2.005]);
        let ways = vec![(1, vec![a, b]), (2, vec![c, b]), (3, vec![c, a])];
        let ring = chain_ring(&ways, &HashSet::new()).unwrap();
        assert_eq!(ring, vec![a, b, c, a]);
        let ring = chain_ring(&ways, &HashSet::from([3])).unwrap(); // way 3 goes c -> a, as chained: kept
        assert_eq!(ring, vec![a, b, c, a]);
        let ways = vec![(1, vec![a, b]), (2, vec![c, b]), (3, vec![a, c])];
        let ring = chain_ring(&ways, &HashSet::from([3])).unwrap(); // way 3 goes a -> c: ridden the other way round
        assert_eq!(ring, vec![a, c, b, a]);
        assert!(chain_ring(&[(1, vec![a, b]), (2, vec![c, [48.02, 2.0]])], &HashSet::new()).is_err()); // gap
        // Smoothing a loop: a bump at the start spreads to both ends, a constant stays constant.
        let mut v = vec![0.0f32; 40];
        v[0] = 10.0;
        let s = smooth_loop(&v, 2.0);
        assert!((s[1] - s[39]).abs() < 1e-4 && s[39] > 0.5 && s[20] < 1e-3);
        assert!(smooth_loop(&[5.0; 30], 5.0).iter().all(|x| (x - 5.0).abs() < 1e-4));
    }

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
    fn capping_spreads_a_wall_and_keeps_the_climb() {
        // 1 km at 5 %, with a 100 m wall at 30 % in the middle.
        let points = (0..=100)
            .scan(0.0, |e, i| { let p = Point { d: i as f32 * 10.0, lat: 0.0, lon: 0.0, ele: *e }; *e += if (45..55).contains(&i) { 3.0 } else { 0.5 }; Some(p) })
            .collect();
        let mut c = Course { points, ..Default::default() };
        let top = c.points.last().unwrap().ele;
        c.cap_grade(12.0);
        assert!((c.points.last().unwrap().ele - top).abs() < 1e-3);
        assert!(c.grade_extremes(100.0).0 <= 12.0 + 1e-3);
        assert!((c.ele_at(100.0) - 5.0).abs() < 1e-3); // far from the wall: untouched
    }

    #[test]
    fn grade_and_elevation() {
        let points = (0..=10)
            .map(|i| Point { d: i as f32 * 10.0, lat: 0.0, lon: 0.0, ele: i as f32 * 0.8 })
            .collect();
        let c = Course { name: "8%".into(), attribution: String::new(), waypoints: vec![], roads: vec![], looped: false, points, ..Default::default() };
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
