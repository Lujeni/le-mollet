<p align="center"><img src="docs/logo.png" alt="Le Mollet" width="300"></p>

# le-mollet

A multiplayer, online indoor cycling training program. Open source, core in Rust.

- **Free forever.** No subscription, no paywall, ever.
- **No account required.** Want to just ride? Open it and train, no sign-up.
- **Self-hostable.** Run the whole thing at home, on your own machine or LAN.
- **Highly modular.** Trainers, courses, terrain and UI are independent pieces you can swap or extend.
- **Just a browser.** No heavy client to install: the UI runs in any modern web browser. An optional, experimental
  native 3D client is in the works for more realistic rendering.

## Ride

- **Real climbs, in real relief.** Legendary climbs rebuilt from OpenStreetMap roads and IGN elevation data,
  validated against their official profiles: Alpe d'Huez, Mont Ventoux, Galibier, Tourmalet, Izoard,
  La Planche des Belles Filles. Pick them as souvenir fridge
  magnets.
- **Free roam.** A procedural "Boucle du jour", the same for everyone today. Ride lap after lap, no finish line.
- **Paris, every street.** All 1,790 km of Paris streets with real elevation: pick left, straight or right at
  each junction, a minimap shows where you go. Montmartre hurts.
- **Ride together.** Everyone on the same server and the same road sees each other in 3D, with name, flag and
  bike. Self-host it and invite your club on your LAN.
- **You feel the road.** Speed comes from your watts, weight, the grade, rolling resistance and thinner air up
  high. A smart trainer gets the slope under your wheels, scaled by a difficulty setting.
- **No trainer? Demo mode.** Ride with a power slider, jump anywhere on the profile to explore a climb.

## Train

- **Structured sessions** calibrated on your FTP: recovery, endurance, sweet spot, threshold, VO2max.
- **ERG mode**: the trainer holds each target power for you. In demo mode, follow the target or turn on autopilot.
- **Interval chart** colored by training zone, with time left in the interval and in the session.
- **Zombie Halloween**: a horde of voxel zombies chases you on a night-time loop, faster each minute. Win mini
  sprints (scaled on your FTP) and grab pumpkins, turbo and shield on the road to keep them behind; your
  survival time, distance and sprints are saved like any ride.

## Your rider

- **Profiles** without an account: name, weight, FTP, optional nationality. Each profile has a unique id; the
  name is just a label.
- **Garage**: a low-poly bike (road, aero, gravel or vintage frame, colors, rims, handlebars) and rider (jersey,
  shorts, shoes, helmet, hair, skin) who pedals on a home trainer.
- **Ride history**: pause anytime, then save or drop the ride. Duration, distance, climbing, average and max
  power and energy are kept and listed on the home screen.

## The world

- **A Lego world** built from real terrain: every voxel is a studded brick in glossy plastic, Lego pines, brick
  buildings, trans-blue lakes, with the road carved into the mountain.
- **Alive**: minifig fans packed toward the summits cheer and wave flags as you pass, cars come the other way,
  pedestrians and pigeons fill the city streets, clouds drift by.
- **Wildlife** placed by land cover and altitude: cows wander and graze, marmots dive into their burrow, deer bolt
  from the road, chamois up high, a raptor circling over the summits.
- **Nature ambience** that follows the terrain: forest and meadow birds, streams, cowbells, wind growing with
  altitude and speed.

## Trainers

Bluetooth smart trainers speaking FTMS (most recent trainers), with the Wahoo proprietary protocol as a fallback
for older KICKRs.

Tested hardware:

| Trainer | Protocol | Connection | Speed | Power | Grade / ERG control |
|---|---|---|---|---|---|
| Wahoo KICKR CORE | FTMS | ✅ | ✅ | ⚠️ only 0 W seen so far, at low speed | ⏳ not verified yet |
| Older Wahoo KICKR (no FTMS) | Wahoo proprietary | ⏳ implemented, untested | ⏳ | ⏳ | ⏳ |

✅ verified on the device · ⚠️ partial · ⏳ not tested yet. Tested another trainer? Add a row.

## Docs

- [Getting started](docs/getting-started.md): run it locally, connect a trainer, play on the LAN, develop.
- [Adding a course](courses/AGENTS.md): turn a real climb into a ridable course.
- [How courses are built](docs/courses.md): data sources, smoothing, validation, recipes.
- [Contributing](docs/CONTRIBUTING.md): principles, checks, commits, testing a trainer.

## Contributing

Contributions are welcome: bug reports, tested trainers, new climbs, code and docs.

- Found a bug or have an idea? [Open an issue](https://github.com/Lujeni/le-mollet/issues/new/choose).
- Own a smart trainer? Test it and add a row to the table above.
- Want to code? Run `cargo fmt`, `cargo clippy --all-targets` and `cargo test`, use
  [Conventional Commits](https://www.conventionalcommits.org), and open a pull request.

Everything else is in the [contributing guide](docs/CONTRIBUTING.md).

## Credits

Road data © OpenStreetMap contributors (ODbL). Elevation © IGN RGE ALTI (Licence Ouverte Etalab 2.0).
Sounds from Wikimedia Commons and Freesound, public domain and CC0: see [web/audio/CREDITS.md](web/audio/CREDITS.md).
