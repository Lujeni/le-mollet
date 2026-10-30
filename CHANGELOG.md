## v0.4.1 (2026-10-30)

### Fix

- **ride**: watts and clock precision, a HUD that patches instead of rebuilding

## v0.4.0 (2026-10-30)

### Feat

- **hud**: W/kg and calories in the cockpit, a ride stats panel, achievements unlocked mid-ride

### Fix

- **web**: serve three's RoomEnvironment, which the studio lighting imports

## v0.3.1 (2026-10-30)

## v0.3.0 (2026-10-30)

### BREAKING CHANGE

- no longer MIT OR Apache-2.0 from this commit on.

### Feat

- **hardware**: the Zwift Click's buttons, and a guided test that reports every Bluetooth frame
- **feedback**: a side tab for a bug, an idea or an opinion, kept with what the page saw
- **mail**: a Reply-To, so a sending-only address can point answers to the contact box
- **friends**: friendships belong to the account, not to a profile
- **mail**: the sign-in mail in text and HTML, written for spam filters
- **audit**: one logfmt line per account event, for journald
- **auth**: the door is two bricks, an e-mail each, the name out of the e-mail
- **auth**: no passwords on a public server, and the logo on the sign-in screen
- **web**: three.js and the fonts ship in the binary, no CDN
- **web**: the page in English too
- **auth**: an e-mail per account in cloud mode, and sign-in links by mail
- **web**: an account brick in the header, with its menu
- **auth**: sign-up quotas and a proof of work against bots
- **site**: open beta, hardware feedback, Bluetooth check
- **account**: settings, a new password, sign out everywhere, delete
- **friends**: handles, requests, and a friend's profile and rides
- **free**: loop cards drawn from above, Paris no longer shipped
- **web**: device reconnect, real gear estimate, country search
- **license**: PolyForm Noncommercial, commercial use on request
- **site**: a landing page
- **courses**: six more cols and two loops
- **course**: an Overpass mirror through LE_MOLLET_OVERPASS
- **web**: smoother rides and a cinema mode
- **events**: a tactical peloton in every race
- **web**: ride HUD laid out like a game's

### Fix

- **auth**: a sign-up with a known address tells nothing, and sends nothing
- **web**: the door's inputs take clicks, and keep their own look
- **web**: the account form's blocks back where they belong
- **web**: a public server lists only the signed-in account's profiles
- **web**: sign out from the header, on every screen
- **web**: load the courses after a cloud sign-in
- **web**: harden the server and lighten the frames

### Refactor

- **auth**: accounts, sessions and magic links on torii

### Perf

- **terrain**: heights sent as row differences, gzip packs them twice as small

## v0.2.0 (2026-10-28)

### Feat

- **events**: group rides on the quarter hours
- **web**: gzip and cacheable track files
- hardware probe, clearer equipment screen, ride cockpit
- **web**: Zwift Ride controller and virtual gears
- **auth**: Le Mollet accounts instead of Strava sign-in
- **metrics**: Prometheus metrics on usage and performance
- **db**: keep server data in one SQLite file
- **security**: fair play between the page and the server
- **events**: races by rank every half hour, waiting room
- **courses**: mythic climbs of Spain and Italy, country flags
- **web**: rider card, energy reserve and record duels
- **courses**: Col de la Madeleine from La Chambre
- group rides and races, new profile screens
- Anneau de Longchamp, built from the real place
- **web**: form, power records and training plans
- living world (time of day, weather, road bonuses) and clubs
- local, LAN and cloud server modes
- sign in with Strava (OAuth 2) and role-based access control
- **web**: progression, XP, levels, badges and garage unlocks
- .zwo workouts, FTP test, Ride On and emotes
- **web**: equipment step between the profile and the home screen
- browser trainers, drafting, pace partners, segments, FIT export
- **web**: ride history page
- **web**: round Lego mosaic minimap for open worlds
- **open-world**: pick a start place on the island and in Paris
- **web**: modern Lego menu with destination thumbnails
- **island**: hand-made Lego island open world
- **web**: race your ghost on the climbs
- **web**: customizable ride HUD
- **web**: Lego rider, split garage and textured asphalt
- **web**: Lego look and living worlds
- **client**: add experimental native 3D client (Bevy)
- **web**: real buildings and realistic rendering for Paris and climbs
- **courses**: add Paris open city
- **city**: ride every street of Paris with a turn at each junction
- **web**: add low-poly bike and rider style
- **garage**: avatar
- **web**: add night ride, menu scenery and rider avatar
- **web**: add light/dark theme toggle and favicon
- **web**: drop random loop generator from the Free screen
- **web**: add Zombie Halloween mode
- **courses**: add Galibier, Tourmalet, Izoard and Planche des Belles Filles
- **web**: extend web server and UI
- **core**: add workouts, procedural world and course tooling
- lets go baby
- initial commit

### Fix

- harden the server and lighten the page
- **server**: send each rider the 50 nearest riders

### Refactor

- **web**: use the low-poly bike and rider everywhere
