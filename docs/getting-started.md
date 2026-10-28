# Getting started

Run le-mollet locally, with or without a smart trainer.

## Prerequisites

- **Rust 1.85 or newer** (edition 2024). Install with [rustup](https://rustup.rs), or use `mise use rust@latest`.
- **A Chromium-based browser or Firefox** for the UI. Chrome or Edge is recommended for 3D performance.
- **For a real trainer:** a Bluetooth LE adapter. On Linux, BlueZ must be running (`systemctl status bluetooth`).
  `libdbus` is vendored, so no `libdbus-1-dev` package is needed.
- **Network access** only when building courses or terrain (OpenStreetMap Overpass and IGN servers).
  Riding works offline.

## Run

```sh
git clone <repo> le-mollet && cd le-mollet
cargo run -- serve
```

Open http://127.0.0.1:8080, create a profile and ride. Without `--trainer`, the server runs in **demo mode**:
a power slider replaces the trainer.

This is **local mode**: no account, everything stays on this PC (profiles in the browser), and the browser on
the server's own PC is admin. To ride with friends on the same network:

```sh
cargo run -- serve --lan     # listens on the whole network and prints the address to share
```

LAN guests ride as Riders. A public server runs in **cloud mode** instead (`--mode cloud`, sign-in required):
see [accounts and roles](rbac.md).

The page and sounds are embedded in the binary at build time. After editing `web/index.html`, restart
`cargo run -- serve`.

### 3D terrain for the climbs

Terrain files are big and not committed. Build them once (about one minute each):

```sh
cargo run -- terrain courses/alpe-dhuez.json
cargo run -- terrain courses/ventoux-bedoin.json
```

The Lego island is drawn in a map file and built offline (about ten seconds), see [island.md](island.md):

```sh
cargo run --release -- island courses/ile-des-briques.map.json
```

Without them, the climbs play with the elevation profile and the bike in its garage. Open-world loops always
have 3D: they are generated on the fly.

## Ride with a smart trainer

### From the browser (recommended)

Start the server without `--trainer`, open the page in **Chrome or Edge** and pick your profile: the
**Equipment** step comes next (and stays reachable from the home screen). Click **Home trainer** (FTMS trainers such as the KICKR CORE, or older KICKRs) and **Cardio** (any Bluetooth heart rate
strap). Each rider connects their own devices, so everyone on a shared server rides their own trainer. The page
sends the road grade (times the **difficulty** slider, 50 % by default) or the workout target in ERG mode.

Web Bluetooth needs a secure page: `http://localhost` works; on the LAN, serve over https, or in Chrome on the
rider's machine allow the address in `chrome://flags/#unsafely-treat-insecure-origin-as-secure`.

### From the server

```sh
cargo run -- serve --trainer kickr           # name substring of the trainer, case-insensitive
cargo run -- serve --trainer kickr --difficulty 0.7   # share of the real grade sent to the trainer (default 0.5)
```

1. Wake the trainer by pedaling.
2. Disconnect it from other apps (Zwift, Wahoo, phone): BLE trainers accept a single controller.
3. Start the server. It scans for 20 s, connects, then serves the UI.

The trainer drives power. On a course, the server sends it the road grade once per second; in a workout, it
holds each target in ERG mode. One trainer per server: a rider who connects their own trainer in the browser
rides with theirs instead.

### Check the trainer alone

```sh
cargo run -- trainer kickr                    # prints power / cadence / speed, type: erg 200 | grade 5 | quit
LE_MOLLET_RAW=1 cargo run -- trainer kickr    # also dumps raw BLE frames (telemetry and control acks)
```

Supported: FTMS trainers, with the Wahoo proprietary protocol as a fallback for older KICKRs. What has been
verified on which trainer is listed in the README ("Tested hardware"). When you test a trainer, run the console
above with `LE_MOLLET_RAW=1`, pedal above 20 km/h, try `erg 150` and `grade 5`, and update that table.

## Play on the LAN

```sh
cargo run -- serve --addr 0.0.0.0:8080
```

Others open `http://<your-ip>:8080`. Everyone on the same course or open-world loop sees each other.
Profiles and ride history stay in each browser.

## Native client (experimental)

A native 3D client (Bevy) renders a course more realistically: IGN aerial photo on the real relief, physically
based sky and sun. It is a rendering prototype: no menu, profiles, garage or history yet; the web UI stays the
full game. The server must be running first:

```sh
cargo run -- serve
cargo run --release -p le-mollet-client -- --course alpe-dhuez
```

Options: `--server` (default `http://127.0.0.1:8080`), `--course`, `--name`, `--weight` (kg), `--power`
(demo mode starting watts), `--seek` (demo mode start distance, m). Keys: Up/Down ±10 W in demo mode, Esc quits.
Always build it with `--release`: Bevy is too slow unoptimized.

## Commands

| Command | What it does |
|---|---|
| `serve [--addr] [--trainer] [--difficulty] [--courses]` | web UI and ride engine |
| `trainer [name]` | trainer console: telemetry and manual ERG / grade commands |
| `course <name> -o <file> --road <ref> <lat,lon>...` | build a real course from OpenStreetMap + IGN |
| `stats <course.json>` | validation figures of a course |
| `terrain <course.json>` | (re)build the 3D terrain of a course |
| `city <name> -o <file> --relation <osm id> --start <lat,lon>` | build an open city, see [city.md](city.md) |
| `role set <athlete> <role>` | give a Strava athlete a role, see [rbac.md](rbac.md) |
| `island <map.json>` | build the hand-made Lego island, see [island.md](island.md) |

`cargo run -- <command> --help` lists every option. Adding a course step by step:
[courses/AGENTS.md](../courses/AGENTS.md).

## Develop

```sh
cargo test                       # unit tests: protocol, physics, courses, rides, workouts, worlds, server input
cargo clippy --all-targets       # must stay clean
cargo build --release            # optimized binary in target/release/le-mollet
```

Layout:

| Path | Content |
|---|---|
| `src/protocol.rs` | BLE trainer payloads (FTMS, Wahoo), pure and tested |
| `src/lib.rs` | trainer connection (btleplug) |
| `src/physics.rs`, `src/ride.rs` | power to speed, ride totals and laps, road bonuses |
| `src/course.rs`, `src/terrain.rs` | real courses and their voxel terrain |
| `src/world.rs` | procedural open-world loops |
| `src/workout.rs` | structured training sessions |
| `src/zombie.rs` | Zombie Halloween game: horde waves, mini sprints, bonuses |
| `src/web.rs`, `src/main.rs` | HTTP server, ride sessions, CLI |
| `web/index.html` | the whole UI (three.js) |
| `client/` | experimental native 3D client (Bevy) |
| `courses/` | course files, see its AGENTS.md |
| `docs/courses.md` | how real roads become courses |
| `src/city.rs`, `docs/city.md` | open city: street graph, turns, growing route |
| `src/auth.rs`, `src/strava.rs`, `docs/rbac.md` | roles and permissions, sign in with Strava |
| `src/decor.rs` | decor from the real place: aerial photo mosaic and trees, OSM pieces (`le-mollet decor`) |
| `src/event.rs` | group rides and races: pen, phases, ranks (Bronze to Légende), races every half hour, results |
| `src/db.rs` | the database: one SQLite file (`le-mollet.db`) of JSON documents, older JSON files imported |
| `src/rider.rs` | fair play: the server's record of each rider (weight, FTP seen, rank) and the checks on each second of power (docs/security.md) |
| `src/club.rs` | clubs: members by profile, kilometres of the week |
| `src/island.rs`, `docs/island.md` | hand-made Lego island: map file to roads, terrain and decor |

## Troubleshooting

| Symptom | Fix |
|---|---|
| `no trainer found within 20s` | Pedal to wake it, make sure no other app holds it, check `bluetoothctl show`. |
| `connection attempt 1 failed ... retrying` | Normal with BlueZ; the engine retries 3 times. |
| `Operation already in progress` | Another scan is running (e.g. `bluetoothctl scan on`): stop it. |
| Power stays at 0 W | The trainer reports 0 at very low speed: pedal above ~20 km/h. Check raw frames with `LE_MOLLET_RAW=1`. |
| No sound | Browsers start audio after a click; the ambience starts when you pick a ride. Check the "♪" button. |
| Climbs without 3D | Build their terrain (see above). |
