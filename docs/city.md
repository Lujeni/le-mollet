# Open city: riding every street of a town

How le-mollet turns a whole town (Paris first) into an open world where the rider picks a turn at each
junction. Climbs follow one road; a city is a graph of streets.

## Idea: a course that grows

The ride engine, the trainer control and the 3D view all follow a plain `Course`: a list of points with a
distance from the start. An open city keeps that model. Each rider has a `Route` (`src/city.rs`): a course
that starts on one street and gets the next street appended whenever the rider comes within 40 m of its
end. At that junction the route takes the turn the rider asked for (left, straight, right), then goes back
to straight. Physics, grade, laps-free totals and the chase camera work unchanged.

## Build (`le-mollet city`)

```sh
cargo run --release -- city "Paris" -o courses/paris.city.json --relation 7444 --start 48.8530,2.3499
```

1. **Streets.** One Overpass query: the boundary relation's bounding box, then every way inside the
   boundary area tagged `highway` = trunk, primary, secondary, tertiary (and their links), unclassified,
   residential, living_street, pedestrian or cycleway. Skipped: motorways (the périphérique), footways,
   steps, service alleys, `area=yes` squares, `access=private|no`, `bicycle=no`. One-ways are ignored:
   Paris has contraflow cycle lanes almost everywhere, and a closed-road feel is the point.
2. **Graph.** Ways are split at junctions (nodes shared by several ways, plus way ends). Only the largest
   connected network is kept: closed courtyards and cut-off paths would trap a rider.
3. **Elevation.** The IGN RGE ALTI grid (5 m cells, one WMS request) over the boundary box + 200 m,
   sampled every 10 m along each street, lightly smoothed (sigma 20 m), junction ends exact so streets meet.
   The grid is bare ground: **bridges and tunnels** (`bridge` / `tunnel` tags) are interpolated between
   their ends instead, otherwise every Seine bridge would dip 7 m into the river.
4. **Terrain** `paris.city.terrain.bin` (git-ignored), same format as the climbs: ground is paved by
   default, then OSM grass/parks, wood, water and buildings, then every street painted as asphalt (half
   width by road class, 1.5 to 7 m). Bridge decks raise the ground to the deck.

The city file stores, per street, its two junction ids, its name and `[lat, lon, ele]` points.

## Real buildings (`le-mollet buildings`)

```sh
cargo run --release -- buildings courses/paris.city.json   # also run by `city`
```

Footprints and measured heights of every building in the terrain box, from **IGN BD TOPO** (`BDTOPO_V3:batiment`,
Licence Ouverte Etalab 2.0) through the Géoplateforme WFS, 5,000 per request sorted by id. Outer rings only,
vertices closer than 0.6 m dropped, buildings under 15 m² skipped; no height: 3 m per floor, else 9 m.
Stored in `paris.city.buildings.bin` (git-ignored), see `src/buildings.rs` for the format.

The page extrudes them per 320 m terrain chunk: stone facade with a window grid (one 3.5 m bay x 3.2 m
floor per window), the top 3.2 m in zinc with dormers like a mansard, a flat roof cap. Textures are drawn
in a canvas, 4 x 4 windows wide so lit windows at night (a third of them) do not repeat. Under real
buildings the voxel ground stays flat and paved.

## Turn choice

At a junction, exits are sorted left to right by heading change. Straight on is the exit with the smallest
heading change; left and right pick the sharpest exit that way, if it turns more than 30°, otherwise
straight. No U-turn unless the street is a dead end.

## Status

- [x] Phase 1: street graph, elevation, terrain, turn logic (`src/city.rs`, `le-mollet city`).
- [x] Phase 2: server. Load `*.city.json`, one `Route` per ride session, `/turn?session=&dir=left|straight|right`,
      new route points and the upcoming junction's exits in each frame, other riders placed by lat/lon.
- [x] Phase 3: page. Route points appended as they come, junction prompt with street names (buttons, ← →
      and space), heading-up minimap in place of the profile, 18 m Haussmann blocks with zinc roofs, 0.5 m
      ground steps in a city instead of 2 m.
- [x] Real buildings: BD TOPO footprints and heights, facades, zinc top floors, lit windows at night.
- [x] Realistic district prototype (Île de la Cité): `le-mollet district courses/paris.city.json --bbox
      48.8510,2.3390,48.8580,2.3560` fetches IGN 1 m elevation, the BD ORTHO aerial photo (4096 px, ~30 cm),
      BD TOPO buildings with eaves and ridge altitudes and OSM trees into `paris.city.district/` (git-ignored).
      The page drops voxels there: photo on a 2 m relief mesh, sloped zinc roofs up to a ~3 m inset ridge with
      the photo on the cap, storefronts, 3D street trees, sun shadows following the rider, ACES tone mapping.
      Limits: the photo blurs at street level (30 cm a pixel seen from 2.5 m), it bakes in cars, shadows and
      tree crowns, and IGN pixelates sensitive sites (Préfecture de police).
- [x] Realistic district, step 2 (direction "kit"): `osm.json` (streets with their width from `width` / `lanes`,
      crossings, street lamps) and `surface.bin` (1 m grid: sidewalk, paved square, water, park) join the
      district files. The ground is CC0 PBR (ambientCG: asphalt road with granite curbs, light asphalt
      sidewalks, granite setts on squares), the Seine flat at its most common level and reflecting the sky,
      parks from the aerial photo, dashed center lines, zebra crossings, bridge decks. Buildings within
      ~200 m of the rider get a 3D facade kit (100 m cells, built as the rider moves): glass, stone window
      surrounds, floor bands, full-width balconies on the 2nd and 5th floors and balconettes elsewhere with
      wrought-iron rails, shop and main cornices, dormers on the zinc slopes. Light: Poly Haven HDRI sky
      (background and image-based lighting), sun shadows, ACES, GTAO ambient occlusion. Textures in
      `web/textures/` (CREDITS.md), served from the binary. Cost on screen: ~0.8 M triangles of facade kit
      + 0.6 M of ground.
- [ ] Phase 4: polish. Performance on the full city, landmarks, start points (Notre-Dame, Concorde,
      Montmartre).

## Known issues

- Railway cuttings: the IGN grid is bare ground, so a street crossing tracks on an untagged span dips into
  the trench (boulevard des Batignolles, avenue de Crimée, rue de Saint-Pétersbourg: 10-12 % for a few
  tens of meters).
- Building the city peaks at ~3 GB of memory (the Overpass answer with ~200k buildings is parsed at once).
- Other riders are placed by position with a ~1 s lag; no lane offset on crossings yet.

Paris (relation 7444, start on the Île de la Cité): 31,973 streets, ~1,790 km, 24.5 to 128 m, terrain
3678x1994 cells (22 MB), built in about 2 minutes.
