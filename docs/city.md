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
