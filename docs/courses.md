# Courses: simulating real roads

How le-mollet turns a real road (Alpe d'Huez, Galibier, Ventoux...) into a ridable course.
Apply this method to every new route.

## Data sources

| Need | Source | Notes |
|---|---|---|
| Road geometry | OpenStreetMap via Overpass API (POST, with User-Agent) | ODbL, attribution required |
| Elevation (France) | IGN RGE ALTI via Géoplateforme altimetry API | 1 m DEM, Licence Ouverte Etalab 2.0 |
| Elevation (elsewhere) | Terrain Tiles on AWS Open Data (Mapzen/Tilezen: SRTM, EU-DEM © Copernicus) | Terrarium PNG tiles at zoom 13, ~30 m of real resolution; used wherever IGN answers -99999 (src/dem.rs), for course points and the terrain grid |
| Country | OpenStreetMap admin boundaries (`is_in`, ISO3166-1) | recorded in the course file, shown as a flag on its magnet |

Never use GPX elevation recorded by phones or GPS units: too noisy for grades. GPX is only good for geometry.

## Pipeline (`le-mollet course`)

1. **Geometry.** Download every drivable OSM road in the bounding box of the waypoints (+0.03°, ~3 km margin) from Overpass.
   Build a graph of drivable roads **ignoring one-ways** (race roads are closed; Alpe d'Huez
   runs a one-way loop in Huez against traffic). Roads whose `ref` matches `--road` cost 1x,
   others 10x, so the route sticks to the named roads and only bridges gaps elsewhere.
   Dijkstra from waypoint to waypoint (each waypoint snaps to the nearest node on a wanted road).
2. **Resample** every 10 m along the line.
3. **Elevation** from IGN for each point (100 points per request). IGN and Overpass calls back off on 429/502/503/504.
4. **Smooth** with a Gaussian, sigma 50 m (100 m when the elevation comes from the Terrain Tiles, ~30 m data). Why: the DEM is exact, but OSM lines cut the inside
   of hairpins and graze embankments, which creates fake 15-20 % ramps and dips at every
   switchback. A 50 m moving average was not enough; sigma 30 m still left 17 % spikes.
5. **Write** `courses/<id>.json`: name, attribution, waypoints, roads, points `{d, lat, lon, ele}`.
   Grade is computed at ride time over a 10 m baseline.
   **Cap** (`le-mollet cap courses/<id>.json <max %>`), only where a ramp still reads steeper than the climb's
   published steepest one: steps above the cap give their extra height to the nearest steps with room, in a window
   widening 100 m at a time, so the total, the finish and the per-km grades stay put. Needed outside France, where the
   ~30 m Terrain Tiles squeeze a short ramp or a hairpin stack into a fake wall (Angliru 32 % for 23.5 %, Stelvio 19.7 %
   for 12 %). Rewrites the course and its terrain; run it after every rebuild.
6. **3D terrain** `courses/<id>.terrain.bin` (git-ignored, rebuild with `le-mollet terrain courses/<id>.json`):
   - IGN elevation grid in one WMS-Raster request (`ELEVATION.ELEVATIONGRIDCOVERAGE.HIGHRES`, float32 BIL),
     5 m cells, course bounding box + 1.5 km. Max 4096 px a side per request.
   - OSM land cover from Overpass: forest/wood, water, bare rock/scree, buildings. Multipolygons are
     filled even-odd over all member edges (no ring assembly needed).
   - Road carved in: cells within 4 m of the course line flattened just under the smoothed course
     elevation, banks clamped to 45° up to 25 m, shoulders cleared of trees and buildings.
   - Sizes: Alpe d'Huez 1198x1278 cells (4.6 MB), Ventoux 2815x1943 (16 MB).

## Loops from an OSM route relation (`course --relation`)

A cycling ring is often mapped as one route relation (`route=bicycle`, `roundtrip=yes`): its ways, in order.
`le-mollet course "<Name>" -o courses/<id>.json --relation <id> [--cell 2] [<start>]` chains them into one closed
line (a gap over 15 m between two ways fails the build), rides it the way most of its one-way members go, starts
from the point nearest `<start>`, and smooths the elevation round the loop so the start is no seam. The course is
`looped`: riders lap it, and the Free screen lists it with the open worlds rather than with the climbs.

`--cell 2` builds the terrain in 2 m cells instead of 5 m: small bricks for a short loop (a 4 km box stays
under IGN's 4096 px a request).

## Decor from the real place (`le-mollet decor`)

After `course` (or `terrain`), `le-mollet decor courses/<id>.json` reads the place and writes it in Lego:

- **Ground**: the IGN aerial photo (BD ORTHO) at one pixel a cell; each ground cell takes the nearest official
  Lego color (14 of them, `MOSAIC` in `src/decor.rs` and the page) after a saturation and brightness boost
  (the photo is dull next to plastic); green-dominant cells stay among the greens (grass in the shade); a 3x3
  majority filter twice turns the noise into a mosaic. Water, buildings and the road keep their codes.
- **Woods away from the road** (over 150 m): dark green cells become `CANOPY`, raised 9 to 15 m by the page as a
  lumpy canopy of bricks.
- **Trees within 150 m**: the photo again at 50 cm a pixel. A crown is green and dark, or green and textured by
  its own shadows (luminance deviation over ~4 m), or reddish and textured in autumn; the racecourse turf is green
  too, but smooth. A majority filter drops lone cells and thin lines (turf edges). Each crown blob gets pieces
  sized by how deep a cell sits in it (chamfer distance), the big ones first, kept apart; the variant carries the
  size and the foliage color (green, dark, olive, yellow, orange, red-brown). Real size: 9 to 19 m tall.
- **OpenStreetMap within 300 m**: trees with their species (conifers as pines), tree rows, benches (facing the
  road), street lamps, fences (white rails), hedges, walls, windmills (the building block under them goes).
- **Flat ground**: with small cells the 1 m elevation model's noise shows as a maze of half-meter steps, so the
  heights are smoothed over ~5 m and the road carved again. Run `decor` on a fresh terrain (`terrain` first
  when rebuilding): smoothing twice would flatten the ground further.

Thresholds (`SATURATION`, `GAIN`, `is_crown`) are calibration knobs tuned on Longchamp, with colors measured on its
photo in the tests. Then `le-mollet buildings courses/<id>.json` adds the real buildings (IGN BD TOPO, footprints
and heights): grandstands, pavilions, the château.

## Validation checklist (do it for every new course)

Compare with the reference figures of the climb (official signage, Tour de France roadbook,
climbbybike / cols-cyclisme sites):

- Length within ~3 %.
- Start and finish altitude within ~10 m.
- Average grade within ~0.3 %.
- Cumulated gain close to net gain on a pure climb (fake ups and downs mean bad smoothing or a detour).
- Max grade over 100 m plausible for the road (Alpe d'Huez: ~13-15 %, not 20 %).
- Per-km grades look like the published profile.

Pitfalls met so far:

- Car routing (OSRM) avoided the Huez one-way loop and took a 22 % village lane. Fixed by the
  OSM graph routing above. Pedestrian routing takes footpath shortcuts across hairpins: unusable.
- The OSM API (`/api/0.6/map`) refuses boxes over 50k nodes (Ventoux forest): use Overpass, which filters
  server side (~240 KB for Ventoux). Overpass needs a POST form and a User-Agent, otherwise 406 or HTML errors.
- "no road between ..." means the road leaves the box: Ventoux from Bédoin goes via Chalet Reynard, east
  of the summit. Add a via waypoint there. To locate a gap, look for the closest nodes between the start
  and finish connected components.

## Recipes

### Alpe d'Huez (from Bourg-d'Oisans, D211)

```sh
cargo run -- course "Alpe d'Huez" -o courses/alpe-dhuez.json \
  --road "D 211" --road "D 211F" \
  45.059055,6.037762 45.090074,6.074208
```

Result: 13.59 km, 721 m to 1803 m, +1085 m, avg 8.0 %, max ~15 % over 100 m.
Reference: ~13.8 km, ~8 %, 21 hairpins. Per-km: 10.7 / 10.0 / 7.5 / 8.6 / 8.3 / 8.1 / 9.3 / 7.1 / 8.0 / 8.5 / 7.6 / 7.4 %.

### Mont Ventoux (from Bédoin, D974)

```sh
cargo run -- course "Mont Ventoux (Bédoin)" -o courses/ventoux-bedoin.json \
  --road "D 974" \
  44.1243,5.1786 44.1485,5.3165 44.17405,5.27866
```

Via = Chalet Reynard. Result: 21.42 km, 309 m to 1889 m, +1580 m, avg 7.4 %, max 11.6 % over 100 m.
Reference: ~21.5 km, ~7.5 %, summit 1909 m (road ends a bit lower). Per-km: 2.2 / 3.2 / 4.4 / 5.7 / 5.6 / 4.5,
then the forest 9.5 / 9.3 / 10.8 / 8.9 / 9.7 / 9.8 / 8.8 / 10.0, Chalet Reynard 7.7 / 7.0 / 6.0, top 7.1 / 7.7 / 8.0 / 10.0 %.

### Col du Galibier (from Valloire, D902)

```sh
cargo run -- course "Col du Galibier (Valloire)" -o courses/galibier-valloire.json \
  --road "D 902" --road "D 902 B" \
  45.165600,6.429400 45.064138,6.407857
```

The summit loop is `D 902 B`; without it, the route takes the untagged tunnel (2556 m) and the DEM above it
gives a fake 40 % ramp. Finish = col, junction of the two `D 902 B` ways.
Result: 18.03 km, 1403 m to 2638 m, +1236 m, avg 6.9 %, max 10.9 % over 100 m.
Reference: ~18.1 km, 6.9 %, col 2642 m, flat Plan Lachat around km 3-5. Per-km: 5.1 / 8.3 / 2.9 / 2.3 / 4.4 /
7.3 / 7.5 / 7.3 / 8.3 / 4.1 / 7.9 / 8.6 / 8.8 / 8.2 / 7.6 / 7.5 / 8.2 / 9.2 %.

### Col du Tourmalet (from Sainte-Marie-de-Campan, D918)

```sh
cargo run -- course "Col du Tourmalet (Sainte-Marie-de-Campan)" -o courses/tourmalet-sainte-marie.json \
  --road "D 918" \
  42.985000,0.223000 42.908000,0.145300
```

Result: 16.97 km, 854 m to 2114 m, +1263 m, avg 7.4 %, max 15.4 % over 100 m (near Gripp and below La Mongie).
Reference: ~17.2 km, 7.4 %, 857 m to 2115 m. Per-km: 4.2 / 3.8 / 2.1 / 3.9 / 7.7 / 7.6 / 8.1 / 8.1 / 8.9 / 8.6 /
9.2 / 9.8 / 9.1 / 8.7 / 8.7 / 8.7 %.

The Luz-Saint-Sauveur side (`--road "D 918" 42.872700,-0.001900 42.908000,0.145300`) builds at 18.71 km,
720 m to 2110 m, avg 7.4 %, but has a 19 % spike over 100 m around 42.8833,0.0314 on the D918 itself
(reference max ~12 %). Not shipped until the cause is found (OSM line vs gorge wall?).

### Col d'Izoard (from Briançon, D902)

```sh
cargo run -- course "Col d'Izoard (Briançon)" -o courses/izoard-briancon.json \
  --road "D 902" \
  44.894950,6.636930 44.880308,6.673103 44.820300,6.735100
```

Start = west end of the D902 (Avenue du Col d'Izoard). The via keeps the route on the D902 at km 4-5, where
it otherwise cut through an unclassified lane. The short descent there is real.
Result: 19.13 km, 1212 m to 2354 m, +1176 m, avg 6.0 %, max 13.6 % over 100 m.
Reference: ~19.1-19.2 km, 1208 m to 2360 m, 5.9 %. Per-km: 6.6 / 6.2 / 6.1 / 3.4 / -1.6 / 3.9 / 7.0 / 3.8 / 3.3 /
5.5 / 7.2 / 5.7 / 7.6 / 9.2 / 8.0 / 8.5 / 7.7 / 7.8 / 7.6 %.

### La Planche des Belles Filles (from Plancher-les-Mines, D16E)

```sh
cargo run -- course "La Planche des Belles Filles" -o courses/planche-belles-filles.json \
  --road "D 16E" \
  47.772620,6.751532 47.772230,6.778320
```

Start = D16 / D16E junction above the village, where the classic climb is measured. Finish = ski station
(classic 1035 m finish, not the gravel "super Planche" at 1140 m).
Result: 5.81 km, 537 m to 1038 m, +503 m, avg 8.6 %, max 16.5 % over 100 m.
Reference: 5.9 km, 8.5 %, 1035 m, ramps up to 20 %. Per-km: 10.5 / 6.7 / 10.7 / 9.8 / 6.6 %.

### Anneau de Longchamp (Bois de Boulogne, Paris)

```sh
cargo run --release -- course "Anneau de Longchamp" -o courses/longchamp.json --relation 163270 --cell 2 48.860500,2.230000
cargo run --release -- decor courses/longchamp.json
cargo run --release -- buildings courses/longchamp.json
```

Relation 163270, "Anneau cyclable de Longchamp" (`bicycle:type=road_cycling`, `roundtrip=yes`), 10 ways. Start on the
Route des Tribunes in front of the grandstands. Ridden clockwise, the way its one-way cycleways go.
Result: 3.54 km a lap, 28 to 39 m, +12 m a lap, max 1.9 % over 100 m, the climb of the lap +8.6 m over 600 m on
the east side up to the Moulin de Longchamp. Reference: 3.5 to 3.6 km, flat with a ~600 m false flat of ~10 m.
Terrain 1848x2199 cells of 2 m. Decor: 13,014 pieces (6,465 broadleaf trees, 5,888 of them from the photo,
20 pines, 4,589 rail sections, 1,871 hedge sections, 28 benches, 27 lamps, the mill); 11,459 IGN buildings.

### Col de la Madeleine (from La Chambre, D213)

```sh
cargo run --release -- course "Col de la Madeleine (La Chambre)" -o courses/madeleine-la-chambre.json \
  --road "D 213" \
  45.359713,6.299329 45.434952,6.375522
```

Start = north end of the Grande Rue in La Chambre, where the D213 turns onto the Route de Saint-Martin and starts
to climb (473 m). Starting in the town centre instead (45.356730,6.302000) adds 400 m of flat street: 19.68 km,
first km 3.3 %. Finish = OSM saddle node of the col (1993 m). No via: the D213 stays inside the box, via
Saint-Martin-sur-la-Chambre and Saint-François-Longchamp.
Result: 19.27 km, 473 m to 1993 m, +1520 m (gain = net), avg 7.9 %, max 11.3 % over 100 m.
Reference: 19.0-19.3 km, ~470-475 m to 1993 m, +1515 to 1522 m, 7.8-8.0 %, max 11-13.5 %. Per-km: 5.6 / 7.7 / 8.2 /
9.4 / 8.7 / 7.9 / 7.2 / 9.7 / 8.0 / 7.6 / 9.1 / 6.3 / 7.8 / 8.4 / 7.9 / 7.8 / 7.1 / 8.0 / 8.0 %, the published shape:
steep 2-4 km above La Chambre, ~10 % around km 7 and 10, a short ease after km 11, steady ~8 % to the top.

### Col du Grand Colombier (from Culoz, D120)

```sh
cargo run --release -- course "Col du Grand Colombier (Culoz)" -o courses/grand-colombier-culoz.json \
  --road "D 120" \
  45.848784,5.783051 45.903459,5.761663
```

Start = Rue du Stade in Culoz, the official start of the Ain tourist board (244 m). Finish = OSM col node (1498 m). No
via: the D120 climbs through the hairpins above Culoz, past the D120a (Anglefort) junction, to the col, all inside the box.
Result: 17.58 km, 248 m to 1497 m, +1256 m (net +1248 m), avg 7.1 %, max 13.0 % over 100 m.
Reference: 17.6 km, 244 m to 1501 m, ~7.1 %, max 14 % (Ain tourisme); cols-cyclisme gives 18.3 km at 6.9 % from a
lower, unspecified start. Per-km: 5.7 / 8.2 / 9.7 / 6.9 / 9.6 / 10.6 / 8.3 / 3.0 / 4.0 / 9.2 / 10.0 / 9.9 / 5.1 / 1.4 /
6.9 / 5.6 / 5.2 %, the published shape: steep first 9 km, a 3 % false flat after the Anglefort junction (878 m), steep
again, easier after La Sapette (1230 m).

### Col d'Aubisque (from Laruns, D934 / D918)

```sh
cargo run --release -- course "Col d'Aubisque (Laruns)" -o courses/aubisque-laruns.json \
  --road "D 934" --road "D 918" \
  42.980205,-0.420696 42.976575,-0.339792
cargo run --release -- cap courses/aubisque-laruns.json 14
```

Start = the D934 bridge over the Gave at the entrance of Laruns. Finish = OSM col node (1709 m). No via. Starting at the
D918/D934 roundabout instead gives 16.38 km from 532 m, 13 m too high.
Result: 16.85 km, 508 m to 1707 m, +1206 m (net +1199 m), avg 7.1 %, max 14.0 % over 100 m (16.1 % before the cap).
Reference: 16.6-17.1 km, ~519 m to 1709 m, +1190-1202 m, 7.17 %, max 13-14 % (13.8 % steepest 100 m on climbfinder).
Per-km: 4.3 / 5.0 / 4.8 / 5.4 / 6.3 / 6.6 / 5.7 / 8.5 / 9.0 / 6.3 / 9.0 / 9.3 / 7.0 / 8.9 / 9.3 / 8.4 %: gentle to
Eaux-Bonnes, then steeper through the avalanche galleries to Gourette, ~8-9 % to the top. The 16 % before the cap sits
right after a `bridge=yes` on the D918 (42.9634,-0.3423): IGN reads the valley floor under the bridge, not the deck.

### Alto de l'Angliru (from La Vega de Riosa)

```sh
cargo run --release -- course "Alto de l'Angliru (Riosa)" -o courses/angliru-riosa.json \
  43.230726,-5.883243 43.225081,-5.938947
cargo run --release -- cap courses/angliru-riosa.json 23.5
```

Spain: elevation from the Terrain Tiles (SRTM / EU-DEM, ~30 m), smoothed over 100 m. No `--road`: the climb road
("Carretera a L'Angliru") has no `ref` in OSM, and `--road` matches refs only (the finish would snap back onto the
RI-2), so the route is the plain shortest path. It follows the real road (every point within ~15 m of the OSM way).
Start = La Vega, on the AS-231 at the east end of the RI-2. Finish = high point of the road (1568 m); the road then
drops ~14 m to the summit car park (finishing there gives 13.01 km and 1553 m).
Result: 12.44 km, 327 m to 1568 m, +1244 m (net +1241), avg 10.0 %, max 23.5 % over 100 m (32.1 % before the cap).
Reference: 12.4-12.55 km, La Vega 315 m to 1570-1573 m, +1266 m, 10.1 %, max 23.5 % (Cueña les Cabres, 1 km at
17.3 %), first 5 km 5-9 %, km 6 ~2 %, last 6 km >13 %. Per-km: 4.7 / 8.4 / 8.5 / 8.4 / 5.9 / 2.6 / 14.7 / 12.8 / 12.2 /
13.6 / 17.4 / 11.9 %. The 100 m max is a DEM artifact at Cueña les Cabres (10.15-10.45 km, 43.2350,-5.9395): the
route is on the road, but the 30 m DEM squeezes the ramp into ~250 m (EU-DEM 25 m gives ~41 % there, SRTM ~27 %);
the kilometre (17.4 %) matches the published 17.3 %, and the cap keeps it.

### Lagos de Covadonga (from Covadonga, AS-262 / CO-4)

```sh
cargo run --release -- course "Lagos de Covadonga" -o courses/lagos-de-covadonga.json \
  --road "AS-262" --road "CO-4" \
  43.309300,-5.049850 43.273700,-4.983500
```

Spain: elevation from the Terrain Tiles, smoothed over 100 m. Start = below the Basilica, on the AS-262 ~270 m before
the CO-4 (Carretera Covadonga - Los Lagos), where the Vuelta's 12.5 km climb is measured. Starting right at the CO-4
puts the first point on a steep ramp and the smoothing reads it ~25 m high (243 m); starting from Soto de Cangas
(AS-114 junction) adds ~6 km at 1-2 % (18.66 km). Finish = end of the CO-4 by Lago Ercina, past Lago Enol.
Result: 12.66 km, 218 m to 1098 m, +974 m (net +880; the extra is the real descents), avg 7.0 %, max 17.7 % over
100 m (La Huesera), min -12.1 %. Top of the road 1120 m at 10.9 km.
Reference: Vuelta 12.5 km at 6.9 %; PJAMM 12.4 km, 219 m, top 1124 m at ~10.7 km, +964 m, 7.0 %, max ~15 %, La
Huesera 3 km above 10 % with ramps of 14-17.5 %. Per-km: 6.8 / 9.8 / 10.2 / 6.5 / 10.2 / 10.8 / 12.1 / 6.6 / 6.7 / 1.7 /
8.6 / -3.3 %: about 10 % through the forest, La Huesera ~11-12 % at km 5-7, easier after, a short descent near km 9,
the top, then down to Lago Enol and up to Ercina.

### Passo dello Stelvio (from Prato allo Stelvio, SS38)

```sh
cargo run --release -- course "Passo dello Stelvio (Prato)" -o courses/stelvio-prato.json \
  --road "SS38" \
  46.616779,10.590713 46.528618,10.453206
cargo run --release -- cap courses/stelvio-prato.json 12
```

Italy: elevation from the Terrain Tiles (SRTM/EU-DEM), smoothed over 100 m. Start = SS38 (Hauptstraße) in the centre of
Prato allo Stelvio (911 m). Finish = OSM pass node (2758 m); the smoothed top reads 2748 m. No via: the SS38 stays
in the box through Gomagoi and Trafoi.
Result: 24.75 km, 911 m to 2748 m, +1843 m (net +1837 m), avg 7.4 %, max 12.0 % over 100 m (19.7 % before the cap).
Reference: 24.7-24.8 km from ~910 m, col 2757 m, +1846-1851 m, 7.4-7.5 %, max ~12 %, 48 hairpins. Per-km: 2.8 / 5.8 /
6.4 / 5.5 / 5.2 / 7.6 / 5.6 / 7.6 / 7.5 / 6.5 / 7.7 / 7.4 / 8.4 / 6.9 / 10.4 / 7.8 / 10.6 / 8.5 / 8.1 / 8.7 / 9.0 / 8.5 /
8.2 / 7.2 %.
The 100 m max is the elevation data, not the road: in the hairpin stack above Trafoi (km 14-15, 46.5376,10.5005)
the legs run 30-70 m apart, finer than the ~30 m data, so one leg reads flat and the next 19 %. OSM has only the
SS38 there. Elsewhere the max is about 16 %, at the same stack. The cap spreads it; km 15 and 17 stay at 10.4 / 10.5 %.

### Mortirolo (from Mazzo di Valtellina)

```sh
cargo run --release -- course "Mortirolo (Mazzo)" -o courses/mortirolo-mazzo.json \
  46.258304,10.255453 46.254417,10.263058 46.247859,10.300372
```

Italy: elevation from the Terrain Tiles, smoothed over 100 m. The Mazzo road ("Strada del Mortirolo") has no `ref`, so
no `--road`. Start = west end of Via Mortirolo, by the Santo Stefano church. Via = the lower Strada del Mortirolo
(46.2544,10.2631): without it the shortest path takes the "via Orti" variant past San Matteo (11.37 km, 22.8 % at
km 1.3). Finish = Passo della Foppa node (1852 m), where the classic 12.4 km is measured to.
Result: 12.43 km, 555 m to 1857 m, +1302 m (gain = net), avg 10.5 %, max 18.7 % over 100 m. No cap needed.
Reference: 12.3-12.4 km, 552 m to 1852 m, +1300 m, 10.5-10.6 %, steepest km 15 %, max 18-20 % (cyclingup,
valtellinamobile, bormio.eu). Per-km: 3.3 / 10.4 / 11.5 / 10.9 / 15.5 / 14.1 / 11.2 / 12.2 / 11.3 / 9.1 / 9.9 / 7.8 %:
the hardest kilometres around the Pantani monument (km 5-8), easier near the top.

## Climb records (duel)

The page lets a rider duel the fastest ascent ever timed on a climb (`RECORDS` in web/index.html). Only public
times, labelled by year, without the rider's name (French law protects a person's name and likeness). Each record
is measured on the race's own stretch of road, which is not exactly ours: its time over that length and gain
gives an even power (67 kg with the bike, the same air and rolling model as the server), and the ghost rides
that power on our profile, grade by grade. A rider can scale it down (85 %, 70 %, or to their own FTP).

| Climb | Year | Time | Measured on | Sources |
|---|---|---|---|---|
| Alpe d'Huez | 1997 | 37'35" | 14.5 km, ~+1130 m | official record for 31 years (Tour Magazin, Sticky Bottle); a 2026 time of 35'27" is reported but not confirmed here |
| Mont Ventoux (Bédoin) | 2004 | 55'51" | 21.6 km, ~+1610 m | Critérium du Dauphiné time trial (Cyclingnews, Cycling Weekly) |
| Col du Tourmalet (Sainte-Marie-de-Campan) | 2023 | 45'35" | ~17.1 km, ~+1258 m | Tour de France stage 6 (Cyclingnews); a later 43'12" is reported without a confirmed date |

No record yet for the Galibier, the Izoard, La Planche des Belles Filles (its 2020 time-trial split is not
published on its own) and the Madeleine: add one only with a time from a reliable source.

## Riding

`le-mollet serve` loads every `courses/*.json`; the web UI lets the rider pick a profile, then a course.
Each browser tab is a ride session running the physics (power, mass, grade, rolling, air density by
altitude) at 10 Hz in the Rust server.

- Demo mode (no `--trainer`): power comes from a slider in the page.
- `--trainer kickr`: power comes from the trainer, and the most recent session sends the grade to it
  once per second, scaled by `--difficulty` (default 50 %, like Zwift's trainer difficulty).

Virtual speed always uses the real grade.

## 3D view

When a course has a terrain file, the ride screen switches to a chase camera in a voxel world:
64x64-cell chunks (320 m) built around the rider nearest first, one per frame, view radius 3 chunks
(2 on narrow screens); the whole area at 40 m as a low-res mesh for far mountains; the road as a smooth
ribbon on the course elevation with skirts hiding the voxel steps; the garage low-poly bike and rider, pedaling.
Demo mode: click the profile to jump anywhere on the course.

## Next steps

- Measure fps on real laptops and phones, then tune view radius / move meshing to a Web Worker.
- Tile the terrain request for routes longer than ~20 km across.
