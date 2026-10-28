//! Tiny HTTP server: embedded web UI + ride sessions.
//!   GET /                                      page
//!   GET /courses                               {trainer, courses: [summary], cities: [summary]}  (real climbs, open cities)
//!   GET /worlds                                [summary + relief + riders]   (open worlds)
//!   GET /worlds/new?length=<5|10|20>&relief=<flat|hills|mountains>   generate an open world, {id}
//!   GET /course?id=<id>                        full course JSON (climb or world)
//!   GET /terrain?id=<id>                       binary voxel terrain (see le_mollet::terrain)
//!   GET /buildings?id=<id>                     open city: binary real buildings (see le_mollet::buildings)
//!   GET /district?id=<id>&f=<file>             open city: realistic district files (see le_mollet::district)
//!   GET /ortho?id=<id>&f=<file>                aerial photo tiles for the realistic render (see le_mollet::ortho)
//!   GET /workouts                              built-in training sessions
//!   GET /turn?session=<n>&dir=<left|straight|right>   open city: turn to take at the next junction
//!   GET /events?course=<id>&mass=<kg>&session=<n>[&name=&color=&frame=&country=]
//!                                              Server-Sent Events, one ride per connection; riders on the
//!                                              same course see each other ("others" in each frame)
//!   GET /events?workout=<id>&ftp=<W>&mass=<kg>&session=<n>   same, workout on a flat road
//!   GET /events?course=<id>&mode=zombie&ftp=<W>&...           same, chased by a zombie horde
//!   GET /power?session=<n>&w=<W>[&cad=&hr=&src=bt]  the rider's power: demo slider, or the browser's own trainer
//!   GET /segments?id=<id>                      KOM and sprint leaderboards of a course or open world
//!   GET /whoami                                the caller's identity, role and permissions (docs/rbac.md)
//!   GET /auth/strava, /auth/strava/callback    sign in with Strava (OAuth 2); /auth/logout signs out
//!   GET /admin/segments/reset?id=<id>&name=<segment>  wipe a leaderboard (moderate)
//!
//!   GET /emote?session=<n>&course=<id>&e=<rideon|wave|fire|laugh|party>[&to=<rider id>]  a bubble over my rider; a Ride On to someone
//!   GET /seek?session=<n>&d=<m>                demo mode: teleport the rider along the course
//!   GET /pause?session=<n>&on=<0|1>            freeze or resume a ride (menu open)
//!   GET /powerup?session=<n>                   use the road bonus the rider holds
//!   GET /event/list                            scheduled group rides and races, {now, events}
//!   GET /event/create?pid=&me=&kind=<group|race>&name=&track=&start=<unix>&laps=&pace=<W/kg>[&cats=PO][&club=1] (ranks, see event::TIERS)
//!                                              schedule one (manage_events, or the founder of one's club for a club ride)
//!   GET /event/join?id=&pid=&me=&wkg=;  /event/leave?id=&pid=;  /event/delete?id=&pid=
//!   GET /events?course=<id>&event=<id>&...    ride an event: held in the pen until the start, then a pace
//!                                              leader (group) or a live ranking and a finish line (race)
//!   GET /club?pid=<profile>                    the profile's club, members ranked by km this week (or null)
//!   GET /club/create?pid=&me=&name=&tag=&color=   found a club;  /club/join?pid=&me=&code=;  /club/leave?pid=
//!   GET /audio/<name>.mp3                      nature ambience loops (web/audio/CREDITS.md)
//!   GET /rider?pid=[&weight=<kg>&ftp=<W>]      the profile as the server knows it (weight, FTP seen, rank); with
//!                                              weight and ftp, register or update it (docs/security.md)
//!   GET /admin/flags                           rides the fair-play checks flagged (moderate)
//!
//! Access (docs/rbac.md): in `Local` mode, whoever sits at the server's PC is admin without an account and LAN
//! guests ride as Riders; in `Cloud` mode, everything but the page and the sign-in needs a Strava sign-in.
//! Signing in sets an `lm_session` cookie; the athlete's role comes from roles.json. Endpoints check `Perm`s.
//!
//! Power, per ride: the browser's own trainer (Web Bluetooth, `src=bt`), else the server's trainer (whose grade the
//! latest ride drives), else the demo slider. Pace partners ride every course and open world around the clock.
//!
//! Fair play (docs/security.md): the server simulates every ride and only takes power, cadence and heart rate
//! from the page. A ride belongs to the browser that opened it; weight and rank come from the server's own
//! record of the profile; each second of power is checked, and only believed rides reach leaderboards and race
//! results. Requests from other sites are refused (CSRF), and profile ids are never sent to other riders.

use le_mollet::Trainer;
use le_mollet::auth::{Identity, Perm, Role, Roles};
use le_mollet::club::{self, Clubs};
use le_mollet::event::{Event, Events, Kind as EventKind, Phase, TIERS};
use le_mollet::strava::{self, Strava};
use le_mollet::city::{City, Exit, Route, Turn};
use le_mollet::course::{Course, Point};
use le_mollet::physics;
use le_mollet::ride::Ride;
use le_mollet::rider::{self, Check, Flagged, Riders, Trust};
use le_mollet::segment::{self, Boards, Entry, Kind, Rank, Window};
use le_mollet::workout::{self, Progress, Workout};
use le_mollet::world::{self, Relief};
use le_mollet::zombie::Game;
use serde::Serialize;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const PAGE: &str = include_str!("../web/index.html");
const AUDIO: &[(&str, &[u8])] = &[
    ("/audio/forest.mp3", include_bytes!("../web/audio/forest.mp3")),
    ("/audio/meadow.mp3", include_bytes!("../web/audio/meadow.mp3")),
    ("/audio/stream.mp3", include_bytes!("../web/audio/stream.mp3")),
    ("/audio/cowbells.mp3", include_bytes!("../web/audio/cowbells.mp3")),
    ("/audio/moo.mp3", include_bytes!("../web/audio/moo.mp3")),
    ("/audio/marmot.mp3", include_bytes!("../web/audio/marmot.mp3")),
    ("/audio/woodpecker.mp3", include_bytes!("../web/audio/woodpecker.mp3")),
    ("/audio/buzzard.mp3", include_bytes!("../web/audio/buzzard.mp3")),
    ("/audio/deer.mp3", include_bytes!("../web/audio/deer.mp3")),
];
const DT: f32 = 0.1;
const DISTRICT_FILES: [&str; 7] = ["meta.json", "dem.bin", "ortho.jpg", "trees.json", "buildings.bin", "osm.json", "surface.bin"];
// ponytail: worlds live in memory, oldest empty ones dropped past this; persist them if people want to keep maps
const MAX_WORLDS: usize = 12;

/// A rider as other riders on the same course see them.
#[derive(Debug, Clone, Serialize)]
struct Presence {
    /// Public id: what the others see. Never the session, which drives the ride (power, pause, turns).
    id: u64,
    #[serde(skip)]
    sid: u64,
    /// Emote bubble over the rider, for a few seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    emote: Option<&'static str>,
    #[serde(skip)]
    emote_at: Option<std::time::Instant>,
    name: String,
    color: String,
    frame: &'static str,
    country: Option<String>,
    distance: f32,
    speed_kmh: f32,
    power: f32,
    /// `[lat, lon]` in an open city, where `distance` along each rider's own route means nothing to others.
    #[serde(skip_serializing_if = "Option::is_none")]
    pos: Option<[f64; 2]>,
    /// A pace partner, not a person.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    bot: bool,
    /// Profile id, for the leaderboards; never sent to the others.
    #[serde(skip)]
    pid: String,
    /// Distance ridden since the start, for an event's ranking and its leader's gap.
    #[serde(skip)]
    ridden: f32,
    /// Event this rider rides in.
    #[serde(skip)]
    event: Option<String>,
}

/// A rideable road: a real climb loaded from disk or a generated open world.
struct Track {
    id: String,
    course: Course,
    json: String,
    terrain: Option<Vec<u8>>,
    relief: Option<Relief>, // Some = open world
    city: Option<City>,
    buildings: Option<Vec<u8>>,
    /// Realistic district files by name (meta.json, dem.bin, ortho.jpg, trees.json, buildings.bin).
    district: HashMap<&'static str, Vec<u8>>,
    /// Aerial photo files by name (meta.json, overview.jpg, <i>_<j>.jpg).
    ortho: HashMap<String, Vec<u8>>,
    riders: Mutex<HashMap<u64, Presence>>,
}

impl Track {
    fn new(id: String, course: Course, terrain: Option<Vec<u8>>, relief: Option<Relief>) -> anyhow::Result<Self> {
        let json = serde_json::to_string(&course)?;
        Ok(Self { id, course, json, terrain, relief, city: None, buildings: None, district: HashMap::new(), ortho: HashMap::new(), riders: Mutex::new(HashMap::new()) })
    }

    /// An open city: its course is only the first street, each rider grows their own route.
    fn city(id: String, mut city: City, terrain: Option<Vec<u8>>, buildings: Option<Vec<u8>>) -> anyhow::Result<Self> {
        // A real town has no hand-made thumbnail: make one from its terrain (streets, water, parks, buildings).
        if let Some(t) = terrain.as_ref().filter(|t| city.preview.is_empty() && t.len() >= 8) {
            let (w, h) = (u32::from_le_bytes(t[0..4].try_into()?) as usize, u32::from_le_bytes(t[4..8].try_into()?) as usize);
            if let Some(cover) = t.get(40 + w * h * 2..40 + w * h * 3) {
                (city.preview, city.preview_w) = le_mollet::island::preview(cover, w, h, (w / 110).max(1));
            }
        }
        let mut t = Self::new(id, Route::start(&city).course, terrain, None)?;
        (t.city, t.buildings) = (Some(city), buildings);
        Ok(t)
    }

