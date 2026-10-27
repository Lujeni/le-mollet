//! Decor of a course read from the real place. The IGN aerial photo (BD ORTHO, Licence Ouverte Etalab 2.0)
//! paints the ground as a mosaic of Lego colors and finds the trees; OpenStreetMap (ODbL) places what a
//! photo cannot tell: trees with their species, benches, street lamps, fences, hedges, the windmill.
//! `le-mollet decor courses/<id>.json` after `course`: rewrites the ground codes of the terrain file and the
//! course's `props`. See docs/courses.md.

use crate::city::Prop;
use crate::course::{Course, UA, haversine_m, request_json};
use crate::terrain::{BUILDING, GRASS, FOREST, ROAD, ROCK, Terrain, WATER};
use anyhow::{Context, Result, bail};
use std::collections::HashMap;

/// Ground colors of the mosaic, official Lego colors; cover code = `MOSAIC0` + index. Same list, same order,
/// in web/index.html (MOSAIC).
pub const MOSAIC: [[u8; 3]; 14] = [
    [0x4b, 0x9f, 0x4a], // bright green
    [0x23, 0x78, 0x41], // green
    [0x18, 0x46, 0x32], // dark green
    [0x9b, 0x9a, 0x5a], // olive green
    [0xa0, 0xbc, 0xac], // sand green
    [0xe4, 0xcd, 0x9e], // tan
    [0x95, 0x8a, 0x73], // dark tan
    [0xa0, 0xa5, 0xa9], // light bluish gray
    [0x6c, 0x6e, 0x68], // dark bluish gray
    [0xf4, 0xf4, 0xf4], // white
    [0x58, 0x2a, 0x12], // reddish brown
    [0xaa, 0x7d, 0x55], // medium nougat
    [0x35, 0x21, 0x00], // dark brown
    [0xa5, 0xca, 0x18], // lime
];
pub const MOSAIC0: u8 = 32;
/// Woods away from the road: a canopy of bricks, raised by the page.
pub const CANOPY: u8 = 13;
/// Trees as 3D pieces within this distance of the road (canopy beyond); other decor within `PROPS_M`.
const TREES_M: f64 = 150.0;
const PROPS_M: f64 = 300.0;
/// No decor on the road itself.
const ROAD_CLEAR_M: f64 = 4.5;
/// Photo resolution for the trees, m a pixel: crowns show their shadows at 50 cm.
const FINE_M: f64 = 0.5;
/// The photo is dull next to Lego plastic: saturation and brightness before picking a color.
/// Calibration knobs, tuned on Longchamp.
const SATURATION: f32 = 2.2;
const GAIN: f32 = 1.45;

/// One RGB picture of a box, fetched in tiles of at most 4096 px a side.
struct Photo {
    w: usize,
    h: usize,
    rgb: Vec<u8>,
}

impl Photo {
    fn fetch([west, south, east, north]: [f64; 4], w: usize, h: usize) -> Result<Self> {
        const TILE: usize = 4096;
        let mut rgb = vec![0u8; w * h * 3];
        let (dlon, dlat) = ((east - west) / w as f64, (north - south) / h as f64);
        for y0 in (0..h).step_by(TILE) {
            for x0 in (0..w).step_by(TILE) {
                let (tw, th) = (TILE.min(w - x0), TILE.min(h - y0));
                let bbox = [north - (y0 + th) as f64 * dlat, west + x0 as f64 * dlon, north - y0 as f64 * dlat, west + (x0 + tw) as f64 * dlon];
                let jpg = crate::ortho::get_jpeg(bbox, tw, th)?;
                let mut dec = jpeg_decoder::Decoder::new(&jpg[..]);
                let px = dec.decode().context("aerial photo: bad JPEG")?;
                let info = dec.info().context("aerial photo: no JPEG info")?;
                if info.pixel_format != jpeg_decoder::PixelFormat::RGB24 || (info.width as usize, info.height as usize) != (tw, th) {
                    bail!("aerial photo: expected {tw}x{th} RGB, got {}x{} {:?}", info.width, info.height, info.pixel_format);
                }
                for row in 0..th {
                    let (src, dst) = (row * tw * 3, ((y0 + row) * w + x0) * 3);
                    rgb[dst..dst + tw * 3].copy_from_slice(&px[src..src + tw * 3]);
                }
                eprintln!("decor: photo tile {x0},{y0} ({tw}x{th})");
            }
        }
        Ok(Self { w, h, rgb })
    }

