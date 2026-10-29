# Fair play and security between the page and the server

Watts come from a trainer the browser reads over Bluetooth. Nothing signs them, so nothing proves them: a
rider can always send numbers of their own, as in any game where the client measures. The design makes cheating
visible and pointless rather than impossible:

1. the server is the authority on everything that is not a measurement;
2. it checks every second of the measurements for what no rider or trainer does;
3. only rides it believes reach leaderboards and race results.

At home (`--mode local`) the PC's owner is admin and may cheat against themselves; the rules matter for LAN
guests and in cloud mode.

## 1. The server is the authority

- **It simulates every ride** (`src/ride.rs`): speed, position, time, segments, race order and finish. The page
  only sends inputs: power, cadence, heart rate (`/power`), a turn, an emote, pause, a road bonus.
- **A ride belongs to whoever opened it.** The first call with a ride's `session` claims it for that browser
  (its address and, signed in, its account); any other browser gets `403 not your ride` on `/power`,
  `/pause`, `/turn`, `/seek`, `/powerup`, `/emote` and `/events`.
- **Weight and rank are the server's** (`src/rider.rs`, kept in the database). The page registers the profile
  with `/rider?pid=&weight=&ftp=`; rides then use the server's weight, never the query's `mass`, and races
  the server's rank, never a `wkg` from the page.
  - The weight moves at most **2 kg a week** (free on the profile's first day), so nobody turns 40 kg for a race.
  - The rank goes by the best of the declared FTP and the **FTP seen in rides over 60 days** (95 % of a 20 min
    best). Declaring a low FTP does not hold a strong rider in a weak rank.
  - A race finish that shows a rider too strong for the race's ranks (95 % of the race's best effort) is kept
    **out of the results** ("above rank"), and moves the rider up.
- **Profile ids stay secret.** At home a profile id is the only key to a profile, so the server never sends one
  to anybody: events and clubs mark the caller's own entries with `me` instead, and leaderboards keep a hash of
  the id, never the id. In cloud mode a profile belongs
  to the account that registered it, and only it may use its id.

## 2. The checks (`rider::Check`)

Each second of power is checked; a flag never stops the ride, it keeps it out of the rankings and lists it for
the moderators (`/admin/flags`, permission `moderate`).

| Flag | When |
|---|---|
| `flat` | the same power to the hundredth of a watt for 2 min (over 50 W): a typed number, not a trainer |
| `spike` | more than 1000 W of change from one second to the next |
| `no cadence` | over 250 W with a cadence of 0 for 30 s, once the trainer has sent a cadence |
| `beyond W′` | the W′ balance (Skiba, FTP of the profile, W′ 25 kJ) below −50 %: not human |
| `20 min over 6.5 W/kg`, `5 min over 8 W/kg`, `1 s over 25 W/kg` | past the best riders in the world |

The thresholds are generous on purpose: a flag should mean "look at this", not "an honest rider on a good day".

## 3. Trust

Every ride carries a trust level, sent to the page in each frame (`fair`) and shown as a chip when it is not
`device`:

| Trust | When | Where it counts |
|---|---|---|
| `demo` | the slider (no trainer), or a profile the server does not know | free riding only |
| `flagged` | a check above fired | free riding only, until a moderator looks |
| `device` | a trainer (the server's, or the browser's own) and nothing odd | segment leaderboards, race results, FTP seen |

The `src=bt` the page sends with a trainer's power is its word: a cheater can send it too. That is what the
checks are for.

## 4. Transport

- **CSRF**: every call but the page itself and the audio is refused when the browser says
  it comes from another site (`Sec-Fetch-Site: cross-site|same-site`, or an `Origin` that is not this host).
  A page elsewhere cannot make a visitor's browser cancel an event or leave a club. Whatever changes something
  (events, clubs, friends, a profile, a world, signing out, a reset) is POST only: a link, a prefetch or a browser
  without `Sec-Fetch-Site` cannot do it by accident.
- **Passwords**: Argon2id hashes; five wrong passwords from one client lock that name for that client for 15
  minutes (another client cannot lock you out), and sign-ins are limited to 10 an hour per client.
- **Sign-ups, against bots**: 3 an hour and 5 a day per client, 100 an hour for the whole server; and each one
  carries a proof of work (`/auth/challenge`, `account::pow_check`): the browser spends about a second of
  SHA-256 on a signed, dated challenge, used once. A human never sees it; a farm of accounts costs a farm of CPU.
  No captcha service, nothing loaded from a third party.
- **Names**: in cloud mode the name shown to others is the account's (with the club tag the server knows), never
  the page's word; at home it is free text, as there is no account.
- **Budgets**, per client address (behind the reverse proxy of cloud mode, the last address of `X-Forwarded-For`,
  the one the proxy appended: a client cannot name another to dodge them): a world
  generation 3 per 10 min, an event or a club 10 an hour, a profile update 30 an hour; 20 profiles per account,
  500 entrants per event, 20 sessions per account, 12 open worlds in all (a new one waits until one is empty).
- **Connections**: a request's head must arrive within 10 s, a ride frame must be taken within 5 s (else the
  rider leaves the road), 64 connections per address and 4096 in all.
- **Host**: a request for a name that is not ours is refused (421), so a page elsewhere cannot point its own
  domain at a home server (DNS rebinding) and act as its admin. Ours: localhost, IP addresses, `.local` names,
  the host of `LE_MOLLET_PUBLIC_URL`, and `LE_MOLLET_HOSTS` (comma-separated).
- **Headers** on every response: `X-Frame-Options: DENY` and `frame-ancestors 'none'` (no clickjacking),
  `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`.
- **Cookies**: the sign-in cookie is `HttpOnly; SameSite=Lax`, and `Secure` when `LE_MOLLET_PUBLIC_URL` is https.
- **Cloud mode** sits behind an https reverse proxy: every request then comes from loopback, so a ride is bound
  to the signed-in account, not to an address.

## Next

- A second power source (pedals or a crank meter next to the trainer), compared second by second, for a
  `verified` level that records and the Légende rank would ask for.
- A moderation screen over `/admin/flags`, and a way to clear a flagged ride.