    /// Aerial photo tiles and real buildings next to a course or city file, when they were fetched.
    fn with_extras(mut self, path: &str) -> Self {
        for e in std::fs::read_dir(crate::ortho_dir(path)).into_iter().flatten().flatten() {
            if let (Some(name), Ok(b)) = (e.file_name().to_str().map(str::to_owned), std::fs::read(e.path())) {
                self.ortho.insert(name, b);
            }
        }
        if !self.ortho.contains_key("meta.json") { self.ortho.clear() }
        if self.buildings.is_none() { self.buildings = std::fs::read(crate::buildings_path(path)).ok() }
        self
    }

    /// Realistic district files from `dir`, all or nothing.
    fn with_district(mut self, dir: &str) -> Self {
        for f in DISTRICT_FILES {
            if let Ok(b) = std::fs::read(format!("{dir}/{f}")) {
                self.district.insert(f, b);
            }
        }
        if self.district.len() < DISTRICT_FILES.len() { self.district.clear() }
        self
    }

    /// `[west, south, east, north]` of the terrain, from its file header.
    fn bbox(&self) -> Option<[f64; 4]> {
        let t = self.terrain.as_ref().filter(|t| t.len() >= 40)?;
        let f = |o: usize| f64::from_le_bytes(t[o..o + 8].try_into().unwrap());
        Some([f(8), f(16), f(24), f(32)])
    }

    fn summary(&self) -> serde_json::Value {
        let c = &self.course;
        // 60-sample elevation profile for the picker thumbnails.
        let profile: Vec<f32> = (0..60).map(|i| c.ele_at(c.length() * i as f32 / 59.0).round()).collect();
        json!({
            "id": self.id, "name": c.name, "country": c.country, "length": c.length(), "gain": c.elevation_gain().round(), "profile": profile,
            "terrain": self.terrain.is_some(), "looped": c.looped,
            "relief": self.relief.map(Relief::label), "riders": self.riders.lock().unwrap().values().filter(|p| !p.bot).count(),
            "city": self.city.as_ref().map(|c| c.streets.len()), "buildings": self.buildings.is_some(), "district": !self.district.is_empty(), "ortho": !self.ortho.is_empty(),
            "island": self.city.as_ref().is_some_and(|c| c.island), "props": self.city.as_ref().map(|c| &c.props), "starts": self.city.as_ref().map(|c| &c.starts), "bbox": self.bbox(),
            "preview": self.city.as_ref().map(|c| &c.preview), "preview_w": self.city.as_ref().map(|c| c.preview_w),
        })
    }
}

fn world_track(seed: u64, length_km: u32, relief: Relief, name: String) -> anyhow::Result<Track> {
    let w = world::generate(seed, length_km as f64 * 1000.0, relief, &name);
    Track::new(format!("world-{seed:x}"), w.course, Some(w.terrain.to_bytes()), Some(relief))
}

/// What one ride reports to the server.
#[derive(Debug, Default, Clone, Copy)]
struct Sensors {
    w: f32,
    cadence: Option<f32>,
    hr: Option<f32>,
    /// From the browser's own trainer: wins over the server's.
    live: bool,
}

pub struct App {
    tracks: RwLock<Vec<Arc<Track>>>,
    list_json: String,
    workouts: Vec<Workout>,
    workouts_json: String,
    flat: Course,
    pub trainer: Option<Trainer>,
    /// Latest (power, cadence) from the trainer.
    pub trainer_sample: Mutex<(f32, Option<f32>)>,
    difficulty: f32,
    /// What each ride reports: demo slider watts, or its own trainer and heart rate strap.
    sensors: Mutex<HashMap<u64, Sensors>>,
    boards: Mutex<Boards>,
    boards_path: String,
    clubs: Mutex<Clubs>,
    clubs_path: String,
    events: Mutex<Events>,
    events_path: String,
    /// Group rides whose pace leader is out on the road.
    beacons: Mutex<HashSet<String>>,
    roles_path: String,
    mode: Mode,
    /// The server's Strava app, when configured (LE_MOLLET_STRAVA_CLIENT_ID / _SECRET).
    strava: Option<Strava>,
    /// Signed-in browsers: session cookie -> who.
    sessions: Mutex<HashMap<String, Session>>,
    /// OAuth states handed out and not used yet, with when: a callback must bring one back (CSRF).
    oauth_states: Mutex<HashMap<String, std::time::Instant>>,
    /// Strava tokens by athlete, to upload rides on their behalf later.
    strava_tokens_path: String,
    demo_seek: Mutex<HashMap<u64, f32>>,
    paused: Mutex<HashSet<u64>>,
    /// Open city: turn each session asked for at its next junction.
    turns: Mutex<HashMap<u64, Turn>>,
    /// Sessions that asked to use their road bonus, applied at their next tick.
    powerup: Mutex<HashSet<u64>>,
    /// Ride Ons waiting for their recipient's next frame, by session.
    inbox: Mutex<HashMap<u64, Vec<Notice>>>,
    /// Session allowed to drive the trainer grade: the most recent one.
    driver: AtomicU64,
    riders: Mutex<Riders>,
    riders_path: String,
    /// Who opened each ride: only they may steer it (power, pause, turns...).
    owners: Mutex<HashMap<u64, Owner>>,
}

/// A ride's owner: the browser's address and, signed in, the athlete.
type Owner = (Option<std::net::IpAddr>, Option<u64>);

/// Where the server runs, which sets who must sign in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// One person's PC (maybe open to the LAN): no account, the PC's own browser is admin.
    Local,
    /// A public server: Strava sign-in required to ride.
    Cloud,
}

impl App {
    /// Load every `*.json` course in `dir` (the id is the file stem) and open today's world.
    pub fn new(dir: &str, trainer: Option<Trainer>, difficulty: f32, roles: &str, mode: Mode) -> anyhow::Result<Self> {
        Roles::load(roles)?; // a broken roles file stops the server now, not at the first admin call
        let strava = Strava::from_env()?;
        anyhow::ensure!(mode == Mode::Local || strava.is_some(), "cloud mode needs a Strava app: set LE_MOLLET_STRAVA_CLIENT_ID and _SECRET (docs/rbac.md)");
        let mut paths: Vec<_> = std::fs::read_dir(dir)
            .map(|rd| rd.filter_map(|e| Some(e.ok()?.path())).filter(|p| p.extension().is_some_and(|x| x == "json") && !p.to_string_lossy().ends_with(".city.json") && !p.to_string_lossy().ends_with(".map.json") && !p.ends_with("leaderboards.json") && !p.ends_with("clubs.json") && !p.ends_with("events.json") && !p.ends_with("riders.json")).collect())
            .unwrap_or_default();
        paths.sort();
        let mut tracks = vec![];
        for p in paths {
            let path = p.to_string_lossy();
            let id = p.file_stem().unwrap().to_string_lossy().into_owned();
            tracks.push(Arc::new(Track::new(id, Course::load(&path)?, std::fs::read(crate::terrain_path(&path)).ok(), None)?.with_extras(&path)));
        }
        // Climbs on the fridge; real loops (Longchamp) ride with the open worlds, see worlds_json.
        let summaries: Vec<_> = tracks.iter().filter(|t| !t.course.looped).map(|t| t.summary()).collect();
        // Open cities: `x.city.json` (+ `x.city.terrain.bin`), id `x`.
        let mut cities = vec![];
        for p in std::fs::read_dir(dir).map(|rd| rd.filter_map(|e| Some(e.ok()?.path())).collect::<Vec<_>>()).unwrap_or_default() {
            let path = p.to_string_lossy();
            let Some(stem) = path.strip_suffix(".city.json") else { continue };
            let id = stem.rsplit('/').next().unwrap_or(stem).to_owned();
            let t = Track::city(id, City::load(&path)?, std::fs::read(crate::terrain_path(&path)).ok(), std::fs::read(crate::buildings_path(&path)).ok())?
                .with_district(&crate::district_dir(&path))
                .with_extras(&path);
            cities.push(t.summary());
            tracks.push(Arc::new(t));
        }
        let list_json = json!({ "trainer": trainer.as_ref().map(|t| &t.name), "courses": summaries, "cities": cities }).to_string();
        // Same daily loop for everyone, so there is always a world to meet in.
        let day = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs() / 86_400;
        tracks.push(Arc::new(world_track(day, 10, Relief::Hills, "Boucle du jour".into())?));
        Ok(Self {
            tracks: RwLock::new(tracks),
            list_json,
            workouts_json: serde_json::to_string(&workout::builtin().iter().map(|w| json!({ "duration": w.duration(), "w": w })).collect::<Vec<_>>())?,
            workouts: workout::builtin(),
            flat: Course::flat(1_000_000.0),
            trainer,
            trainer_sample: Mutex::new((0.0, None)),
            difficulty,
            sensors: Mutex::new(HashMap::new()),
            boards: Mutex::new(Boards::load(&format!("{dir}/leaderboards.json"))),
            boards_path: format!("{dir}/leaderboards.json"),
            clubs: Mutex::new(Clubs::load(&format!("{dir}/clubs.json"))),
            clubs_path: format!("{dir}/clubs.json"),
            events: Mutex::new(Events::load(&format!("{dir}/events.json"))),
            events_path: format!("{dir}/events.json"),
            beacons: Mutex::new(HashSet::new()),
            roles_path: roles.to_owned(),
            mode,
            strava,
            sessions: Mutex::new(HashMap::new()),
            oauth_states: Mutex::new(HashMap::new()),
            strava_tokens_path: std::path::Path::new(roles).with_file_name("strava-tokens.json").to_string_lossy().into_owned(),
            demo_seek: Mutex::new(HashMap::new()),
            paused: Mutex::new(HashSet::new()),
            turns: Mutex::new(HashMap::new()),
            powerup: Mutex::new(HashSet::new()),
            inbox: Mutex::new(HashMap::new()),
            driver: AtomicU64::new(0),
            riders: Mutex::new(Riders::load(&format!("{dir}/riders.json"))),
            riders_path: format!("{dir}/riders.json"),
            owners: Mutex::new(HashMap::new()),
        })
    }

