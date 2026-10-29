# Contributing

le-mollet is free, open source and built by riders. Bug reports, tested trainers, new climbs, code and docs are
all welcome. This page explains how to help and what a change needs before it is merged.

## Ways to help

| You have | Do this |
|---|---|
| A bug | Open a [bug report](https://github.com/Lujeni/le-mollet/issues/new?template=bug_report.yml). |
| An idea | Open a [feature request](https://github.com/Lujeni/le-mollet/issues/new?template=feature_request.yml) before writing code. |
| A smart trainer | Test it and add a row to the "Tested hardware" tables in [hardware.md](hardware.md) (see [Testing a trainer](#testing-a-trainer)). |
| A favorite climb | Add it as a course: follow [courses/AGENTS.md](../courses/AGENTS.md). |
| Some time | Pick an open issue, say in a comment that you take it, then open a pull request. |

For anything bigger than a small fix, open an issue first so the design can be agreed on before you spend
time on it.

## Project principles

Every change must keep these promises (see the [README](../README.md)):

- **Free forever, no account.** No paywall, no mandatory sign-up, no tracking.
- **Self-hostable.** Everything runs on one machine or a LAN. Riding works offline; network access is only for
  building courses and terrain.
- **Just a browser.** The UI is a single page (`web/index.html`), no client to install, no frontend build step.
- **Rust only.** The engine, server and tooling are Rust. No Python, shell or other scripts land in the repo.
- **Real data.** Courses come from OpenStreetMap and IGN, validated against official figures, never hand-edited.
- **Keep it small.** Prefer the standard library and the dependencies already in `Cargo.toml`. A new dependency
  needs a reason in the pull request.

## Setup

Follow [Getting started](getting-started.md): Rust 1.85+ (edition 2024), then `cargo run -- serve`. No trainer
is needed: demo mode replaces it with a power slider.

`web/index.html` and the sounds are embedded in the binary at build time: restart `cargo run -- serve` after
editing them.

## Before opening a pull request

```sh
cargo fmt -p le-mollet    # what the CI checks (rustfmt.toml: wide lines, small things on one line)
cargo test -p le-mollet   # and runs
cargo clippy -p le-mollet # advice, worth a look, not a gate
```

- **Tests.** New logic comes with a unit test next to it (`#[cfg(test)] mod tests` in the same file). Pure code
  (protocol parsing, physics, courses, workouts) is easy to test: keep BLE and network calls at the edges.
- **UI changes.** Try them in a browser, in demo mode at least, and add a screenshot or a short clip to the
  pull request.
- **Trainer changes.** Say which trainer you tested on, and paste the relevant `LE_MOLLET_RAW=1` frames if the
  protocol changed.
- **Courses.** Commit the `courses/<id>.json` only; `*.terrain.bin` is git-ignored. Paste the `stats` output
  next to the reference figures in the pull request.
- **Docs.** Update the README, `docs/` or `courses/AGENTS.md` when behavior, commands or options change.

## Code style

- Match the surrounding code: naming, comment density, error handling with `anyhow`.
- Small modules with one job each (see the layout table in [Getting started](getting-started.md#develop)).
- Comments explain *why*, not *what*. Deliberate shortcuts are marked so they can be found later:
  `// ponytail: <what was simplified>, <when to do better>`.
- No dead code, no speculative abstractions, no config for values that never change.

## Releasing

A tag `v*` on `main` makes the release: the workflow in `.github/workflows/release.yml` builds the binary for
Linux (x86_64, ARM), macOS (Apple Silicon, Intel) and Windows, packs each with the courses and the docs, writes
`SHA256SUMS`, and publishes a GitHub release with generated notes.

1. `cz bump` ([commitizen](https://commitizen-tools.github.io/commitizen/), config in `.cz.toml`): it reads the commits since
   the last tag, picks the version (feat → minor, fix → patch; major stays 0), bumps `[workspace.package].version`
   in `Cargo.toml`, updates `CHANGELOG.md`, commits and tags `vX.Y.Z`.
2. `git push && git push --tags`.
3. Watch the Actions tab; the release appears under Releases when the five builds are done.

Every push and pull request runs `.github/workflows/ci.yml`: `cargo fmt --check` (rustfmt.toml keeps the dense
style: 200 columns, small things on one line) and `cargo test`, on the server crate. Clippy is advice, run
locally: a gate on it moves with every toolchain.

## Commits and pull requests

Commits follow [Conventional Commits](https://www.conventionalcommits.org):

```
<type>(<optional scope>): <short summary, imperative, lower case>
```

| Type | For |
|---|---|
| `feat` | a new feature for riders |
| `fix` | a bug fix |
| `docs` | documentation only |
| `refactor` | code change with no behavior change |
| `test` | tests only |
| `perf` | faster or lighter |
| `chore` | tooling, dependencies, housekeeping |

Scopes in use: `core`, `web`, `courses`, `trainer`. Examples: `feat(courses): add Galibier from Valloire`,
`fix(trainer): retry FTMS control point on BlueZ timeout`.

Pull requests:

1. Fork, then branch from `main` (`feat/galibier`, `fix/erg-retry`).
2. One topic per pull request; keep it small enough to review in one sitting.
3. Describe what changed and why, how you tested it, and link the issue (`Closes #12`).
4. Rebase on `main` if it moved; keep the history clean (squash fixup commits).

## Testing a trainer

[hardware.md](hardware.md) lists which trainers have been verified. To add yours:

```sh
LE_MOLLET_RAW=1 cargo run -- trainer <name>
```

1. Pedal above 20 km/h and check that power, cadence and speed look right.
2. Type `erg 150`, then `grade 5`, and feel the resistance change.
3. Type `quit`, then ride a course with `cargo run -- serve --trainer <name>`.
4. Add or update the row in [hardware.md](hardware.md) (✅ / ⚠️ / ⏳) and open a pull request. If something fails, open a
   bug report with the raw frames instead.

## Debugging hardware

Most hardware bugs come down to "what does the device actually send?". The tools, from the outside in:

1. **`le-mollet probe`** lists the Bluetooth devices around, or maps one: its services and characteristics, the
   readable values (maker, model, **firmware**), then every notification for a while, timestamped, in hex.
   `--write <characteristic>=<hex|text:…>` sends a handshake first, for devices that stay silent until spoken
   to. Full guide and the devices mapped so far: [hardware.md](hardware.md).
2. **Map buttons three presses at a time**: press each button three times quickly, then wait 5 s before the next,
   and write down the order. Three identical messages in a row, then silence, cannot be mistaken; compare each
   group with the released frame to find the bit.
3. **`LE_MOLLET_RAW=1`** prints every raw frame of the trainer the server drives (`trainer` and `serve
   --trainer`).
4. **In the browser**: `chrome://bluetooth-internals` (Devices, Start scan) shows what Chrome itself sees. If a
   device shows in `probe` but not there, the problem is between Chrome and the system, not in the page. On
   Linux, Web Bluetooth needs `chrome://flags/#enable-experimental-web-platform-features`.
5. **A device that does not show up** is often still connected to something: close the other apps, and on Linux
   `bluetoothctl devices Connected` then `bluetoothctl disconnect <address>`. BlueZ also drops the first
   connection attempt now and then: `probe` retries three times.

## Debugging performance

In a ride, `window.lemolletDebug()` in the browser console gives the frame time, the JavaScript time per frame, the
draw calls and triangles, the chunks loaded (and how many at the far level of detail), the other riders drawn, the
pixel ratio and whether shadows are in their plain mode. Quality is automatic: when frames run past ~22 ms the
render drops pixels (down to 0.75 of the screen's ratio) then plainer shadows; when they run under ~13 ms it
grows back. Terrain past the rider's chunk and its neighbours is built at a quarter of the triangles without
decor or shadows (`chunkMesh(T, i, j, step)` in `web/index.html`); `CHUNK_RADIUS` sets how far it goes.

Where the code lives: the page reads trainers (`connectTrainer`, `parseBikeData`, `parseCps`) and controllers
(`connectPad`, the `PAD` button table, `PAD_ACTIONS`) in `web/index.html`; virtual gears are `virtualGrade`
there too. The server's own trainer is `Trainer` in `src/lib.rs` and `src/protocol.rs`; the probe is
`src/probe.rs`. When you add a device, put what you measured in [hardware.md](hardware.md) ("Known devices"):
the next person starts from there.

## Licensing and data

- Code contributions are released under the project license, the **PolyForm Noncommercial License 1.0.0**, and
  you also grant Lujeni the right to license them under other terms, commercial ones included (the dual license).
  By opening a pull request you agree to this.
- Only add data and assets you are allowed to redistribute: OpenStreetMap (ODbL), IGN (Licence Ouverte),
  public domain or CC0 sounds. Credit them (README "Credits", `web/audio/CREDITS.md`).
- No trademarks or proprietary data: no official race logos, route files or names used as branding.

## Asking for help

Open an issue, or comment on the pull request. Questions are welcome; there is no silly one.
