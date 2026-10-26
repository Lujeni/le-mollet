//! The world around a course: terrain grid from the server (IGN elevation + land cover), smooth chunk
//! meshes under the IGN aerial photo tiles, the far mountains, the road ribbon and the conifers.
//! Coordinates: meters, x east, y up (altitude), z south, origin at the course start.

use anyhow::{Result, bail};
use bevy::asset::RenderAssetUsages;
use bevy::image::{CompressedImageFormats, ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor, ImageType};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use le_mollet::course::Point;

/// Terrain cells per chunk side (5 m cells: 320 m), as in the server's terrain format.
pub const CHUNK: usize = 64;
const FOREST: u8 = 1;
const ROAD: u8 = 5;

pub struct Terrain {
    pub w: usize,
    pub h: usize,
    heights: Vec<f32>,
    pub cover: Vec<u8>,
    lat0: f64,
    lon0: f64,
    kx: f64,
    /// Scene position of the north-west corner, and cell size, m.
    pub ox: f32,
    pub oz: f32,
    pub cx: f32,
    pub cz: f32,
}

impl Terrain {
    /// Server terrain file: u32 w, h; f64 west, south, east, north; u16 dm heights; u8 cover codes.
    pub fn parse(b: &[u8], start: &Point) -> Result<Self> {
        if b.len() < 40 { bail!("terrain file too short") }
        let u = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize;
        let f = |o: usize| f64::from_le_bytes(b[o..o + 8].try_into().unwrap());
        let (w, h) = (u(0), u(4));
        let (west, south, east, north) = (f(8), f(16), f(24), f(32));
        if b.len() < 40 + w * h * 3 { bail!("terrain file truncated") }
        let heights = b[40..40 + w * h * 2].chunks_exact(2).map(|x| u16::from_le_bytes([x[0], x[1]]) as f32 / 10.0).collect();
        let cover = b[40 + w * h * 2..40 + w * h * 3].to_vec();
        let kx = start.lat.to_radians().cos() * 111_320.0;
        let mut t = Self { w, h, heights, cover, lat0: start.lat, lon0: start.lon, kx, ox: 0.0, oz: 0.0, cx: 0.0, cz: 0.0 };
        (t.ox, t.oz) = t.to_xz(north, west);
        (t.cx, t.cz) = (((east - west) / w as f64 * kx) as f32, ((north - south) / h as f64 * 110_540.0) as f32);
        Ok(t)
    }

    pub fn to_xz(&self, lat: f64, lon: f64) -> (f32, f32) {
        (((lon - self.lon0) * self.kx) as f32, ((self.lat0 - lat) * 110_540.0) as f32)
    }

    fn ele(&self, c: usize, r: usize) -> f32 {
        self.heights[r.min(self.h - 1) * self.w + c.min(self.w - 1)]
    }

    /// Height at a grid corner: mean of the cells around it, or the lowest road cell when one touches it,
    /// so the 5 m relief never pokes through the road ribbon's edges.
    fn corner(&self, c: usize, r: usize) -> f32 {
        let (c0, r0) = (c.saturating_sub(1), r.saturating_sub(1));
        let cells = [(c0, r0), (c, r0), (c0, r), (c, r)].map(|(cc, rr)| (cc.min(self.w - 1), rr.min(self.h - 1)));
        let road = cells.iter().filter(|&&(cc, rr)| self.cover[rr * self.w + cc] == ROAD).map(|&(cc, rr)| self.ele(cc, rr)).reduce(f32::min);
        road.unwrap_or_else(|| cells.iter().map(|&(cc, rr)| self.ele(cc, rr)).sum::<f32>() / 4.0)
    }

    pub fn chunks(&self) -> (usize, usize) {
        (self.w.div_ceil(CHUNK), self.h.div_ceil(CHUNK))
    }
}

/// Decode a JPEG with a full mip chain (Bevy does not build mips for runtime images) and a trilinear,
/// anisotropic sampler: without mips, aerial photos shimmer and alias at a distance.
pub fn decode(bytes: &[u8], srgb: bool, repeat: bool) -> Result<Image> {
    let mut img = Image::from_buffer(bytes, ImageType::Extension("jpg"), CompressedImageFormats::NONE, srgb, ImageSampler::Default, RenderAssetUsages::RENDER_WORLD)?;
    let (mut w, mut h) = (img.width() as usize, img.height() as usize);
    let mut level = img.data.take().unwrap_or_default();
    if level.len() == w * h * 4 {
        let mut all = level.clone();
        let mut count = 1;
        while w > 1 || h > 1 {
            let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
            let mut next = vec![0u8; nw * nh * 4];
            for y in 0..nh {
                for x in 0..nw {
                    for k in 0..4 {
                        let px = |xx: usize, yy: usize| level[((yy.min(h - 1)) * w + xx.min(w - 1)) * 4 + k] as u32;
                        next[(y * nw + x) * 4 + k] = ((px(2 * x, 2 * y) + px(2 * x + 1, 2 * y) + px(2 * x, 2 * y + 1) + px(2 * x + 1, 2 * y + 1)) / 4) as u8;
                    }
                }
            }
            all.extend_from_slice(&next);
            (level, w, h, count) = (next, nw, nh, count + 1);
        }
        img.texture_descriptor.mip_level_count = count;
        img.data = Some(all);
    } else {
        img.data = Some(level);
    }
    let mode = if repeat { ImageAddressMode::Repeat } else { ImageAddressMode::ClampToEdge };
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: mode,
        address_mode_v: mode,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: 16,
        ..default()
    });
    Ok(img)
}