    /// Mean color, mean luminance and its standard deviation over a pixel window (clipped to the picture).
    fn stats(&self, x0: isize, y0: isize, x1: isize, y1: isize) -> ([f32; 3], f32, f32) {
        let (mut c, mut l, mut l2, mut n) = ([0f32; 3], 0f32, 0f32, 0f32);
        for y in y0.max(0)..y1.min(self.h as isize) {
            for x in x0.max(0)..x1.min(self.w as isize) {
                let i = (y as usize * self.w + x as usize) * 3;
                let p = [self.rgb[i] as f32, self.rgb[i + 1] as f32, self.rgb[i + 2] as f32];
                let lum = 0.3 * p[0] + 0.59 * p[1] + 0.11 * p[2];
                for k in 0..3 { c[k] += p[k] }
                (l, l2, n) = (l + lum, l2 + lum * lum, n + 1.0);
            }
        }
        if n == 0.0 { return ([0.0; 3], 0.0, 0.0) }
        let l = l / n;
        (c.map(|v| v / n), l, (l2 / n - l * l).max(0.0).sqrt())
    }
}

/// Greens of the mosaic: grass in the shade is dull enough to look gray, but stays a green.
const GREENS: [usize; 6] = [0, 1, 2, 3, 4, 13];

/// Nearest mosaic color to a photo color, after the dull-photo correction.
pub fn mosaic_index([r, g, b]: [f32; 3]) -> u8 {
    let green = g > r + 3.0 && g > b + 3.0;
    let m = (r + g + b) / 3.0;
    let [r, g, b] = [r, g, b].map(|v| ((m + (v - m) * SATURATION) * GAIN).min(255.0));
    let d = |p: &[u8; 3]| 2.0 * (r - p[0] as f32).powi(2) + 4.0 * (g - p[1] as f32).powi(2) + 3.0 * (b - p[2] as f32).powi(2);
    let pick = |set: &mut dyn Iterator<Item = usize>| set.min_by(|&a, &c| d(&MOSAIC[a]).total_cmp(&d(&MOSAIC[c]))).unwrap() as u8;
    if green { pick(&mut GREENS.into_iter()) } else { pick(&mut (0..MOSAIC.len())) }
}

/// A tree crown seen from above: green and dark, or green and textured by its own shadows; or an autumn
/// crown, reddish and textured. Luminance and its deviation over a ~4 m window at 50 cm a pixel.
/// Calibration knobs, tuned on Longchamp: the racecourse turf is green too, but smooth.
pub fn is_crown([r, g, b]: [f32; 3], lum: f32, sd: f32) -> bool {
    let green = g >= r - 2.0 && g >= b - 4.0;
    let autumn = r > g + 4.0 && r > b + 12.0 && lum < 115.0 && sd > 11.0;
    (green && (lum < 58.0 || (lum < 92.0 && sd > 9.0))) || autumn
}

/// Foliage of a crown, for the tree variant: green, dark green, olive, yellow, orange, red-brown.
fn foliage([r, g, b]: [f32; 3]) -> u32 {
    let lum = 0.3 * r + 0.59 * g + 0.11 * b;
    if r > g + 12.0 { 5 } else if r > g + 4.0 { 4 } else if r > g - 4.0 && r > b + 15.0 { 3 } else if lum < 55.0 { 1 } else if g - b < 6.0 { 2 } else { 0 }
}

/// Distance from each cell to the course line, m (capped past `PROPS_M`).
fn road_distance(t: &Terrain, course: &Course) -> Vec<f32> {
    let (cx, cz) = cell_m(t);
    let mut dist = vec![f32::INFINITY; t.w * t.h];
    let reach = (PROPS_M / cx.min(cz)).ceil() as isize;
    for p in course.points.windows(2) {
        for k in 0..5 {
            let f = k as f64 / 5.0;
            let (lat, lon) = (p[0].lat + (p[1].lat - p[0].lat) * f, p[0].lon + (p[1].lon - p[0].lon) * f);
            let (gx, gy) = t.grid(lat, lon);
            for r in (gy as isize - reach).max(0)..(gy as isize + reach).min(t.h as isize) {
                for c in (gx as isize - reach).max(0)..(gx as isize + reach).min(t.w as isize) {
                    let d = (((c as f64 + 0.5 - gx) * cx).hypot((r as f64 + 0.5 - gy) * cz)) as f32;
                    let i = r as usize * t.w + c as usize;
                    if d < dist[i] { dist[i] = d }
                }
            }
        }
    }
    dist
}

