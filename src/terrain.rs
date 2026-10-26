//! Voxel terrain around a course: IGN elevation grid + OpenStreetMap land cover,
//! with the road carved in. Built offline next to the course JSON, see docs/courses.md.
//!
//! File format (little-endian): u32 width, u32 height, f64 west, south, east, north,
//! then width*height u16 elevations in decimeters, then width*height u8 cover codes.
//! Rows go north to south, columns west to east.

use crate::course::{Course, UA, request_json};
use anyhow::{Result, bail};
use std::io::Read;

/// Cell size, m.
pub const CELL_M: f64 = 5.0;
/// Terrain kept around the course bounding box, m.
const MARGIN_M: f64 = 1500.0;
/// Road half width carved flat, m.
const ROAD_HALF_M: f64 = 4.0;
/// Banks beyond the road are clamped to 45° up to this distance, m.
const BANK_M: f64 = 25.0;

pub const GRASS: u8 = 0;
pub const FOREST: u8 = 1;
pub const WATER: u8 = 2;
pub const BUILDING: u8 = 3;
pub const ROCK: u8 = 4;
pub const ROAD: u8 = 5;
/// City ground between streets and buildings: sidewalks, squares, courtyards.
pub const PAVE: u8 = 6;

/// Polygon ring in fractional grid coordinates (column, row).
type Ring = Vec<(f64, f64)>;

pub struct Terrain {
    pub w: usize,
    pub h: usize,
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
    pub heights: Vec<f32>,
    pub cover: Vec<u8>,
}

impl Terrain {
    pub fn build(course: &Course) -> Result<Self> {
        let lat0 = course.points[0].lat;
        let (kx, kz) = (lat0.to_radians().cos() * 111_320.0, 110_540.0);
        let fold = |f: fn(f64, f64) -> f64, init: f64, get: fn(&crate::course::Point) -> f64| course.points.iter().map(get).fold(init, f);
        let (south, north) = (fold(f64::min, 90.0, |p| p.lat) - MARGIN_M / kz, fold(f64::max, -90.0, |p| p.lat) + MARGIN_M / kz);
        let (west, east) = (fold(f64::min, 180.0, |p| p.lon) - MARGIN_M / kx, fold(f64::max, -180.0, |p| p.lon) + MARGIN_M / kx);
        let mut t = Self::for_bbox(west, south, east, north)?;
        t.paint_land_cover()?;
        t.carve_road(course);
        Ok(t)
    }

    /// Bare terrain (IGN elevation grid, all grass) over a lat/lon box.
    pub fn for_bbox(west: f64, south: f64, east: f64, north: f64) -> Result<Self> {
        let kx = ((south + north) / 2.0).to_radians().cos() * 111_320.0;
        let w = ((east - west) * kx / CELL_M).round() as usize;
        let h = ((north - south) * 110_540.0 / CELL_M).round() as usize;
        // ponytail: single WMS request, IGN accepts up to 4096 px a side; tile it for longer routes
        if w > 4096 || h > 4096 { bail!("terrain {w}x{h} cells exceeds one IGN request (4096 max)") }
        Ok(Self { w, h, west, south, east, north, heights: fetch_heights(west, south, east, north, w, h)?, cover: vec![GRASS; w * h] })
    }

    /// Ground elevation at a point, bilinear between cell centers, m.
    pub fn ele_at(&self, lat: f64, lon: f64) -> f32 {
        let (gx, gy) = self.grid(lat, lon);
        let (x, y) = ((gx - 0.5).clamp(0.0, (self.w - 1) as f64), (gy - 0.5).clamp(0.0, (self.h - 1) as f64));
        let (c, r) = ((x as usize).min(self.w.saturating_sub(2)), (y as usize).min(self.h.saturating_sub(2)));
        let (tx, ty) = ((x - c as f64) as f32, (y - r as f64) as f32);
        let at = |c: usize, r: usize| self.heights[r * self.w + c];
        let top = at(c, r) + (at(c + 1, r) - at(c, r)) * tx;
        let bottom = at(c, r + 1) + (at(c + 1, r + 1) - at(c, r + 1)) * tx;
        top + (bottom - top) * ty
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(40 + self.w * self.h * 3);
        b.extend((self.w as u32).to_le_bytes());
        b.extend((self.h as u32).to_le_bytes());
        for v in [self.west, self.south, self.east, self.north] {
            b.extend(v.to_le_bytes());
        }
        for &e in &self.heights {
            b.extend(((e * 10.0).round().clamp(0.0, u16::MAX as f32) as u16).to_le_bytes());
        }
        b.extend(&self.cover);
        b
    }