fn mesh(pos: Vec<[f32; 3]>, uv: Vec<[f32; 2]>, idx: Vec<u32>) -> Mesh {
    let mut m = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, pos)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uv)
        .with_inserted_indices(Indices::U32(idx));
    m.compute_smooth_normals();
    m
}

/// One chunk as a smooth mesh; UVs map into the aerial photo tile `(ti, tj)` it belongs to.
pub fn chunk_mesh(t: &Terrain, ci: usize, cj: usize, tile_cells: usize) -> (Mesh, (usize, usize)) {
    let (ti, tj) = (ci * CHUNK / tile_cells, cj * CHUNK / tile_cells);
    let (tw, th) = (tile_cells.min(t.w - ti * tile_cells) as f32, tile_cells.min(t.h - tj * tile_cells) as f32);
    let (c0, r0) = (ci * CHUNK, cj * CHUNK);
    let (c1, r1) = ((c0 + CHUNK).min(t.w), (r0 + CHUNK).min(t.h));
    let nx = (c1 - c0 + 1) as u32;
    let (mut pos, mut uv, mut idx) = (vec![], vec![], vec![]);
    for r in r0..=r1 {
        for c in c0..=c1 {
            pos.push([t.ox + c as f32 * t.cx, t.corner(c, r), t.oz + r as f32 * t.cz]);
            uv.push([(c - ti * tile_cells) as f32 / tw, (r - tj * tile_cells) as f32 / th]);
        }
    }
    for r in 0..(r1 - r0) as u32 {
        for c in 0..(c1 - c0) as u32 {
            let a = r * nx + c;
            idx.extend([a, a + nx, a + 1, a + 1, a + nx, a + nx + 1]);
        }
    }
    (mesh(pos, uv, idx), (ti, tj))
}

/// The whole area every 8 cells under the overview photo, sunk 3 m so near chunks win.
pub fn far_mesh(t: &Terrain) -> Mesh {
    const F: usize = 8;
    let (w, h) = ((t.w - 1) / F + 1, (t.h - 1) / F + 1);
    let (mut pos, mut uv, mut idx) = (vec![], vec![], vec![]);
    for j in 0..h {
        for i in 0..w {
            let (c, r) = (i * F, j * F);
            pos.push([t.ox + (c as f32 + 0.5) * t.cx, t.ele(c, r) - 3.0, t.oz + (r as f32 + 0.5) * t.cz]);
            uv.push([(c as f32 + 0.5) / t.w as f32, (r as f32 + 0.5) / t.h as f32]);
        }
    }
    for j in 0..(h - 1) as u32 {
        for i in 0..(w - 1) as u32 {
            let a = j * w as u32 + i;
            idx.extend([a, a + w as u32, a + 1, a + 1, a + w as u32, a + w as u32 + 1]);
        }
    }
    mesh(pos, uv, idx)
}

fn hash(x: usize, y: usize, s: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(374_761_393) ^ (y as u32).wrapping_mul(668_265_263) ^ s.wrapping_mul(2_246_822_519);
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    ((h ^ (h >> 16)) & 0xFFFF) as f32 / 65_535.0
}

/// Conifers on the forest cells of a chunk, merged into one vertex-colored mesh: a trunk and three
/// stacked cones, ~14 m tall, one tree per ~3 forest cells.
pub fn tree_mesh(t: &Terrain, ci: usize, cj: usize) -> Option<Mesh> {
    let (mut pos, mut col) = (vec![], vec![]);
    let mut cone = |x: f32, y: f32, z: f32, r: f32, h: f32, rgb: [f32; 4]| {
        const N: usize = 7;
        for k in 0..N {
            let (a0, a1) = (k as f32 / N as f32 * std::f32::consts::TAU, (k + 1) as f32 / N as f32 * std::f32::consts::TAU);
            pos.extend([[x, y + h, z], [x + a1.cos() * r, y, z + a1.sin() * r], [x + a0.cos() * r, y, z + a0.sin() * r]]);
            col.extend([rgb; 3]);
        }
    };
    for r in cj * CHUNK..((cj + 1) * CHUNK).min(t.h) {
        for c in ci * CHUNK..((ci + 1) * CHUNK).min(t.w) {
            if t.cover[r * t.w + c] != FOREST || hash(c, r, 21) > 0.33 { continue }
            let (x, z) = (t.ox + (c as f32 + hash(c, r, 22)) * t.cx, t.oz + (r as f32 + hash(c, r, 23)) * t.cz);
            let (y, s) = (t.corner(c, r), 0.7 + hash(c, r, 24) * 0.6);
            let shade = 0.85 + hash(c, r, 25) * 0.3;
            let green = [0.045 * shade, 0.085 * shade, 0.05 * shade, 1.0]; // dark spruce, linear RGB
            cone(x, y - 0.5, z, 0.35 * s, 4.0 * s, [0.22, 0.16, 0.1, 1.0]); // trunk
            for (rr, hh, yy) in [(2.8, 7.0, 2.0), (2.2, 5.5, 5.5), (1.5, 4.5, 8.5)] {
                cone(x, y + yy * s, z, rr * s, hh * s, green);
            }
        }
    }
    if pos.is_empty() { return None }
    let mut m = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD)
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, pos)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, col);
    m.compute_flat_normals();
    Some(m)
}

