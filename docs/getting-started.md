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

The page and sounds are embedded in the binary at build time. After editing `web/index.html`, restart
`cargo run -- serve`.

### 3D terrain for the climbs

Terrain files are big and not committed. Build them once (about one minute each):

```sh
cargo run -- terrain courses/alpe-dhuez.json
cargo run -- terrain courses/ventoux-bedoin.json
```

Without them, the climbs play with the elevation profile and the bike in its garage. Open-world loops always
have 3D: they are generated on the fly.

## Ride with a smart trainer

```sh
cargo run -- serve --trainer kickr           # name substring of the trainer, case-insensitive
cargo run -- serve --trainer kickr --difficulty 0.7   # share of the real grade sent to the trainer (default 0.5)
```

1. Wake the trainer by pedaling.
2. Disconnect it from other apps (Zwift, Wahoo, phone): BLE trainers accept a single controller.
3. Start the server. It scans for 20 s, connects, then serves the UI.

The trainer drives power. On a course, the server sends it the road grade once per second; in a workout, it
holds each target in ERG mode.

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

## Commands

| Command | What it does |
|---|---|
| `serve [--addr] [--trainer] [--difficulty] [--courses]` | web UI and ride engine |
| `trainer [name]` | trainer console: telemetry and manual ERG / grade commands |
| `course <name> -o <file> --road <ref> <lat,lon>...` | build a real course from OpenStreetMap + IGN |
| `stats <course.json>` | validation figures of a course |
| `terrain <course.json>` | (re)build the 3D terrain of a course |

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
| `src/physics.rs`, `src/ride.rs` | power to speed, ride totals and laps |
| `src/course.rs`, `src/terrain.rs` | real courses and their voxel terrain |
| `src/world.rs` | procedural open-world loops |
| `src/workout.rs` | structured training sessions |
| `src/web.rs`, `src/main.rs` | HTTP server, ride sessions, CLI |
| `web/index.html` | the whole UI (three.js) |
| `courses/` | course files, see its AGENTS.md |
| `docs/courses.md` | how real roads become courses |

## Troubleshooting

| Symptom | Fix |
|---|---|
| `no trainer found within 20s` | Pedal to wake it, make sure no other app holds it, check `bluetoothctl show`. |
| `connection attempt 1 failed ... retrying` | Normal with BlueZ; the engine retries 3 times. |
| `Operation already in progress` | Another scan is running (e.g. `bluetoothctl scan on`): stop it. |
| Power stays at 0 W | The trainer reports 0 at very low speed: pedal above ~20 km/h. Check raw frames with `LE_MOLLET_RAW=1`. |
| No sound | Browsers start audio after a click; the ambience starts when you pick a ride. Check the "♪" button. |
| Climbs without 3D | Build their terrain (see above). |
