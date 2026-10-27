# Adding a course

Instructions for agents (and humans) adding a real climb to le-mollet. Each course in this folder is one
`<id>.json`, plus a git-ignored `<id>.terrain.bin` for the 3D world. The method and its rationale are in
[../docs/courses.md](../docs/courses.md): read it once, follow this file every time.

## Rules

- **Rust only.** Build and check with the `le-mollet` commands below. Ad hoc analysis in a scratch dir is
  fine, but no Python, shell or other scripts land in the repo or in the instructions.
- **France only for now.** Elevation comes from IGN, and the build fails elsewhere ("IGN has no elevation").
  A route abroad needs a Copernicus fallback first: say so instead of faking data.
- **Real data, validated.** Never hand-edit points or elevations to match a reference. If the figures are
  off, fix the inputs (waypoints, roads) and rebuild.
- **Commit the JSON only.** `*.terrain.bin` is rebuilt by `terrain` and ignored by git.

## Steps

### 1. Gather the reference

Before building, write down the climb's official figures from its signage, the Tour de France roadbook or
climb sites (cols-cyclisme, climbbybike):

- Start town and point, finish point, road number (e.g. D 211).
- Length, start and finish altitude, average grade.
- Per-km grades or the published profile, max grade.

### 2. Pick the id, name, roads and waypoints

- **id**: kebab-case file stem, with the side for climbs that have several: `galibier-valloire`, `ventoux-malaucene`.
- **name**: display name on the magnet, side in parentheses: `"Mont Ventoux (Bédoin)"`.
- **roads**: the OSM `ref` of the climb road, exactly as tagged, with its space: `"D 211"`, not `D211`.
  Repeat `--road` when the climb changes road (Alpe d'Huez ends on `D 211F`). Check a road's ref by
  clicking it on openstreetmap.org (query features tool).
- **waypoints** as `lat,lon` with 6 decimals: the start, optional vias, the finish.
  - Start and finish exactly where the reference measures them, on the road itself.
  - Add a via when the road leaves the start-finish box by more than ~3 km (Ventoux goes through Chalet
    Reynard, east of the summit), or when the route takes a shortcut the race does not.

### 3. Build

```sh
cargo run -- course "<Name>" -o courses/<id>.json --road "<ref>" [--road "<ref2>"] <start> [<via>...] <finish>
```

This writes `<id>.json`, then downloads the 3D terrain into `<id>.terrain.bin`, which takes about a minute.
The OSM Overpass and IGN servers rate-limit, and the builder retries on 429/502/503/504. Re-running the
command is safe.

A loop mapped as an OpenStreetMap route relation (a cycling ring) builds with `--relation <id>` instead of
`--road` and waypoints; give the start as the only waypoint. Add `--cell 2` for small bricks on a short loop.
Then `cargo run --release -- decor courses/<id>.json` (ground mosaic and trees from the aerial photo, OSM
benches, lamps, fences...) and `cargo run --release -- buildings courses/<id>.json` (real buildings). See
"Loops" and "Decor" in [../docs/courses.md](../docs/courses.md).

### 4. Validate

```sh
cargo run -- stats courses/<id>.json
```

Compare with the reference. Every line must pass:

| Check | Tolerance |
|---|---|
| Length | within ~3 % |
| Start and finish altitude | within ~10 m (summit roads often stop a bit below the top) |
| Average grade | within ~0.3 % |
| Gain vs net gain on a pure climb | gain at most ~5 % above net, otherwise there are fake ups and downs |
| Max grade over 100 m | plausible for this road, usually 10-15 %; 18 %+ means a shortcut or a bad line |
| Per-km grades | same shape as the published profile |

Then look at it: `cargo run -- serve`, open http://127.0.0.1:8080, pick the course, and click along the
profile in demo mode to jump through the climb in 3D.

### 5. Fix what fails

| Symptom | Cause | Fix |
|---|---|---|
| `no road between A and B` | the road leaves the downloaded box, or the ref is wrong | add a via on the road where it strays; check the `ref` spelling |
| Too short, with an 18-25 % spike | the route took a lane or a footpath | add a via on the real road at that spot; add the missing `--road` |
| Too long, with a dip | detour around a one-way or a loop | move or remove the via that caused it |
| `no road matching [...]` | ref not found near the waypoints | check the exact tag on openstreetmap.org |
| `IGN has no elevation` | outside France | out of scope for now, see Rules |
| `terrain ... exceeds one IGN request` | route spans more than ~20 km across | out of scope for now, needs tiled requests |

Smoothing (Gaussian, sigma 50 m) is tuned in `src/course.rs`. Do not change it for one course: it applies to
every course and is documented in docs/courses.md.

### 6. Record the recipe

Append a section under "Recipes" in [../docs/courses.md](../docs/courses.md) with:

- the exact `course` command;
- the `stats` result next to the reference figures;
- any via you added and why.

The next person can then rebuild the course identically.

### 7. Ship

- `git add courses/<id>.json docs/courses.md`. The terrain file stays out of git.
- Restart `cargo run -- serve`. Courses are loaded at startup, and the new magnet appears on the fridge.
- On another machine, rebuild the terrain with `cargo run -- terrain courses/<id>.json`; a course with decor
  then needs `decor` and `buildings` again (the terrain file carries the ground mosaic).

## Attribution

Every course carries the OSM (ODbL) and IGN (Etalab 2.0) attribution, written by the builder and shown during
rides. Do not remove it, and do not add data from sources with incompatible licences, such as Strava segments
or commercial GPX files.