    pub fn course_count(&self) -> usize {
        self.tracks.read().unwrap().iter().filter(|t| t.relief.is_none()).count()
    }

    fn track(&self, id: &str) -> Option<Arc<Track>> {
        self.tracks.read().unwrap().iter().find(|t| t.id == id).cloned()
    }

    /// Who sent a request, from its session cookie and, in local mode, where from: the server's own PC is admin.
    /// Only signed-in requests read the roles file, so `le-mollet role` takes effect at once without a restart.
    /// Never trusted in cloud mode: behind a reverse proxy every request comes from loopback.
    /// ponytail: one small file read per signed-in request; cache by mtime if it ever shows up in a profile
    fn identify(&self, cookie: Option<&str>, peer: Option<std::net::IpAddr>) -> Identity {
        let owner = self.mode == Mode::Local && peer.is_some_and(|ip| ip.is_loopback());
        let mut who = match cookie.and_then(|c| self.sessions.lock().unwrap().get(c).filter(|s| s.expires > std::time::SystemTime::now()).cloned()) {
            Some(s) => Identity { role: Roles::load(&self.roles_path).map_or(Role::Rider, |r| r.role_of(s.athlete)), name: Some(s.name), athlete: Some(s.athlete) },
            None => Identity::ANONYMOUS,
        };
        if owner { who.role = Role::Admin }
        who
    }

    /// A fresh OAuth state; old unused ones are dropped after 10 minutes.
    fn new_state(&self, state: String) {
        let mut states = self.oauth_states.lock().unwrap();
        states.retain(|_, t| t.elapsed().as_secs() < 600);
        states.insert(state, std::time::Instant::now());
    }

    fn save_strava_tokens(&self, athlete: u64, t: strava::Tokens) {
        let path = &self.strava_tokens_path;
        let mut all: HashMap<u64, strava::Tokens> = std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        all.insert(athlete, t);
        let res = serde_json::to_string(&all).map_err(std::io::Error::other).and_then(|s| write_private(path, &s));
        if let Err(e) = res { eprintln!("{path}: {e}") }
    }

    /// Free rides: real loops first, then the generated worlds.
    fn worlds_json(&self) -> String {
        let tracks = self.tracks.read().unwrap();
        let real = tracks.iter().filter(|t| t.relief.is_none() && t.city.is_none() && t.course.looped);
        json!(real.chain(tracks.iter().filter(|t| t.relief.is_some())).map(|t| t.summary()).collect::<Vec<_>>()).to_string()
    }

    fn add_world(&self, track: Track) -> String {
        let id = track.id.clone();
        let mut tracks = self.tracks.write().unwrap();
        tracks.push(Arc::new(track));
        while tracks.iter().filter(|t| t.relief.is_some()).count() > MAX_WORLDS {
            // Worlds are kept in creation order after the climbs: drop the oldest empty one.
            match tracks.iter().position(|t| t.relief.is_some() && t.riders.lock().unwrap().values().all(|p| p.bot)) {
                Some(i) => drop(tracks.remove(i)),
                None => break,
            }
        }
        id
    }
}

pub async fn serve(addr: &str, app: Arc<App>) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    spawn_pacers(&app);
    eprintln!("web UI on http://{addr}");
    loop {
        let (sock, _) = listener.accept().await?;
        let app = app.clone();
        tokio::spawn(async move {
            let _ = handle(sock, app).await; // client gone: nothing to do
        });
    }
}

fn param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
}

