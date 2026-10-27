# Accounts and roles

## Modes

| | `serve` (local, default) | `serve --lan` | `serve --mode cloud` |
|---|---|---|---|
| Listens on | this PC only | the whole network | `--addr` |
| Account | none | none (Strava optional) | **required** |
| Admin | the browser on this PC | the browser on this PC | athletes given `admin` in `roles.json` |
| Others | | LAN guests ride as Riders | signed-in athletes ride as Riders |

In local mode, profiles, history and progression stay in the browser; the server keeps leaderboards and roles
next to it. "This PC" means requests from loopback: cloud mode never trusts that, since behind a reverse proxy
every request comes from loopback.

Cloud mode refuses to start without a sign-in provider, and answers `401` to everything but the page,
`/whoami` and `/auth/*` until the rider signs in; the page then shows only the sign-in. Strava is the only
provider for now.

## Roles and permissions

The whole policy is `Role::can` in `src/auth.rs`. Endpoints check a permission (`Perm`), never a role.

| Permission | Rider | Organizer | Admin | Used by |
|---|---|---|---|---|
| `ride` | ✅ | ✅ | ✅ | riding, Ride Ons, one's own power |
| `create_world` | ✅ | ✅ | ✅ | `/worlds/new` |
| `manage_events` | | ✅ | ✅ | events and group rides (to come) |
| `moderate` | | | ✅ | `/admin/*`: reset a leaderboard (button in the pause menu) |
| `manage_roles` | | | ✅ | (to come: roles from the page) |

To protect a new endpoint: add a `Perm` variant if none fits, a line in `Role::can`, and the path in the gate at
the top of `handle` in `src/web.rs`. The page asks `/whoami` for the caller's permissions (`can('moderate')`)
to show or hide controls; the server checks again on every request.

## Sign in with Strava

Sign-in uses Strava's OAuth 2 flow, run by the [`oauth2`](https://crates.io/crates/oauth2) crate: the server sends
the browser to Strava, checks the `state` it gets back (CSRF), trades the code for the athlete and their tokens,
and sets an `lm_session` cookie (HttpOnly, SameSite=Lax, 30 days). Sessions live in memory: a restart signs
everyone out.

1. Create an app on [strava.com/settings/api](https://www.strava.com/settings/api). **Authorization Callback
   Domain**: the host riders use (`localhost` works for testing).
2. Start the server with its credentials (never commit them):

   ```sh
   LE_MOLLET_STRAVA_CLIENT_ID=12345 \
   LE_MOLLET_STRAVA_CLIENT_SECRET=... \
   LE_MOLLET_PUBLIC_URL=http://localhost:8080 \
   cargo run --release -- serve
   ```

   `LE_MOLLET_PUBLIC_URL` is where riders reach the server; the callback is `<it>/auth/strava/callback`. With
   `https://`, the session cookie is also `Secure`.
3. In the page: **Profile > Compte > Se connecter avec Strava**.

Scopes asked: `read,activity:write`, so a later version can upload rides to Strava. The tokens are kept in
`strava-tokens.json` next to `roles.json` (readable by the server's user only, git-ignored).

Strava limits a new app to its creator ("single player mode"); raise it to 10 athletes in the app settings, and
ask Strava for a review beyond that. Fine for a club, a bottleneck for a public server.

## Giving roles

Roles are given by Strava athlete id (shown in Profile > Compte once signed in), in `roles.json`, at once:

```sh
cargo run -- role set 4242 admin --note Julien    # or organizer, rider
cargo run -- role list
cargo run -- role remove 4242
```

`serve --roles <file>` picks another file. A missing file means everyone rides and nobody administers.
