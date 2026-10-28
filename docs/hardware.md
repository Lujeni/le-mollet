# Hardware

## Tested hardware

✅ verified on the device · ⚠️ partial · ⏳ not tested yet. Tested something? Add a row (see
[Testing a trainer](CONTRIBUTING.md#testing-a-trainer)).

| Trainer | Firmware | Protocol | Connection | Speed | Cadence | Power | Grade (ride) | ERG (workout) |
|---|---|---|---|---|---|---|---|---|
| Wahoo KICKR CORE | 1.3.17 | FTMS | ✅ | ✅ | ✅ | ✅ | ✅ from the browser | ⏳ |
| Older Wahoo KICKR (no FTMS) | | Wahoo proprietary | ⏳ implemented, untested | ⏳ | ⏳ | ⏳ | ⏳ | ⏳ |

| Controller | Firmware | Connection | Virtual gears | Other buttons |
|---|---|---|---|---|
| Zwift Ride (both hoods) | 1.1.0 | ✅ one per hood | ✅ right shifts up, left down | ✅ Ride On, power-up, turns, pause; brake levers read, unused |

| Heart rate strap | Connection | Heart rate |
|---|---|---|
| any Bluetooth strap (Heart Rate service) | ⏳ implemented, untested | ⏳ |

The KICKR CORE was verified with the probe below (speed, cadence and power once a second on both FTMS and Cycling
Power) and by riding with the virtual gears from the page. The Zwift Ride's two hoods were mapped button by button
(see "Known devices").

## Supporting new hardware: `le-mollet probe`

A trainer, a power meter, a controller that le-mollet does not know yet? Run the probe and send its output: it is
what is needed to write the support. It uses the computer's own Bluetooth (Windows 10+, macOS, Linux), not the
browser, so it sees every service, even the proprietary ones.

Before starting: wake the device (pedal, press a button), and **close Zwift and any other app** that could be
connected to it. A Bluetooth device talks to one app at a time.

### 1. List the devices around

```sh
le-mollet probe
```

Each device with its name, address, the services it advertises and its maker data. Find yours in the list.

### 2. Map it and record what it sends

```sh
le-mollet probe "KICKR" > probe.txt        # a part of its name, or its address
```

It connects, prints every service and characteristic (with the readable values: maker, model, **firmware**),
subscribes to everything that can be subscribed to, then logs every message for 60 s (`--seconds 120` for
longer, Ctrl-C to stop early). Meanwhile, **do one thing at a time, slowly, and note the time**:

- a trainer: pedal easy, then harder, stop pedalling;
- a controller: press each button once, one after the other, a few seconds apart, in an order you write down
  (for example: shift up right, shift down right, A, B...);
- a heart rate strap: just wear it.

Send `probe.txt` with what you did and when. The lines look like:

```
  12.408s 00000002-19ca-4651-86e5-fa29dcdd09d1 Zwift async (notify)  23 08 ff ff ff ff 0f
```

### 3. Devices that wait for a word first

Some devices stay silent until the app says something. `--write` sends bytes once everything is subscribed,
as hex or text (repeatable):

```sh
# Zwift Ride / Click: the "RideOn" handshake
le-mollet probe "Zwift" --write 00000003-19ca-4651-86e5-fa29dcdd09d1=text:RideOn
# An FTMS trainer: take control, start
le-mollet probe "KICKR" --write 2ad9=00 --write 2ad9=07
```

### Running it

There is no installer yet: build it once with [Rust](https://rustup.rs) (on Windows, rustup installs the Visual
Studio build tools it needs), then:

```sh
git clone https://github.com/lujeni/le-mollet && cd le-mollet
cargo run --release -- probe
```

On Linux, Bluetooth goes through BlueZ (`bluetoothd` running, your user allowed to use it). On macOS, allow the
terminal to use Bluetooth when asked.

## Known devices

### Zwift Ride controllers (firmware 1.1.0, measured)

Two devices, one per hood: serial `07-…` the right one, `08-…` the left one, maker "Zwift Inc", service
`00000001-19ca-4651-86e5-fa29dcdd09d1`. After `RideOn` written to `00000003`, they answer `RideOn` on `00000004`
and send on `00000002`:

- `15` every second (alive), `19 10 <n>` every 5 s (battery, n %);
- `23 08 <varint> …` on every change of the buttons, then every ~0.1 s while one is held: the varint is the
  button map, **active low** (a cleared bit is a pressed button; `ff ff ff ff 0f` is all released);
- after it, `1a 04 08 <hood> 10 <value>` for each brake lever: hood 0 left, 1 right, value a zigzag sint32
  from 0 to −100 (`c7 01`) as it is pulled.

| Bit | Button | Bit | Button |
|---|---|---|---|
| 1 | left arrow | 256 | left black button, top |
| 2 | up arrow | 512 | left black button, bottom |
| 4 | right arrow | 1024 | left power-up |
| 8 | down arrow | 4096 | right black button, top |
| 16 | A | 8192 | right black button, bottom |
| 32 | B | 16384 | right power-up |
| 64 | Y | | |
| 128 | Z | | |

The on/off buttons send nothing on a short press. BikeControl's table differs for Z (256) and the power-ups.

### Wahoo KICKR CORE (firmware 1.3.17)

FTMS (Indoor Bike Data `44 00`: speed 0.01 km/h, cadence 0.5 rpm, power W, once a second), Cycling Power with
crank and wheel revolutions, the Wahoo control point, and **also the Zwift service** (`00000001-19ca-…`): the
trainer side of Zwift's own virtual shifting, not decoded yet.