fn cell_m(t: &Terrain) -> (f64, f64) {
    ((t.east - t.west) / t.w as f64 * ((t.south + t.north) / 2.0).to_radians().cos() * 111_320.0, (t.north - t.south) / t.h as f64 * 110_540.0)
}

/// Props already placed, bucketed by 10 m square, to keep pieces apart.
#[derive(Default)]
struct Spacing(HashMap<(i64, i64), Vec<Placed>>);
/// Where a piece stands and its radius, m.
type Placed = ([f64; 2], f64);

impl Spacing {
    fn key(p: [f64; 2]) -> (i64, i64) {
        ((p[0] * 11_054.0) as i64, (p[1] * 7_300.0) as i64)
    }
    /// Free when no piece within `(r + its r) * k` m.
    fn free(&self, p: [f64; 2], r: f64, k: f64) -> bool {
        let (a, b) = Self::key(p);
        (a - 1..=a + 1).all(|i| (b - 1..=b + 1).all(|j| self.0.get(&(i, j)).is_none_or(|v| v.iter().all(|&(q, rq)| haversine_m(p, q) >= (r + rq) * k))))
    }
    fn add(&mut self, p: [f64; 2], r: f64) {
        self.0.entry(Self::key(p)).or_default().push((p, r));
    }
}