/// Percent-decode a query value ("%C3%A9" -> "é", "+" -> " "); malformed escapes stay as is.
fn decode(s: &str) -> String {
    let (b, mut out, mut i) = (s.as_bytes(), Vec::with_capacity(s.len()), 0);
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        match (b[i], b.get(i + 1).copied().and_then(hex), b.get(i + 2).copied().and_then(hex)) {
            (b'%', Some(hi), Some(lo)) => {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
            (b'+', ..) => out.push(b' '),
            (c, ..) => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What the others see of a session: a scrambled id, so nobody can steer someone else's ride.
fn public_id(sid: u64) -> u64 {
    (sid.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 12) | 1 << 40 // < 2^53 for the page, never a pace partner's small id
}

/// Emotes a rider can send.
const EMOTES: [&str; 5] = ["rideon", "wave", "fire", "laugh", "party"];
/// How long an emote bubble stays up.
const EMOTE_SECS: u64 = 3;

/// A Ride On received, shown to its recipient.
#[derive(Serialize)]
struct Notice {
    from: String,
    e: &'static str,
}

/// Who this session shows to the others, from untrusted query values.
/// A profile id, as the page makes them (a uuid).
fn valid_pid(p: &str) -> bool {
    !p.is_empty() && p.len() <= 40 && p.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

fn presence(query: &str, sid: u64) -> Presence {
    let name: String = param(query, "name").map(decode).unwrap_or_default().chars().filter(|c| !c.is_control()).take(24).collect();
    let color = param(query, "color").map(decode).filter(|c| c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|h| h.is_ascii_hexdigit()));
    let frame = ["road", "aero", "gravel", "vintage"].into_iter().find(|f| param(query, "frame") == Some(*f)).unwrap_or("road");
    let country = param(query, "country").filter(|c| c.len() == 2 && c.bytes().all(|b| b.is_ascii_uppercase())).map(str::to_owned);
    Presence {
        id: public_id(sid),
        sid,
        emote: None,
        emote_at: None,
        name: if name.trim().is_empty() { "Cycliste".into() } else { name.trim().to_owned() },
        color: color.unwrap_or_else(|| "#c91a09".into()),
        frame,
        country,
        distance: 0.0,
        speed_kmh: 0.0,
        power: 0.0,
        pos: None,
        bot: false,
        pid: param(query, "pid").filter(|p| valid_pid(p)).unwrap_or_default().to_owned(),
        ridden: 0.0,
        event: None,
    }
}

async fn handle(mut sock: TcpStream, app: Arc<App>) -> std::io::Result<()> {
    let mut buf = [0u8; 16384]; // an imported workout travels in the query
    let n = sock.read(&mut buf).await?;
    let req = String::from_utf8_lossy(&buf[..n]);
    let target = req.split_whitespace().nth(1).unwrap_or("/");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let track = param(query, "id").or(param(query, "course")).and_then(|id| app.track(id));
    let session: Option<u64> = param(query, "session").and_then(|s| s.parse().ok());
    let peer = sock.peer_addr().ok().map(|a| a.ip());
    let who = app.identify(cookie(&req, SESSION_COOKIE), peer);
    // Cloud: only the page and the way to sign in are open to strangers.
    let open = matches!(path, "/" | "/whoami") || path.starts_with("/auth/") || path.starts_with("/audio/");
    // Another site's page must not drive this one in the visitor's browser (CSRF): every call but a visit is refused
    // when the browser says it comes from elsewhere.
    if !open && cross_site(&req) {
        return reply(&mut sock, "403 Forbidden", "text/plain", b"cross-site request").await;
    }
    // A ride is steered only by whoever opened it; the first call claims a fresh session.
    let owner: Owner = (peer, who.athlete);
    if matches!(path, "/power" | "/seek" | "/turn" | "/powerup" | "/pause" | "/emote" | "/events")
        && let Some(sid) = session
        && *app.owners.lock().unwrap().entry(sid).or_insert(owner) != owner
    {
        return reply(&mut sock, "403 Forbidden", "text/plain", b"not your ride").await;
    }
    // A profile id in the query: in cloud mode, only its owner may use it.
    let pid = param(query, "pid").filter(|p| valid_pid(p));
    if pid.is_some_and(|p| !app.riders.lock().unwrap().may_use(p, who.athlete)) {
        return reply(&mut sock, "403 Forbidden", "text/plain", b"this profile belongs to another account").await;
    }
    if app.mode == Mode::Cloud && who.athlete.is_none() && !open {
        return reply(&mut sock, "401 Unauthorized", "text/plain", b"sign in with Strava first").await;
    }
    // The one gate: a permission per endpoint that is not plain riding.
    let needs = match path {
        "/worlds/new" => Some(Perm::CreateWorld),
        p if p.starts_with("/admin/") => Some(Perm::Moderate),
        _ => None,
    };
    if let Some(p) = needs.filter(|p| !who.can(*p)) {
        return reply(&mut sock, "403 Forbidden", "text/plain", format!("needs {p:?}").as_bytes()).await;
    }
    match (path, track) {
        ("/", _) => reply(&mut sock, "200 OK", "text/html; charset=utf-8", PAGE.as_bytes()).await,
        (p, _) if p.starts_with("/audio/") => match AUDIO.iter().find(|a| a.0 == p) {
            Some((_, bytes)) => reply(&mut sock, "200 OK", "audio/mpeg", bytes).await,
            None => reply(&mut sock, "404 Not Found", "text/plain", b"not found").await,
        },
        ("/workouts", _) => reply(&mut sock, "200 OK", "application/json", app.workouts_json.as_bytes()).await,
        ("/courses", _) => reply(&mut sock, "200 OK", "application/json", app.list_json.as_bytes()).await,
        ("/whoami", _) => {
            let body = json!({ "role": who.role, "name": who.name, "athlete": who.athlete, "perms": who.role.perms(), "strava": app.strava.is_some(), "mode": app.mode }).to_string();
            reply(&mut sock, "200 OK", "application/json", body.as_bytes()).await
        }
        ("/auth/strava", _) => match &app.strava {
            Some(s) => {
                let (url, state) = s.authorize_url();
                app.new_state(state);
                redirect(&mut sock, &url, None).await
            }
            None => reply(&mut sock, "404 Not Found", "text/plain", b"no Strava app on this server (docs/rbac.md)").await,
        },
        ("/auth/strava/callback", _) => {
            // The state must be one we handed out (and is used once): otherwise this is not our sign-in.
            let state_ok = param(query, "state").is_some_and(|st| app.oauth_states.lock().unwrap().remove(&decode(st)).is_some_and(|t| t.elapsed().as_secs() < 600));
            let (Some(s), Some(code), true) = (app.strava.as_ref(), param(query, "code"), state_ok) else {
                return redirect(&mut sock, "/?signin=failed", None).await; // refused on Strava, expired or forged
            };
            let code = decode(code);
            match tokio::task::block_in_place(|| s.exchange(&code)) {
                Ok((athlete, mut tokens)) => {
                    // Strava tells which scopes were granted on the callback, not in the token response.
                    tokens.scope = param(query, "scope").map(decode).unwrap_or_default();
                    let id = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
                    let name = athlete.name();
                    app.sessions.lock().unwrap().insert(id.clone(), Session { athlete: athlete.id, name: name.clone(), expires: std::time::SystemTime::now() + SESSION_TTL });
                    app.save_strava_tokens(athlete.id, tokens);
                    eprintln!("signed in: {name} (Strava athlete {})", athlete.id);
                    let secure = if std::env::var("LE_MOLLET_PUBLIC_URL").is_ok_and(|u| u.starts_with("https://")) { "; Secure" } else { "" };
                    let cookie = format!("{SESSION_COOKIE}={id}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{secure}", SESSION_TTL.as_secs());
                    redirect(&mut sock, "/?signin=strava", Some(&cookie)).await
                }
                Err(e) => {
                    eprintln!("Strava sign-in failed: {e:#}");
                    redirect(&mut sock, "/?signin=failed", None).await
                }
            }
        }
        ("/auth/logout", _) => {
            if let Some(c) = cookie(&req, SESSION_COOKIE) { app.sessions.lock().unwrap().remove(c); }
            redirect(&mut sock, "/", Some(&format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0"))).await
        }
        ("/admin/segments/reset", Some(t)) => {
            let name = param(query, "name").map(decode).unwrap_or_default();
            let done = {
                let mut boards = app.boards.lock().unwrap();
                let done = boards.reset(&format!("{}:{name}", t.id));
                if done && let Err(e) = boards.save(&app.boards_path) { eprintln!("leaderboards: {e}") }
                done
            };
            if done { eprintln!("{} reset the leaderboard {}:{name}", who.name.as_deref().unwrap_or("?"), t.id) }
            reply(&mut sock, if done { "204 No Content" } else { "404 Not Found" }, "text/plain", b"").await
        }
        ("/rider", _) => {
            let Some(pid) = pid else { return reply(&mut sock, "400 Bad Request", "text/plain", b"need pid").await };
            let num = |k| param(query, k).and_then(|v| v.parse::<f32>().ok());
            let now = unix_now();
            let (status, body) = {
                let mut riders = app.riders.lock().unwrap();
                let res = match (num("weight"), num("ftp")) {
                    (Some(w), Some(f)) => riders.register(pid, who.athlete, w, f, now).cloned(),
                    _ => riders.riders.get(pid).cloned().ok_or("no such rider"),
                };
                let view = |r: &rider::Rider| json!({ "weight": r.weight, "ftp": r.ftp, "seen_ftp": r.seen_ftp(now), "rank": r.rank(now).to_string(), "wkg": r.wkg(now), "flags": r.flags.len() });
                match res {
                    Ok(r) => {
                        if num("weight").is_some() && let Err(e) = riders.save(&app.riders_path) { eprintln!("riders: {e}") }
                        ("200 OK", view(&r))
                    }
                    // Refused: what the server keeps, so the page can put it back.
                    Err(e) => ("409 Conflict", json!({ "error": e, "rider": riders.riders.get(pid).map(view) })),
                }
            };
            reply(&mut sock, status, "application/json", body.to_string().as_bytes()).await
        }
        ("/admin/flags", _) => {
            let body = {
                let riders = app.riders.lock().unwrap();
                let mut all: Vec<_> = riders.riders.values().flat_map(|r| r.flags.iter().cloned()).collect();
                all.sort_by_key(|f| std::cmp::Reverse(f.at));
                json!(all).to_string()
            };
            reply(&mut sock, "200 OK", "application/json", body.as_bytes()).await
        }
        ("/emote", Some(t)) => {
            let e = param(query, "e").and_then(|e| EMOTES.iter().find(|x| **x == e).copied());
            let to = param(query, "to").and_then(|v| v.parse::<u64>().ok());
            let sent = session.zip(e).is_some_and(|(sid, e)| {
                let mut riders = t.riders.lock().unwrap();
                let now = std::time::Instant::now();
                let Some(me) = riders.get_mut(&sid) else { return false };
                (me.emote, me.emote_at) = (Some(e), Some(now));
                let from = me.name.clone();
                if e == "rideon" && let Some(o) = to.and_then(|to| riders.values_mut().find(|p| p.id == to)) {
                    if o.bot { (o.emote, o.emote_at) = (Some("wave"), Some(now)) } // pace partners wave back
                    else { app.inbox.lock().unwrap().entry(o.sid).or_default().push(Notice { from, e }) }
                }
                true
            });
            if sent { reply(&mut sock, "204 No Content", "text/plain", b"").await } else { reply(&mut sock, "400 Bad Request", "text/plain", b"need session (riding this course) and e").await }
        }
        ("/segments", Some(t)) => {
            let defs: Vec<(String, Kind)> = match &t.city {
                Some(c) => c.segments.iter().map(|s| (s.name.clone(), s.kind)).collect(),
                None => segment::course_windows(&t.course).into_iter().map(|w| (w.name, w.kind)).collect(),
            };
            let body = { // the lock must be gone before the await below
                let (now, boards) = (unix_now(), app.boards.lock().unwrap());
                let list: Vec<_> = defs.iter().map(|(name, kind)| {
                    let (all, today) = boards.top(&format!("{}:{name}", t.id), 10, now);
                    json!({ "name": name, "kind": kind, "all": all, "today": today })
                }).collect();
                serde_json::to_string(&list).unwrap_or_default()
            };
            reply(&mut sock, "200 OK", "application/json", body.as_bytes()).await
        }
        ("/worlds", _) => reply(&mut sock, "200 OK", "application/json", app.worlds_json().as_bytes()).await,
        ("/worlds/new", _) => {
            let length = param(query, "length").and_then(|l| l.parse::<u32>().ok()).filter(|l| [5, 10, 20].contains(l));
            let (Some(length), Some(relief)) = (length, param(query, "relief").and_then(Relief::parse)) else {
                return reply(&mut sock, "400 Bad Request", "text/plain", b"need length=5|10|20 and relief=flat|hills|mountains").await;
            };
            let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
            let name = format!("Boucle {length} km {} n°{}", relief.label().to_lowercase(), seed % 1000);
            match tokio::task::spawn_blocking(move || world_track(seed, length, relief, name)).await {
                Ok(Ok(t)) => {
                    let id = app.add_world(t);
                    reply(&mut sock, "200 OK", "application/json", json!({ "id": id }).to_string().as_bytes()).await
                }
                _ => reply(&mut sock, "500 Internal Server Error", "text/plain", b"world generation failed").await,
            }
        }
        ("/course", Some(t)) => reply(&mut sock, "200 OK", "application/json", t.json.as_bytes()).await,
        ("/terrain", Some(t)) if t.terrain.is_some() => {
            reply(&mut sock, "200 OK", "application/octet-stream", t.terrain.as_deref().unwrap()).await
        }
        ("/buildings", Some(t)) if t.buildings.is_some() => {
            reply(&mut sock, "200 OK", "application/octet-stream", t.buildings.as_deref().unwrap()).await
        }
        ("/ortho", Some(t)) => match param(query, "f").and_then(|f| t.ortho.get(f)) {
            Some(b) => reply(&mut sock, "200 OK", if param(query, "f").is_some_and(|f| f.ends_with(".json")) { "application/json" } else { "image/jpeg" }, b).await,
            None => reply(&mut sock, "404 Not Found", "text/plain", b"no such ortho file").await,
        },
        ("/district", Some(t)) => match param(query, "f").and_then(|f| t.district.get(f)) {
            Some(b) => {
                let ctype = match param(query, "f") { Some("ortho.jpg") => "image/jpeg", Some(f) if f.ends_with(".json") => "application/json", _ => "application/octet-stream" };
                reply(&mut sock, "200 OK", ctype, b).await
            }
            None => reply(&mut sock, "404 Not Found", "text/plain", b"no such district file").await,
        },
        ("/power", _) => {
            let w = param(query, "w").and_then(|w| w.parse::<f32>().ok()).filter(|w| (0.0..=2000.0).contains(w));
            match (session, w) {
                (Some(s), Some(w)) => {
                    let num = |k: &str, max: f32| param(query, k).and_then(|v| v.parse::<f32>().ok()).filter(|v| (0.0..=max).contains(v));
                    let sensors = Sensors { w, cadence: num("cad", 250.0), hr: num("hr", 250.0), live: param(query, "src") == Some("bt") };
                    app.sensors.lock().unwrap().insert(s, sensors);
                    reply(&mut sock, "204 No Content", "text/plain", b"").await
                }
                _ => reply(&mut sock, "400 Bad Request", "text/plain", b"need session and w in 0..=2000").await,
            }
        }
        ("/seek", _) if app.trainer.is_none() => {
            match (session, param(query, "d").and_then(|d| d.parse::<f32>().ok()).filter(|d| d.is_finite() && *d >= 0.0)) {
                (Some(s), Some(d)) => {
                    app.demo_seek.lock().unwrap().insert(s, d);
                    reply(&mut sock, "204 No Content", "text/plain", b"").await
                }
                _ => reply(&mut sock, "400 Bad Request", "text/plain", b"need session and d >= 0").await,
            }
        }
        ("/turn", _) => match (session, param(query, "dir").and_then(Turn::parse)) {
            (Some(s), Some(t)) => {
                app.turns.lock().unwrap().insert(s, t);
                reply(&mut sock, "204 No Content", "text/plain", b"").await
            }
            _ => reply(&mut sock, "400 Bad Request", "text/plain", b"need session and dir=left|straight|right").await,
        },
        ("/powerup", _) => match session {
            Some(s) => {
                app.powerup.lock().unwrap().insert(s);
                reply(&mut sock, "204 No Content", "text/plain", b"").await
            }
            None => reply(&mut sock, "400 Bad Request", "text/plain", b"need session").await,
        },
        (p, _) if p.starts_with("/event/") => {
            let text = |k| param(query, k).map(decode).unwrap_or_default().chars().filter(|c| !c.is_control()).take(40).collect::<String>();
            let now = unix_now();
            let pid = pid.map(str::to_owned);
            let res: Result<serde_json::Value, &str> = (|| {
                let mut events = app.events.lock().unwrap();
                // Races by level on every half hour, on the short courses in turn (loops, climbs up to 15 km).
                let short: Vec<_> = app.tracks.read().unwrap().iter().filter(|t| t.city.is_none() && t.relief.is_none() && (t.course.looped || t.course.length() <= 15_000.0))
                    .map(|t| (t.id.clone(), t.course.name.clone(), t.course.length(), t.course.looped)).collect();
                if events.schedule(now, &short) && let Err(e) = events.save(&app.events_path) { eprintln!("events: {e}") }
                let changed = match p {
                    "/event/list" => return Ok(json!({ "now": now, "events": events_for(&events.current(now), pid.as_deref()) })),
                    "/event/create" => {
                        let pid = pid.clone().ok_or("need pid")?;
                        // A club ride: the founder of the profile's club may schedule it; anything else needs manage_events.
                        let club = (param(query, "club") == Some("1")).then(|| app.clubs.lock().unwrap().of(&pid).filter(|c| c.members[0].pid == pid).map(|c| c.tag.clone())).flatten();
                        if !who.can(Perm::ManageEvents) && club.is_none() { return Err("needs manage_events, or to found a club for a club ride") }
                        let kind = match param(query, "kind") { Some("race") => EventKind::Race, _ => EventKind::Group };
                        let track = app.track(param(query, "track").unwrap_or("")).filter(|t| t.city.is_none()).ok_or("no such course")?;
                        let start = param(query, "start").and_then(|s| s.parse::<u64>().ok()).filter(|&s| s + 60 >= now && s < now + 60 * 86_400).ok_or("start: a time in the next 60 days")?;
                        let name = text("name");
                        events.events.push(Event {
                            id: uuid::Uuid::new_v4().simple().to_string()[..8].into(),
                            kind,
                            name: if name.trim().is_empty() { track.course.name.clone() } else { name },
                            track: track.id.clone(),
                            start,
                            laps: if track.course.looped { param(query, "laps").and_then(|l| l.parse().ok()).unwrap_or(3u32).clamp(1, 50) } else { 1 },
                            pace: param(query, "pace").and_then(|p| p.parse().ok()).unwrap_or(2.5f32).clamp(1.0, 5.0),
                            cats: { let mut c: Vec<char> = text("cats").chars().filter(|c| TIERS.iter().any(|t| t.0 == *c)).collect(); c.sort(); c.dedup(); c.into_iter().collect() },
                            club,
                            by: pid,
                            by_name: text("me"),
                            entrants: vec![],
                            results: vec![],
                        });
                        true
                    }
                    "/event/join" => {
                        // The rank the server knows, never the page's word.
                        let wkg = app.riders.lock().unwrap().riders.get(pid.as_deref().unwrap_or("")).map(|r| r.wkg(now)).ok_or("unknown rider")?;
                        events.join(param(query, "id").unwrap_or(""), pid.as_deref().ok_or("need pid")?, &text("me"), wkg, now)?;
                        true
                    }
                    "/event/leave" => { events.leave(param(query, "id").unwrap_or(""), pid.as_deref().ok_or("need pid")?); true }
                    "/event/delete" => {
                        let id = param(query, "id").unwrap_or("");
                        let mine = events.get(id).is_some_and(|e| Some(&e.by) == pid.as_ref());
                        if !mine && !who.can(Perm::ManageEvents) { return Err("only its organizer can cancel it") }
                        events.events.retain(|e| e.id != id);
                        true
                    }
                    _ => return Err("not found"),
                };
                if changed && let Err(e) = events.save(&app.events_path) { eprintln!("events: {e}") }
                Ok(json!({ "now": now, "events": events_for(&events.current(now), pid.as_deref()) }))
            })();
            match res {
                Ok(v) => reply(&mut sock, "200 OK", "application/json", v.to_string().as_bytes()).await,
                Err(e) => reply(&mut sock, "400 Bad Request", "text/plain", e.as_bytes()).await,
            }
        }
        (p, _) if p == "/club" || p.starts_with("/club/") => {
            let Some(pid) = pid else {
                return reply(&mut sock, "400 Bad Request", "text/plain", b"need pid").await;
            };
            let text = |k| param(query, k).map(decode).unwrap_or_default().chars().filter(|c| !c.is_control()).take(30).collect::<String>();
            let me = text("me");
            let res = {
                let mut clubs = app.clubs.lock().unwrap();
                let res = match p {
                    "/club" => Ok(clubs.of(pid).cloned()),
                    "/club/create" => {
                        let color = Some(text("color")).filter(|c| c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|h| h.is_ascii_hexdigit())).unwrap_or_else(|| "#0055bf".into());
                        let code = uuid::Uuid::new_v4().simple().to_string()[..6].to_uppercase();
                        clubs.create(pid, &me, &text("name"), &text("tag").to_uppercase(), &color, code).map(|c| Some(c.clone()))
                    }
                    "/club/join" => clubs.join(&text("code"), pid, &me).cloned().map(Some).ok_or("no club with this code"),
                    "/club/leave" => { clubs.leave(pid); Ok(None) }
                    _ => Err("not found"),
                };
                if res.is_ok() && p != "/club" && let Err(e) = clubs.save(&app.clubs_path) { eprintln!("clubs: {e}") }
                res
            };
            match res {
                Ok(mut c) => {
                    let wk = club::week(unix_now());
                    if let Some(c) = c.as_mut() {
                        for m in &mut c.members { if m.week != wk { m.km = 0.0 } } // last week's count
                        c.members.sort_by(|a, b| b.km.total_cmp(&a.km));
                    }
                    let mut v = json!(c);
                    for m in v["members"].as_array_mut().into_iter().flatten() { // no profile ids for the others
                        m["me"] = json!(m["pid"].as_str() == Some(pid));
                        m.as_object_mut().map(|o| o.remove("pid"));
                    }
                    reply(&mut sock, "200 OK", "application/json", v.to_string().as_bytes()).await
                }
                Err(e) => reply(&mut sock, "400 Bad Request", "text/plain", e.as_bytes()).await,
            }
        }
        ("/pause", _) => match (session, param(query, "on")) {
            (Some(s), Some(on)) => {
                {
                    let mut p = app.paused.lock().unwrap();
                    if on == "1" { p.insert(s) } else { p.remove(&s) };
                }
                reply(&mut sock, "204 No Content", "text/plain", b"").await
            }
            _ => reply(&mut sock, "400 Bad Request", "text/plain", b"need session and on=0|1").await,
        },
        ("/events", track) => {
            // A built-in workout, or one the page sends step by step (an imported .zwo).
            let custom = param(query, "steps").and_then(|s| Workout::from_query(&param(query, "wname").map(decode).unwrap_or_default(), s));
            let workout = param(query, "workout").and_then(|id| app.workouts.iter().find(|w| w.id == id)).or(custom.as_ref());
            let ftp = param(query, "ftp").and_then(|f| f.parse::<f32>().ok()).filter(|f| (30.0..=1000.0).contains(f));
            if track.is_none() && (workout.is_none() || ftp.is_none()) {
                return reply(&mut sock, "400 Bad Request", "text/plain", b"need course=<id>, or workout=<id>&ftp=<W>").await;
            }
            let Some(sid) = session else { return reply(&mut sock, "400 Bad Request", "text/plain", b"need session").await };
            // Weight and FTP from the server's record of the profile, never from the query (docs/security.md).
            // A workout, alone on a flat road, races nobody: the page's mass does.
            let known = pid.filter(|_| track.is_some()).and_then(|p| app.riders.lock().unwrap().riders.get(p).cloned());
            let asked = param(query, "mass").and_then(|m| m.parse::<f32>().ok()).filter(|m| (30.0..=250.0).contains(m));
            let mass = known.as_ref().map(|r| r.weight + rider::BIKE_KG).or(asked);
            let mut check = Check::new(known.as_ref().map(|r| r.ftp_now(unix_now())), mass.unwrap_or(83.0) - rider::BIKE_KG);
            sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\r\n").await?;
            app.driver.store(sid, Ordering::Relaxed);
            let plan = workout.zip(ftp).filter(|_| track.is_none());
            let game = ftp.filter(|_| track.is_some() && param(query, "mode") == Some("zombie")).map(Game::new);
            // An event on this course: held in its pen, then raced or ridden behind its leader.
            let event = param(query, "event").and_then(|id| app.events.lock().unwrap().get(id).cloned()).filter(|e| track.as_ref().is_some_and(|t| t.id == e.track));
            if let Some(t) = &track {
                t.riders.lock().unwrap().insert(sid, Presence { event: event.as_ref().map(|e| e.id.clone()), ..presence(query, sid) });
            }
            let start = param(query, "start").and_then(|s| s.parse::<usize>().ok());
            // Road bonuses on courses and open worlds, not in workouts nor against the zombies.
            let mut ride = Ride { powerups: plan.is_none() && game.is_none() && event.as_ref().is_none_or(|e| e.kind == EventKind::Race), ..Ride::new(mass.unwrap_or(83.0)) };
            let res = ride_session(&mut sock, &app, track.as_deref(), plan, game, (sid, start, event), (&mut ride, &mut check)).await;
            if let (Some(r), Some(t)) = (&known, &track) && ride.ridden >= 100.0 {
                let now = unix_now();
                let seen = [check.seen_ftp(), check.race_ftp].into_iter().flatten().reduce(f32::max);
                let flagged = (check.trust() == Trust::Flagged).then(|| Flagged { at: now, track: t.id.clone(), name: presence(query, sid).name, why: check.why() });
                if let Some(f) = &flagged { eprintln!("fair play: {} flagged on {}: {}", f.name, f.track, f.why.join(", ")) }
                let mut riders = app.riders.lock().unwrap();
                riders.ride_done(pid.unwrap_or(""), seen.filter(|s| *s > r.ftp_now(now)), flagged, now);
                if let Err(e) = riders.save(&app.riders_path) { eprintln!("riders: {e}") }
            }
            if let Some(me) = track.as_ref().and_then(|t| t.riders.lock().unwrap().remove(&sid)).filter(|p| !p.pid.is_empty() && ride.ridden >= 100.0) {
                let mut clubs = app.clubs.lock().unwrap();
                if clubs.add_km(&me.pid, &me.name, ride.ridden / 1000.0, club::week(unix_now())) && let Err(e) = clubs.save(&app.clubs_path) { eprintln!("clubs: {e}") }
            }
            app.sensors.lock().unwrap().remove(&sid);
            app.paused.lock().unwrap().remove(&sid);
            app.turns.lock().unwrap().remove(&sid);
            app.inbox.lock().unwrap().remove(&sid);
            app.powerup.lock().unwrap().remove(&sid);
            app.owners.lock().unwrap().remove(&sid);
            res
        }
        _ => reply(&mut sock, "404 Not Found", "text/plain", b"not found").await,
    }
}

/// Other riders sent in each frame; past that, only the nearest.
const OTHERS_MAX: usize = 50;

/// The `max` riders closest to the one at `distance` along the course (wrapping on a loop of `lap` m),
/// or to `pos` in an open city, where each rider has a route of their own.
fn nearest<'a>(riders: impl Iterator<Item = &'a Presence>, distance: f32, pos: Option<[f64; 2]>, lap: Option<f32>, max: usize) -> Vec<Presence> {
    let gap = |p: &Presence| match (pos, p.pos) {
        (Some(a), Some(b)) => ((a[0] - b[0]) * 110_540.0).hypot((a[1] - b[1]) * 111_320.0 * a[0].to_radians().cos()) as f32,
        _ => {
            let d = (p.distance - distance).abs();
            lap.map_or(d, |l| d.min(l - d))
        }
    };
    let mut v: Vec<(f32, &Presence)> = riders.map(|p| (gap(p), p)).collect();
    if v.len() > max {
        v.select_nth_unstable_by(max, |a, b| a.0.total_cmp(&b.0));
        v.truncate(max);
    }
    v.into_iter().map(|(_, p)| {
        let mut c = p.clone();
        if c.emote_at.is_none_or(|t| t.elapsed().as_secs() >= EMOTE_SECS) { c.emote = None }
        c
    }).collect()
}

/// One SSE frame: the ride, plus the workout progress when training or the zombie game, plus the other riders.
#[derive(Serialize)]
struct Frame<'a> {
    #[serde(flatten)]
    ride: &'a Ride,
    #[serde(skip_serializing_if = "Option::is_none")]
    workout: Option<Progress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    zombie: Option<&'a Game>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    others: Vec<Presence>,
    /// Open city: route points added since the last frame (all of them in the first one).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    route: Vec<Point>,
    #[serde(skip_serializing_if = "Option::is_none")]
    junction: Option<Junction>,
    /// Segments along this rider's road, sent once each (all of a course's in the first frame).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    windows: Vec<Window>,
    /// The segment being ridden, and the one just finished.
    #[serde(skip_serializing_if = "Option::is_none")]
    segment: Option<SegLive>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<SegResult>,
    /// Ride Ons received since the last frame.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    inbox: Vec<Notice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    event: Option<EventLive>,
    /// How far the server believes this ride's power (docs/security.md).
    fair: Fair,
}

#[derive(Serialize)]
struct Fair {
    trust: Trust,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    why: Vec<String>,
}

/// Where a rider stands in their event, each frame.
#[derive(Serialize)]
struct EventLive {
    id: String,
    kind: EventKind,
    name: String,
    phase: Phase,
    /// Seconds to the start (negative once started).
    starts_in: f32,
    laps: u32,
    /// Race distance and how much of it is done, m.
    race_m: f32,
    done_m: f32,
    /// Group ride: the leader's lead over this rider, m (negative: the rider is ahead).
    #[serde(skip_serializing_if = "Option::is_none")]
    leader_gap: Option<f32>,
    /// In the bunch: event riders within 200 m behind to 25 m ahead of the leader.
    bunch: usize,
    rank: usize,
    of: usize,
    /// The first five and this rider: name, category, gap to this rider (m, positive = ahead).
    board: Vec<BoardRow>,
    /// Crossed the line: place overall, place in category, time.
    #[serde(skip_serializing_if = "Option::is_none")]
    finish: Option<(usize, usize, f32)>,
    /// Crossed the line but out of the results: "demo", "flagged" or "above rank".
    #[serde(skip_serializing_if = "Option::is_none")]
    unranked: Option<&'static str>,
}

#[derive(Serialize)]
struct BoardRow {
    name: String,
    cat: char,
    gap: f32,
    me: bool,
}

/// The event as this rider sees it now, from the riders on its course.
fn event_live(e: &Event, riders: &HashMap<u64, Presence>, sid: u64, ride: &Ride, phase: Option<Phase>, finish: Option<(usize, usize, f32)>, course: &Course) -> EventLive {
    let mut field: Vec<&Presence> = riders.values().filter(|p| !p.bot && p.event.as_deref() == Some(&e.id)).collect();
    field.sort_by(|a, b| b.ridden.total_cmp(&a.ridden));
    let rank = field.iter().position(|p| p.sid == sid).map_or(field.len() + 1, |i| i + 1);
    let cat = |p: &Presence| e.entrants.iter().find(|x| x.pid == p.pid).map_or('-', |x| x.cat);
    let board = field.iter().enumerate().filter(|&(i, p)| i < 5 || p.sid == sid)
        .map(|(_, p)| BoardRow { name: p.name.clone(), cat: cat(p), gap: p.ridden - ride.ridden, me: p.sid == sid }).collect();
    let leader = riders.values().find(|p| p.bot && p.event.as_deref() == Some(&e.id)).map(|p| p.ridden);
    let bunch = leader.map_or(0, |l| field.iter().filter(|p| (-25.0..200.0).contains(&(l - p.ridden))).count());
    EventLive {
        id: e.id.clone(), kind: e.kind, name: e.name.clone(), phase: phase.unwrap_or(Phase::Upcoming),
        starts_in: (e.start as f64 - now_f()) as f32, laps: e.laps, race_m: e.distance(course.length(), course.looped), done_m: ride.ridden,
        leader_gap: leader.map(|l| l - ride.ridden), bunch, rank, of: field.len(), board, finish, unranked: None,
    }
}

/// Group ride leader: rides the event's course at its pace from the start time until the event is over,
/// as a yellow-jersey bot everyone sees.
async fn beacon(t: Arc<Track>, e: Event) {
    let id = 900_000 + e.id.bytes().fold(0u64, |h, b| (h * 31 + b as u64) % 90_000);
    let mut ride = Ride::new(83.0);
    ride.power = e.pace * 75.0;
    // Out late (the first rider came after the start): where the leader would be by now.
    for _ in 0..((unix_now().saturating_sub(e.start)) as f32 / DT) as usize { ride.tick(&t.course, DT) }
    let (mut tick, mut n) = (tokio::time::interval(Duration::from_secs_f32(DT)), 0u32);
    while e.phase(unix_now()) != Phase::Over {
        tick.tick().await;
        ride.tick(&t.course, DT);
        n += 1;
        if n % 3 == 0 {
            let mut riders = t.riders.lock().unwrap();
            let p = riders.entry(id).or_insert_with(|| Presence {
                id, sid: id, emote: None, emote_at: None, name: format!("Meneur · {:.1} W/kg", e.pace).replace('.', ","), color: "#f2cd37".into(), frame: "road",
                country: None, distance: 0.0, speed_kmh: 0.0, power: 0.0, pos: None, bot: true, pid: String::new(), ridden: 0.0, event: Some(e.id.clone()),
            });
            (p.distance, p.speed_kmh, p.power, p.ridden) = (ride.distance, ride.speed_kmh, ride.power, ride.ridden);
        }
    }
    t.riders.lock().unwrap().remove(&id);
}

fn now_f() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())
}

