//! Hand-made Lego island: a map file (`courses/*.map.json`) where the coast, the land areas, the relief,
//! the named roads and every piece of decor are placed by hand, rasterized into an open city (roads as a
//! junction graph, ridden like Paris), its terrain and its decor list. See docs/island.md.
//!
//! Map coordinates are meters, x east and z south from the north-west corner.

use crate::city::{City, Prop, Street};
use crate::course::{STEP_M, smooth};
use crate::terrain::{self, Terrain};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::HashMap;

/// Heights are stored above this datum so the sea floor stays positive, m.
pub const SEA: f64 = 50.0;
/// Where the island sits on the globe (nowhere in particular, open sea off Corsica).
const LAT0: f64 = 42.0;
const LON0: f64 = 7.5;
/// Road ends blend into their junction height over this distance, m.
const END_BLEND_M: f64 = 60.0;
/// Steepest grade a road keeps; the ground is carved or filled to follow it.
const MAX_GRADE: f64 = 0.12;
/// Banks beside a road blend back into the terrain over this distance, m.
const BANK_M: f64 = 8.0;

type Xz = [f64; 2];

#[derive(Deserialize)]
struct Map {
    name: String,
    /// Width (x) and depth (z) of the whole map, sea included, m.
    size: Xz,
    /// Terrain cell, m.
    cell: f64,
    /// Node riders start from.
    start: String,
    coast: Vec<Xz>,
    #[serde(default)]
    areas: Vec<Area>,
    #[serde(default)]
    relief: Vec<Relief>,
    #[serde(default)]
    rivers: Vec<River>,
    nodes: HashMap<String, Xz>,
    roads: Vec<Road>,
    #[serde(default)]
    props: Vec<MapProp>,
    #[serde(default)]
    rows: Vec<Row>,
    #[serde(default)]
    scatter: Vec<Scatter>,
}

