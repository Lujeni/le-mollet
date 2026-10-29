//! Open city: every street of a town as a graph. Riders roam freely and pick a turn at each
//! junction, while the ride engine keeps following a plain `Course` that grows one street at a time.
//! Built offline from OpenStreetMap streets inside an admin boundary + the IGN elevation grid,
//! see docs/city.md.

use crate::course::{Course, Point, STEP_M, UA, haversine_m, overpass, request_json, resample, smooth};
use crate::terrain::{self, Terrain};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize)]
pub struct City {
    pub name: String,
    pub attribution: String,
    /// `[lat, lon]` riders start from, snapped to the nearest junction.
    pub start: [f64; 2],
    /// Streets timed as segments (hand-made island: KOMs, sprints).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<crate::segment::Def>,
    /// Named places a rider can pick to start from, each snapped to its nearest junction.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub starts: Vec<Start>,
    pub streets: Vec<Street>,
    /// Hand-made island (see island.rs) rather than a real town: relief, decor.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub island: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub props: Vec<Prop>,
    /// Island thumbnail: one cover code per block, rows north to south, `preview_w` blocks wide (a = 0).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub preview: String,
    #[serde(default)]
    pub preview_w: usize,
    /// Junction -> streets leaving it, as (street, walked a -> b).
    #[serde(skip)]
    links: Vec<Vec<(usize, bool)>>,
}

/// A street between two junctions `a` and `b`.
#[derive(Serialize, Deserialize)]
pub struct Street {
    pub a: u32,
    pub b: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// `[lat, lon, ele]` about every 10 m from `a` to `b`.
    pub pts: Vec<[f64; 3]>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Start {
    pub name: String,
    pub at: [f64; 2],
}

/// One piece of decor: kind, where, ground height (m), heading (degrees) and variant.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Prop {
    pub k: String,
    pub lat: f64,
    pub lon: f64,
    pub e: f32,
    pub y: f32,
    pub v: u32,
}

/// One way out of a junction.
#[derive(Debug, Clone, Serialize)]
pub struct Exit {
    pub street: usize,
    pub forward: bool,
    /// Heading change, degrees, positive to the left.
    pub turn: f32,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Turn {
    Left,
    #[default]
    Straight,
    Right,
}

impl Turn {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "left" => Some(Self::Left),
            "straight" => Some(Self::Straight),
            "right" => Some(Self::Right),
            _ => None,
        }
    }
}

/// Smaller heading changes than this count as going straight on.
const STRAIGHT_DEG: f32 = 30.0;
/// The route always extends this far past the rider, m: enough for the 3D chase camera and the
/// grade baseline, short enough that a turn can still be picked ~this close to the junction.
const LOOKAHEAD_M: f32 = 40.0;

impl City {
    pub fn load(path: &str) -> Result<Self> {
        let mut c: Self = serde_json::from_str(&std::fs::read_to_string(path).with_context(|| format!("read {path}"))?)?;
        if c.streets.is_empty() { bail!("{path}: city without streets") }
        c.link();
        Ok(c)
    }

    pub fn island(name: String, attribution: String, start: [f64; 2], starts: Vec<Start>, streets: Vec<Street>, props: Vec<Prop>) -> Self {
        let mut c = Self { name, attribution, start, streets, island: true, starts, segments: vec![], props, preview: String::new(), preview_w: 0, links: vec![] };
        c.link();
        c
    }

    fn link(&mut self) {
        let n = self.streets.iter().map(|s| s.a.max(s.b) as usize + 1).max().unwrap_or(0);
        self.links = vec![vec![]; n];
        for (i, s) in self.streets.iter().enumerate() {
            self.links[s.a as usize].push((i, true));
            self.links[s.b as usize].push((i, false));
        }
    }

    /// Points of a street in riding order.
    fn walk(&self, street: usize, forward: bool) -> Vec<[f64; 3]> {
        let mut p = self.streets[street].pts.clone();
        if !forward { p.reverse() }
        p
    }

    fn end(&self, street: usize, forward: bool) -> u32 {
        let s = &self.streets[street];
        if forward { s.b } else { s.a }
    }