/// Rewrite the terrain's ground with the photo mosaic and the canopy, and the course's props.
pub fn build(course: &mut Course, t: &mut Terrain) -> Result<()> {
    let (cx, cz) = cell_m(t);
    // Small cells show the 1 m elevation model's noise as a maze of half-meter steps on flat ground:
    // smooth it over ~5 m, then carve the road again.
    if cx < 3.0 { smooth_heights(t, 1.5); t.carve_road(course) }
    let dist = road_distance(t, course);
    let (north, south, west, east, tw, th) = (t.north, t.south, t.west, t.east, t.w as f64, t.h as f64);
    let geo = move |c: f64, r: f64| [north - r / th * (north - south), west + c / tw * (east - west)];

    // 1. The ground, one photo pixel a cell: mosaic colors; dark woods away from the road become canopy.
    let photo = Photo::fetch([t.west, t.south, t.east, t.north], t.w, t.h)?;
    let mut counts = [0usize; MOSAIC.len()];
    for r in 0..t.h {
        for c in 0..t.w {
            let i = r * t.w + c;
            let ground = [GRASS, FOREST, ROCK, CANOPY].contains(&t.cover[i]) || t.cover[i] >= MOSAIC0; // painted before: paint again
            if !ground { continue }
            let (col, lum, _) = photo.stats(c as isize - 1, r as isize - 1, c as isize + 2, r as isize + 2);
            let woods = lum < 58.0 && col[1] >= col[0] - 2.0 && col[1] >= col[2] - 4.0;
            t.cover[i] = if woods && dist[i] as f64 > TREES_M { CANOPY } else {
                let k = mosaic_index(col);
                counts[k as usize] += 1;
                MOSAIC0 + k
            };
        }
    }
    // A mosaic, not noise: each tile takes the color most of its 3x3 neighbors have, twice.
    for _ in 0..2 { majority(t) }
    eprintln!("decor: mosaic {counts:?}, canopy {}", t.cover.iter().filter(|&&c| c == CANOPY).count());

    // 2. OpenStreetMap pieces near the road.
    let mut props = vec![];
    let mut taken = Spacing::default();
    let near = |p: [f64; 2]| {
        let (gx, gy) = t.grid(p[0], p[1]);
        let (c, r) = (gx as isize, gy as isize);
        if c < 0 || r < 0 || c >= t.w as isize || r >= t.h as isize { return None }
        let i = r as usize * t.w + c as usize;
        let d = dist[i] as f64;
        (d > ROAD_CLEAR_M && d <= PROPS_M && t.cover[i] != WATER && t.cover[i] != ROAD).then_some((i, d))
    };
    let at = |k: &str, p: [f64; 2], yaw: f64, v: u32, i: usize| Prop { k: k.into(), lat: round(p[0], 1e6), lon: round(p[1], 1e6), e: t.heights[i], y: yaw.round() as f32, v };
    let mut paved = vec![];
    let osm = osm_features(t)?;
    let kind = |f: &Feature| match f { Feature::Tree { .. } => "tree", Feature::Line { kind, .. } | Feature::Point { kind, .. } => kind, Feature::Mill { .. } => "windmill" };
    let mut found = std::collections::BTreeMap::new();
    for f in &osm { *found.entry(kind(f)).or_insert(0) += 1 }
    eprintln!("decor: OpenStreetMap {found:?}");
    for f in &osm {
        match f {
            Feature::Tree { at: p, conifer, crown } => {
                let Some((i, _)) = near(*p) else { continue };
                let r = crown.unwrap_or(3.5).clamp(1.5, 7.0);
                if !taken.free(*p, r, 0.6) { continue }
                taken.add(*p, r);
                props.push(at(if *conifer { "pine" } else { "oak" }, *p, yaw_of(p), size_class(r) * 8, i));
            }
            Feature::Line { kind, pts } => {
                let (step, k) = match *kind { "tree_row" => (8.0, "oak"), "hedge" => (1.6, "hedge"), "wall" => (1.6, "lowwall"), _ => (2.4, "rail") };
                for (p, yaw) in along(pts, step) {
                    let Some((i, _)) = near(p) else { continue };
                    if k == "oak" {
                        if !taken.free(p, 3.5, 0.6) { continue }
                        taken.add(p, 3.5);
                        props.push(at(k, p, yaw_of(&p), size_class(3.5) * 8, i));
                    } else {
                        props.push(at(k, p, yaw, 0, i));
                    }
                }
            }
            Feature::Point { kind, at: p } => {
                let Some((i, _)) = near(*p) else { continue };
                // Benches face the road.
                let yaw = if *kind == "bench" { facing(course, *p) - 90.0 } else { yaw_of(p) }; // the bench model sits facing +z
                taken.add(*p, 1.0);
                props.push(at(kind, *p, yaw, 0, i));
            }
            Feature::Mill { at: p, height } => {
                let (gx, gy) = t.grid(p[0], p[1]);
                let i = (gy.max(0.0) as usize).min(t.h - 1) * t.w + (gx.max(0.0) as usize).min(t.w - 1);
                // The piece replaces the building block on its footprint: paving at ground level.
                let reach = (6.0 / cx.min(cz)).ceil() as isize;
                for r in (gy as isize - reach).max(0)..(gy as isize + reach).min(t.h as isize) {
                    for c in (gx as isize - reach).max(0)..(gx as isize + reach).min(t.w as isize) {
                        paved.push(r as usize * t.w + c as usize);
                    }
                }
                taken.add(*p, 6.0);
                props.push(at("towermill", *p, facing(course, *p), height.round() as u32, i));
            }
        }
    }
    for j in paved { if t.cover[j] == BUILDING { t.cover[j] = MOSAIC0 + 7 } }
    let n_osm = props.len();

    // 3. Trees from the photo at 50 cm, within TREES_M of the road: crown cells, then pieces sized by
    //    how deep they sit in their crown, the big ones first.
    let fold = |f: fn(f64, f64) -> f64, init: f64, get: fn(&crate::course::Point) -> f64| course.points.iter().map(get).fold(init, f);
    let (mlat, mlon) = ((TREES_M + 20.0) / 110_540.0, (TREES_M + 20.0) / (111_320.0 * course.points[0].lat.to_radians().cos()));
    let bbox = [fold(f64::min, 180.0, |p| p.lon) - mlon, fold(f64::min, 90.0, |p| p.lat) - mlat, fold(f64::max, -180.0, |p| p.lon) + mlon, fold(f64::max, -90.0, |p| p.lat) + mlat];
    let (fw, fh) = (((bbox[2] - bbox[0]) * 111_320.0 * course.points[0].lat.to_radians().cos() / FINE_M) as usize, ((bbox[3] - bbox[1]) * 110_540.0 / FINE_M) as usize);
    let fine = Photo::fetch(bbox, fw, fh)?;
    let px = |lat: f64, lon: f64| (((lon - bbox[0]) / (bbox[2] - bbox[0]) * fw as f64) as isize, ((bbox[3] - lat) / (bbox[3] - bbox[1]) * fh as f64) as isize);
    let half = (2.0 / FINE_M) as isize; // ~4 m window
    let mut crown = vec![0u8; t.w * t.h];
    let mut color = vec![[0f32; 3]; t.w * t.h];
    for r in 0..t.h {
        for c in 0..t.w {
            let i = r * t.w + c;
            if dist[i] as f64 > TREES_M || dist[i] as f64 <= ROAD_CLEAR_M || [WATER, BUILDING, ROAD].contains(&t.cover[i]) { continue }
            let p = geo(c as f64 + 0.5, r as f64 + 0.5);
            let (x, y) = px(p[0], p[1]);
            let (col, lum, sd) = fine.stats(x - half, y - half, x + half, y + half);
            if is_crown(col, lum, sd) { crown[i] = 1 }
            color[i] = col;
        }
    }
    // Majority filter: lone cells and thin lines (turf edges, hedges seen side on) go, holes fill.
    let mut mask = vec![false; t.w * t.h];
    for r in 1..t.h - 1 {
        for c in 1..t.w - 1 {
            let i = r * t.w + c;
            let k: u8 = [i - t.w - 1, i - t.w, i - t.w + 1, i - 1, i + 1, i + t.w - 1, i + t.w, i + t.w + 1].iter().map(|&j| crown[j]).sum();
            mask[i] = if crown[i] == 1 { k >= 5 } else { k >= 7 && dist[i] as f64 <= TREES_M };
        }
    }
    // Depth in the crown: chamfer distance to the nearest open cell, in cells.
    let depth = chamfer(&mask, t.w, t.h);
    let mut cells: Vec<usize> = (0..t.w * t.h).filter(|&i| mask[i]).collect();
    cells.sort_by(|&a, &b| depth[b].total_cmp(&depth[a]).then(a.cmp(&b)));
    let cell = (cx + cz) / 2.0;
    for i in cells {
        let (c, r) = (i % t.w, i / t.w);
        let jit = |s: u64| (unit(c as u64, r as u64, s) - 0.5) * 0.8;
        let p = geo(c as f64 + 0.5 + jit(1), r as f64 + 0.5 + jit(2));
        let radius = (depth[i] as f64 * cell + 1.0).clamp(1.5, 6.5);
        if !taken.free(p, radius, 0.7) { continue }
        taken.add(p, radius);
        props.push(at("oak", p, yaw_of(&p), size_class(radius) * 8 + foliage(color[i]), i));
    }
    eprintln!("decor: {n_osm} pieces from OpenStreetMap, {} trees from the photo", props.len() - n_osm);

    props.sort_by(|a, b| a.k.cmp(&b.k).then(a.lat.total_cmp(&b.lat)).then(a.lon.total_cmp(&b.lon)));
    course.props = props;
    const CREDIT: &str = " Décor : photo aérienne IGN BD ORTHO (Licence Ouverte Etalab 2.0) et OpenStreetMap.";
    if !course.attribution.contains("BD ORTHO") { course.attribution.push_str(CREDIT) }
    Ok(())
}

