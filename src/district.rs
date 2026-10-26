//! Realistic district: a small box of an open city drawn from real data instead of voxels. IGN 1 m
//! elevation under the IGN aerial photo (BD ORTHO, ~30 cm a pixel), BD TOPO buildings with their
//! eaves and ridge altitudes, OpenStreetMap trees. See docs/city.md.
//!
//! Files in `<city>.district/`: `meta.json` (box and grid size), `dem.bin` (f32 elevations, rows north
//! to south), `ortho.jpg`, `trees.json` (`[[lat, lon], ...]`), `buildings.bin` (see `to_bytes`), `osm.json` (streets
//! with their half width, crossings, street lamps) and `surface.bin` (u8 per elevation cell: 0 sidewalk,
//! 1 paved square, 2 water, 3 park).

use crate::buildings::{self, Building};
use crate::course::{UA, request_json};
use crate::terrain::{Ring, fetch_heights, fill_even_odd};
use anyhow::{Context, Result, bail};
use serde_json::json;
use std::io::Read;

/// Elevation cell, m.
const DEM_M: f64 = 1.0;
/// Widest aerial photo one WMS request returns, px.
const ORTHO_MAX_PX: usize = 4096;

/// Fetch everything for `[south, west, north, east]` into `dir`.
pub fn build(dir: &str, [south, west, north, east]: [f64; 4]) -> Result<()> {
    if south >= north || west >= east { bail!("bbox must be south,west,north,east") }
    let kx = ((south + north) / 2.0).to_radians().cos() * 111_320.0;
    let (wm, hm) = ((east - west) * kx, (north - south) * 110_540.0);
    let (w, h) = ((wm / DEM_M).round() as usize, (hm / DEM_M).round() as usize);
    if w > 4096 || h > 4096 { bail!("district {wm:.0}x{hm:.0} m is too big for one 1 m elevation request") }
    std::fs::create_dir_all(dir)?;

    let dem = fetch_heights(west, south, east, north, w, h)?;
    std::fs::write(format!("{dir}/dem.bin"), dem.iter().flat_map(|e| e.to_le_bytes()).collect::<Vec<u8>>())?;

    let (ow, oh) = if wm >= hm { (ORTHO_MAX_PX, (ORTHO_MAX_PX as f64 * hm / wm).round() as usize) } else { ((ORTHO_MAX_PX as f64 * wm / hm).round() as usize, ORTHO_MAX_PX) };
    let url = format!(
        "https://data.geopf.fr/wms-r/wms?SERVICE=WMS&VERSION=1.3.0&REQUEST=GetMap&LAYERS=ORTHOIMAGERY.ORTHOPHOTOS\
         &STYLES=&CRS=EPSG:4326&BBOX={south},{west},{north},{east}&WIDTH={ow}&HEIGHT={oh}&FORMAT=image/jpeg"
    );
    let mut jpg = vec![];
    ureq::get(&url).set("User-Agent", UA).call()?.into_reader().read_to_end(&mut jpg)?;
    if !jpg.starts_with(&[0xFF, 0xD8]) { bail!("IGN aerial photo: not a JPEG ({} bytes)", jpg.len()) }
    std::fs::write(format!("{dir}/ortho.jpg"), &jpg)?;

    let q = format!("[out:json][timeout:90];node[natural=tree]({south},{west},{north},{east});out;");
    let r = request_json(|| ureq::post("https://overpass-api.de/api/interpreter").set("User-Agent", UA).send_form(&[("data", &q)]))?;
    let trees: Vec<[f64; 2]> = r["elements"].as_array().context("Overpass: no elements")?.iter().filter_map(|n| Some([n["lat"].as_f64()?, n["lon"].as_f64()?])).collect();
    std::fs::write(format!("{dir}/trees.json"), serde_json::to_string(&trees)?)?;

    let (osm, surface) = streets_and_surfaces([south, west, north, east], w, h)?;
    std::fs::write(format!("{dir}/osm.json"), osm.to_string())?;
    std::fs::write(format!("{dir}/surface.bin"), surface)?;

    let b = buildings::fetch(west, south, east, north)?;
    std::fs::write(format!("{dir}/buildings.bin"), to_bytes(&b, west, north))?;

    let meta = json!({ "south": south, "west": west, "north": north, "east": east, "dem_w": w, "dem_h": h, "ortho": [ow, oh],
        "attribution": "Photo aérienne, altitudes et bâtiments © IGN (BD ORTHO, RGE ALTI, BD TOPO, Licence Ouverte Etalab 2.0). Arbres © OpenStreetMap contributors (ODbL)." });
    std::fs::write(format!("{dir}/meta.json"), meta.to_string())?;
    println!("{dir}: {w}x{h} m elevation, {ow}x{oh} px photo, {} buildings, {} trees", b.len(), trees.len());
    Ok(())
}