    /// Ways out of `node`, left to right. No U-turn unless it is a dead end.
    pub fn exits(&self, node: u32, from: Option<(usize, bool)>) -> Vec<Exit> {
        let heading_in = from.map(|(s, f)| {
            let p = self.walk(s, f);
            heading(p[p.len().saturating_sub(3)], p[p.len() - 1])
        });
        let all: Vec<Exit> = self.links[node as usize]
            .iter()
            .map(|&(street, forward)| {
                let p = self.walk(street, forward);
                let out = heading(p[0], p[2.min(p.len() - 1)]);
                let turn = heading_in.map_or(0.0, |h| wrap(out - h));
                Exit { street, forward, turn, name: self.streets[street].name.clone() }
            })
            .collect();
        let back = |e: &Exit| from.is_some_and(|(s, f)| e.street == s && e.forward != f);
        let mut out: Vec<Exit> = all.iter().filter(|e| !back(e)).cloned().collect();
        if out.is_empty() { out = all }
        out.sort_by(|a, b| b.turn.total_cmp(&a.turn));
        out
    }

    /// Junction closest to a point.
    fn nearest(&self, at: [f64; 2]) -> u32 {
        let d = |s: &Street, i: usize| haversine_m([s.pts[i][0], s.pts[i][1]], at);
        let (s, first) = self
            .streets
            .iter()
            .flat_map(|s| [(s, true), (s, false)])
            .min_by(|x, y| {
                let dx = d(x.0, if x.1 { 0 } else { x.0.pts.len() - 1 });
                let dy = d(y.0, if y.1 { 0 } else { y.0.pts.len() - 1 });
                dx.total_cmp(&dy)
            })
            .unwrap();
        if first { s.a } else { s.b }
    }
}

/// The exit a turn request picks: the sharpest one that way, else the straightest.
pub fn choose(exits: &[Exit], t: Turn) -> &Exit {
    let straight = exits.iter().min_by(|a, b| a.turn.abs().total_cmp(&b.turn.abs())).unwrap();
    match t {
        Turn::Left => exits.first().filter(|e| e.turn > STRAIGHT_DEG).unwrap_or(straight),
        Turn::Right => exits.last().filter(|e| e.turn < -STRAIGHT_DEG).unwrap_or(straight),
        Turn::Straight => straight,
    }
}

/// Heading of a -> b, degrees counterclockwise from east.
fn heading(a: [f64; 3], b: [f64; 3]) -> f32 {
    let dx = (b[1] - a[1]) * a[0].to_radians().cos();
    (b[0] - a[0]).atan2(dx).to_degrees() as f32
}

fn wrap(deg: f32) -> f32 {
    (deg + 540.0).rem_euclid(360.0) - 180.0
}

/// One rider's way through the city: a course that grows street by street as they ride.
pub struct Route {
    pub course: Course,
    at: (usize, bool),
    /// Turn to take at the next junction; back to straight once used.
    pub next: Turn,
    /// Streets appended so far: (street, walked a -> b, distance at its start, at its end).
    pub legs: Vec<(usize, bool, f32, f32)>,
}

impl Route {
    pub fn start(city: &City) -> Self {
        Self::start_at(city, city.start)
    }

    /// Start from the junction nearest to `at` (`[lat, lon]`).
    pub fn start_at(city: &City, at: [f64; 2]) -> Self {
        let node = city.nearest(at);
        let first = city.exits(node, None).into_iter().next().expect("junction has a street");
        let mut r = Self {
            course: Course { name: city.name.clone(), attribution: city.attribution.clone(), waypoints: vec![], roads: vec![], looped: false, points: vec![], ..Default::default() },
            at: (first.street, first.forward),
            next: Turn::Straight,
            legs: vec![],
        };
        r.append(city, first.street, first.forward);
        r
    }

    fn append(&mut self, city: &City, street: usize, forward: bool) {
        let d0 = self.course.length();
        let p = &mut self.course.points;
        for q in city.walk(street, forward) {
            let d = p.last().map_or(0.0, |l| l.d + haversine_m([l.lat, l.lon], [q[0], q[1]]) as f32);
            if p.last().is_some_and(|l| d - l.d < 0.5) { continue } // the shared junction point
            p.push(Point { d, lat: q[0], lon: q[1], ele: q[2] as f32 });
        }
        self.legs.push((street, forward, d0, self.course.length()));
    }