/// Mosaic tiles take the most common mosaic color around them when at least 5 of the 9 share it.
fn majority(t: &mut Terrain) {
    let src = t.cover.clone();
    for r in 1..t.h - 1 {
        for c in 1..t.w - 1 {
            let i = r * t.w + c;
            if src[i] < MOSAIC0 { continue }
            let mut n = [0u8; MOSAIC.len()];
            for j in [i - t.w - 1, i - t.w, i - t.w + 1, i - 1, i, i + 1, i + t.w - 1, i + t.w, i + t.w + 1] {
                if src[j] >= MOSAIC0 { n[(src[j] - MOSAIC0) as usize] += 1 }
            }
            let (k, &most) = n.iter().enumerate().max_by_key(|&(_, v)| v).unwrap();
            if most >= 5 { t.cover[i] = MOSAIC0 + k as u8 }
        }
    }
}

/// Gaussian blur of the terrain heights, `sigma` in cells (separable, edges clamped).
fn smooth_heights(t: &mut Terrain, sigma: f32) {
    let r = (3.0 * sigma).ceil() as isize;
    let k: Vec<f32> = (-r..=r).map(|i| (-0.5 * (i as f32 / sigma).powi(2)).exp()).collect();
    let sum: f32 = k.iter().sum();
    let (w, h) = (t.w as isize, t.h as isize);
    for (along_x, n, m) in [(true, w, h), (false, h, w)] {
        let src = t.heights.clone();
        for line in 0..m {
            for i in 0..n {
                let v: f32 = (-r..=r).zip(&k).map(|(o, kk)| {
                    let j = (i + o).clamp(0, n - 1);
                    kk * src[if along_x { (line * w + j) as usize } else { (j * w + line) as usize }]
                }).sum();
                t.heights[if along_x { (line * w + i) as usize } else { (i * w + line) as usize }] = v / sum;
            }
        }
    }
}

