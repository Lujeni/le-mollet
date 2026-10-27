<p align="center"><img src="docs/logo.png" alt="Le Mollet" width="300"></p>

# le-mollet

A multiplayer, online indoor cycling training program. Open source, core in Rust.

- **Free forever.** No subscription, no paywall, ever.
- **No account required.** Run it on your PC and ride: no sign-up, everything stays local, you are admin. Open it to
  your LAN with `serve --lan`. A public server (`--mode cloud`) asks riders to sign in with Strava
  ([accounts and roles](docs/rbac.md)).
- **Self-hostable.** Run the whole thing at home, on your own machine or LAN.
- **Highly modular.** Trainers, courses, terrain and UI are independent pieces you can swap or extend.
- **Just a browser.** No heavy client to install: the UI runs in any modern web browser. An optional, experimental
  native 3D client is in the works for more realistic rendering.

<p align="center"><img src="docs/screenshots/alpe.jpg" alt="Climbing the last hairpins of Alpe d'Huez, minifig fans cheering"></p>

## Screenshots

<table>
<tr>
<td width="50%"><img src="docs/screenshots/home.jpg" alt="Home screen: menu bricks and the Lego rider on the trainer"><br><sub><b>Home.</b> Your rider on the trainer, everything one brick away.</sub></td>
<td width="50%"><img src="docs/screenshots/destinations.jpg" alt="Where to ride: climbs, the Lego island, Paris, the daily loop"><br><sub><b>Where to ride.</b> Real climbs, a hand-made Lego island, Paris, the daily loop.</sub></td>
</tr>
<tr>
<td><img src="docs/screenshots/island.jpg" alt="Riding the pirate port of the Île des Briques, round minimap"><br><sub><b>Île des Briques.</b> The pirate port, its cars and walkers, the round Lego minimap.</sub></td>
<td><img src="docs/screenshots/start-picker.jpg" alt="Start picker: the island as a Lego mosaic with numbered pins"><br><sub><b>Pick a start.</b> The island as a Lego mosaic, a pin per place.</sub></td>
</tr>
<tr>
<td><img src="docs/screenshots/longchamp.jpg" alt="Anneau de Longchamp: the road along the Étang des Tribunes, Lego trees and a ground mosaic read from the aerial photo"><br><sub><b>Anneau de Longchamp.</b> Ground and trees read from the IGN aerial photo, the Étang des Tribunes.</sub></td>
<td><img src="docs/screenshots/longchamp-woods.jpg" alt="Anneau de Longchamp in the rain through the Bois de Boulogne"><br><sub><b>Bois de Boulogne.</b> Every tree where the photo shows one, here in the rain.</sub></td>
</tr>
<tr>
<td><img src="docs/screenshots/climbs.jpg" alt="Climbs as fridge magnets"><br><sub><b>Climbs.</b> Legendary climbs as fridge magnets.</sub></td>
<td><img src="docs/screenshots/garage.jpg" alt="Garage: the Lego minifig rider and its options"><br><sub><b>Garage.</b> Your minifig: jersey and pattern, helmet, glasses, face.</sub></td>
</tr>
<tr>
<td><img src="docs/screenshots/history.jpg" alt="Ride history: totals, weekly bricks, records, ride details"><br><sub><b>History.</b> Totals, weekly bricks, records, curves and FIT export.</sub></td>
<td><img src="docs/screenshots/equipment.jpg" alt="Equipment step: connect the trainer and heart rate strap"><br><sub><b>Equipment.</b> Connect your trainer and heart rate strap from the browser.</sub></td>
</tr>
<tr>
<td><img src="docs/screenshots/badges.jpg" alt="Badge wall: 27 badges, earned ones lit"><br><sub><b>Badges.</b> 27 to earn, levels unlock garage pieces.</sub></td>
<td><img src="docs/screenshots/training.jpg" alt="Training: structured sessions and .zwo import"><br><sub><b>Training.</b> Sessions on your FTP, import Zwift .zwo files, FTP test.</sub></td>
</tr>
<tr>
<td><img src="docs/screenshots/rain-night.jpg" alt="Riding Alpe d'Huez at night in the rain, lit windows, headlamp"><br><sub><b>Night and rain.</b> The sky follows your clock, the weather turns every 20 minutes.</sub></td>
<td><img src="docs/screenshots/snow.jpg" alt="Snow falling on the Galibier road"><br><sub><b>Snow.</b> On the high passes, rain falls as Lego snowflakes.</sub></td>
</tr>
<tr>
<td><img src="docs/screenshots/bonus-arch.jpg" alt="A blue and yellow bonus arch over the road at sunset"><br><sub><b>Bonus arches.</b> One every kilometre, here at sunset.</sub></td>
<td><img src="docs/screenshots/bonus.jpg" alt="Draft bonus in use, its time bar in the ride panel"><br><sub><b>Road bonuses.</b> Feather, Aero or Draft: press B when it counts.</sub></td>
</tr>
<tr>
<td><img src="docs/screenshots/plan.jpg" alt="FTP plan calendar: sessions done, missed and to come, the next one"><br><sub><b>Training plans.</b> A calendar from the day you start, sessions ticked off as you ride.</sub></td>
<td><img src="docs/screenshots/form.jpg" alt="Form chart: fitness, fatigue and daily stress over 90 days, power records"><br><sub><b>Form.</b> Fitness, fatigue, freshness, power records and an FTP estimate.</sub></td>
</tr>
<tr>
<td><img src="docs/screenshots/events.jpg" alt="Events: a group ride and a race with entrants by category, the form to schedule one"><br><sub><b>Events.</b> Group rides and races, sign up, categories A to D.</sub></td>
<td><img src="docs/screenshots/pen.jpg" alt="In the start pen at Longchamp: the countdown before the group ride"><br><sub><b>Start pen.</b> Everyone waits on the line, then goes at once.</sub></td>
</tr>
<tr>
<td><img src="docs/screenshots/duel.jpg" alt="Duel the record: the Alpe d'Huez record of 1997 at 100 %, 85 %, 70 % or at your FTP"><br><sub><b>Duel the record.</b> The fastest ascent as a ghost, scaled to a fair fight.</sub></td>
<td><img src="docs/screenshots/rider-card.jpg" alt="Rider card: overall score, rider type and six ratings from your own power"><br><sub><b>Rider card.</b> Ratings from your own power, like a manager game.</sub></td>
</tr>
<tr>
<td><img src="docs/screenshots/club.jpg" alt="Club page: tag, invite code, members ranked by kilometres this week"><br><sub><b>Clubs.</b> Your tag on the road, the week's kilometres, an invite code.</sub></td>
<td><img src="docs/screenshots/fog.jpg" alt="Alpe d'Huez in thick fog"><br><sub><b>Fog.</b> Some spells you only see the next hairpin.</sub></td>
</tr>
</table>

