//! IGN aerial photo (BD ORTHO, Licence Ouverte Etalab 2.0) over a course's terrain, for the realistic
//! render: tiles of `TILE_CELLS` terrain cells (aligned with the page's 64-cell chunks), 4096 px each,
//! plus one overview for the distant mountains. See docs/courses.md.
//!
//! Files in `<course>.ortho/`: `meta.json`, `overview.jpg`, `<i>_<j>.jpg` (column i, row j from the north-west).

use crate::course::UA;
use anyhow::{Result, bail};
use serde_json::json;
use std::io::Read;

/// Terrain cells per tile side: 10 chunks of 64 cells, 3.2 km at 5 m.
pub const TILE_CELLS: usize = 640;
const TILE_PX: usize = 4096;
const OVERVIEW_PX: usize = 2048;

pub(crate) fn get_jpeg([south, west, north, east]: [f64; 4], w: usize, h: usize) -> Result<Vec<u8>> {
    let url = format!(
        "https://data.geopf.fr/wms-r/wms?SERVICE=WMS&VERSION=1.3.0&REQUEST=GetMap&LAYERS=ORTHOIMAGERY.ORTHOPHOTOS\
         &STYLES=&CRS=EPSG:4326&BBOX={south},{west},{north},{east}&WIDTH={w}&HEIGHT={h}&FORMAT=image/jpeg"
    );
    let mut jpg = vec![];
    ureq::get(&url).set("User-Agent", UA).call()?.into_reader().read_to_end(&mut jpg)?;
    if !jpg.starts_with(&[0xFF, 0xD8]) {
        bail!("IGN aerial photo: not a JPEG ({} bytes)", jpg.len())
    }
    Ok(jpg)
}

/// Fetch the tiles for a terrain of `w` x `h` cells over the given box into `dir`.
pub fn build(dir: &str, [west, south, east, north]: [f64; 4], w: usize, h: usize) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let (nx, ny) = (w.div_ceil(TILE_CELLS), h.div_ceil(TILE_CELLS));
    let (dlon, dlat) = ((east - west) / w as f64, (north - south) / h as f64); // per cell
    for j in 0..ny {
        for i in 0..nx {
            let (c0, r0) = (i * TILE_CELLS, j * TILE_CELLS);
            let (cw, rh) = (TILE_CELLS.min(w - c0), TILE_CELLS.min(h - r0));
            let bbox = [north - (r0 + rh) as f64 * dlat, west + c0 as f64 * dlon, north - r0 as f64 * dlat, west + (c0 + cw) as f64 * dlon];
            let px = |cells: usize| (TILE_PX * cells).div_ceil(TILE_CELLS).max(16);
            std::fs::write(format!("{dir}/{i}_{j}.jpg"), get_jpeg(bbox, px(cw), px(rh))?)?;
            eprintln!("ortho: tile {i},{j} of {nx}x{ny}");
        }
    }
    let (ow, oh) = if w >= h { (OVERVIEW_PX, (OVERVIEW_PX * h / w).max(16)) } else { ((OVERVIEW_PX * w / h).max(16), OVERVIEW_PX) };
    std::fs::write(format!("{dir}/overview.jpg"), get_jpeg([south, west, north, east], ow, oh)?)?;
    std::fs::write(format!("{dir}/meta.json"), json!({ "tile_cells": TILE_CELLS, "nx": nx, "ny": ny }).to_string())?;
    println!("{dir}: {nx}x{ny} tiles of {TILE_PX} px (~{:.2} m a pixel) + overview", TILE_CELLS as f64 * 5.0 / TILE_PX as f64);
    Ok(())
}