/// Land cover painted inside a polygon; later areas win. `flat` levels the ground there, m above sea.
#[derive(Deserialize)]
struct Area {
    cover: String,
    poly: Vec<Xz>,
    #[serde(default)]
    flat: Option<f64>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum Relief {
    /// Volcano rising from `r` to the rim at `crater`, a lava lake `depth` below the rim. The flanks steepen
    /// toward the foot (height in r² - d²) so a spiral road climbs them at a constant grade.
    Cone { at: Xz, r: f64, h: f64, crater: f64, depth: f64 },
    /// Rounded hill.
    Hill { at: Xz, r: f64, h: f64 },
    /// Flat top with steep sides `edge` m wide: mesas, cliffs.
    Plateau { poly: Vec<Xz>, h: f64, edge: f64 },
    /// Sand ripples across a polygon.
    Dunes { poly: Vec<Xz>, amp: f64, wave: f64 },
}

/// Water channel cut along a polyline.
#[derive(Deserialize)]
struct River {
    pts: Vec<Xz>,
    width: f64,
    depth: f64,
}

#[derive(Deserialize)]
struct Road {
    name: String,
    from: String,
    to: String,
    /// Hand-placed bends between the two junctions.
    #[serde(default)]
    via: Vec<Xz>,
    /// Deck between its two ends: no terrain under it is followed nor carved.
    #[serde(default)]
    bridge: bool,
    /// Climb circling a center: `to` is placed where the spiral ends.
    #[serde(default)]
    spiral: Option<Spiral>,
    #[serde(default = "default_half")]
    half: f64,
}

fn default_half() -> f64 {
    3.5
}

#[derive(Deserialize)]
struct Spiral {
    center: Xz,
    /// Radius at the top, m.
    r1: f64,
    turns: f64,
    /// Counterclockwise seen from above.
    #[serde(default)]
    ccw: bool,
}

#[derive(Deserialize)]
struct MapProp {
    k: String,
    at: Xz,
    #[serde(default)]
    yaw: f64,
    #[serde(default)]
    v: u32,
    /// Floats on the sea instead of standing on the ground.
    #[serde(default)]
    sea: bool,
}

/// Pieces of one kind lined up every `step` m from `from` to `to`: houses along a street, lamps, fences.
/// Variants cycle along the row.
#[derive(Deserialize)]
struct Row {
    k: String,
    from: Xz,
    to: Xz,
    step: f64,
    #[serde(default)]
    yaw: f64,
    #[serde(default)]
    variants: u32,
}

/// `count` pieces of one decor kind spread over a hand-drawn polygon (not on roads or water).
#[derive(Deserialize)]
struct Scatter {
    k: String,
    poly: Vec<Xz>,
    count: usize,
    #[serde(default)]
    variants: u32,
}

/// Cover codes past the terrain module's: the island's own grounds.
pub const SAND: u8 = 7;
pub const LAVA: u8 = 8;
pub const FIELD: u8 = 9;
pub const DIRT: u8 = 10;
pub const BASALT: u8 = 11;
/// Tropical forest: palms, ferns, big leaves (plain forest grows pines and round trees).
pub const JUNGLE: u8 = 12;

fn cover_code(name: &str) -> Result<u8> {
    Ok(match name {
        "grass" => terrain::GRASS,
        "forest" => terrain::FOREST,
        "water" => terrain::WATER,
        "rock" => terrain::ROCK,
        "pave" => terrain::PAVE,
        "sand" => SAND,
        "lava" => LAVA,
        "field" => FIELD,
        "dirt" => DIRT,
        "basalt" => BASALT,
        "jungle" => JUNGLE,
        _ => bail!("unknown cover {name}"),
    })
}

pub struct Island {
    pub city: City,
    pub terrain: Terrain,
}

pub fn build(path: &str) -> Result<Island> {
    let mut map: Map = serde_json::from_str(&std::fs::read_to_string(path).with_context(|| format!("read {path}"))?).with_context(|| format!("parse {path}"))?;
    let (w, h) = ((map.size[0] / map.cell).round() as usize, (map.size[1] / map.cell).round() as usize);
    let cell = map.cell;
    let center = |c: usize, r: usize| [(c as f64 + 0.5) * cell, (r as f64 + 0.5) * cell];

    // 1. Ground: sea outside the coast, beaches rising inland, then hand-placed relief and level areas.
    let mut height = vec![0.0f64; w * h];
    let mut cover = vec![terrain::WATER; w * h];
    let coast = closed(&map.coast);
    for r in 0..h {
        for c in 0..w {
            let p = center(c, r);
            let d = dist_to_line(&coast, p);
            let i = r * w + c;
            if inside(&coast, p) {
                height[i] = 1.5 + 12.0 * smoothstep(0.0, 350.0, d);
                cover[i] = if d < 35.0 { SAND } else { terrain::GRASS };
            } else {
                height[i] = -1.0; // flat sea: one level of trans-blue plates
            }
        }
    }
    for rel in &map.relief {
        apply_relief(rel, &mut height, &mut cover, w, h, cell);
    }
    for a in &map.areas {
        let code = cover_code(&a.cover)?;
        let poly = closed(&a.poly);
        for_cells_in(&poly, w, h, cell, |i, p| {
            cover[i] = code;
            if let Some(f) = a.flat {
                // Level toward `f`, blended over 25 m from the edge so it never makes a wall.
                let k = smoothstep(0.0, 25.0, dist_to_line(&poly, p));
                height[i] += (f - height[i]) * k;
            }
        });
    }
    for river in &map.rivers {
        for_cells_near(&river.pts, river.width / 2.0 + BANK_M, w, h, cell, |i, d| {
            let k = 1.0 - smoothstep(river.width / 2.0, river.width / 2.0 + BANK_M, d);
            height[i] -= river.depth * k;
            if d <= river.width / 2.0 { cover[i] = terrain::WATER }
        });
    }

    // 2. Roads: hand-drawn polylines between named junctions, following the ground then carved into it.
    let lerp_at = |height: &[f64], p: Xz| sample(height, w, h, cell, p);
    let mut ids: HashMap<String, u32> = HashMap::new();
    let mut id_of = |name: &str| -> u32 {
        let n = ids.len() as u32;
        *ids.entry(name.to_owned()).or_insert(n)
    };
    let mut lines: Vec<(Vec<Xz>, Vec<f64>, &Road)> = vec![];
    // Spirals first: they place their `to` node.
    for road in map.roads.iter().filter(|r| r.spiral.is_some()) {
        let s = road.spiral.as_ref().unwrap();
        let from = *map.nodes.get(&road.from).with_context(|| format!("road {}: no node {}", road.name, road.from))?;
        let pts = spiral(from, s);
        map.nodes.insert(road.to.clone(), *pts.last().unwrap());
    }
    let node_ele: HashMap<&String, f64> = map.nodes.iter().map(|(k, &p)| (k, lerp_at(&height, p).max(1.0))).collect();
    for road in &map.roads {
        let node = |n: &String| map.nodes.get(n).copied().with_context(|| format!("road {}: no node {n}", road.name));
        let (a, b) = (node(&road.from)?, node(&road.to)?);
        let line = match &road.spiral {
            Some(s) => spiral(a, s),
            None => catmull_rom(&[vec![a], road.via.clone(), vec![b]].concat()),
        };
        let pts = resample_xz(&line, STEP_M);
        let (ea, eb) = (node_ele[&road.from], node_ele[&road.to]);
        let total = arc(&pts);
        let ele: Vec<f64> = if road.bridge {
            pts.iter().enumerate().map(|(i, _)| ea + (eb - ea) * arc(&pts[..=i]) / total.max(1.0)).collect()
        } else {
            let raw: Vec<f32> = pts.iter().map(|&p| lerp_at(&height, p).max(1.0) as f32).collect();
            let sm = smooth(&raw, 6.0);
            let mut walked = 0.0;
            pts.iter().enumerate().map(|(i, _)| {
                if i > 0 { walked += dist(pts[i - 1], pts[i]) }
                // Pin both ends to their junction so roads meet at the same height.
                let (ka, kb) = (1.0 - smoothstep(0.0, END_BLEND_M, walked), 1.0 - smoothstep(0.0, END_BLEND_M, total - walked));
                let e = sm[i] as f64;
                e + (ea - e) * ka + (eb - e) * kb * (1.0 - ka)
            }).collect()
        };
        let ele = if road.bridge { ele } else { cap_grade(ele, &pts) };
        lines.push((pts, ele, road));
    }
    // Carve: flat road surface, banks blending back into the ground; roads paint over everything.
    for (pts, ele, road) in &lines {
        if road.bridge { continue }
        carve(pts, ele, road.half, &mut height, &mut cover, (w, h, cell));
    }

    // 3. Junction graph, as an open city.
    let (kx, kz) = (LAT0.to_radians().cos() * 111_320.0, 110_540.0);
    let geo = |p: Xz| [LAT0 - p[1] / kz, LON0 + p[0] / kx];
    let round = |v: f64, k: f64| (v * k).round() / k;
    let mut streets = vec![];
    let mut graded = vec![];
    for (pts, ele, road) in &lines {
        let steepest = (1..pts.len()).map(|i| ((ele[i] - ele[i - 1]) / dist(pts[i - 1], pts[i]).max(0.1)).abs()).fold(0.0, f64::max);
        graded.push(format!("{} {:.1} km max {:.0} %", road.name, arc(pts) / 1000.0, steepest * 100.0));
        streets.push(Street {
            a: id_of(&road.from),
            b: id_of(&road.to),
            name: road.name.clone(),
            pts: pts.iter().zip(ele).map(|(&p, &e)| { let g = geo(p); [round(g[0], 1e6), round(g[1], 1e6), round(e + SEA, 10.0)] }).collect(),
        });
    }
    for g in graded {
        eprintln!("  road {g}");
    }

    // 4. Decor: hand-placed pieces, then scattered ones, each standing on the finished ground.
    let mut props = vec![];
    let mut put = |k: &str, p: Xz, yaw: f64, v: u32, sea: bool, height: &[f64]| {
        let g = geo(p);
        let e = if sea { SEA } else { sample(height, w, h, cell, p) + SEA };
        props.push(Prop { k: k.to_owned(), lat: round(g[0], 1e6), lon: round(g[1], 1e6), e: round(e, 10.0) as f32, y: yaw.round() as f32, v });
    };
    for p in &map.props {
        put(&p.k, p.at, p.yaw, p.v, p.sea, &height);
    }
    for row in &map.rows {
        let n = (dist(row.from, row.to) / row.step).floor() as usize;
        for i in 0..=n {
            let t = if n == 0 { 0.0 } else { i as f64 / n as f64 };
            let p = [row.from[0] + (row.to[0] - row.from[0]) * t, row.from[1] + (row.to[1] - row.from[1]) * t];
            put(&row.k, p, row.yaw, i as u32 % row.variants.max(1), false, &height);
        }
    }
    for (n, s) in map.scatter.iter().enumerate() {
        let poly = closed(&s.poly);
        let (x0, x1) = s.poly.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p[0]), b.max(p[0])));
        let (z0, z1) = s.poly.iter().fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p[1]), b.max(p[1])));
        let (mut placed, mut k) = (0, 0u64);
        while placed < s.count && k < s.count as u64 * 50 {
            k += 1;
            let p = [x0 + unit(n as u64, k, 1) * (x1 - x0), z0 + unit(n as u64, k, 2) * (z1 - z0)];
            let (c, r) = ((p[0] / cell) as usize, (p[1] / cell) as usize);
            if c >= w || r >= h || !inside(&poly, p) { continue }
            if matches!(cover[r * w + c], terrain::WATER | terrain::ROAD | LAVA) { continue }
            put(&s.k, p, unit(n as u64, k, 3) * 360.0, (unit(n as u64, k, 4) * s.variants.max(1) as f64) as u32, false, &height);
            placed += 1;
        }
    }

    let start = map.nodes.get(&map.start).with_context(|| format!("no start node {}", map.start))?;
    let terrain = Terrain {
        w,
        h,
        west: LON0,
        east: LON0 + map.size[0] / kx,
        north: LAT0,
        south: LAT0 - map.size[1] / kz,
        heights: height.iter().map(|&e| (e + SEA).max(0.0) as f32).collect(),
        cover,
    };
    let mut city = City::island(map.name, "Île dessinée à la main pour le-mollet.".into(), geo(*start), streets, props);
    (city.preview, city.preview_w) = preview(&terrain.cover, w, h, 20);
    Ok(Island { city, terrain })
}