#[derive(Serialize)]
struct SegLive {
    name: String,
    kind: Kind,
    done: f32,
    len: f32,
    time: f32,
    best: Option<f32>,
}

#[derive(Serialize)]
struct SegResult {
    name: String,
    kind: Kind,
    time: f32,
    rank: Rank,
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Drag saved behind the closest rider ahead: along the course (wrapping on a loop of `lap` m), or, in an open
/// world where `pos` is (me, a point 5 m ahead of me), within 2 m of my line.
fn draft_behind<'a>(others: impl Iterator<Item = &'a Presence>, distance: f32, pos: Option<([f64; 2], [f64; 2])>, lap: Option<f32>) -> f32 {
    let gap = |p: &Presence| match (pos, p.pos) {
        (Some((a, h)), Some(b)) => {
            let k = a[0].to_radians().cos();
            let (hx, hz) = ((h[1] - a[1]) * k, a[0] - h[0]);
            let n = hx.hypot(hz).max(1e-12);
            let (dx, dz) = ((b[1] - a[1]) * k, a[0] - b[0]);
            let side = ((dx * hz - dz * hx) / n * 110_540.0).abs();
            if side < 2.0 { ((dx * hx + dz * hz) / n * 110_540.0) as f32 } else { -1.0 }
        }
        _ => {
            let g = p.distance - distance;
            lap.map_or(g, |l| if g > l / 2.0 { g - l } else if g < -l / 2.0 { g + l } else { g })
        }
    };
    physics::draft(others.map(gap).filter(|g| *g > 0.0).fold(f32::MAX, f32::min))
}