/// Course point at distance `d` and the riding direction, in scene coordinates.
pub fn point_at(t: &Terrain, pts: &[Point], d: f32) -> (Vec3, Vec3) {
    let at = |d: f32| {
        let d = d.clamp(0.0, pts.last().map_or(0.0, |p| p.d));
        let i = pts.partition_point(|p| p.d <= d).clamp(1, pts.len() - 1);
        let (a, b) = (&pts[i - 1], &pts[i]);
        let k = ((d - a.d) / (b.d - a.d).max(1e-3)).clamp(0.0, 1.0);
        let (x, z) = t.to_xz(a.lat + (b.lat - a.lat) * k as f64, a.lon + (b.lon - a.lon) * k as f64);
        Vec3::new(x, a.ele + (b.ele - a.ele) * k + 0.15, z)
    };
    let dir = (at(d + 6.0) - at(d - 6.0)).with_y(0.0).normalize_or(Vec3::X);
    (at(d), dir)
}

/// Road ribbon (asphalt, UVs in meters / 3) and its markings: white edge lines, dashed center line.
/// The course's 10 m points are smoothed with a Catmull-Rom spline every 2 m so hairpins stay round.
pub fn road_meshes(t: &Terrain, pts: &[Point]) -> (Mesh, Mesh) {
    let raw: Vec<Vec3> = pts.iter().map(|p| { let (x, z) = t.to_xz(p.lat, p.lon); Vec3::new(x, p.ele + 0.15, z) }).collect();
    let mut line = vec![];
    for i in 0..raw.len().saturating_sub(1) {
        let (p0, p1, p2, p3) = (raw[i.saturating_sub(1)], raw[i], raw[i + 1], raw[(i + 2).min(raw.len() - 1)]);
        for k in 0..5 {
            let s = k as f32 / 5.0;
            line.push(0.5 * (2.0 * p1 + (p2 - p0) * s + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * s * s + (3.0 * p1 - p0 - 3.0 * p2 + p3) * s * s * s));
        }
    }
    line.extend(raw.last());
    const HALF: f32 = 3.2;
    let side = |i: usize| {
        let (a, b) = (line[i.saturating_sub(1)], line[(i + 1).min(line.len() - 1)]);
        let d = (b - a).with_y(0.0).normalize_or(Vec3::X);
        Vec3::new(-d.z, 0.0, d.x)
    };
    let (mut rp, mut ru, mut ri) = (vec![], vec![], vec![]);
    let (mut mp, mut mu, mut mi) = (vec![], vec![], vec![]);
    let mut along = 0.0;
    let strip = |p: &mut Vec<[f32; 3]>, u: &mut Vec<[f32; 2]>, idx: &mut Vec<u32>, a: [Vec3; 4], uv: [[f32; 2]; 4]| {
        let n = p.len() as u32;
        p.extend(a.map(|v| v.to_array()));
        u.extend(uv);
        idx.extend([n, n + 1, n + 2, n, n + 2, n + 3]);
    };
    for i in 0..line.len() - 1 {
        let (p, q, sp, sq) = (line[i], line[i + 1], side(i), side(i + 1));
        let step = (q - p).length();
        let (v0, v1) = (along / 3.0, (along + step) / 3.0);
        // Band between lateral offsets lo < hi (lo = left of the direction): p(lo), p(hi), q(hi), q(lo) is
        // counterclockwise seen from above, so the faces point up.
        let band = |lo: f32, hi: f32, dy: f32| [p + sp * lo, p + sp * hi, q + sq * hi, q + sq * lo].map(|v| v + Vec3::Y * dy);
        strip(&mut rp, &mut ru, &mut ri, band(-HALF, HALF, 0.0), [[0.0, v0], [2.1, v0], [2.1, v1], [0.0, v1]]);
        for (lo, hi) in [(-HALF + 0.2, -HALF + 0.35), (HALF - 0.35, HALF - 0.2)] {
            strip(&mut mp, &mut mu, &mut mi, band(lo, hi, 0.02), [[0.0; 2]; 4]); // edge lines
        }
        if along % 12.0 < 3.0 {
            strip(&mut mp, &mut mu, &mut mi, band(-0.07, 0.07, 0.02), [[0.0; 2]; 4]); // dashed center line
        }
        along += step;
    }
    (mesh(rp, ru, ri), mesh(mp, mu, mi))
}