/// Thumbnail: the main ground of each `block`-cell square, roads first so they show; one letter per block.
fn preview(cover: &[u8], w: usize, h: usize, block: usize) -> (String, usize) {
    let (pw, ph) = (w / block, h / block);
    let mut s = String::with_capacity(pw * ph);
    for br in 0..ph {
        for bc in 0..pw {
            let mut count = [0usize; 16];
            for r in br * block..(br + 1) * block {
                for c in bc * block..(bc + 1) * block {
                    count[cover[r * w + c] as usize & 15] += 1;
                }
            }
            let main = if count[terrain::ROAD as usize] * 12 > block * block { terrain::ROAD } else { (0..16).max_by_key(|&k| count[k]).unwrap() as u8 };
            s.push((b'a' + main) as char);
        }
    }
    (s, pw)
}

fn apply_relief(rel: &Relief, height: &mut [f64], cover: &mut [u8], w: usize, h: usize, cell: f64) {
    let cells = |at: Xz, r: f64| {
        let (c0, c1) = (((at[0] - r) / cell).max(0.0) as usize, (((at[0] + r) / cell) as usize + 1).min(w));
        let (r0, r1) = (((at[1] - r) / cell).max(0.0) as usize, (((at[1] + r) / cell) as usize + 1).min(h));
        (r0..r1).flat_map(move |rr| (c0..c1).map(move |cc| (cc, rr)))
    };
    match rel {
        Relief::Cone { at, r, h: top, crater, depth } => {
            for (c, rr) in cells(*at, *r) {
                let p = [(c as f64 + 0.5) * cell, (rr as f64 + 0.5) * cell];
                let d = dist(p, *at);
                let i = rr * w + c;
                if d >= *r { continue }
                let slope = if d > *crater { top * (r * r - d * d) / (r * r - crater * crater) } else { top - depth * smoothstep(0.0, 18.0, crater - d) };
                height[i] += slope;
                if d < crater - 10.0 { cover[i] = LAVA } else if d < r * 0.75 { cover[i] = BASALT }
            }
        }
        Relief::Hill { at, r, h: top } => {
            for (c, rr) in cells(*at, *r) {
                let d = dist([(c as f64 + 0.5) * cell, (rr as f64 + 0.5) * cell], *at);
                if d < *r { height[rr * w + c] += top * 0.5 * (1.0 + (std::f64::consts::PI * d / r).cos()) }
            }
        }
        Relief::Plateau { poly, h: top, edge } => {
            let poly = closed(poly);
            for_cells_in(&poly, w, h, cell, |i, p| height[i] += top * smoothstep(0.0, *edge, dist_to_line(&poly, p)));
        }
        Relief::Dunes { poly, amp, wave } => {
            let poly = closed(poly);
            for_cells_in(&poly, w, h, cell, |i, p| {
                let k = smoothstep(0.0, 80.0, dist_to_line(&poly, p));
                height[i] += amp * k * (0.5 + 0.5 * (p[0] / wave + (p[1] / (wave * 2.7)).sin() * 1.5).sin());
            });
        }
    }
}