/// Pace partners: riders that never stop, each at a steady W/kg, so no road is ever empty and there is
/// always a wheel to follow.
const PACERS: [(&str, f32, &str); 3] = [("Coco", 2.0, "#fc97ac"), ("Gaston", 3.0, "#36aebf"), ("Mamie Turbo", 4.0, "#a5ca18")];

pub fn spawn_pacers(app: &App) {
    let tracks = app.tracks.read().unwrap().clone();
    let mut id = 1;
    for t in tracks {
        let n = if t.city.is_some() || t.relief.is_some() { 3 } else { 2 };
        for k in 0..n {
            tokio::spawn(pacer(t.clone(), id, k));
            id += 1;
        }
    }
}

async fn pacer(t: Arc<Track>, id: u64, k: usize) {
    let (name, wkg, color) = PACERS[k];
    let mut rng = id.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut rand = move || { rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17; (rng >> 11) as f32 / (1u64 << 53) as f32 };
    let city = t.city.as_ref();
    let start = |c: &City| c.starts.get(k % c.starts.len().max(1)).map_or(c.start, |s| s.at);
    let mut route = city.map(|c| Route::start_at(c, start(c)));
    let mut ride = Ride::new(83.0);
    ride.distance = if route.is_none() { t.course.length() * k as f32 / 3.0 } else { 0.0 }; // spread out on a course
    let (mut tick, mut n, mut rest) = (tokio::time::interval(Duration::from_secs_f32(DT)), 0u32, 0.0);
    loop {
        tick.tick().await;
        n += 1;
        if let (Some(c), Some(r)) = (city, route.as_mut()) {
            if r.next == Turn::Straight && rand() < 0.004 { r.next = if rand() < 0.5 { Turn::Left } else { Turn::Right } }
            r.extend(c, ride.distance);
            // The route only grows: start a fresh one from here every 50 km so it never weighs much.
            if ride.distance > 50_000.0 {
                let here = r.course.latlon_at(ride.distance);
                *r = Route::start_at(c, here);
                (ride.distance, ride.ridden) = (0.0, 0.0);
            }
        }
        let course = route.as_ref().map_or(&t.course, |r| &r.course);
        ride.power = wkg * 75.0 * (1.0 + 0.04 * (n as f32 * 0.013 + k as f32).sin());
        if ride.finished {
            rest += DT;
            if rest > 20.0 { (ride, rest) = (Ride::new(83.0), 0.0) } // at the top of a climb: back down to the start
        }
        ride.tick(course, DT);
        if n % 3 == 0 {
            let pos = route.as_ref().map(|r| (r.course.latlon_at(ride.distance), r.course.latlon_at(ride.distance + 5.0)));
            let mut riders = t.riders.lock().unwrap();
            ride.draft = draft_behind(riders.values().filter(|p| p.sid != id), ride.distance, pos, t.course.looped.then(|| t.course.length()));
            let p = riders.entry(id).or_insert_with(|| Presence {
                id, sid: id, emote: None, emote_at: None, name: format!("{name} · {wkg:.1} W/kg").replace('.', ","), color: color.into(), frame: "road", country: None,
                distance: 0.0, speed_kmh: 0.0, power: 0.0, pos: None, bot: true, pid: String::new(), ridden: 0.0, event: None,
            });
            (p.distance, p.speed_kmh, p.power, p.pos) = (ride.distance, ride.speed_kmh, ride.power, pos.map(|x| x.0));
        }
    }
}