/// Streets as centerlines with a half width, plus a `w` x `h` grid of ground surfaces over the box.
fn streets_and_surfaces([south, west, north, east]: [f64; 4], w: usize, h: usize) -> Result<(serde_json::Value, Vec<u8>)> {
    let b = format!("({south},{west},{north},{east})");
    let q = format!(
        "[out:json][timeout:180];(way[highway]{b};way[leisure~\"^(park|garden)$\"]{b};relation[leisure~\"^(park|garden)$\"]{b};\
         way[natural=water]{b};relation[natural=water]{b};way[place=square]{b};node[highway~\"^(crossing|street_lamp)$\"]{b};);out geom;"
    );
    let r = request_json(|| ureq::post("https://overpass-api.de/api/interpreter").set("User-Agent", UA).send_form(&[("data", &q)]))?;
    let grid = |lat: f64, lon: f64| ((lon - west) / (east - west) * w as f64, (north - lat) / (north - south) * h as f64);
    let ring = |g: &serde_json::Value| -> Ring { g.as_array().into_iter().flatten().filter_map(|p| Some(grid(p["lat"].as_f64()?, p["lon"].as_f64()?))).collect() };
    let (mut roads, mut crossings, mut lamps, mut areas) = (vec![], vec![], vec![], vec![]);
    for el in r["elements"].as_array().context("Overpass: no elements")? {
        let tag = |k: &str| el["tags"][k].as_str().unwrap_or("");
        if el["type"] == "node" {
            let p = json!([el["lat"], el["lon"]]);
            if tag("highway") == "crossing" { crossings.push(p) } else { lamps.push(p) }
            continue;
        }
        let hw = tag("highway");
        let code = if tag("natural") == "water" { 2 } else if !tag("leisure").is_empty() { 3 } else if tag("area") == "yes" && hw == "pedestrian" || tag("place") == "square" { 1 } else { 255 };
        if code != 255 {
            let rings = match el["type"].as_str() {
                Some("way") => vec![ring(&el["geometry"])],
                _ => el["members"].as_array().into_iter().flatten().map(|m| ring(&m["geometry"])).collect(),
            };
            areas.push((code, rings));
            continue;
        }
        let kind = match hw {
            "pedestrian" => "setts",
            "cycleway" => "cycle",
            "footway" | "path" | "steps" | "corridor" | "elevator" | "platform" | "proposed" | "construction" => continue,
            _ => "road",
        };
        let lanes: f64 = tag("lanes").parse().unwrap_or(0.0);
        let half = tag("width").trim_end_matches(" m").parse::<f64>().ok().map(|w| w / 2.0).unwrap_or(match hw {
            "trunk" | "primary" => 7.0,
            "secondary" => 5.5,
            "tertiary" => 4.5,
            "service" => 2.5,
            "cycleway" => 1.2,
            _ if lanes > 0.0 => lanes * 1.6,
            _ => 3.5,
        });
        let pts: Vec<_> = el["geometry"].as_array().into_iter().flatten().filter_map(|p| Some(json!([p["lat"].as_f64()?, p["lon"].as_f64()?]))).collect();
        if pts.len() >= 2 {
            roads.push(json!({ "kind": kind, "half": half, "lanes": lanes, "oneway": tag("oneway") == "yes", "bridge": !matches!(tag("bridge"), "" | "no"), "pts": pts }));
        }
    }
    // Paint order: parks, squares, then water wins.
    let mut surface = vec![0u8; w * h];
    areas.sort_by_key(|a| [3, 1, 2].iter().position(|&c| c == a.0));
    for (code, rings) in &areas {
        fill_even_odd(rings, w, h, |c, r| surface[r * w + c] = *code);
    }
    Ok((json!({ "roads": roads, "crossings": crossings, "lamps": lamps }), surface))
}

/// Like `buildings::to_bytes`, plus NGF ground, eaves and ridge altitudes (NaN when unknown):
/// u32 count, then per building f32 height, ground, eaves, ridge, u16 n, n x f32 (lon - west, north - lat).
fn to_bytes(list: &[Building], west: f64, north: f64) -> Vec<u8> {
    let mut b = (list.len() as u32).to_le_bytes().to_vec();
    for bd in list {
        for v in [Some(bd.height), bd.ground, bd.eaves, bd.ridge] {
            b.extend(v.unwrap_or(f32::NAN).to_le_bytes());
        }
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
    fn encodes_roof_altitudes() {
        let b = Building { height: 20.0, ground: Some(34.0), eaves: Some(52.0), ridge: None, ring: vec![[48.85, 2.35]; 3] };
        let bytes = to_bytes(&[b], 2.3, 48.9);
        let f = |o: usize| f32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
        assert_eq!((f(4), f(8), f(12)), (20.0, 34.0, 52.0));
        assert!(f(16).is_nan());
        assert_eq!(bytes.len(), 4 + 16 + 2 + 3 * 8);
    }
}