/// Limit the grade to MAX_GRADE, sweeping from both ends so the junction heights stay put.
fn cap_grade(mut ele: Vec<f64>, pts: &[Xz]) -> Vec<f64> {
    let n = ele.len();
    for _ in 0..4 {
        for i in 1..n - 1 {
            let m = MAX_GRADE * dist(pts[i - 1], pts[i]);
            ele[i] = ele[i].clamp(ele[i - 1] - m, ele[i - 1] + m);
        }
        for i in (1..n - 1).rev() {
            let m = MAX_GRADE * dist(pts[i], pts[i + 1]);
            ele[i] = ele[i].clamp(ele[i + 1] - m, ele[i + 1] + m);
        }
    }
    ele
}

/// Flatten the road strip at the road's height, banks blended back over `BANK_M`, cover set to road.
fn carve(pts: &[Xz], ele: &[f64], half: f64, height: &mut [f64], cover: &mut [u8], (w, h, cell): (usize, usize, f64)) {
    let reach = half + BANK_M;
    let (mut best_d, mut best_e) = (vec![f64::MAX; w * h], vec![0.0; w * h]);
    for i in 0..pts.len().saturating_sub(1) {
        let (a, b) = (pts[i], pts[i + 1]);
        let (c0, c1) = (((a[0].min(b[0]) - reach) / cell).max(0.0) as usize, (((a[0].max(b[0]) + reach) / cell) as usize + 1).min(w));
        let (r0, r1) = (((a[1].min(b[1]) - reach) / cell).max(0.0) as usize, (((a[1].max(b[1]) + reach) / cell) as usize + 1).min(h));
        for r in r0..r1 {
            for c in c0..c1 {
                let p = [(c as f64 + 0.5) * cell, (r as f64 + 0.5) * cell];
                let (d, t) = seg_dist(p, a, b);
                let k = r * w + c;
                if d < best_d[k] { best_d[k] = d; best_e[k] = ele[i] + (ele[i + 1] - ele[i]) * t }
            }
        }
    }
    for k in 0..w * h {
        let d = best_d[k];
        if d >= reach { continue }
        let bank = smoothstep(half, reach, d);
        height[k] = best_e[k] + (height[k] - best_e[k]) * bank;
        if d <= half { cover[k] = terrain::ROAD }
    }
}

