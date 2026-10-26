<p align="center"><img src="docs/logo.png" alt="Le Mollet" width="300"></p>

# le-mollet

A multiplayer, online cycling and running physical training program. Open source, core in Rust.

- **Free forever.** No subscription, no paywall, ever.
- **No account required.** Want to just ride? Open it and train, no sign-up.
- **Self-hostable.** Run the whole thing at home, on your own machine or LAN.
- **Highly modular.** Trainers, courses, terrain and UI are independent pieces you can swap or extend.
- **Just a browser.** No heavy client to install: the UI runs in any modern web browser.

## Trainer engine (BLE)

Supports FTMS trainers and Wahoo KICKR (proprietary protocol fallback).

```sh
cargo run -- trainer kickr                   # connect, print power/cadence/speed
# then type: erg 200 | grade 5 | quit
LE_MOLLET_RAW=1 cargo run -- trainer kickr   # also dump raw BLE frames
```

## Web UI: profiles, garage, courses

```sh
cargo run -- serve                         # demo mode: pick a course, ride with a power slider
cargo run -- serve --trainer kickr         # ride for real: power from the trainer, grade sent to it
cargo run -- serve --addr 0.0.0.0:8080     # expose on the LAN
```

Open http://127.0.0.1:8080 and pick a profile. The home screen leads to:

- **Rouler**: two tiles. **Circuits**: the climbs, shown as fridge magnets on a fridge door. **Free**, the open world: procedurally generated loops (5, 10 or 20 km; flat, rolling or mountainous) built by the Rust core (`src/world.rs`), with a "Boucle du jour" always open. Everyone connected to the server rides together: other riders show up in 3D with their name and flag, on every course.
- **Training**: structured sessions in % of FTP (recovery, endurance, sweet spot, threshold, VO2max). With a trainer, it holds each target in ERG mode; in demo mode, follow the target with the slider or turn on "Auto".
- **Garage**: customize the bike: frame shape (road, aero, gravel, vintage), colors, rims, bars, brick or cube style. Other riders see your frame.
- **Profil**: name, weight, FTP, optional nationality, delete. Each profile has a UUID v4 id (stable key for a future backend); the name is only a label and may be shared.

During a ride or a workout, "‖ Pause" (or Escape) freezes the ride and opens a menu: resume, finish and save, or quit without saving. It also opens by itself at the finish line or when the workout ends. Saved rides (duration, ridden distance, climbing, average and max power, energy) are kept in the profile and listed on the home screen.
Rides play a nature ambience that follows the terrain (forest birds, meadow birds, streams, wind with altitude and speed). Wildlife is placed from land cover and altitude (cows in alpine pastures, marmots and chamois up high, deer at forest edges, a raptor over the summits) and calls when you ride by. Sound sources and licences: [web/audio/CREDITS.md](web/audio/CREDITS.md).

The page (`web/index.html`) and sounds are embedded in the binary at build time. Profiles are stored in the browser (no login yet).

## Real-world courses

Built from OpenStreetMap roads and IGN elevation, in `courses/`: Alpe d'Huez, Mont Ventoux (Bédoin).
To add one, follow [courses/AGENTS.md](courses/AGENTS.md) (`cargo run -- stats courses/<id>.json` prints the validation figures). Method and recipes: [docs/courses.md](docs/courses.md).

The 3D voxel terrain files are not committed. Build them once (needs network, ~1 min each):

```sh
cargo run -- terrain courses/alpe-dhuez.json
cargo run -- terrain courses/ventoux-bedoin.json
```
