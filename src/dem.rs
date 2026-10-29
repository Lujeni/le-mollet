//! Elevation outside France, where IGN has none: the Terrain Tiles on AWS Open Data (Mapzen/Tilezen,
//! "elevation-tiles-prod"), built from SRTM, EU-DEM (Copernicus) and other open sources. Terrarium PNG tiles
//! in Web Mercator, elevation = R * 256 + G + B / 256 - 32768 m. About 30 m of real resolution in Europe:
//! good enough under the 50 m smoothing of course grades. See docs/courses.md.

use crate::course::UA;
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::io::Read;

/// Attribution for courses whose elevation comes from here.
pub const ATTRIBUTION: &str = "Elevation: Terrain Tiles on AWS Open Data (SRTM, EU-DEM © Copernicus).";
/// Zoom 13: ~13 m a pixel at 45° N, finer than the data underneath.
const ZOOM: u32 = 13;
const PX: usize = 256;

/// Tiles fetched so far, decoded to meters.
#[derive(Default)]
pub struct Tiles(HashMap<(u32, u32), Vec<f32>>);

impl Tiles {
    /// Elevation at a point, m: bilinear between pixel centers.
    pub fn ele(&mut self, lat: f64, lon: f64) -> Result<f32> {
        let (x, y) = pixel(lat, lon);
        let (x0, y0) = ((x - 0.5).floor(), (y - 0.5).floor());
        let (fx, fy) = ((x - 0.5 - x0) as f32, (y - 0.5 - y0) as f32);
        let mut at = |dx: f64, dy: f64| self.px((x0 + dx) as i64, (y0 + dy) as i64);
        let (a, b, c, d) = (at(0.0, 0.0)?, at(1.0, 0.0)?, at(0.0, 1.0)?, at(1.0, 1.0)?);
        Ok((a + (b - a) * fx) * (1.0 - fy) + (c + (d - c) * fx) * fy)
    }

    /// One global pixel at `ZOOM`, fetching its tile on first use.
    fn px(&mut self, x: i64, y: i64) -> Result<f32> {
        let n = (PX as i64) << ZOOM;
        let (x, y) = (x.rem_euclid(n) as u64, y.clamp(0, n - 1) as u64);
        let key = ((x / PX as u64) as u32, (y / PX as u64) as u32);
        if let std::collections::hash_map::Entry::Vacant(e) = self.0.entry(key) {
            e.insert(fetch(key.0, key.1)?);
        }
        Ok(self.0[&key][(y as usize % PX) * PX + x as usize % PX])
    }
}

/// Fractional global pixel coordinates at `ZOOM` (Web Mercator).
fn pixel(lat: f64, lon: f64) -> (f64, f64) {
    let n = (PX << ZOOM) as f64;
    let r = lat.to_radians();
    ((lon + 180.0) / 360.0 * n, (1.0 - (r.tan() + 1.0 / r.cos()).ln() / std::f64::consts::PI) / 2.0 * n)
}

fn fetch(x: u32, y: u32) -> Result<Vec<f32>> {
    let url = format!("https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{ZOOM}/{x}/{y}.png");
    let mut bytes = vec![];
    ureq::get(&url).set("User-Agent", UA).call().with_context(|| format!("elevation tile {x},{y}"))?.into_reader().read_to_end(&mut bytes)?;
    decode(&bytes)
}

/// A Terrarium PNG to meters.
fn decode(png_bytes: &[u8]) -> Result<Vec<f32>> {
    let mut reader = png::Decoder::new(png_bytes).read_info().context("elevation tile: not a PNG")?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf)?;
    let ch = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        c => bail!("elevation tile: color type {c:?}"),
    };
    if (info.width as usize, info.height as usize) != (PX, PX) || info.bit_depth != png::BitDepth::Eight {
        bail!("elevation tile: unexpected format")
    }
    Ok(buf.chunks_exact(ch).take(PX * PX).map(|p| p[0] as f32 * 256.0 + p[1] as f32 + p[2] as f32 / 256.0 - 32768.0).collect())
}

/// Elevation of `w` x `h` cell centers over a lat/lon box, rows north to south (like the terrain file).
pub fn grid(west: f64, south: f64, east: f64, north: f64, w: usize, h: usize) -> Result<Vec<f32>> {
    let mut tiles = Tiles::default();
    let mut out = Vec::with_capacity(w * h);
    for r in 0..h {
        let lat = north - (r as f64 + 0.5) / h as f64 * (north - south);
        for c in 0..w {
            out.push(tiles.ele(lat, west + (c as f64 + 0.5) / w as f64 * (east - west))?);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terrarium_pixels_decode_to_meters() {
        // One 256x256 RGB tile where every pixel encodes 1234.5 m.
        let e = 1234.5f32 + 32768.0;
        let (r, g, b) = ((e / 256.0).floor() as u8, (e % 256.0).floor() as u8, ((e.fract()) * 256.0) as u8);
        let mut png_bytes = vec![];
        {
            let mut enc = png::Encoder::new(&mut png_bytes, 256, 256);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(&[r, g, b].repeat(256 * 256)).unwrap();
        }
        let v = decode(&png_bytes).unwrap();
        assert_eq!(v.len(), 256 * 256);
        assert!((v[0] - 1234.5).abs() < 0.01, "{}", v[0]);
        // Web Mercator: (0, 0) is the middle of the world; the date line is the left edge.
        let n = (PX << ZOOM) as f64;
        assert!((pixel(0.0, 0.0).0 - n / 2.0).abs() < 1e-6 && (pixel(0.0, 0.0).1 - n / 2.0).abs() < 1e-6);
        assert!(pixel(0.0, -180.0).0.abs() < 1e-6);
    }
}