/// Spiral from `from` around `s.center`, its radius shrinking linearly to `s.r1` over `s.turns`.
fn spiral(from: Xz, s: &Spiral) -> Vec<Xz> {
    let (r0, a0) = (dist(from, s.center), (from[1] - s.center[1]).atan2(from[0] - s.center[0]));
    let dir = if s.ccw { -1.0 } else { 1.0 }; // z points south: +angle turns clockwise seen from above
    let n = (s.turns * 360.0) as usize;
    (0..=n).map(|i| {
        let t = i as f64 / n as f64;
        let (r, a) = (r0 + (s.r1 - r0) * t, a0 + dir * t * s.turns * std::f64::consts::TAU);
        [s.center[0] + r * a.cos(), s.center[1] + r * a.sin()]
    }).collect()
}

/// Smooth curve through hand-placed points (centripetal enough for road bends at this spacing).
fn catmull_rom(p: &[Xz]) -> Vec<Xz> {
    if p.len() < 3 { return p.to_vec() }
    let mut out = vec![];
    for i in 0..p.len() - 1 {
        let (p0, p1, p2, p3) = (p[i.saturating_sub(1)], p[i], p[i + 1], p[(i + 2).min(p.len() - 1)]);
        let n = (dist(p1, p2) / 4.0).ceil().max(1.0) as usize;
        for k in 0..n {
            let t = k as f64 / n as f64;
            let (t2, t3) = (t * t, t * t * t);
            let f = |a: f64, b: f64, c: f64, d: f64| 0.5 * (2.0 * b + (-a + c) * t + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2 + (-a + 3.0 * b - 3.0 * c + d) * t3);
            out.push([f(p0[0], p1[0], p2[0], p3[0]), f(p0[1], p1[1], p2[1], p3[1])]);
        }
    }
    out.push(*p.last().unwrap());
    out
}