/// Open city: the next junction, how far it is and the ways out, left to right.
#[derive(Serialize)]
struct Junction {
    in_m: f32,
    exits: Vec<Exit>,
    next: Turn,
}

async fn ride_session(
    sock: &mut TcpStream,
    app: &App,
    track: Option<&Track>,
    plan: Option<(&Workout, f32)>,
    mut game: Option<Game>,
    (sid, start, event): (u64, Option<usize>, Option<Event>),
    (ride, check): (&mut Ride, &mut Check),
) -> std::io::Result<()> {
    // A race finish kept out of the results, and why.
    let mut unranked: Option<&'static str> = None;
    let mut finish: Option<(usize, usize, f32)> = None;
    let base = track.map_or(&app.flat, |t| &t.course);
    // Open city: this rider's own route, grown street by street; `sent` route points already streamed.
    let city = track.and_then(|t| t.city.as_ref());
    // Open city: start from the place the rider picked, else the city's own start.
    let route = city.map(|c| Route::start_at(c, start.and_then(|i| c.starts.get(i)).map_or(c.start, |s| s.at)));
    let (mut route, mut sent_pts) = (route, 0);
    // Segments: fixed windows on a course; in an open world, timed streets as the route takes them.
    let mut windows: Vec<Window> = if city.is_some() || track.is_none() { vec![] } else { segment::course_windows(base) };
    let (mut sent_windows, mut legs_seen, mut timer, mut result) = (0, 0, segment::Timer::default(), None::<SegResult>);
    let key = |w: &Window| format!("{}:{}", track.map_or("", |t| t.id.as_str()), w.name);
    let mut tick = tokio::time::interval(Duration::from_secs_f32(DT));
    let (mut n, mut sent) = (0u32, None::<f32>);
    loop {
        tick.tick().await;
        let seek = app.demo_seek.lock().unwrap().remove(&sid).filter(|_| event.is_none()); // no shortcuts in an event
        if let (Some(c), Some(r)) = (city, route.as_mut()) {
            if let Some(t) = app.turns.lock().unwrap().remove(&sid) {
                r.next = t;
            }
            r.extend(c, seek.unwrap_or(0.0).max(ride.distance)); // a demo jump ahead: grow the road there first
        }
        let course = route.as_ref().map_or(base, |r| &r.course);
        let sensors = app.sensors.lock().unwrap().get(&sid).copied().unwrap_or_default();
        (ride.power, ride.cadence, ride.hr) = match app.trainer {
            Some(_) if !sensors.live => {
                let (w, cadence) = *app.trainer_sample.lock().unwrap();
                (w, cadence, sensors.hr)
            }
            _ => (sensors.w, sensors.cadence, sensors.hr),
        };
        if let Some(d) = seek {
            ride.distance = d.min(course.length());
            ride.finished = false;
        }
        // Paused: time, distance and totals freeze; frames keep flowing so the page stays live.
        // Caught by the zombies: the ride freezes like a finish line.
        let caught = game.as_ref().is_some_and(|g| g.over);
        let paused = app.paused.lock().unwrap().contains(&sid);
        if app.powerup.lock().unwrap().remove(&sid) { ride.use_powerup() }
        // An event: on the start line until the gun.
        let phase = event.as_ref().map(|e| e.phase(unix_now()));
        let held = matches!(phase, Some(Phase::Upcoming | Phase::Pen));
        if held { (ride.distance, ride.speed_kmh) = (0.0, 0.0) }
        if let (Some(e), Some(Phase::Live)) = (&event, phase) && e.kind == EventKind::Group && app.beacons.lock().unwrap().insert(e.id.clone())
            && let Some(t) = app.track(&e.track)
        {
            tokio::spawn(beacon(t, e.clone()));
        }
        // Every second of power is checked, warm-up in the pen included: the slider or no profile makes a demo ride.
        if !paused && !caught { check.tick(ride.power, ride.cadence, sensors.live || app.trainer.is_some(), DT) }
        if !paused && !caught && !held {
            ride.tick(course, DT);
            if let (Some(e), Some(Phase::Live), None, None) = (&event, phase, finish, unranked)
                && e.kind == EventKind::Race
                && ride.ridden >= e.distance(base.length(), base.looped)
                && let Some(me) = track.and_then(|t| t.riders.lock().unwrap().get(&sid).cloned())
                && e.entrants.iter().any(|x| x.pid == me.pid)
            {
                let time = (now_f() - e.start as f64) as f32;
                // Too strong for the race's ranks: moved up, out of its results.
                check.race_ftp = check.effort_ftp(time as usize);
                let above = check.race_ftp.is_some_and(|f| !e.open_to(le_mollet::event::category(f / check.kg)));
                unranked = match check.trust() {
                    Trust::Demo => Some("demo"),
                    Trust::Flagged => Some("flagged"),
                    Trust::Device if above => Some("above rank"),
                    Trust::Device => None,
                };
                if unranked.is_none() {
                    let mut events = app.events.lock().unwrap();
                    finish = events.finish(&e.id, &me.pid, &me.name, time).map(|(o, c)| (o, c, time));
                    if let Err(err) = events.save(&app.events_path) { eprintln!("events: {err}") }
                }
            }
            // The horde waits for the first pedal stroke, like the ride clock.
            if let Some(g) = game.as_mut().filter(|_| ride.elapsed > 0.0) {
                g.tick(ride, DT);
            }
        }
        if let (Some(c), Some(r)) = (city, route.as_ref()) {
            for &(street, forward, d0, d1) in &r.legs[legs_seen..] {
                let name = &c.streets[street].name;
                if let Some(s) = c.segments.iter().find(|s| &s.street == name && (forward || s.kind == Kind::Sprint)) {
                    windows.push(Window { name: s.name.clone(), kind: s.kind, d0, d1 });
                }
            }
            legs_seen = r.legs.len();
        }
        if !paused && let Some((i, time)) = timer.update(&windows, ride.distance, ride.elapsed)
            && check.trust() == Trust::Device // only believed rides on the leaderboards
            && let Some(me) = track.and_then(|t| t.riders.lock().unwrap().get(&sid).cloned())
        {
            let w = &windows[i];
            let who = if me.pid.is_empty() { sid.to_string() } else { me.pid };
            let mut boards = app.boards.lock().unwrap();
            let rank = boards.record(&key(w), Entry { who, name: me.name, color: me.color, time, date: unix_now() });
            if let Err(e) = boards.save(&app.boards_path) { eprintln!("leaderboards: {e}") }
            result = Some(SegResult { name: w.name.clone(), kind: w.kind, time, rank });
        }
        // Workout clock = riding time, so it starts with the first pedal stroke.
        let progress = plan.map(|(w, ftp)| w.progress(ride.elapsed, ftp));
        n += 1;
        if n % 3 == 0 {
            // Publish where we are, read where the others are: the nearest ones, they are the ones on screen.
            let mut live = None;
            let others = track.map_or(vec![], |t| {
                let mut riders = t.riders.lock().unwrap();
                let me = riders.get_mut(&sid).and_then(|me| {
                    (me.distance, me.speed_kmh, me.power, me.ridden) = (ride.distance, ride.speed_kmh, ride.power, ride.ridden);
                    me.pos = city.map(|_| course.latlon_at(ride.distance));
                    me.pos
                });
                let lap = t.course.looped.then(|| t.course.length());
                // Drafting: the rider just ahead cuts the wind for the next ticks.
                ride.draft = draft_behind(riders.values().filter(|p| p.sid != sid), ride.distance, me.map(|a| (a, course.latlon_at(ride.distance + 5.0))), lap);
                live = event.as_ref().map(|e| EventLive { unranked, ..event_live(e, &riders, sid, ride, phase, finish, base) });
                // Group ride: anyone up to 200 m behind the leader rides on the bunch's wheel; ahead of it, alone.
                if let Some(gap) = live.as_ref().and_then(|l| l.leader_gap) {
                    if gap > 0.0 && gap < 200.0 { ride.draft = ride.draft.max(0.3) } else if gap < -25.0 { ride.draft = 0.0 }
                }
                nearest(riders.values().filter(|p| p.sid != sid), ride.distance, me, lap, OTHERS_MAX)
            });
            let fresh = route.as_ref().map_or(vec![], |r| r.course.points[sent_pts..].to_vec());
            sent_pts += fresh.len();
            let junction = city.zip(route.as_ref()).map(|(c, r)| {
                let (in_m, exits) = r.upcoming(c, ride.distance);
                Junction { in_m, exits, next: r.next }
            });
            let segment = timer.running().map(|(i, t0)| {
                let w = &windows[i];
                let best = app.boards.lock().unwrap().0.get(&key(w)).and_then(|l| l.first()).map(|e| e.time);
                SegLive { name: w.name.clone(), kind: w.kind, done: ride.distance - w.d0, len: w.d1 - w.d0, time: ride.elapsed - t0, best }
            });
            let new_windows = windows[sent_windows..].to_vec();
            sent_windows = windows.len();
            let frame = Frame { fair: Fair { trust: check.trust(), why: check.why() }, event: live, ride, workout: progress, zombie: game.as_ref(), others, route: fresh, junction, windows: new_windows, segment, result: result.take(), inbox: app.inbox.lock().unwrap().remove(&sid).unwrap_or_default() };
            let json = serde_json::to_string(&frame).expect("frame serializes");
            sock.write_all(format!("data: {json}\n\n").as_bytes()).await?;
        }
        // Workout: ERG holds the target watts. Course: grade scaled by difficulty.
        // ponytail: 1 W / 0.2 % deadbands at 1 Hz keep BLE writes low, tune if it feels steppy
        if let Some(t) = &app.trainer
            && !paused
            && !sensors.live
            && n % 10 == 0
            && app.driver.load(Ordering::Relaxed) == sid
        {
            let (value, erg) = match progress {
                Some(p) if p.target > 0.0 => (p.target, true),
                Some(_) => (0.0, false), // a free block: flat road, the legs choose
                None if ride.finished || caught => (0.0, false),
                None => ((ride.grade * app.difficulty * 10.0).round() / 10.0, false),
            };
            if sent.is_none_or(|v| (v - value).abs() >= if erg { 1.0 } else { 0.2 }) {
                let res = if erg { t.set_erg(value as u16).await } else { t.set_grade(value).await };
                match res {
                    Ok(()) => sent = Some(value),
                    Err(e) => eprintln!("trainer command failed: {e:#}"),
                }
            }
        }
    }
}