    /// Fractional grid coordinates (column, row) of a point.
    pub(crate) fn grid(&self, lat: f64, lon: f64) -> (f64, f64) {
        ((lon - self.west) / (self.east - self.west) * self.w as f64, (self.north - lat) / (self.north - self.south) * self.h as f64)
    }

    pub(crate) fn paint_land_cover(&mut self) -> Result<()> {
        let (s, w, n, e) = (self.south, self.west, self.north, self.east);
        let b = format!("({s},{w},{n},{e})");
        let query = format!(
            "[out:json][timeout:180];(\
             way[natural~\"^(wood|water|bare_rock|scree)$\"]{b};relation[natural~\"^(wood|water|bare_rock|scree)$\"]{b};\
             way[landuse~\"^(forest|reservoir)$\"]{b};relation[landuse~\"^(forest|reservoir)$\"]{b};\
             way[leisure~\"^(park|garden)$\"]{b};way[landuse~\"^(grass|meadow|cemetery)$\"]{b};\
             way[building]{b};);out geom;"
        );
        let r = request_json(|| ureq::post("https://overpass-api.de/api/interpreter").set("User-Agent", UA).send_form(&[("data", &query)]))?;
        let mut features: Vec<(u8, Vec<Ring>)> = vec![];
        for el in r["elements"].as_array().into_iter().flatten() {
            let tag = |k: &str| el["tags"][k].as_str().unwrap_or("");
            let code = match (tag("natural"), tag("landuse")) {
                _ if !tag("building").is_empty() => BUILDING,
                ("water", _) | (_, "reservoir") => WATER,
                _ if !tag("leisure").is_empty() => GRASS,
                (_, "grass" | "meadow" | "cemetery") => GRASS,
                ("bare_rock" | "scree", _) => ROCK,
                _ => FOREST,
            };
            let ring = |g: &serde_json::Value| -> Ring {
                g.as_array().into_iter().flatten().filter_map(|p| Some(self.grid(p["lat"].as_f64()?, p["lon"].as_f64()?))).collect()
            };
            // Relations: even-odd fill over all member edges handles holes without assembling rings.
            let rings = match el["type"].as_str() {
                Some("way") => vec![ring(&el["geometry"])],
                _ => el["members"].as_array().into_iter().flatten().map(|m| ring(&m["geometry"])).collect(),
            };
            features.push((code, rings));
        }
        // Paint order: later wins.
        features.sort_by_key(|f| [GRASS, FOREST, ROCK, WATER, BUILDING].iter().position(|&c| c == f.0));
        for (code, rings) in &features {
            fill_even_odd(rings, self.w, self.h, |c, r| self.cover[r * self.w + c] = *code);
        }
        Ok(())
    }