fn resample_xz(line: &[Xz], step: f64) -> Vec<Xz> {
    let mut out = vec![line[0]];
    let (mut walked, mut next) = (0.0, step);
    for s in line.windows(2) {
        let seg = dist(s[0], s[1]);
        while seg > 0.0 && next <= walked + seg {
            let t = (next - walked) / seg;
            out.push([s[0][0] + (s[1][0] - s[0][0]) * t, s[0][1] + (s[1][1] - s[0][1]) * t]);
            next += step;
        }
        walked += seg;
    }
    let last = *line.last().unwrap();
    if dist(*out.last().unwrap(), last) > 1.0 { out.push(last) } else { *out.last_mut().unwrap() = last }
    out
}

fn closed(p: &[Xz]) -> Vec<Xz> {
    let mut v = p.to_vec();
    if v.first() != v.last() { v.push(v[0]) }
    v
}

fn dist(a: Xz, b: Xz) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

fn arc(p: &[Xz]) -> f64 {
    p.windows(2).map(|s| dist(s[0], s[1])).sum()
}

/// Distance from p to segment ab, and where along it (0..1).
fn seg_dist(p: Xz, a: Xz, b: Xz) -> (f64, f64) {
    let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
    let l2 = dx * dx + dz * dz;
    let t = if l2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dz) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (dist(p, [a[0] + dx * t, a[1] + dz * t]), t)
}

