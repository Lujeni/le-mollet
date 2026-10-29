# Accounts and roles

## Modes

| | `serve` (local, default) | `serve --lan` | `serve --mode cloud` |
|---|---|---|---|
| Listens on | this PC only | the whole network | `--addr` |
| Account | none | none (optional) | **required** |
| Admin | the browser on this PC | the browser on this PC | accounts given `admin` (`le-mollet role`) |
| Others | | LAN guests ride as Riders | signed-in accounts ride as Riders |

In local mode, profiles, history and progression stay in the browser; the server keeps leaderboards and roles
next to it. "This PC" means requests from loopback: cloud mode never trusts that, since behind a reverse proxy
every request comes from loopback.

Cloud mode answers `401` to everything but the page, `/whoami` and `/auth/*` until the rider signs in; the
page then shows only the sign-in.

## Roles and permissions

The whole policy is `Role::can` in `src/auth.rs`. Endpoints check a permission (`Perm`), never a role.

| Permission | Rider | Organizer | Admin | Used by |
|---|---|---|---|---|
| `ride` | ✅ | ✅ | ✅ | riding, Ride Ons, one's own power |
| `create_world` | ✅ | ✅ | ✅ | `/worlds/new` |
| `manage_events` | | ✅ | ✅ | `/event/create`: schedule group rides and races (a club's founder may schedule its club rides) |
| `moderate` | | | ✅ | `/admin/*`: reset a leaderboard (button in the pause menu) |
| `manage_roles` | | | ✅ | (to come: roles from the page) |

To protect a new endpoint: add a `Perm` variant if none fits, a line in `Role::can`, and the path in the gate at
the top of `handle` in `src/web.rs`. The page asks `/whoami` for the caller's permissions (`can('moderate')`)
to show or hide controls; the server checks again on every request.

## Accounts

A Le Mollet account (`src/account.rs`) is, at home, a name and a password (**Profile > Compte**: *Se connecter* or
*Créer un compte*); on a public server, a name and an e-mail and no password at all (**the sign-in screen**): every
sign-in is a link sent to the mailbox (15 minutes, one use; docs/security.md), and the first one proves the address.
The e-mail is changed from the account settings; at home, password in hand.

- **Name**: 3 to 24 letters, digits, `-` or `_`; "Julien" and "julien" are one account.
- **Password**: 8 to 128 characters, stored as an Argon2id hash (its own salt, PHC string), never as such.
- `POST /auth/signup` or `/auth/login` with `name` and `password` (form-encoded) set an `lm_session` cookie:
  HttpOnly, SameSite=Lax, 30 days, and `Secure` when `LE_MOLLET_PUBLIC_URL` starts with `https://`. Sessions are
  kept in the database: a restart signs nobody out. `/auth/logout` ends one, `/auth/logout?all=1` every one of the account.
- **Settings** (Profile > Compte, signed in): `POST /auth/password` (`password`, `new`) changes the password and signs
  the other browsers out; `POST /auth/delete` (`password`) removes the account, its sessions and the profiles it
  owns (their club seats and friendships with them). `/whoami` tells how many browsers are signed in (`devices`).
- **Password guessing**: five wrong passwords for a name lock that name for 15 minutes. A wrong name and a wrong
  password get the same answer, in the same time (an unknown name is checked against a dummy hash).
- Requests from another site's page are refused, sign-in included (docs/security.md).

## Giving roles

Roles are given by account name, in the server's database; the server rereads them every 5 s, even while it runs:

```sh
cargo run -- role set julien admin --note "le patron"    # or organizer, rider
cargo run -- role list
cargo run -- role remove julien
```

`serve --db <file>` and `role --db <file>` pick another database. No roles means everyone rides and nobody
administers. A `roles.json` from an older version is imported on the first start (kept as `roles.json.bak`).
