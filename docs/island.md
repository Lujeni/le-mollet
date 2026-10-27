# Hand-made island: the Île des Briques

An open world drawn by hand, not generated: every place, road and landmark of the Lego island is placed
in a map file. It rides like Paris: pick left, straight or right at each junction.

```sh
cargo run --release -- island courses/ile-des-briques.map.json   # ~10 s, offline
```

It writes `ile-des-briques.city.json` (roads as a junction graph, plus the decor list) and
`ile-des-briques.city.terrain.bin` (git-ignored), then the server lists it as an island in the ride menu.

## The map file

Meters, x east and z south from the north-west corner. Everything below is drawn by hand.

| Key | What it draws |
|---|---|
| `size`, `cell` | map size with the sea, terrain cell (2.5 m: four times finer than the climbs) |
| `coast` | the island outline; sand within 35 m of it, sea outside (one flat level) |
| `relief` | `cone` (volcano: rim, lava lake, flanks a spiral road climbs at a steady grade), `hill`, `plateau` (mesas, cliffs), `dunes` |
| `areas` | ground per polygon: `grass`, `forest`, `jungle`, `sand`, `field`, `dirt`, `pave`, `rock`, `basalt`, `water`, `lava`; `flat` levels it (lakes, the town) |
| `rivers` | water channels along a polyline |
| `nodes`, `roads` | junctions and named roads between them, bends in `via`; `bridge` for a deck, `spiral` for a climb around a center |
| `props` | landmarks and single pieces: `{ "k": kind, "at": [x, z], "yaw": degrees, "v": variant, "sea": floats }` |
| `rows` | pieces lined up along a street: houses, lamps, fences, benches |
| `scatter` | many pieces of one kind over a polygon: palms on the beach, cacti, flowers |

Roads follow the ground smoothed, are capped at 12 % and carved into it; the build prints each road's length
and steepest grade. Heights are stored 50 m above the sea so the format stays the climbs' one.

## In the page

- **Ground**: real minifig scale on the island (0.4 m studs, 1.6 m bricks, 1 m terraces).
- **Decor kit**: every model is built from Lego-sized pieces (bricks with studs, plates, tiles, slopes,
  round bricks) in `MODELS`: houses with brick courses, shuttered windows and flower boxes, tavern,
  market stalls, fountain, cranes, docks, pirate ships, fort, cannons, lighthouse, beach huts, parasols,
  palms, wreck and treasure, pyramid, obelisks, oasis well, tents, camels, temple, tiki idols, waterfall,
  rope bridge, windmill, barn, silo, tractor, scarecrows, castle, lamps, benches, trees, flowers...
- **Landmarks** (castle, volcano, lighthouse, ships, pyramid...) are always there; the rest is built with
  the terrain chunk under it, one instanced mesh per model.
- **Vegetation**: woods, jungle, meadows and desert are filled cell by cell from their ground, so they are
  dense without listing thousands of trees in the map.
- **Moving**: windmill sails, waving flags, the lighthouse beam at night, lamps lit at night, smoke and
  eruptions from the volcano, the waterfall, boats bobbing and a ship sailing the bay, cows and sheep
  grazing, camels walking, crabs scuttling, parrots flying.

## Adding a place

Draw it in the map file: a node and a road to reach it, its ground as an area, its landmark as a prop, its
details as rows and scatter. Rebuild, then ride there. New kinds of decor go in `MODELS` (and `MOVING` for
parts that move) in `web/index.html`.