    /// Keep the route `LOOKAHEAD_M` past `distance`, taking `next` at each junction.
    /// Returns the index of the first new point, if any were added.
    pub fn extend(&mut self, city: &City, distance: f32) -> Option<usize> {
        let first = self.course.points.len();
        while self.course.length() - distance < LOOKAHEAD_M {
            let exits = city.exits(city.end(self.at.0, self.at.1), Some(self.at));
            let e = choose(&exits, self.next);
            self.at = (e.street, e.forward);
            self.next = Turn::Straight;
            self.append(city, e.street, e.forward);
        }
        (self.course.points.len() > first).then_some(first)
    }

    /// Choices at the junction at the end of the route, and how far it is from `distance`.
    pub fn upcoming(&self, city: &City, distance: f32) -> (f32, Vec<Exit>) {
        (self.course.length() - distance, city.exits(city.end(self.at.0, self.at.1), Some(self.at)))
    }
}

// ---------- offline build ----------

const STREETS: &str = "trunk|primary|secondary|tertiary|unclassified|residential|living_street|pedestrian|cycleway|\
                       trunk_link|primary_link|secondary_link|tertiary_link";
/// Terrain kept around the boundary box, m.
const MARGIN_M: f64 = 200.0;

/// Half width painted as asphalt, m.
fn half_width(highway: &str) -> f64 {
    match highway {
        "trunk" | "primary" | "trunk_link" | "primary_link" => 7.0,
        "secondary" | "secondary_link" => 5.5,
        "tertiary" | "tertiary_link" => 4.5,
        "cycleway" => 1.5,
        "pedestrian" => 3.0,
        _ => 3.5,
    }
}