    /// Flatten a road strip at the course's smoothed elevation, 45° banks beside it.
    pub fn carve_road(&mut self, course: &Course) {
        let cell_x = (self.east - self.west) / self.w as f64 * course.points[0].lat.to_radians().cos() * 111_320.0;
        let cell_z = (self.north - self.south) / self.h as f64 * 110_540.0;
        let mut best = vec![(f64::INFINITY, 0.0f64); self.w * self.h]; // (distance to road, road elevation)
        let reach = (BANK_M / CELL_M).ceil() as isize + 1;
        for p in course.points.windows(2) {
            for k in 0..4 {
                let t = k as f64 / 4.0;
                let (lat, lon) = (p[0].lat + (p[1].lat - p[0].lat) * t, p[0].lon + (p[1].lon - p[0].lon) * t);
                let ele = p[0].ele as f64 + (p[1].ele - p[0].ele) as f64 * t;
                let (gx, gy) = self.grid(lat, lon);
                for r in (gy as isize - reach)..=(gy as isize + reach) {
                    for c in (gx as isize - reach)..=(gx as isize + reach) {
                        if r < 0 || c < 0 || r as usize >= self.h || c as usize >= self.w { continue }
                        let d = (((c as f64 + 0.5 - gx) * cell_x).powi(2) + ((r as f64 + 0.5 - gy) * cell_z).powi(2)).sqrt();
                        let cell = &mut best[r as usize * self.w + c as usize];
                        if d < cell.0 { *cell = (d, ele) }
                    }
                }
            }
        }
        for (i, &(d, e)) in best.iter().enumerate() {
            if d < ROAD_HALF_M {
                self.heights[i] = (e - 0.3) as f32;
                self.cover[i] = ROAD;
            } else if d < BANK_M {
                let slack = d - ROAD_HALF_M;
                self.heights[i] = (self.heights[i] as f64).clamp(e - slack - 0.3, e + slack) as f32;
                if d < 2.0 * ROAD_HALF_M && matches!(self.cover[i], FOREST | BUILDING) {
                    self.cover[i] = GRASS; // clear the shoulders
                }
            }
        }
    }
}

fn fetch_heights(west: f64, south: f64, east: f64, north: f64, w: usize, h: usize) -> Result<Vec<f32>> {
    let url = format!(
        "https://data.geopf.fr/wms-r/wms?SERVICE=WMS&VERSION=1.3.0&REQUEST=GetMap&LAYERS=ELEVATION.ELEVATIONGRIDCOVERAGE.HIGHRES\
         &STYLES=&CRS=EPSG:4326&BBOX={south},{west},{north},{east}&WIDTH={w}&HEIGHT={h}&FORMAT=image/x-bil;bits=32"
    );
    let mut raw = Vec::with_capacity(w * h * 4);
    ureq::get(&url).set("User-Agent", UA).call()?.into_reader().read_to_end(&mut raw)?;
    if raw.len() != w * h * 4 { bail!("IGN elevation grid: got {} bytes, expected {}", raw.len(), w * h * 4) }
    let v: Vec<f32> = raw.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
    if v.iter().any(|&e| e < -1000.0) { bail!("IGN has no elevation for part of the area; course outside France?") }
    Ok(v)
}

/// Scanline even-odd fill of polygon edges given in fractional grid coordinates.
fn fill_even_odd(rings: &[Ring], w: usize, h: usize, mut set: impl FnMut(usize, usize)) {
    let edges: Vec<_> = rings.iter().flat_map(|r| r.windows(2).map(|e| (e[0], e[1]))).collect();
    let (lo, hi) = edges.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), (a, b)| (lo.min(a.1).min(b.1), hi.max(a.1).max(b.1)));
    if edges.is_empty() { return }
    for r in (lo.floor().max(0.0) as usize)..(hi.ceil().min(h as f64) as usize) {
        let y = r as f64 + 0.5;
        let mut xs: Vec<f64> = edges
            .iter()
            .filter(|(a, b)| (a.1 <= y) != (b.1 <= y))
            .map(|(a, b)| a.0 + (y - a.1) / (b.1 - a.1) * (b.0 - a.0))
            .collect();
        xs.sort_by(f64::total_cmp);
        for pair in xs.chunks_exact(2) {
            let (from, to) = ((pair[0] - 0.5).ceil().max(0.0) as usize, ((pair[1] - 0.5).floor() + 1.0).min(w as f64).max(0.0) as usize);
            for c in from..to {
                set(c, r);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn even_odd_fill_with_hole() {
        let sq = |a: f64, b: f64| vec![(a, a), (b, a), (b, b), (a, b), (a, a)];
        let mut grid = [0u8; 100];
        fill_even_odd(&[sq(1.0, 9.0), sq(4.0, 6.0)], 10, 10, |c, r| grid[r * 10 + c] = 1);
        assert_eq!(grid[0], 0); // outside
        assert_eq!(grid[2 * 10 + 2], 1); // inside outer ring
        assert_eq!(grid[5 * 10 + 5], 0); // inside the hole
        assert_eq!(grid.iter().filter(|&&v| v == 1).count(), 64 - 4);
    }
}