## Ride

- **Real climbs, in real relief.** Legendary climbs rebuilt from OpenStreetMap roads and IGN elevation data,
  validated against their official profiles: Alpe d'Huez, Mont Ventoux, Galibier, Tourmalet, Izoard,
  La Planche des Belles Filles. Pick them as souvenir fridge magnets.
- **Duel the record.** On the Alpe d'Huez, the Ventoux and the Tourmalet, race the fastest ascent ever timed as a
  ghost: its public time turned into even power on our road, at 100 % or scaled down to a fair fight at your FTP.
- **Race your ghost.** Finish a climb and your best time rides with you next time, see-through, with the gap in
  seconds live and your record on the magnet.
- **Free roam.** A procedural "Boucle du jour", the same for everyone today. Ride lap after lap, no finish line.
- **Paris, every street.** All 1,790 km of Paris streets with real elevation: pick left, straight or right at
  each junction, a round Lego minimap shows where you go. Start from Notre-Dame, Concorde, Montmartre and more.
  Montmartre hurts.
- **Anneau de Longchamp.** The Paris cyclists' ring around the racecourse, 3.5 km a lap, built from the real
  place: the IGN aerial photo paints the ground as a Lego mosaic and finds every tree (6,000 of them, autumn
  colors included), OpenStreetMap places the benches, lamps, white rails, hedges and the Moulin de Longchamp,
  IGN the grandstands. Lap it in Free with everyone else.
- **Île des Briques.** A Lego island drawn by hand: a volcano you climb in a spiral to its lava lake, a
  pirate port with ships in the bay, a desert with a pyramid and an oasis, a jungle temple and waterfall, farms
  with a windmill, a castle on its hill, thousands of Lego pieces. Pick a start place on its mosaic map, then
  your way at each junction.
- **Group rides and races.** Schedule an event on any course or loop (organizers, or a club's founder for a club
  ride), riders sign up and wait in the start pen with a countdown, then everyone goes at once. A group ride
  follows a yellow pace leader at a set W/kg, and anyone up to 200 m behind rides on the bunch's wheel. A race
  ranks riders live with their category (A to D by W/kg), counts the laps and records the finish, overall and
  per category.