/// Every street inside OSM boundary relation `relation`, with the terrain around it.
pub fn build(name: &str, relation: u64, start: [f64; 2]) -> Result<(City, Terrain)> {
    let area = 3_600_000_000 + relation;
    let query = format!(
        "[out:json][timeout:300];rel({relation});out bb;area({area})->.p;\
         way(area.p)[highway~\"^({STREETS})$\"][area!=yes][access!~\"^(private|no)$\"][bicycle!=no];out body geom;"
    );
    let r = request_json(|| ureq::post(&overpass()).set("User-Agent", UA).send_form(&[("data", &query)]))?;
    let els = r["elements"].as_array().context("Overpass: no elements")?;
    let bb = &els.first().context("Overpass: boundary relation not found")?["bounds"];
    let (s, w, n, e) = (bb["minlat"].as_f64(), bb["minlon"].as_f64(), bb["maxlat"].as_f64(), bb["maxlon"].as_f64());
    let (Some(s), Some(w), Some(n), Some(e)) = (s, w, n, e) else { bail!("Overpass: relation {relation} has no bounds") };
    let (mlat, mlon) = (MARGIN_M / 110_540.0, MARGIN_M / (((s + n) / 2.0).to_radians().cos() * 111_320.0));

    let mut t = Terrain::for_bbox(w - mlon, s - mlat, e + mlon, n + mlat, crate::terrain::CELL_M)?;
    t.cover.fill(terrain::PAVE);
    t.paint_land_cover()?;

    // Split ways at junctions: nodes shared by several ways, plus way ends.
    struct Way { nodes: Vec<(i64, [f64; 2])>, name: String, half: f64, deck: bool }
    let ways: Vec<Way> = els[1..]
        .iter()
        .filter_map(|w| {
            let tag = |k: &str| w["tags"][k].as_str().unwrap_or("").to_owned();
            let nodes: Vec<_> = w["nodes"].as_array()?.iter().zip(w["geometry"].as_array()?).filter_map(|(id, g)| Some((id.as_i64()?, [g["lat"].as_f64()?, g["lon"].as_f64()?]))).collect();
            // Bridges and tunnels: the IGN grid is bare ground, so interpolate between the ends.
            let deck = ["bridge", "tunnel"].iter().any(|k| !matches!(tag(k).as_str(), "" | "no"));
            (nodes.len() >= 2).then(|| Way { nodes, name: tag("name"), half: half_width(&tag("highway")), deck })
        })
        .collect();
    let mut uses: HashMap<i64, u32> = HashMap::new();
    for w in &ways {
        for (i, (id, _)) in w.nodes.iter().enumerate() {
            *uses.entry(*id).or_default() += if i == 0 || i == w.nodes.len() - 1 { 2 } else { 1 };
        }
    }
    let mut raw: Vec<(i64, i64, &Way, Vec<[f64; 2]>)> = vec![];
    for w in &ways {
        let mut from = 0;
        for i in 1..w.nodes.len() {
            if uses[&w.nodes[i].0] >= 2 {
                raw.push((w.nodes[from].0, w.nodes[i].0, w, w.nodes[from..=i].iter().map(|n| n.1).collect()));
                from = i;
            }
        }
    }

    // Keep the biggest connected network: islands (closed courtyards, cut-off paths) are unreachable.
    let mut parent: HashMap<i64, i64> = HashMap::new();
    fn root(p: &mut HashMap<i64, i64>, x: i64) -> i64 {
        let up = *p.entry(x).or_insert(x);
        if up == x { return x }
        let r = root(p, up);
        p.insert(x, r);
        r
    }
    for (a, b, ..) in &raw {
        let (ra, rb) = (root(&mut parent, *a), root(&mut parent, *b));
        parent.insert(ra, rb);
    }
    let mut size: HashMap<i64, usize> = HashMap::new();
    for (a, ..) in &raw {
        *size.entry(root(&mut parent, *a)).or_default() += 1;
    }
    let main = *size.iter().max_by_key(|(_, n)| **n).context("no streets")?.0;

    let mut ids: HashMap<i64, u32> = HashMap::new();
    let mut streets = vec![];
    for (a, b, way, line) in raw {
        if a == b || root(&mut parent, a) != main { continue }
        let samples = resample(&line, STEP_M);
        let ele: Vec<f32> = if way.deck {
            let (e0, e1) = (t.ele_at(line[0][0], line[0][1]), t.ele_at(line[line.len() - 1][0], line[line.len() - 1][1]));
            let len = samples.last().unwrap().0.max(1.0);
            samples.iter().map(|(d, _)| e0 + (e1 - e0) * (*d / len) as f32).collect()
        } else {
            let mut v = smooth(&samples.iter().map(|(_, p)| t.ele_at(p[0], p[1])).collect::<Vec<_>>(), 2.0);
            // Junction ends exact, so streets meet at the same height.
            let n = v.len() - 1;
            (v[0], v[n]) = (t.ele_at(line[0][0], line[0][1]), t.ele_at(line[line.len() - 1][0], line[line.len() - 1][1]));
            v
        };
        let round = |x: f64| (x * 1e6).round() / 1e6;
        let pts: Vec<[f64; 3]> = samples.iter().zip(&ele).map(|((_, p), e)| [round(p[0]), round(p[1]), (*e as f64 * 10.0).round() / 10.0]).collect();
        paint_street(&mut t, &pts, way.half, way.deck);
        let next = ids.len() as u32;
        let a = *ids.entry(a).or_insert(next);
        let next = ids.len() as u32;
        let b = *ids.entry(b).or_insert(next);
        streets.push(Street { a, b, name: way.name.clone(), pts });
    }

    let mut city = City {
        name: name.into(),
        attribution: "Streets and buildings © OpenStreetMap contributors (ODbL). Elevation © IGN RGE ALTI (Licence Ouverte Etalab 2.0).".into(),
        start,
        starts: vec![],
        segments: vec![],
        streets,
        island: false,
        props: vec![],
        preview: String::new(),
        preview_w: 0,
        links: vec![],
    };
    city.link();
    Ok((city, t))
}

