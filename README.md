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

Open http://127.0.0.1:8080, create a profile, then "Choisir un circuit".
Rides play a nature ambience that follows the terrain (forest birds, meadow birds, streams, wind with altitude and speed); sources and licences in [web/audio/CREDITS.md](web/audio/CREDITS.md).

The page (`web/index.html`) and sounds are embedded in the binary at build time. Profiles are stored in the browser (no login yet).

## Real-world courses

Built from OpenStreetMap roads and IGN elevation, in `courses/`: Alpe d'Huez, Mont Ventoux (Bédoin).
Method, validation checklist and recipes: [docs/courses.md](docs/courses.md).

The 3D voxel terrain files are not committed. Build them once (needs network, ~1 min each):

```sh
cargo run -- terrain courses/alpe-dhuez.json
cargo run -- terrain courses/ventoux-bedoin.json
```