/// Tree size class from its crown radius, m: 0 small to 3 large.
fn size_class(r: f64) -> u32 {
    match r { r if r < 2.5 => 0, r if r < 3.5 => 1, r if r < 4.8 => 2, _ => 3 }
}

/// A fixed, varied heading for a piece at this place, degrees.
fn yaw_of(p: &[f64; 2]) -> f64 {
    unit((p[0] * 1e6) as u64, (p[1] * 1e6) as u64, 7) * 360.0
}

/// Heading from a point toward the nearest course point, degrees (map yaw: +x east toward +z south).
fn facing(course: &Course, p: [f64; 2]) -> f64 {
    let q = course.points.iter().min_by(|a, b| haversine_m(p, [a.lat, a.lon]).total_cmp(&haversine_m(p, [b.lat, b.lon]))).unwrap();
    let (dx, dz) = ((q.lon - p[1]) * p[0].to_radians().cos(), p[0] - q.lat);
    dz.atan2(dx).to_degrees()
}

/// Points every `step` m along a line, with the line's heading there (degrees, map yaw).
fn along(pts: &[[f64; 2]], step: f64) -> Vec<([f64; 2], f64)> {
    let mut out = vec![];
    let mut next = step / 2.0;
    let mut walked = 0.0;
    for w in pts.windows(2) {
        let seg = haversine_m(w[0], w[1]);
        let yaw = ((w[0][0] - w[1][0]).atan2((w[1][1] - w[0][1]) * w[0][0].to_radians().cos())).to_degrees();
        while seg > 0.0 && next <= walked + seg {
            let f = (next - walked) / seg;
            out.push(([w[0][0] + (w[1][0] - w[0][0]) * f, w[0][1] + (w[1][1] - w[0][1]) * f], yaw));
            next += step;
        }
        walked += seg;
    }
    out
}

/// Distance in cells from each true cell to the nearest false one (3-4 chamfer, divided by 3).
fn chamfer(mask: &[bool], w: usize, h: usize) -> Vec<f32> {
    let mut d: Vec<f32> = mask.iter().map(|&m| if m { f32::INFINITY } else { 0.0 }).collect();
    let pass = |d: &mut Vec<f32>, order: &mut dyn Iterator<Item = usize>, back: bool| {
        for i in order {
            if d[i] == 0.0 { continue }
            let (c, r) = ((i % w) as isize, (i / w) as isize);
            let s: isize = if back { 1 } else { -1 };
            for (dc, dr, cost) in [(s, 0, 3.0), (0, s, 3.0), (s, s, 4.0), (-s, s, 4.0)] {
                let (nc, nr) = (c + dc, r + dr);
                if nc < 0 || nr < 0 || nc >= w as isize || nr >= h as isize { d[i] = d[i].min(cost); continue }
                d[i] = d[i].min(d[nr as usize * w + nc as usize] + cost);
            }
        }
    };
    pass(&mut d, &mut (0..w * h), false);
    pass(&mut d, &mut (0..w * h).rev(), true);
    d.iter().map(|v| v / 3.0).collect()
}