/// Asphalt under a street; bridge decks also raise the ground to the deck.
fn paint_street(t: &mut Terrain, pts: &[[f64; 3]], half: f64, deck: bool) {
    let (cx, cz) = ((t.east - t.west) / t.w as f64 * ((t.south + t.north) / 2.0).to_radians().cos() * 111_320.0, (t.north - t.south) / t.h as f64 * 110_540.0);
    let reach = (half / cx.min(cz)).ceil() as isize + 1;
    for p in pts.windows(2) {
        for k in 0..4 {
            let f = k as f64 / 4.0;
            let (lat, lon, ele) = (p[0][0] + (p[1][0] - p[0][0]) * f, p[0][1] + (p[1][1] - p[0][1]) * f, p[0][2] + (p[1][2] - p[0][2]) * f);
            let (gx, gy) = t.grid(lat, lon);
            for r in (gy as isize - reach)..=(gy as isize + reach) {
                for c in (gx as isize - reach)..=(gx as isize + reach) {
                    if r < 0 || c < 0 || r as usize >= t.h || c as usize >= t.w { continue }
                    if ((c as f64 + 0.5 - gx) * cx).hypot((r as f64 + 0.5 - gy) * cz) > half { continue }
                    let i = r as usize * t.w + c as usize;
                    t.cover[i] = terrain::ROAD;
                    if deck { t.heights[i] = ele as f32 - 0.3 }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plus-shaped crossing: junction 0 in the middle, arms to 1 (east), 2 (north), 3 (west), 4 (south).
    fn plus() -> City {
        let arm = |b: u32, dlat: f64, dlon: f64| Street {
            a: 0,
            b,
            name: format!("arm{b}"),
            pts: (0..=10).map(|i| [48.85 + dlat * i as f64 / 10.0, 2.35 + dlon * i as f64 / 10.0, 35.0]).collect(),
        };
        let mut c = City {
            name: "Plus".into(),
            attribution: String::new(),
            start: [48.85, 2.35 - 0.0015],
            starts: vec![],
            segments: vec![],
            streets: vec![arm(1, 0.0, 0.0015), arm(2, 0.001, 0.0), arm(3, 0.0, -0.0015), arm(4, -0.001, 0.0)],
            island: false,
            props: vec![],
            preview: String::new(),
            preview_w: 0,
            links: vec![],
        };
        c.link();
        c
    }

    #[test]
    fn turns_pick_the_right_arm() {
        let c = plus();
        // Arriving from the west arm (riding it backwards, 3 -> 0), heading east.
        let exits = c.exits(0, Some((2, false)));
        assert_eq!(exits.iter().map(|e| e.street).collect::<Vec<_>>(), vec![1, 0, 3]); // left (north), straight (east), right (south)
        assert_eq!(choose(&exits, Turn::Straight).street, 0);
        assert_eq!(choose(&exits, Turn::Left).street, 1);
        assert_eq!(choose(&exits, Turn::Right).street, 3);
        assert!((exits[0].turn - 90.0).abs() < 1.0 && exits[1].turn.abs() < 1.0);
        // Dead end: the only way out is back.
        let back = c.exits(1, Some((0, true)));
        assert_eq!((back.len(), back[0].street, back[0].forward), (1, 0, false));
    }

    #[test]
    fn route_grows_street_by_street() {
        let c = plus();
        let mut r = Route::start(&c);
        let len0 = r.course.length();
        assert!(len0 > 100.0, "starts on a full street: {len0}");
        r.next = Turn::Right;
        let added = r.extend(&c, len0 - 10.0);
        assert!(added.is_some());
        let p = &r.course.points;
        assert!(p.windows(2).all(|w| w[1].d > w[0].d), "distances keep increasing");
        assert_eq!(r.next, Turn::Straight, "a turn is used once");
        assert!(r.extend(&c, 0.0).is_none(), "nothing to add while far from the end");
        let (to_go, exits) = r.upcoming(&c, r.course.length() - 20.0);
        assert!((to_go - 20.0).abs() < 1e-3 && !exits.is_empty());
    }

    #[test]
    fn riding_a_city_never_finishes() {
        let c = plus();
        let (mut r, mut ride) = (Route::start(&c), crate::ride::Ride::new(80.0));
        ride.power = 250.0;
        for i in 0..6000 {
            if i % 400 == 0 { r.next = [Turn::Left, Turn::Right, Turn::Straight][i / 400 % 3] }
            r.extend(&c, ride.distance);
            ride.tick(&r.course, 0.1);
            assert!(!ride.finished, "finished at tick {i}");
        }
        assert!(ride.distance > 4000.0, "rode {} m in 10 min", ride.distance);
        assert!((ride.ridden - ride.distance).abs() < 1.0);
    }
}