/// Sign-in sessions: an HttpOnly cookie naming an entry of `App::sessions`.
/// ponytail: sessions live in memory, a restart signs everyone out; persist them if that gets annoying
const SESSION_COOKIE: &str = "lm_session";
const SESSION_TTL: Duration = Duration::from_secs(30 * 86_400);

#[derive(Clone)]
struct Session {
    athlete: u64,
    name: String,
    expires: std::time::SystemTime,
}

/// A header's value from the request head.
fn header<'a>(req: &'a str, name: &str) -> Option<&'a str> {
    req.lines().take_while(|l| !l.is_empty()).find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.trim()))
}

/// The browser says this request comes from another site's page: `Sec-Fetch-Site`, or an `Origin` that is not us.
fn cross_site(req: &str) -> bool {
    header(req, "sec-fetch-site").is_some_and(|s| s == "cross-site" || s == "same-site")
        || header(req, "origin").is_some_and(|o| o.split_once("://").map(|x| x.1) != header(req, "host"))
}

/// Events as one profile sees them: no profile ids (they are the only key to a profile at home), `me` instead.
fn events_for(list: &[Event], pid: Option<&str>) -> serde_json::Value {
    json!(list.iter().map(|e| {
        let mut v = json!(e);
        v["by"] = json!(if e.by == "auto" { "auto" } else if Some(e.by.as_str()) == pid { "me" } else { "" });
        for k in ["entrants", "results"] {
            for x in v[k].as_array_mut().into_iter().flatten() {
                x["me"] = json!(pid.is_some() && x["pid"].as_str() == pid);
                x.as_object_mut().map(|o| o.remove("pid"));
            }
        }
        v
    }).collect::<Vec<_>>())
}

/// A cookie's value from the request head.
fn cookie<'a>(req: &'a str, name: &str) -> Option<&'a str> {
    req.lines().take_while(|l| !l.is_empty())
        .filter_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("cookie")).map(|(_, v)| v))
        .flat_map(|v| v.split(';'))
        .find_map(|kv| kv.trim().split_once('=').filter(|(k, _)| *k == name).map(|(_, v)| v))
}

/// Tokens on disk, readable by the server's user only.
fn write_private(path: &str, s: &str) -> std::io::Result<()> {
    std::fs::write(path, s)?;
    #[cfg(unix)]
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    Ok(())
}

async fn redirect(sock: &mut TcpStream, to: &str, set_cookie: Option<&str>) -> std::io::Result<()> {
    let cookie = set_cookie.map(|c| format!("Set-Cookie: {c}\r\n")).unwrap_or_default();
    let head = format!("HTTP/1.1 302 Found\r\nLocation: {to}\r\n{cookie}Cache-Control: no-store\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    sock.write_all(head.as_bytes()).await
}

async fn reply(sock: &mut TcpStream, status: &str, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    let head = format!("HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    sock.write_all(head.as_bytes()).await?;
    sock.write_all(body).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_values_are_decoded_and_sanitized() {
        assert_eq!(decode("Ren%C3%A9+Vi%25"), "René Vi%");
        assert_eq!(decode("bad%zz%4"), "bad%zz%4");
        let p = presence("name=%3Cb%3EBob%0A&color=%23ff00AA&frame=gravel&country=FR", 7);
        assert_eq!((p.name.as_str(), p.color.as_str(), p.frame, p.country.as_deref()), ("<b>Bob", "#ff00AA", "gravel", Some("FR")));
        let p = presence("color=red&frame=bmx&country=fr", 7);
        assert_eq!((p.name.as_str(), p.color.as_str(), p.frame, p.country), ("Cycliste", "#c91a09", "road", None));
    }

    fn rider(id: u64, distance: f32) -> Presence {
        Presence { id, sid: id, emote: None, emote_at: None, name: String::new(), color: String::new(), frame: "road", country: None, distance, speed_kmh: 0.0, power: 0.0, pos: None, bot: false, pid: String::new(), ridden: 0.0, event: None }
    }

    #[test]
    fn cookies_are_read_from_the_head_only() {
        let req = "GET / HTTP/1.1\r\nHost: x\r\nCookie: a=1; lm_session=abc123; b=2\r\n\r\nCookie: lm_session=body";
        assert_eq!(cookie(req, "lm_session"), Some("abc123"));
        assert_eq!(cookie(req, "missing"), None);
        assert_eq!(cookie("GET / HTTP/1.1\r\n\r\n", "lm_session"), None);
    }

    #[test]
    fn public_ids_hide_sessions() {
        for sid in [1u64, 42, 1 << 51, u64::MAX] {
            let id = public_id(sid);
            assert!(id != sid && id < 1 << 53 && id > 1000, "{sid} -> {id}");
        }
        assert_ne!(public_id(7), public_id(8));
    }

    #[test]
    fn nearest_riders_first_and_around_the_loop() {
        let rs: Vec<Presence> = [100.0, 5000.0, 900.0, 9950.0, 400.0].iter().enumerate().map(|(i, &d)| rider(i as u64, d)).collect();
        let ids = |v: Vec<Presence>| {
            let mut i: Vec<u64> = v.iter().map(|p| p.id).collect();
            i.sort();
            i
        };
        assert_eq!(ids(nearest(rs.iter(), 0.0, None, None, 2)), vec![0, 4]); // 100 m and 400 m ahead
        assert_eq!(ids(nearest(rs.iter(), 0.0, None, Some(10_000.0), 2)), vec![0, 3]); // 50 m behind, across the line
        assert_eq!(nearest(rs.iter(), 0.0, None, None, 10).len(), 5);
    }
}