enum Feature {
    Tree { at: [f64; 2], conifer: bool, crown: Option<f64> },
    Line { kind: &'static str, pts: Vec<[f64; 2]> },
    Point { kind: &'static str, at: [f64; 2] },
    Mill { at: [f64; 2], height: f64 },
}

fn osm_features(t: &Terrain) -> Result<Vec<Feature>> {
    let b = format!("({},{},{},{})", t.south, t.west, t.north, t.east);
    let query = format!(
        "[out:json][timeout:180];(node[natural=tree]{b};way[natural=tree_row]{b};node[amenity=bench]{b};\
         node[highway=street_lamp]{b};way[barrier~\"^(fence|hedge|wall)$\"]{b};nwr[man_made=windmill]{b};);out tags geom;"
    );
    let r = request_json(|| ureq::post("https://overpass-api.de/api/interpreter").set("User-Agent", UA).send_form(&[("data", &query)]))?;
    let mut out = vec![];
    for el in r["elements"].as_array().into_iter().flatten() {
        let tag = |k: &str| el["tags"][k].as_str().unwrap_or("");
        let line = || el["geometry"].as_array().into_iter().flatten().filter_map(|p| Some([p["lat"].as_f64()?, p["lon"].as_f64()?])).collect::<Vec<_>>();
        // A node's position, or the middle of a way's outline.
        let point = || match (el["lat"].as_f64(), el["lon"].as_f64()) {
            (Some(lat), Some(lon)) => Some([lat, lon]),
            _ => { let g = line(); (!g.is_empty()).then(|| [g.iter().map(|p| p[0]).sum::<f64>() / g.len() as f64, g.iter().map(|p| p[1]).sum::<f64>() / g.len() as f64]) }
        };
        let num = |k: &str| tag(k).trim_end_matches(" m").parse::<f64>().ok();
        let f = if tag("natural") == "tree" {
            let genus = format!("{} {}", tag("genus"), tag("species")).to_lowercase();
            let conifer = tag("leaf_type") == "needleleaved" || ["pinus", "pin ", "cedrus", "cèdre", "abies", "picea", "taxus", "sequoia", "cupressus"].iter().any(|g| genus.contains(g));
            point().map(|at| Feature::Tree { at, conifer, crown: num("diameter_crown").map(|d| d / 2.0) })
        } else if tag("man_made") == "windmill" {
            point().map(|at| Feature::Mill { at, height: num("height").unwrap_or(15.0) })
        } else if tag("natural") == "tree_row" {
            Some(Feature::Line { kind: "tree_row", pts: line() })
        } else if !tag("barrier").is_empty() {
            Some(Feature::Line { kind: match tag("barrier") { "hedge" => "hedge", "wall" => "wall", _ => "fence" }, pts: line() })
        } else if tag("amenity") == "bench" {
            point().map(|at| Feature::Point { kind: "bench", at })
        } else if tag("highway") == "street_lamp" {
            point().map(|at| Feature::Point { kind: "lamp", at })
        } else {
            None
        };
        out.extend(f);
    }
    Ok(out)
}

fn round(x: f64, k: f64) -> f64 {
    (x * k).round() / k
}

/// Deterministic 0..1 from three integers: the same decor every build.
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
    fn photo_colors_read_as_lego() {
        // Colors measured on the Longchamp aerial photo.
        let name = |c| MOSAIC[mosaic_index(c) as usize];
        assert_eq!(name([70.0, 92.0, 76.0]), MOSAIC[0], "racecourse turf: bright green");
        assert!(GREENS.contains(&(mosaic_index([60.0, 70.0, 63.0]) as usize)), "grass in the shade stays green");
        assert_eq!(name([163.0, 153.0, 142.0]), MOSAIC[5], "pale gravel path: tan");
        assert!(is_crown([43.0, 48.0, 44.0], 47.0, 7.0), "dark woods");
        assert!(is_crown([64.0, 72.0, 60.0], 69.0, 12.0), "a textured crown");
        assert!(!is_crown([70.0, 92.0, 76.0], 84.0, 6.0), "smooth turf is not a tree");
        assert!(!is_crown([189.0, 181.0, 175.0], 183.0, 21.0), "the bare infield is not a tree");
        assert!(is_crown([120.0, 90.0, 70.0], 97.0, 15.0), "an autumn crown");
    }

    #[test]
    fn chamfer_counts_cells_to_the_edge() {
        let mask: Vec<bool> = (0..49).map(|i| (1..6).contains(&(i % 7)) && (1..6).contains(&(i / 7))).collect();
        let d = chamfer(&mask, 7, 7);
        assert_eq!((d[0], d[8], d[24]), (0.0, 1.0, 3.0)); // outside, edge cell, center of a 5x5 block
    }
}