fn dist_to_line(line: &[Xz], p: Xz) -> f64 {
    line.windows(2).map(|s| seg_dist(p, s[0], s[1]).0).fold(f64::MAX, f64::min)
}

fn inside(poly: &[Xz], p: Xz) -> bool {
    let mut odd = false;
    for s in poly.windows(2) {
        let (a, b) = (s[0], s[1]);
        if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]) { odd = !odd }
    }
    odd
}

fn for_cells_in(poly: &[Xz], w: usize, h: usize, cell: f64, mut f: impl FnMut(usize, Xz)) {
    let ring: terrain::Ring = poly.iter().map(|p| (p[0] / cell, p[1] / cell)).collect();
    terrain::fill_even_odd(&[ring], w, h, |c, r| f(r * w + c, [(c as f64 + 0.5) * cell, (r as f64 + 0.5) * cell]));
}

fn for_cells_near(line: &[Xz], reach: f64, w: usize, h: usize, cell: f64, mut f: impl FnMut(usize, f64)) {
    let line = resample_xz(&catmull_rom(line), 5.0);
    let mut best = vec![f64::MAX; w * h];
    for s in line.windows(2) {
        let (c0, c1) = (((s[0][0].min(s[1][0]) - reach) / cell).max(0.0) as usize, (((s[0][0].max(s[1][0]) + reach) / cell) as usize + 1).min(w));
        let (r0, r1) = (((s[0][1].min(s[1][1]) - reach) / cell).max(0.0) as usize, (((s[0][1].max(s[1][1]) + reach) / cell) as usize + 1).min(h));
        for r in r0..r1 {
            for c in c0..c1 {
                let d = seg_dist([(c as f64 + 0.5) * cell, (r as f64 + 0.5) * cell], s[0], s[1]).0;
                best[r * w + c] = best[r * w + c].min(d);
            }
        }
    }
    for (i, &d) in best.iter().enumerate() {
        if d < reach { f(i, d) }
    }
}

/// Bilinear ground height at p between cell centers.
fn sample(height: &[f64], w: usize, h: usize, cell: f64, p: Xz) -> f64 {
    let (x, y) = ((p[0] / cell - 0.5).clamp(0.0, (w - 1) as f64), (p[1] / cell - 0.5).clamp(0.0, (h - 1) as f64));
    let (c, r) = ((x as usize).min(w - 2), (y as usize).min(h - 2));
    let (tx, ty) = (x - c as f64, y - r as f64);
    let at = |c: usize, r: usize| height[r * w + c];
    let top = at(c, r) + (at(c + 1, r) - at(c, r)) * tx;
    let bottom = at(c, r + 1) + (at(c + 1, r + 1) - at(c, r + 1)) * tx;
    top + (bottom - top) * ty
}

fn smoothstep(a: f64, b: f64, x: f64) -> f64 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Deterministic 0..1 from three integers: scattered decor lands in the same place every build.
fn unit(a: u64, b: u64, c: u64) -> f64 {
    let mut x = a.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ b.wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ c.wrapping_mul(0x1656_67B1_9E37_79F9);
    x ^= x >> 31;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 29;
    (x >> 11) as f64 / (1u64 << 53) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spiral_ends_at_inner_radius() {
        let s = Spiral { center: [0.0, 0.0], r1: 100.0, turns: 2.0, ccw: false };
        let p = spiral([500.0, 0.0], &s);
        assert!((dist(p[0], [500.0, 0.0])) < 1e-6);
        assert!((dist(*p.last().unwrap(), [0.0, 0.0]) - 100.0).abs() < 1e-6);
    }

    #[test]
    fn inside_and_distance() {
        let sq = closed(&[[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]);
        assert!(inside(&sq, [5.0, 5.0]) && !inside(&sq, [15.0, 5.0]));
        assert!((dist_to_line(&sq, [5.0, 5.0]) - 5.0).abs() < 1e-9);
    }
}