- **Ride together.** Everyone on the same server and the same road sees each other in 3D, with name, flag and
  bike. Self-host it and invite your club on your LAN (`serve --lan`).
- **A living world.** The sky follows your clock: golden dawn, day, sunset, then night with stars, lit windows
  and a headlamp (or pick one). The weather turns every 20 minutes, the same for everyone on a road: sun,
  clouds, rain, fog, and snow on the high passes.
- **Road bonuses.** An arch every kilometre hands out a bonus to use when you want (key B): Feather (8 kg
  lighter for 15 s), Aero (a quarter less drag) or Draft (50 % more on a wheel for 30 s).
- **Clubs.** Found a club or join one with its code: your tag rides before your name, and the club ranks its
  members by kilometres this week.
- **Ride On!** Send a thumbs-up to the rider next to you, or a quick emote (keys 1 to 5): it pops up over your
  rider, and pace partners wave back.
- **Ride in a bunch.** Sit on a wheel and drafting saves up to 30 % of the air drag. Pace partners (2, 3 and 4
  W/kg) ride every course and world around the clock, so there is always a wheel to follow.
- **Segments.** KOM climbs and sprints with arches on the road, a live banner, the time and rank at the line,
  daily and all-time leaderboards with the leader in the polka-dot or green jersey.
- **You feel the road.** Speed comes from your watts, weight, the grade, rolling resistance and thinner air up
  high. A smart trainer gets the slope under your wheels, scaled by a difficulty setting.
- **Your screen, your way.** Drag the ride panels where you want, pick which figures show, resize, fade, or press
  H to hide it all. Presets (Cockpit, Sides, Minimal) and your layout saved per profile.
- **No trainer? Demo mode.** Ride with a power slider, jump anywhere on the profile to explore a climb.

## Train

- **Structured sessions** calibrated on your FTP: recovery, endurance, sweet spot, threshold, VO2max.
- **Training plans.** Back on the bike (4 weeks), FTP +5 % (6 weeks), sportive prep (8 weeks): a calendar from the
  day you start, today's session one click away, sessions ticked off as you ride them.
- **Rider card.** Ratings like Pro Cycling Manager's (climbing, time trial, punch, sprint, endurance, recovery)
  computed from your own power records, longest ride and form, with your rider type and an overall score.
- **Energy reserve.** A gauge of what is left above your FTP (W' balance, sized from your 1 and 5 min records):
  it drains in an attack and refills as you ease off, so you know how many more you have in you.
- **Form and power records.** Training stress (TSS) and normalized power for each ride, fitness, fatigue and
  freshness over 90 days, best 5 s, 1, 5 and 20 min efforts, and an FTP estimate you can apply in one click.
- **Bring your own**: import Zwift workout files (.zwo), thousands are free online; they stay in your profile.
- **FTP test**: 20 minutes all out after a warm-up, and your FTP updates itself (95 % of the 20-minute average).
- **ERG mode**: the trainer holds each target power for you. In demo mode, follow the target or turn on autopilot.
- **Interval chart** colored by training zone, with time left in the interval and in the session.
- **Zombie Halloween**: a horde of voxel zombies chases you on a night-time loop, faster each minute. Win mini
  sprints (scaled on your FTP) and grab pumpkins, turbo and shield on the road to keep them behind; your
  survival time, distance and sprints are saved like any ride.

## Your rider

- **Profiles** without an account: name, weight, FTP, optional nationality. Each profile has a unique id; the
  name is just a label.
- **Garage**, bike and rider apart: the bike (road, aero, gravel or vintage frame, colors, rims, handlebars) on
  its trainer, and your Lego minifig standing on its plate (jersey and its pattern, shorts, shoes, helmet,
  glasses, face, hair, skin). The same rider pedals in every ride. Feeling lucky? "Surprise me".
- **Progression**: XP for every km, workout minute and segment, levels, 27 badges (Everest, Gran fondo, all the
  climbs, the volcano, a perfect week…). Levels unlock garage pieces: aero, gravel and vintage frames, deep rims,
  TT bars, glasses, jersey patterns up to the world champion's rainbow, gold paint; the polka-dot jersey is won as
  King of the Mountains.
- **Ride history**: a page of its own with totals, weekly bricks, personal records and each ride's power and
  elevation curves. Export any ride as a **.fit** file for Strava, Garmin Connect or intervals.icu.

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
for older KICKRs, plus Bluetooth heart rate straps. Each rider connects their own from the browser (Chrome, Edge).

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
- [Accounts and roles](docs/rbac.md): sign in with Strava, roles and permissions.
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
