//! Real buildings for an open city: footprints and measured heights from IGN BD TOPO (Licence Ouverte
//! Etalab 2.0), extruded in the page. Built offline next to the city terrain, see docs/city.md.
//!
//! File format (little-endian): u32 count, then per building: f32 height (m), u16 n, then n vertices
//! as f32 (lon - west, north - lat) in degrees, relative to the terrain's north-west corner.

use crate::course::{UA, request_json};
use anyhow::{Context, Result};

/// WFS answers at most this many features per request.
const PAGE: usize = 5000;
/// Below this, sheds and kiosks: skip.
const MIN_AREA_M2: f64 = 15.0;
/// Vertices closer than this to the previous one are dropped, m.
const MIN_EDGE_M: f64 = 0.6;
/// When BD TOPO has no height: one floor of a small building.
const DEFAULT_HEIGHT_M: f32 = 9.0;

pub struct Building {
    pub height: f32,
    /// NGF altitudes (same datum as the IGN elevation grid), when BD TOPO has them: ground, eaves, ridge.
    pub ground: Option<f32>,
    pub eaves: Option<f32>,
    pub ridge: Option<f32>,
    /// Outer ring, `[lat, lon]`, not closed.
    pub ring: Vec<[f64; 2]>,
}

/// Every building in the box, paging through the Géoplateforme WFS.
pub fn fetch(west: f64, south: f64, east: f64, north: f64) -> Result<Vec<Building>> {
    let kx = ((south + north) / 2.0).to_radians().cos() * 111_320.0;
    let mut out = vec![];
    for start in (0..).step_by(PAGE) {
        let url = format!(
            "https://data.geopf.fr/wfs/ows?SERVICE=WFS&VERSION=2.0.0&REQUEST=GetFeature&TYPENAMES=BDTOPO_V3:batiment\
             &BBOX={south},{west},{north},{east},urn:ogc:def:crs:EPSG::4326&PROPERTYNAME=hauteur,nombre_d_etages,altitude_minimale_sol,altitude_minimale_toit,altitude_maximale_toit,geometrie\
             &SORTBY=cleabs&COUNT={PAGE}&STARTINDEX={start}&outputFormat=application/json"
        );
        let r = request_json(|| ureq::get(&url).set("User-Agent", UA).call())?;
        let feats = r["features"].as_array().context("BD TOPO: no features")?;
        for f in feats {
            let p = &f["properties"];
            let height = p["hauteur"].as_f64().map(|h| h as f32).or_else(|| p["nombre_d_etages"].as_f64().map(|n| n as f32 * 3.0)).unwrap_or(DEFAULT_HEIGHT_M);
            let alt = |k: &str| p[k].as_f64().map(|v| v as f32);
            let (ground, eaves, ridge) = (alt("altitude_minimale_sol"), alt("altitude_minimale_toit"), alt("altitude_maximale_toit"));
            // MultiPolygon -> polygons -> rings -> [lon, lat, z]; keep each polygon's outer ring.
            for poly in f["geometry"]["coordinates"].as_array().into_iter().flatten() {
                let Some(outer) = poly.get(0).and_then(|r| r.as_array()) else { continue };
                let mut ring: Vec<[f64; 2]> = vec![];
                for c in outer {
                    let (Some(lon), Some(lat)) = (c[0].as_f64(), c[1].as_f64()) else { continue };
                    if ring.last().is_none_or(|l| ((lat - l[0]) * 110_540.0).hypot((lon - l[1]) * kx) >= MIN_EDGE_M) {
                        ring.push([lat, lon]);
                    }
                }
                if ring.len() > 3 && ring.first() == ring.last() {
                    ring.pop();
                }
                if ring.len() >= 3 && area_m2(&ring, kx) >= MIN_AREA_M2 {
                    out.push(Building { height, ground, eaves, ridge, ring });
                }
            }
        }
        eprintln!("buildings: {} so far", out.len());
        if feats.len() < PAGE {
            break;
        }
    }
    Ok(out)
}

fn area_m2(ring: &[[f64; 2]], kx: f64) -> f64 {
    let n = ring.len();
    (0..n)
        .map(|i| {
            let (a, b) = (ring[i], ring[(i + 1) % n]);
            (a[1] * kx) * (b[0] * 110_540.0) - (b[1] * kx) * (a[0] * 110_540.0)
        })
        .sum::<f64>()
        .abs()
        / 2.0
}

pub fn to_bytes(buildings: &[Building], west: f64, north: f64) -> Vec<u8> {
    let mut b = Vec::with_capacity(4 + buildings.len() * 64);
    b.extend((buildings.len() as u32).to_le_bytes());
    for bd in buildings {
        b.extend(bd.height.to_le_bytes());
        b.extend((bd.ring.len().min(u16::MAX as usize) as u16).to_le_bytes());
        for p in bd.ring.iter().take(u16::MAX as usize) {
            b.extend(((p[1] - west) as f32).to_le_bytes());
            b.extend(((north - p[0]) as f32).to_le_bytes());
        }
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn area_and_encoding() {
        let kx = 48.85f64.to_radians().cos() * 111_320.0;
        // ~10 m x 10 m square
        let (dlat, dlon) = (10.0 / 110_540.0, 10.0 / kx);
        let ring = vec![[48.85, 2.35], [48.85, 2.35 + dlon], [48.85 + dlat, 2.35 + dlon], [48.85 + dlat, 2.35]];
        assert!((area_m2(&ring, kx) - 100.0).abs() < 0.5);
        let bytes = to_bytes(&[Building { height: 21.5, ground: None, eaves: None, ridge: None, ring }], 2.3, 48.9);
        assert_eq!(bytes.len(), 4 + 4 + 2 + 4 * 8);
        assert_eq!(u32::from_le_bytes(bytes[0..4].try_into().unwrap()), 1);
        assert_eq!(f32::from_le_bytes(bytes[4..8].try_into().unwrap()), 21.5);
        let dlon0 = f32::from_le_bytes(bytes[10..14].try_into().unwrap());
        assert!((dlon0 - 0.05).abs() < 1e-6);
    }
}
