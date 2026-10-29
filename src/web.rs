//! Tiny HTTP server: embedded web UI + ride sessions.
//!   GET /                                      page
//!   GET /courses                               {trainer, courses: [summary], cities: [summary]}  (real climbs, open cities)
//!   GET /worlds                                [summary + relief + riders]   (open worlds)
//!   POST /worlds/new?length=<5|10|20>&relief=<flat|hills|mountains>   generate an open world, {id}
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
//!   POST /auth/signup, /auth/login  (name=&password=, form-encoded)   a Le Mollet account; POST /auth/logout[?all=1] signs out
//!   POST /auth/password  (password=&new=)   a new password, the other browsers signed out;  POST /auth/delete  (password=)   the account gone
//!   POST /admin/segments/reset?id=<id>&name=<segment>  wipe a leaderboard (moderate)
//!
//!   GET /emote?session=<n>&course=<id>&e=<rideon|wave|fire|laugh|party>[&to=<rider id>]  a bubble over my rider; a Ride On to someone
//!   GET /seek?session=<n>&d=<m>                demo mode: teleport the rider along the course
//!   GET /pause?session=<n>&on=<0|1>            freeze or resume a ride (menu open)
//!   GET /powerup?session=<n>                   use the road bonus the rider holds
//!   GET /event/list                            scheduled group rides and races, {now, events}
//!   POST /event/create?pid=&me=&kind=<group|race>&name=&track=&start=<unix>&laps=&pace=<W/kg>[&cats=PO][&club=1] (ranks, see event::TIERS)
//!                                              schedule one (manage_events, or the founder of one's club for a club ride)
//!   POST /event/join?id=&pid=&me=&wkg=;  /event/leave?id=&pid=;  /event/delete?id=&pid=
//!   GET /events?course=<id>&event=<id>&...    ride an event: held in the pen until the start, then a pace
//!                                              leader (group) or a live ranking and a finish line (race)
//!   GET /club?pid=<profile>                    the profile's club, members ranked by km this week (or null)
//!   POST /club/create?pid=&me=&name=&tag=&color=   found a club;  /club/join?pid=&me=&code=;  /club/leave?pid=
//!   GET /friend?pid=&me=[&country=]           the profile's handle (Name#1234), friends, requests in and out
//!   POST /friend/ask?pid=&h=<Name#1234>;  /friend/accept?pid=&h=;  /friend/remove?pid=&h=   (no, take back, unfriend)
//!   GET /friend/view?pid=&h=                   a friend's profile and rides (friends only)
//!   POST /friend/ride?pid=  (a ride as JSON);  POST /friend/unride?pid=&id=   the rides friends see
//!   GET /audio/<name>.mp3                      nature ambience loops (web/audio/CREDITS.md)
//!   GET /rider?pid= ; POST /rider?pid=&weight=<kg>&ftp=<W>   the profile as the server knows it (weight, FTP seen, rank); with
//!                                              weight and ftp, register or update it (docs/security.md)
//!   GET /admin/flags                           rides the fair-play checks flagged (moderate)
//!   GET /metrics                               Prometheus metrics, usage and performance (docs/metrics.md)
//!
//! Access (docs/rbac.md): in `Local` mode, whoever sits at the server's PC is admin without an account and LAN
//! guests ride as Riders; in `Cloud` mode, everything but the page and the sign-in needs an account.
//! Signing in sets an `lm_session` cookie; the account's role comes from the database (`le-mollet role`). Endpoints check `Perm`s.
//!
//! Power, per ride: the browser's own trainer (Web Bluetooth, `src=bt`), else the server's trainer (whose grade the
//! latest ride drives), else the demo slider. Pace partners ride every course and open world around the clock.
//!
//! Fair play (docs/security.md): the server simulates every ride and only takes power, cadence and heart rate
//! from the page. A ride belongs to the browser that opened it; weight and rank come from the server's own
//! record of the profile; each second of power is checked, and only believed rides reach leaderboards and race
//! results. Requests from other sites are refused (CSRF), whatever changes something is POST only, and profile ids
//! are never sent to other riders.

use le_mollet::Trainer;
use le_mollet::auth::{Identity, Perm, Role, Roles};
use le_mollet::club::{self, Clubs};
use le_mollet::friend::Friends;
use le_mollet::event::{Event, Events, Kind as EventKind, Phase, TIERS};
use le_mollet::peloton::{self, View};
use le_mollet::account;
use le_mollet::city::{City, Exit, Route, Turn};
use le_mollet::course::{Course, Point};
use le_mollet::metrics;
use le_mollet::db::Db;
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
static PAGE_GZ: std::sync::LazyLock<Arc<Vec<u8>>> = std::sync::LazyLock::new(|| Arc::new(gzip(PAGE.as_bytes())));
static PAGE_TAG: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    let mut h = Fnv::default();
    h.add(PAGE.as_bytes());
    h.hex()
});

/// FNV-1a, 64 bits: a fingerprint for cache tags, not a security hash.
struct Fnv(u64);
impl Default for Fnv {
    fn default() -> Self { Self(0xcbf2_9ce4_8422_2325) }
}
impl Fnv {
    fn add(&mut self, b: &[u8]) {
        for x in b { self.0 = (self.0 ^ *x as u64).wrapping_mul(0x0100_0000_01b3) }
    }
    fn hex(&self) -> String { format!("{:016x}", self.0) }
}

fn gzip(raw: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()); // as small as best on terrain, 4x faster
    e.write_all(raw).expect("gzip into memory");
    e.finish().expect("gzip into memory")
}
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
    /// Power over the rider's own weight, for the board next to the road.
    wkg: f32,
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
    /// Fingerprint of all its files, for caching (see `reply_static`); and its files gzipped, made on first ask.
    tag: std::sync::OnceLock<String>,
    gz: Mutex<HashMap<&'static str, Arc<Vec<u8>>>>,
}

impl Track {
    fn new(id: String, course: Course, terrain: Option<Vec<u8>>, relief: Option<Relief>) -> anyhow::Result<Self> {
        let json = serde_json::to_string(&course)?;
        Ok(Self { id, course, json, terrain, relief, city: None, buildings: None, district: HashMap::new(), ortho: HashMap::new(), riders: Mutex::new(HashMap::new()), tag: Default::default(), gz: Default::default() })
    }

    /// Changes when any of its files does: a cache may keep a URL carrying it for good.
    fn tag(&self) -> &str {
        self.tag.get_or_init(|| {
            let mut h = Fnv::default();
            h.add(self.json.as_bytes());
            for b in [&self.terrain, &self.buildings].into_iter().flatten() { h.add(b) }
            let mut files: Vec<(&str, &Vec<u8>)> = self.ortho.iter().map(|(k, v)| (k.as_str(), v)).chain(self.district.iter().map(|(k, v)| (*k, v))).collect();
            files.sort_by_key(|f| f.0);
            for (k, v) in files { h.add(k.as_bytes()); h.add(v) }
            h.hex()
        })
    }

    /// One of its files gzipped, once (`course`, `terrain`, `buildings`).
    fn gz(&self, key: &'static str, raw: &[u8]) -> Arc<Vec<u8>> {
        if let Some(z) = self.gz.lock().unwrap_or_else(|e| e.into_inner()).get(key) { return z.clone() }
        let z = Arc::new(tokio::task::block_in_place(|| gzip(raw))); // outside the lock, off the async threads: a big terrain takes a moment
        self.gz.lock().unwrap_or_else(|e| e.into_inner()).insert(key, z.clone());
        z
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
        // A loop's shape seen from above, 80 `[lat, lon]`, for the Free cards.
        let n = c.points.len();
        let outline: Option<Vec<[f64; 2]>> = (c.looped && n > 1).then(|| (0..80).map(|i| { let p = &c.points[i * (n - 1) / 79]; [p.lat, p.lon] }).collect());
        json!({
            "outline": outline,
            "id": self.id, "name": c.name, "country": c.country, "length": c.length(), "gain": c.elevation_gain().round(), "profile": profile,
            "terrain": self.terrain.is_some(), "looped": c.looped,
            "relief": self.relief.map(Relief::label), "riders": self.riders.lock().unwrap_or_else(|e| e.into_inner()).values().filter(|p| !p.bot).count(),
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
    /// Everything kept across restarts (src/db.rs); the structures below are its working copy.
    db: Db,
    boards: Mutex<Boards>,
    clubs: Mutex<Clubs>,
    friends: Mutex<Friends>,
    events: Mutex<Events>,
    /// Group rides whose pace leader is out on the road.
    beacons: Mutex<HashSet<String>>,
    /// Races whose peloton is on the road (src/peloton.rs).
    pelotons: Mutex<HashSet<String>>,
    /// The race radio's last line per event, and when it was said.
    radio: Mutex<HashMap<String, (String, u64)>>,
    /// Each event's field by distance, shared by its riders' frames (see `Standing`).
    standings: Mutex<HashMap<String, (std::time::Instant, Arc<Standing>)>>,
    mode: Mode,
    /// Signed-in browsers: session cookie -> who.
    sessions: Mutex<HashMap<String, Session>>,
    /// Failed logins by account name, and since when: a few tries, then a pause (no password guessing).
    login_fails: Mutex<HashMap<String, (u32, std::time::Instant)>>,
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
    /// Who opened each ride: only they may steer it (power, pause, turns...).
    owners: Mutex<HashMap<u64, Owner>>,
    started: std::time::Instant,
    /// Rate limits: calls left in the window and when it started, by "<what>:<client>" (docs/security.md).
    limits: Mutex<HashMap<String, (u32, std::time::Instant)>>,
    /// Open connections by client address, capped.
    conns: Mutex<HashMap<std::net::IpAddr, u32>>,
    /// Roles, reread from the database every few seconds (`le-mollet role` takes effect within that).
    roles: Mutex<(Roles, std::time::Instant)>,
    /// Last Ride On sent by each session: one every few seconds, so nobody floods an inbox.
    rideons: Mutex<HashMap<u64, std::time::Instant>>,
}

/// Connections at once: in all, and per client address.
const MAX_CONNS: usize = 4096;
const MAX_CONNS_PER_IP: u32 = 64;
/// A request's head must arrive within this; a ride frame must be taken within this, else the client is gone.
const READ_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// Roles are reread from the database after this.
const ROLES_TTL: Duration = Duration::from_secs(5);

/// A ride's owner: the browser's address and, signed in, the account.
type Owner = (Option<std::net::IpAddr>, Option<u64>);

/// Where the server runs, which sets who must sign in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// One person's PC (maybe open to the LAN): no account, the PC's own browser is admin.
    Local,
    /// A public server: an account required to ride.
    Cloud,
}

impl App {
    /// Load every `*.json` course in `dir` (the id is the file stem) and open today's world.
    pub fn new(dir: &str, trainer: Option<Trainer>, difficulty: f32, db: Db, mode: Mode) -> anyhow::Result<Self> {
        Roles::load(&db)?; // broken roles stop the server now, not at the first admin call
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
            boards: Mutex::new(Boards::load(&db)?),
            clubs: Mutex::new(Clubs::load(&db)?),
            friends: Mutex::new(Friends::load(&db)?),
            events: Mutex::new(Events::load(&db)?),
            beacons: Mutex::new(HashSet::new()),
            pelotons: Mutex::new(HashSet::new()),
            radio: Mutex::new(HashMap::new()),
            standings: Mutex::new(HashMap::new()),
            mode,
            // Signed-in browsers stay signed in across a restart.
            sessions: Mutex::new(db.all::<Session>("sessions")?.into_iter().filter(|(_, s)| s.expires > std::time::SystemTime::now()).collect()),
            login_fails: Mutex::new(HashMap::new()),
            demo_seek: Mutex::new(HashMap::new()),
            paused: Mutex::new(HashSet::new()),
            turns: Mutex::new(HashMap::new()),
            powerup: Mutex::new(HashSet::new()),
            inbox: Mutex::new(HashMap::new()),
            driver: AtomicU64::new(0),
            riders: Mutex::new(Riders::load(&db)?),
            owners: Mutex::new(HashMap::new()),
            started: std::time::Instant::now(),
            limits: Mutex::new(HashMap::new()),
            conns: Mutex::new(HashMap::new()),
            roles: Mutex::new((Roles::load(&db)?, std::time::Instant::now())),
            rideons: Mutex::new(HashMap::new()),
            db,
        })
    }

    pub fn course_count(&self) -> usize {
        self.tracks.read().unwrap().iter().filter(|t| t.relief.is_none()).count()
    }

    fn track(&self, id: &str) -> Option<Arc<Track>> {
        self.tracks.read().unwrap().iter().find(|t| t.id == id).cloned()
    }

    /// Who sent a request, from its session cookie and, in local mode, where from: the server's own PC is admin.
    /// Roles come from the database, reread every `ROLES_TTL`: `le-mollet role` takes effect within seconds.
    /// Never trusted in cloud mode: behind a reverse proxy every request comes from loopback.
    fn identify(&self, cookie: Option<&str>, peer: Option<std::net::IpAddr>) -> Identity {
        let owner = self.mode == Mode::Local && peer.is_some_and(|ip| ip.is_loopback());
        let mut who = match cookie.and_then(|c| self.sessions.lock().unwrap_or_else(|e| e.into_inner()).get(c).filter(|s| s.expires > std::time::SystemTime::now()).cloned()) {
            Some(s) => Identity { role: self.role_of(s.account), name: Some(s.name), account: Some(s.account) },
            None => Identity::ANONYMOUS,
        };
        if owner { who.role = Role::Admin }
        who
    }

    fn role_of(&self, account: u64) -> Role {
        let mut roles = self.roles.lock().unwrap_or_else(|e| e.into_inner());
        if roles.1.elapsed() > ROLES_TTL && let Ok(r) = Roles::load(&self.db) { *roles = (r, std::time::Instant::now()) }
        roles.0.role_of(account)
    }

    /// Whether `client` may do `what` again: `max` times per `window`. Old windows are dropped as they are met.
    fn allow(&self, what: &str, client: &str, max: u32, window: Duration) -> bool {
        let mut limits = self.limits.lock().unwrap_or_else(|e| e.into_inner());
        if limits.len() > 10_000 { limits.retain(|_, (_, t)| t.elapsed() < window) }
        let e = limits.entry(format!("{what}:{client}")).or_insert((max, std::time::Instant::now()));
        if e.1.elapsed() > window { *e = (max, std::time::Instant::now()) }
        if e.0 == 0 { return false }
        e.0 -= 1;
        true
    }

    /// `/metrics`: the registry (src/metrics.rs), plus what is read off the server's state now.
    fn metrics(&self) -> String {
        let now = unix_now();
        let mut g: Vec<metrics::Gauge> = vec![];
        let mut types: std::collections::BTreeMap<&str, f64> = Default::default();
        for t in self.tracks.read().unwrap().iter() {
            let riders = t.riders.lock().unwrap_or_else(|e| e.into_inner()).values().filter(|p| !p.bot).count();
            g.push(("le_mollet_riders_online", vec![("track", t.id.clone())], riders as f64));
            *types.entry(if t.city.is_some() { "city" } else if t.relief.is_some() { "world" } else if t.course.looped { "loop" } else { "climb" }).or_default() += 1.0;
        }
        g.extend(types.into_iter().map(|(k, n)| ("le_mollet_tracks", vec![("type", k.to_owned())], n)));
        {
            let events = self.events.lock().unwrap_or_else(|e| e.into_inner());
            for (kind, k) in [(EventKind::Race, "race"), (EventKind::Group, "group")] {
                let live: Vec<_> = events.events.iter().filter(|e| e.kind == kind && e.phase(now) != Phase::Over).collect();
                g.push(("le_mollet_events_upcoming", vec![("kind", k.into())], live.len() as f64));
                g.push(("le_mollet_event_entrants", vec![("kind", k.into())], live.iter().map(|e| e.entrants.len()).sum::<usize>() as f64));
            }
        }
        {
            let riders = self.riders.lock().unwrap_or_else(|e| e.into_inner());
            g.push(("le_mollet_riders_registered", vec![], riders.riders.len() as f64));
            g.push(("le_mollet_riders_flagged", vec![], riders.riders.values().filter(|r| !r.flags.is_empty()).count() as f64));
        }
        {
            let clubs = self.clubs.lock().unwrap_or_else(|e| e.into_inner());
            g.push(("le_mollet_clubs", vec![], clubs.clubs.len() as f64));
            g.push(("le_mollet_club_members", vec![], clubs.clubs.iter().map(|c| c.members.len()).sum::<usize>() as f64));
        }
        let signed_in = self.sessions.lock().unwrap_or_else(|e| e.into_inner()).values().filter(|s| s.expires > std::time::SystemTime::now()).count();
        g.push(("le_mollet_signed_in", vec![], signed_in as f64));
        g.push(("le_mollet_trainer_connected", vec![], self.trainer.is_some() as u8 as f64));
        g.push(("le_mollet_uptime_seconds", vec![], self.started.elapsed().as_secs_f64()));
        // Linux: resident pages in /proc/self/statm (4 KiB pages).
        if let Some(pages) = std::fs::read_to_string("/proc/self/statm").ok().and_then(|s| s.split_whitespace().nth(1)?.parse::<f64>().ok()) {
            g.push(("le_mollet_resident_memory_bytes", vec![], pages * 4096.0));
        }
        metrics::render(&g)
    }

    /// Free rides: real loops first, then the generated worlds.
    fn worlds_json(&self) -> String {
        let tracks = self.tracks.read().unwrap();
        let real = tracks.iter().filter(|t| t.relief.is_none() && t.city.is_none() && t.course.looped);
        json!(real.chain(tracks.iter().filter(|t| t.relief.is_some())).map(|t| t.summary()).collect::<Vec<_>>()).to_string()
    }

    /// Room for one more world: drops the oldest empty ones past the cap; false when every world is ridden.
    fn make_room_for_world(&self) -> bool {
        let mut tracks = self.tracks.write().unwrap();
        while tracks.iter().filter(|t| t.relief.is_some()).count() >= MAX_WORLDS {
            // Worlds are kept in creation order after the climbs: drop the oldest empty one.
            match tracks.iter().position(|t| t.relief.is_some() && t.riders.lock().unwrap_or_else(|e| e.into_inner()).values().all(|p| p.bot)) {
                Some(i) => drop(tracks.remove(i)),
                None => return false,
            }
        }
        true
    }

    /// End one sign-in session.
    fn end_session(&self, id: &str) -> usize {
        let gone = self.sessions.lock().unwrap_or_else(|e| e.into_inner()).remove(id).is_some();
        if let Err(e) = self.db.delete("sessions", id) { eprintln!("sessions: {e}") }
        gone as usize
    }

    /// End an account's sessions, all but `keep` (this browser's, after a password change). How many went.
    fn end_sessions(&self, account: u64, keep: Option<&str>) -> usize {
        let gone: Vec<String> = self.sessions.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|(k, s)| s.account == account && Some(k.as_str()) != keep).map(|(k, _)| k.clone()).collect();
        gone.iter().map(|k| self.end_session(k)).sum()
    }

    /// An account is gone: so are the profiles it owned, and their club seats and friendships.
    fn forget_profiles(&self, account: u64) {
        let pids: Vec<String> = {
            let mut riders = self.riders.lock().unwrap_or_else(|e| e.into_inner());
            let pids: Vec<String> = riders.riders.iter().filter(|(_, r)| r.owner == Some(account)).map(|(k, _)| k.clone()).collect();
            for p in &pids { riders.riders.remove(p); if let Err(e) = self.db.delete("riders", p) { eprintln!("riders: {e}") } }
            pids
        };
        if pids.is_empty() { return }
        {
            let mut clubs = self.clubs.lock().unwrap_or_else(|e| e.into_inner());
            for p in &pids { clubs.leave(p) }
            if let Err(e) = clubs.save(&self.db) { eprintln!("clubs: {e}") }
        }
        let mut friends = self.friends.lock().unwrap_or_else(|e| e.into_inner());
        for p in &pids {
            let handles: Vec<String> = friends.people.get(p).map(|me| me.friends.iter().chain(&me.asked).filter_map(|k| friends.people.get(k)).map(|o| o.handle()).collect()).unwrap_or_default();
            for h in handles { let _ = friends.remove(p, &h); }
            let others: Vec<String> = friends.sent(p);
            for h in others { let _ = friends.remove(p, &h); }
            friends.people.remove(p);
            if let Err(e) = self.db.delete("friends", p) { eprintln!("friends: {e}") }
        }
    }

    fn add_world(&self, track: Track) -> String {
        let id = track.id.clone();
        self.tracks.write().unwrap().push(Arc::new(track));
        id
    }
}

pub async fn serve(addr: &str, app: Arc<App>) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    spawn_pacers(&app);
    eprintln!("web UI on http://{addr}");
    let slots = Arc::new(tokio::sync::Semaphore::new(MAX_CONNS));
    loop {
        let (sock, peer) = listener.accept().await?;
        let Ok(slot) = slots.clone().try_acquire_owned() else { continue }; // full: the client retries
        let app = app.clone();
        // Per address too, so one client cannot hold every slot.
        {
            let mut conns = app.conns.lock().unwrap_or_else(|e| e.into_inner());
            let n = conns.entry(peer.ip()).or_insert(0);
            if *n >= MAX_CONNS_PER_IP { metrics::inc("le_mollet_rejected_total", &[("reason", "too_many_connections")]); continue }
            *n += 1;
        }
        tokio::spawn(async move {
            let _ = handle(sock, app.clone()).await; // client gone: nothing to do
            drop(slot);
            let mut conns = app.conns.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(n) = conns.get_mut(&peer.ip()) { *n -= 1; if *n == 0 { conns.remove(&peer.ip()); } }
        });
    }
}

/// Who the client is for rate limits and connection caps: the peer, or behind the reverse proxy of cloud mode
/// (every peer is loopback then) the address it forwards.
fn client_ip(req: &str, peer: Option<std::net::IpAddr>, mode: Mode) -> String {
    // The last address is the one our proxy appended; anything before it the client may have written itself
    // (a made-up one would dodge the budgets and the password lock). An address, or nothing.
    if mode == Mode::Cloud && peer.is_some_and(|p| p.is_loopback()) && let Some(ip) = header(req, "x-forwarded-for").and_then(|h| h.rsplit(',').next()).and_then(|s| s.trim().parse::<std::net::IpAddr>().ok()) { return ip.to_string() }
    peer.map_or_else(|| "?".into(), |p| p.to_string())
}

/// The Host the browser asked for must be one of ours: a name a page elsewhere could point at this server (DNS
/// rebinding) is refused, since in local mode the server's own PC is admin. IP literals, localhost, mDNS names
/// and the public URL's host are ours.
fn host_ok(req: &str) -> bool {
    let Some(host) = header(req, "host") else { return true }; // HTTP/1.0 tools
    let name = host.trim_start_matches('[').split([']', ':']).next().unwrap_or("").to_ascii_lowercase();
    name.is_empty() || name == "localhost" || name.ends_with(".local") || name.ends_with(".localhost") || name.parse::<std::net::IpAddr>().is_ok()
        || std::env::var("LE_MOLLET_PUBLIC_URL").ok().and_then(|u| u.split("://").nth(1).map(|h| h.split(['/', ':']).next().unwrap_or("").to_ascii_lowercase())).is_some_and(|h| h == name)
        || std::env::var("LE_MOLLET_HOSTS").is_ok_and(|hs| hs.split(',').any(|h| h.trim().eq_ignore_ascii_case(&name)))
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

/// Calls that change something: POST only (`/rider` with a weight too). Steering one's own ride stays GET: it
/// needs the ride's session, which only its page knows.
const POST_ONLY: [&str; 17] = ["/worlds/new", "/event/create", "/event/join", "/event/leave", "/event/delete", "/club/create", "/club/join", "/club/leave",
    "/friend/ask", "/friend/accept", "/friend/remove", "/friend/ride", "/friend/unride", "/auth/logout", "/auth/password", "/auth/delete", "/admin/segments/reset"];

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
        wkg: 0.0,
        pos: None,
        bot: false,
        pid: param(query, "pid").filter(|p| valid_pid(p)).unwrap_or_default().to_owned(),
        ridden: 0.0,
        event: None,
    }
}

async fn handle(mut sock: TcpStream, app: Arc<App>) -> std::io::Result<()> {
    let mut buf = [0u8; 16384]; // an imported workout travels in the query
    // The whole request within READ_TIMEOUT: a client that connects and says nothing holds no task.
    let read = tokio::time::timeout(READ_TIMEOUT, async {
        let mut n = sock.read(&mut buf).await?;
        // A form (sign-in) may come after the head in a later packet: read up to its Content-Length.
        while let Some(end) = buf[..n].windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..end]);
            let want = header(&head, "content-length").and_then(|l| l.parse::<usize>().ok()).unwrap_or(0);
            if n >= (end + 4 + want).min(buf.len()) { break }
            match sock.read(&mut buf[n..]).await? { 0 => break, k => n += k }
        }
        Ok::<_, std::io::Error>(n)
    }).await;
    let Ok(Ok(n)) = read else { metrics::inc("le_mollet_rejected_total", &[("reason", "slow_request")]); return Ok(()) };
    let req = String::from_utf8_lossy(&buf[..n]);
    if !host_ok(&req) {
        metrics::inc("le_mollet_rejected_total", &[("reason", "host")]);
        return reply(&mut sock, "421 Misdirected Request", "text/plain", b"this server is not reached by that name (LE_MOLLET_PUBLIC_URL or LE_MOLLET_HOSTS names it)").await;
    }
    let target = req.split_whitespace().nth(1).unwrap_or("/");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let route = route_label(path);
    metrics::inc("le_mollet_http_requests_total", &[("route", route)]);
    let _timed = (route != "/events").then(|| Timed(route, std::time::Instant::now()));
    if path == "/metrics" {
        // Open at home; on a public server only with LE_MOLLET_METRICS_TOKEN as a bearer token.
        let token = std::env::var("LE_MOLLET_METRICS_TOKEN").ok().filter(|t| !t.is_empty());
        let ok = match &token {
            Some(t) => header(&req, "authorization").is_some_and(|h| h.strip_prefix("Bearer ").is_some_and(|b| same_secret(b, t))),
            None => app.mode == Mode::Local,
        };
        if !ok { return reply(&mut sock, "404 Not Found", "text/plain", b"not found").await }
        return reply(&mut sock, "200 OK", "text/plain; version=0.0.4", app.metrics().as_bytes()).await;
    }
    // What changes something is asked with POST: a link, a prefetch or an old browser without `Sec-Fetch-Site` cannot do it.
    if (POST_ONLY.contains(&path) || path == "/rider" && param(query, "weight").is_some()) && !req.starts_with("POST ") {
        return reply(&mut sock, "405 Method Not Allowed", "text/plain", b"POST").await;
    }
    let track = param(query, "id").or(param(query, "course")).and_then(|id| app.track(id));
    let session: Option<u64> = param(query, "session").and_then(|s| s.parse().ok());
    let peer = sock.peer_addr().ok().map(|a| a.ip());
    let who = app.identify(cookie(&req, SESSION_COOKIE), peer);
    let client = client_ip(&req, peer, app.mode);
    // What costs the server something (a password hash, a world, a row that stays) is counted per client.
    let budget = match path {
        "/auth/signup" | "/auth/password" | "/auth/delete" => Some(("auth", 10, 3600)),
        "/auth/login" => Some(("auth", 10, 3600)),
        "/worlds/new" => Some(("world", 3, 600)),
        "/club/create" | "/event/create" => Some(("create", 10, 3600)),
        "/friend/ask" => Some(("friend", 30, 3600)),
        "/friend/ride" => Some(("ride", 300, 3600)), // a first share sends the history, one ride a call
        "/rider" if param(query, "weight").is_some() => Some(("rider", 30, 3600)),
        _ => None,
    };
    if let Some((what, max, secs)) = budget && !app.allow(what, &client, max, Duration::from_secs(secs)) {
        metrics::inc("le_mollet_rejected_total", &[("reason", "rate_limit")]);
        return reply(&mut sock, "429 Too Many Requests", "text/plain", b"too many requests: try again later").await;
    }
    // Cloud: only the page and the way to sign in are open to strangers.
    // The track files are open data (OSM, IGN...) and the same for everyone: public, so a CDN can keep them.
    let open = matches!(path, "/" | "/whoami" | "/course" | "/terrain" | "/buildings" | "/ortho" | "/district") || path.starts_with("/auth/") || path.starts_with("/audio/");
    // Another site's page must not drive this one in the visitor's browser (CSRF): every call but a visit is refused
    // when the browser says it comes from elsewhere.
    if !(path == "/" || path.starts_with("/audio/")) && cross_site(&req) {
        metrics::inc("le_mollet_rejected_total", &[("reason", "cross_site")]);
        return reply(&mut sock, "403 Forbidden", "text/plain", b"cross-site request").await;
    }
    // A ride is steered only by whoever opened it: opening the stream claims the session, the other calls need it.
    let owner: Owner = (peer, who.account);
    if matches!(path, "/power" | "/seek" | "/turn" | "/powerup" | "/pause" | "/emote" | "/events") && let Some(sid) = session {
        let mine = {
            let owners = app.owners.lock().unwrap_or_else(|e| e.into_inner());
            // The stream only claims the ride once it is sure to open (below): a claim left by a refused call would stay for good.
            if path == "/events" { owners.get(&sid).is_none_or(|o| *o == owner) } else { owners.get(&sid) == Some(&owner) }
        };
        if !mine {
            metrics::inc("le_mollet_rejected_total", &[("reason", "not_your_ride")]);
            return reply(&mut sock, "403 Forbidden", "text/plain", b"not your ride").await;
        }
    }
    // A profile id in the query: in cloud mode, only its owner may use it.
    let pid = param(query, "pid").filter(|p| valid_pid(p));
    if pid.is_some_and(|p| !app.riders.lock().unwrap_or_else(|e| e.into_inner()).may_use(p, who.account)) {
        metrics::inc("le_mollet_rejected_total", &[("reason", "profile_owner")]);
        return reply(&mut sock, "403 Forbidden", "text/plain", b"this profile belongs to another account").await;
    }
    if app.mode == Mode::Cloud && who.account.is_none() && !open {
        return reply(&mut sock, "401 Unauthorized", "text/plain", b"sign in first").await;
    }
    // Cloud: a profile the server does not know yet cannot act (the page registers it on `/rider` first).
    if app.mode == Mode::Cloud && (path.starts_with("/club") || path.starts_with("/event/") || path.starts_with("/friend")) && pid.is_some_and(|p| !app.riders.lock().unwrap_or_else(|e| e.into_inner()).riders.contains_key(p)) {
        return reply(&mut sock, "403 Forbidden", "text/plain", b"unknown rider").await;
    }
    // Cloud: names come from the account, so nobody appears as someone else.
    let me_name = who.name.clone().filter(|_| app.mode == Mode::Cloud);
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
        // The page: always asked again (a new version shows at once), gzipped, 304 when unchanged.
        ("/", _) => reply_static(&mut sock, &req, "", "text/html; charset=utf-8", PAGE.as_bytes(), Some(PAGE_GZ.clone()), &PAGE_TAG).await,
        (p, _) if p.starts_with("/audio/") => match AUDIO.iter().find(|a| a.0 == p) {
            Some((_, bytes)) => reply_static(&mut sock, &req, "", "audio/mpeg", bytes, None, &PAGE_TAG).await, // they ship with the page
            None => reply(&mut sock, "404 Not Found", "text/plain", b"not found").await,
        },
        ("/workouts", _) => reply(&mut sock, "200 OK", "application/json", app.workouts_json.as_bytes()).await,
        ("/courses", _) => reply(&mut sock, "200 OK", "application/json", app.list_json.as_bytes()).await,
        ("/whoami", _) => {
            // `devices`: the account's live sessions, for its settings.
            let devices = who.account.map(|id| app.sessions.lock().unwrap_or_else(|e| e.into_inner()).values().filter(|s| s.account == id && s.expires > std::time::SystemTime::now()).count());
            let body = json!({ "role": who.role, "name": who.name, "account": who.account, "perms": who.role.perms(), "mode": app.mode, "devices": devices }).to_string();
            reply(&mut sock, "200 OK", "application/json", body.as_bytes()).await
        }
        ("/auth/signup" | "/auth/login", _) if req.starts_with("POST ") => {
            let form = req.split_once("\r\n\r\n").map_or("", |(_, b)| b);
            let (name, password) = (param(form, "name").map(decode).unwrap_or_default(), param(form, "password").map(decode).unwrap_or_default());
            let name = name.trim().to_owned();
            // Five wrong passwords for a name from one client: that client waits 15 minutes for that name (the
            // per-client budget above slows the rest).
            let key = format!("{client}:{}", name.to_ascii_lowercase());
            let locked = {
                let mut fails = app.login_fails.lock().unwrap_or_else(|e| e.into_inner());
                fails.retain(|_, (_, t)| t.elapsed().as_secs() < 900);
                fails.get(&key).is_some_and(|(n, _)| *n >= 5)
            };
            let res = if locked { Err("too many tries: wait 15 minutes") } else if path == "/auth/signup" {
                tokio::task::block_in_place(|| account::signup(&app.db, &name, &password, unix_now())) // the hash takes a while, on purpose
            } else {
                tokio::task::block_in_place(|| account::login(&app.db, &name, &password))
            };
            match res {
                Ok(a) => {
                    metrics::inc("le_mollet_signins_total", &[("result", if path == "/auth/signup" { "signup" } else { "ok" })]);
                    app.login_fails.lock().unwrap_or_else(|e| e.into_inner()).remove(&key);
                    let id = format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple());
                    let s = Session { account: a.id, name: a.name.clone(), expires: std::time::SystemTime::now() + SESSION_TTL };
                    if let Err(e) = app.db.put("sessions", &id, &s) { eprintln!("sessions: {e}") }
                    {
                        // Expired sessions go; an account keeps its 20 most recent (older browsers sign out).
                        let mut sessions = app.sessions.lock().unwrap_or_else(|e| e.into_inner());
                        let now = std::time::SystemTime::now();
                        let gone: Vec<String> = sessions.iter().filter(|(_, s)| s.expires <= now).map(|(k, _)| k.clone()).collect();
                        let mut mine: Vec<(String, std::time::SystemTime)> = sessions.iter().filter(|(_, s)| s.account == a.id).map(|(k, s)| (k.clone(), s.expires)).collect();
                        mine.sort_by_key(|m| m.1);
                        let extra: Vec<String> = mine.iter().rev().skip(19).map(|m| m.0.clone()).collect();
                        for k in gone.iter().chain(&extra) { sessions.remove(k); if let Err(e) = app.db.delete("sessions", k) { eprintln!("sessions: {e}") } }
                        sessions.insert(id.clone(), s);
                    }
                    // Cloud sits behind https; LE_MOLLET_PUBLIC_URL=http://... says otherwise (a test on a LAN).
                    let secure = if app.mode == Mode::Cloud && !std::env::var("LE_MOLLET_PUBLIC_URL").is_ok_and(|u| u.starts_with("http://")) || std::env::var("LE_MOLLET_PUBLIC_URL").is_ok_and(|u| u.starts_with("https://")) { "; Secure" } else { "" };
                    let cookie = format!("{SESSION_COOKIE}={id}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{secure}", SESSION_TTL.as_secs());
                    let body = json!({ "name": a.name, "account": a.id }).to_string();
                    let head = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nSet-Cookie: {cookie}\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    metrics::inc("le_mollet_http_responses_total", &[("code", "200")]);
                    sock.write_all(head.as_bytes()).await?;
                    sock.write_all(body.as_bytes()).await
                }
                Err(e) => {
                    if e == "wrong name or password" {
                        let mut fails = app.login_fails.lock().unwrap_or_else(|e| e.into_inner());
                        let f = fails.entry(key).or_insert((0, std::time::Instant::now()));
                        if f.1.elapsed().as_secs() >= 900 { *f = (0, std::time::Instant::now()) }
                        f.0 += 1;
                    }
                    metrics::inc("le_mollet_signins_total", &[("result", if locked { "locked" } else { "refused" })]);
                    reply(&mut sock, if locked { "429 Too Many Requests" } else { "400 Bad Request" }, "text/plain", e.as_bytes()).await
                }
            }
        }
        ("/auth/logout", _) => {
            // `all=1`: every browser of the account, not only this one.
            match (cookie(&req, SESSION_COOKIE), who.account, param(query, "all")) {
                (_, Some(id), Some("1")) => app.end_sessions(id, None),
                (Some(c), ..) => app.end_session(c),
                _ => 0,
            };
            redirect(&mut sock, "/", Some(CLEAR_COOKIE)).await
        }
        // The account's own settings: a new password (the other browsers sign out), or the account gone with its
        // sessions and the profiles it owns.
        ("/auth/password" | "/auth/delete", _) if req.starts_with("POST ") => {
            let Some((id, name)) = who.account.zip(who.name.clone()) else { return reply(&mut sock, "401 Unauthorized", "text/plain", b"sign in first").await };
            let form = req.split_once("\r\n\r\n").map_or("", |(_, b)| b);
            let (password, new) = (param(form, "password").map(decode).unwrap_or_default(), param(form, "new").map(decode).unwrap_or_default());
            let res = tokio::task::block_in_place(|| if path == "/auth/password" { account::set_password(&app.db, &name, &password, &new) } else { account::delete(&app.db, &name, &password) });
            match res {
                Ok(_) if path == "/auth/password" => {
                    app.end_sessions(id, cookie(&req, SESSION_COOKIE));
                    reply(&mut sock, "204 No Content", "text/plain", b"").await
                }
                Ok(_) => {
                    app.end_sessions(id, None);
                    app.forget_profiles(id);
                    eprintln!("account {name} deleted itself");
                    redirect(&mut sock, "/", Some(CLEAR_COOKIE)).await
                }
                Err(e) => reply(&mut sock, "400 Bad Request", "text/plain", e.as_bytes()).await,
            }
        }
        ("/admin/segments/reset", Some(t)) => {
            let name = param(query, "name").map(decode).unwrap_or_default();
            let done = {
                let mut boards = app.boards.lock().unwrap_or_else(|e| e.into_inner());
                let done = boards.reset(&format!("{}:{name}", t.id));
                if done && let Err(e) = boards.save(&app.db, &format!("{}:{name}", t.id)) { eprintln!("leaderboards: {e}") }
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
                let mut riders = app.riders.lock().unwrap_or_else(|e| e.into_inner());
                let res = match (num("weight"), num("ftp")) {
                    (Some(_), Some(_)) if who.account.is_some() && !riders.riders.contains_key(pid) && riders.riders.values().filter(|r| r.owner == who.account).count() >= 20 => Err("too many profiles on this account"),
                    (Some(w), Some(f)) => riders.register(pid, who.account, w, f, now).cloned(),
                    _ => riders.riders.get(pid).cloned().ok_or("no such rider"),
                };
                let view = |r: &rider::Rider| json!({ "weight": r.weight, "ftp": r.ftp, "seen_ftp": r.seen_ftp(now), "rank": r.rank(now).to_string(), "wkg": r.wkg(now), "flags": r.flags.len() });
                match res {
                    Ok(r) => {
                        if num("weight").is_some() && let Err(e) = riders.save(&app.db, pid) { eprintln!("riders: {e}") }
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
                let riders = app.riders.lock().unwrap_or_else(|e| e.into_inner());
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
                let mut riders = t.riders.lock().unwrap_or_else(|e| e.into_inner());
                let now = std::time::Instant::now();
                let Some(me) = riders.get_mut(&sid) else { return false };
                (me.emote, me.emote_at) = (Some(e), Some(now));
                metrics::inc("le_mollet_emotes_total", &[("e", e)]);
                let from = me.name.clone();
                if e == "rideon" && let Some(o) = to.and_then(|to| riders.values_mut().find(|p| p.id == to)) {
                    // One Ride On every 5 s per sender, ten waiting at most per recipient: no flooding a screen.
                    let fresh = { let mut r = app.rideons.lock().unwrap_or_else(|e| e.into_inner()); let ok = r.get(&sid).is_none_or(|t| t.elapsed().as_secs() >= 5); if ok { r.insert(sid, now); } ok };
                    if o.bot { (o.emote, o.emote_at) = (Some("wave"), Some(now)) } // pace partners wave back
                    else if fresh { let mut inbox = app.inbox.lock().unwrap_or_else(|e| e.into_inner()); let q = inbox.entry(o.sid).or_default(); if q.len() < 10 { q.push(Notice { from, e }) } }
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
                let (now, boards) = (unix_now(), app.boards.lock().unwrap_or_else(|e| e.into_inner()));
                let list: Vec<_> = defs.iter().map(|(name, kind)| {
                    let (all, today) = boards.top(&format!("{}:{name}", t.id), 10, now);
                    let public = |l: Vec<Entry>| l.into_iter().map(|e| json!({ "name": e.name, "color": e.color, "time": e.time, "date": e.date })).collect::<Vec<_>>();
                    json!({ "name": name, "kind": kind, "all": public(all), "today": public(today) })
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
            if !app.make_room_for_world() {
                return reply(&mut sock, "429 Too Many Requests", "text/plain", b"every open world is being ridden: join one, or try again later").await;
            }
            let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
            let name = format!("Boucle {length} km {} n°{}", relief.label().to_lowercase(), seed % 1000);
            match tokio::task::spawn_blocking(move || world_track(seed, length, relief, name)).await {
                Ok(Ok(t)) => {
                    let id = app.add_world(t);
                    metrics::inc("le_mollet_worlds_generated_total", &[]);
                    reply(&mut sock, "200 OK", "application/json", json!({ "id": id }).to_string().as_bytes()).await
                }
                _ => reply(&mut sock, "500 Internal Server Error", "text/plain", b"world generation failed").await,
            }
        }
        // A track's files: its tag as ETag, gzipped, cached for good when the URL carries the tag (`&v=`).
        ("/course", Some(t)) => reply_static(&mut sock, &req, query, "application/json", t.json.as_bytes(), Some(t.gz("course", t.json.as_bytes())), t.tag()).await,
        ("/terrain", Some(t)) if t.terrain.is_some() => {
            let raw = t.terrain.as_deref().unwrap();
            reply_static(&mut sock, &req, query, "application/octet-stream", raw, Some(t.gz("terrain", raw)), t.tag()).await
        }
        ("/buildings", Some(t)) if t.buildings.is_some() => {
            let raw = t.buildings.as_deref().unwrap();
            reply_static(&mut sock, &req, query, "application/octet-stream", raw, Some(t.gz("buildings", raw)), t.tag()).await
        }
        ("/ortho", Some(t)) => match param(query, "f").and_then(|f| t.ortho.get(f)) {
            Some(b) => reply_static(&mut sock, &req, query, if param(query, "f").is_some_and(|f| f.ends_with(".json")) { "application/json" } else { "image/jpeg" }, b, None, t.tag()).await,
            None => reply(&mut sock, "404 Not Found", "text/plain", b"no such ortho file").await,
        },
        ("/district", Some(t)) => match param(query, "f").and_then(|f| t.district.get(f)) {
            Some(b) => {
                let ctype = match param(query, "f") { Some("ortho.jpg") => "image/jpeg", Some(f) if f.ends_with(".json") => "application/json", _ => "application/octet-stream" };
                reply_static(&mut sock, &req, query, ctype, b, None, t.tag()).await
            }
            None => reply(&mut sock, "404 Not Found", "text/plain", b"no such district file").await,
        },
        ("/power", _) => {
            let w = param(query, "w").and_then(|w| w.parse::<f32>().ok()).filter(|w| (0.0..=2000.0).contains(w));
            match (session, w) {
                (Some(s), Some(w)) => {
                    let num = |k: &str, max: f32| param(query, k).and_then(|v| v.parse::<f32>().ok()).filter(|v| (0.0..=max).contains(v));
                    let sensors = Sensors { w, cadence: num("cad", 250.0), hr: num("hr", 250.0), live: param(query, "src") == Some("bt") };
                    app.sensors.lock().unwrap_or_else(|e| e.into_inner()).insert(s, sensors);
                    reply(&mut sock, "204 No Content", "text/plain", b"").await
                }
                _ => reply(&mut sock, "400 Bad Request", "text/plain", b"need session and w in 0..=2000").await,
            }
        }
        ("/seek", _) if app.trainer.is_none() => {
            match (session, param(query, "d").and_then(|d| d.parse::<f32>().ok()).filter(|d| d.is_finite() && *d >= 0.0)) {
                (Some(s), Some(d)) => {
                    app.demo_seek.lock().unwrap_or_else(|e| e.into_inner()).insert(s, d);
                    reply(&mut sock, "204 No Content", "text/plain", b"").await
                }
                _ => reply(&mut sock, "400 Bad Request", "text/plain", b"need session and d >= 0").await,
            }
        }
        ("/turn", _) => match (session, param(query, "dir").and_then(Turn::parse)) {
            (Some(s), Some(t)) => {
                app.turns.lock().unwrap_or_else(|e| e.into_inner()).insert(s, t);
                reply(&mut sock, "204 No Content", "text/plain", b"").await
            }
            _ => reply(&mut sock, "400 Bad Request", "text/plain", b"need session and dir=left|straight|right").await,
        },
        ("/powerup", _) => match session {
            Some(s) => {
                app.powerup.lock().unwrap_or_else(|e| e.into_inner()).insert(s);
                metrics::inc("le_mollet_powerups_total", &[]);
                reply(&mut sock, "204 No Content", "text/plain", b"").await
            }
            None => reply(&mut sock, "400 Bad Request", "text/plain", b"need session").await,
        },
        (p, _) if p.starts_with("/event/") => {
            let text = |k| param(query, k).map(decode).unwrap_or_default().chars().filter(|c| !c.is_control()).take(40).collect::<String>();
            let now = unix_now();
            let pid = pid.map(str::to_owned);
            let res: Result<serde_json::Value, &str> = (|| {
                let mut events = app.events.lock().unwrap_or_else(|e| e.into_inner());
                // Races by rank on every half hour and group rides on the quarter hours between, on the short courses
                // in turn (loops, climbs up to 15 km).
                let short: Vec<_> = app.tracks.read().unwrap().iter().filter(|t| t.city.is_none() && t.relief.is_none() && (t.course.looped || t.course.length() <= 15_000.0))
                    .map(|t| (t.id.clone(), t.course.name.clone(), t.course.length(), t.course.looped)).collect();
                if events.schedule(now, &short) {
                    for e in events.events.iter_mut() { peloton::enroll(e, peloton::FIELD) }
                    if let Err(e) = events.save(&app.db) { eprintln!("events: {e}") }
                }
                let changed = match p {
                    "/event/list" => return Ok(json!({ "now": now, "events": events_for(&events.current(now), pid.as_deref()) })),
                    "/event/create" => {
                        let pid = pid.clone().ok_or("need pid")?;
                        // A club ride: the founder of the profile's club may schedule it; anything else needs manage_events.
                        let club = (param(query, "club") == Some("1")).then(|| app.clubs.lock().unwrap_or_else(|e| e.into_inner()).of(&pid).filter(|c| c.members[0].pid == pid).map(|c| c.tag.clone())).flatten();
                        if !who.can(Perm::ManageEvents) && club.is_none() { return Err("needs manage_events, or to found a club for a club ride") }
                        let kind = match param(query, "kind") { Some("race") => EventKind::Race, _ => EventKind::Group };
                        let track = app.track(param(query, "track").unwrap_or("")).filter(|t| t.city.is_none()).ok_or("no such course")?;
                        let start = param(query, "start").and_then(|s| s.parse::<u64>().ok()).filter(|&s| s.saturating_add(60) >= now && s < now + 60 * 86_400).ok_or("start: a time in the next 60 days")?;
                        let name = text("name");
                        metrics::inc("le_mollet_events_created_total", &[("kind", if kind == EventKind::Race { "race" } else { "group" }), ("by", if club.is_some() { "club" } else { "organizer" })]);
                        events.events.push(Event {
                            id: uuid::Uuid::new_v4().simple().to_string()[..8].into(),
                            kind,
                            name: if name.trim().is_empty() { track.course.name.clone() } else { name },
                            track: track.id.clone(),
                            start,
                            laps: if track.course.looped { param(query, "laps").and_then(|l| l.parse().ok()).unwrap_or(3u32).clamp(1, 50) } else { 1 },
                            pace: param(query, "pace").and_then(|p| p.parse().ok()).unwrap_or(2.5f32).clamp(1.0, 5.0),
                            keep: 0,
                            cats: { let mut c: Vec<char> = text("cats").chars().filter(|c| TIERS.iter().any(|t| t.0 == *c)).collect(); c.sort(); c.dedup(); c.into_iter().collect() },
                            club,
                            by: pid,
                            by_name: me_name.clone().unwrap_or_else(|| text("me")),
                            entrants: vec![],
                            results: vec![],
                        });
                        if let Some(e) = events.events.last_mut() { peloton::enroll(e, peloton::FIELD) }
                        true
                    }
                    "/event/join" => {
                        // The rank the server knows, never the page's word.
                        let wkg = app.riders.lock().unwrap_or_else(|e| e.into_inner()).riders.get(pid.as_deref().unwrap_or("")).map(|r| r.wkg(now)).ok_or("unknown rider")?;
                        if events.get(param(query, "id").unwrap_or("")).is_some_and(|e| e.entrants.len() >= 500) { return Err("the event is full") }
                        let e = events.join(param(query, "id").unwrap_or(""), pid.as_deref().ok_or("need pid")?, &me_name.clone().unwrap_or_else(|| text("me")), wkg, now)?;
                        metrics::inc("le_mollet_event_signups_total", &[("kind", if e.kind == EventKind::Race { "race" } else { "group" })]);
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
                if changed && let Err(e) = events.save(&app.db) { eprintln!("events: {e}") }
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
            let me = me_name.clone().unwrap_or_else(|| text("me"));
            let res = {
                let mut clubs = app.clubs.lock().unwrap_or_else(|e| e.into_inner());
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
                if res.is_ok() && p != "/club" {
                    metrics::inc("le_mollet_club_actions_total", &[("action", p.trim_start_matches("/club/"))]);
                    if let Err(e) = clubs.save(&app.db) { eprintln!("clubs: {e}") }
                }
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
                    // get_mut, not v["members"]: indexing null would turn "no club" into an object
                    for m in v.get_mut("members").and_then(|m| m.as_array_mut()).into_iter().flatten() { // no profile ids for the others
                        m["me"] = json!(m["pid"].as_str() == Some(pid));
                        m.as_object_mut().map(|o| o.remove("pid"));
                    }
                    reply(&mut sock, "200 OK", "application/json", v.to_string().as_bytes()).await
                }
                Err(e) => reply(&mut sock, "400 Bad Request", "text/plain", e.as_bytes()).await,
            }
        }
        (p, _) if p == "/friend" || p.starts_with("/friend/") => {
            let Some(pid) = pid else {
                return reply(&mut sock, "400 Bad Request", "text/plain", b"need pid").await;
            };
            let handle = param(query, "h").map(decode).unwrap_or_default();
            let res: Result<Vec<String>, &str> = {
                let mut friends = app.friends.lock().unwrap_or_else(|e| e.into_inner());
                let res = match p {
                    "/friend" => {
                        let me = me_name.clone().unwrap_or_else(|| param(query, "me").map(decode).unwrap_or_default());
                        let country = param(query, "country").filter(|c| c.len() == 2 && c.bytes().all(|b| b.is_ascii_uppercase())).map(str::to_owned);
                        let was = friends.people.get(pid).map(|p| (p.handle(), p.country.clone()));
                        let p = friends.me(pid, &me, country, (uuid::Uuid::new_v4().as_u128() % 10_000) as u16);
                        Ok(if was == Some((p.handle(), p.country.clone())) { vec![] } else { vec![pid.to_owned()] })
                    }
                    "/friend/ask" => friends.ask(pid, &handle).map(|o| vec![pid.to_owned(), o]),
                    "/friend/accept" => friends.accept(pid, &handle).map(|o| vec![pid.to_owned(), o]),
                    "/friend/remove" => friends.remove(pid, &handle).map(|o| vec![pid.to_owned(), o]),
                    "/friend/view" => friends.view(pid, &handle).map(|_| vec![]).ok_or("not your friend"),
                    "/friend/ride" if req.starts_with("POST ") => {
                        let body = req.split_once("\r\n\r\n").map_or("", |(_, b)| b);
                        serde_json::from_str::<serde_json::Value>(body).map_err(|_| "ride: bad JSON")
                            .and_then(|r| if r.is_object() { Ok(r) } else { Err("ride: not an object") })
                            .and_then(|r| friends.add_ride(&app.db, pid, r)).map(|_| vec![pid.to_owned()])
                    }
                    "/friend/unride" => { friends.drop_ride(&app.db, pid, &param(query, "id").map(decode).unwrap_or_default()); Ok(vec![pid.to_owned()]) }
                    _ => Err("not found"),
                };
                if let Ok(changed) = &res && let Err(e) = friends.save(&app.db, &changed.iter().map(String::as_str).collect::<Vec<_>>()) { eprintln!("friends: {e}") }
                res
            };
            if let Err(e) = res {
                return reply(&mut sock, "400 Bad Request", "text/plain", e.as_bytes()).await;
            }
            // What the page shows: never a profile id, only handles.
            let body = {
                let friends = app.friends.lock().unwrap_or_else(|e| e.into_inner());
                let riders = app.riders.lock().unwrap_or_else(|e| e.into_inner());
                let now = unix_now();
                let card = |k: &str| friends.people.get(k).map(|p| {
                    let r = riders.riders.get(k);
                    json!({ "handle": p.handle(), "name": p.name, "country": p.country, "last": p.kept.first().map(|r| &r.1), "rides": p.kept.len(),
                        "weight": r.map(|r| r.weight), "ftp": r.map(|r| r.ftp), "rank": r.map(|r| r.rank(now).to_string()) })
                });
                match (p, friends.people.get(pid)) {
                    ("/friend/view", _) => friends.view(pid, &handle).map(|(k, _)| { let mut v = card(&k).unwrap(); v["rides"] = json!(Friends::rides(&app.db, &k)); v }).unwrap_or_default(),
                    (_, Some(me)) => json!({ "handle": me.handle(), "friends": me.friends.iter().filter_map(|k| card(k)).collect::<Vec<_>>(),
                        "asked": me.asked.iter().filter_map(|k| card(k)).collect::<Vec<_>>(), "sent": friends.sent(pid) }),
                    _ => json!(null),
                }
            };
            reply(&mut sock, "200 OK", "application/json", body.to_string().as_bytes()).await
        }
        ("/pause", _) => match (session, param(query, "on")) {
            (Some(s), Some(on)) => {
                {
                    let mut p = app.paused.lock().unwrap_or_else(|e| e.into_inner());
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
            // Claim the ride for this browser now that the stream opens; the entry goes when it ends (below).
            let claimed = {
                let mut owners = app.owners.lock().unwrap_or_else(|e| e.into_inner());
                let mine = owners.get(&sid).is_none_or(|o| *o == owner);
                if mine { owners.insert(sid, owner); }
                mine
            };
            if !claimed { return reply(&mut sock, "403 Forbidden", "text/plain", b"not your ride").await }
            // Weight and FTP from the server's record of the profile, never from the query (docs/security.md).
            // A workout, alone on a flat road, races nobody: the page's mass does.
            let known = pid.filter(|_| track.is_some()).and_then(|p| app.riders.lock().unwrap_or_else(|e| e.into_inner()).riders.get(p).cloned());
            let asked = param(query, "mass").and_then(|m| m.parse::<f32>().ok()).filter(|m| (30.0..=250.0).contains(m));
            let mass = known.as_ref().map(|r| r.weight + rider::BIKE_KG).or(asked);
            let mut check = Check::new(known.as_ref().map(|r| r.ftp_now(unix_now())), mass.unwrap_or(83.0) - rider::BIKE_KG);
            sock.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n{SAFE_HEADERS}\r\n").as_bytes()).await?;
            app.driver.store(sid, Ordering::Relaxed);
            let plan = workout.zip(ftp).filter(|_| track.is_none());
            let game = ftp.filter(|_| track.is_some() && param(query, "mode") == Some("zombie")).map(Game::new);
            // An event on this course: held in its pen, then raced or ridden behind its leader.
            let event = param(query, "event").and_then(|id| app.events.lock().unwrap_or_else(|e| e.into_inner()).get(id).cloned()).filter(|e| track.as_ref().is_some_and(|t| t.id == e.track));
            if let Some(t) = &track {
                let mut p = presence(query, sid);
                if let Some(n) = &me_name { // the account's name, with the club's tag the server knows
                    let tag = pid.and_then(|p| app.clubs.lock().unwrap_or_else(|e| e.into_inner()).of(p).map(|c| c.tag.clone()));
                    p.name = match tag { Some(t) => format!("[{t}] {n}"), None => n.clone() };
                }
                t.riders.lock().unwrap_or_else(|e| e.into_inner()).insert(sid, Presence { event: event.as_ref().map(|e| e.id.clone()), ..p });
            }
            let start = param(query, "start").and_then(|s| s.parse::<usize>().ok());
            let kind = match (&plan, &game, &event, &track) {
                (Some(_), ..) => "workout",
                (_, Some(_), ..) => "zombie",
                (_, _, Some(e), _) => if e.kind == EventKind::Race { "race" } else { "group" },
                (.., Some(t)) if t.city.is_some() => "city",
                (.., Some(t)) if t.relief.is_some() => "world",
                (.., Some(t)) if t.course.looped => "loop",
                _ => "climb",
            };
            metrics::add("le_mollet_streams_open", &[], 1.0);
            // Road bonuses on courses and open worlds, not in workouts nor against the zombies.
            let mut ride = Ride { powerups: plan.is_none() && game.is_none() && event.as_ref().is_none_or(|e| e.kind == EventKind::Race), ..Ride::new(mass.unwrap_or(83.0)) };
            let res = ride_session(&mut sock, &app, track.as_deref(), plan, game, (sid, start, event), (&mut ride, &mut check)).await;
            metrics::add("le_mollet_streams_open", &[], -1.0);
            if ride.elapsed > 0.0 {
                let trust = match check.trust() { Trust::Demo => "demo", Trust::Flagged => "flagged", Trust::Device => "device" };
                metrics::inc("le_mollet_rides_total", &[("kind", kind), ("trust", trust)]);
                metrics::add("le_mollet_ride_seconds_total", &[("kind", kind)], ride.elapsed as f64);
                metrics::add("le_mollet_ride_km_total", &[("kind", kind)], ride.ridden as f64 / 1000.0);
                metrics::add("le_mollet_ride_kj_total", &[("kind", kind)], ride.energy_kj as f64);
                if check.trust() == Trust::Flagged { for w in check.why() { metrics::inc("le_mollet_fair_flags_total", &[("why", rider::FLAGS.iter().find(|f| **f == w).copied().unwrap_or("other"))]) } }
            }
            if let (Some(r), Some(t)) = (&known, &track) && ride.ridden >= 100.0 {
                let now = unix_now();
                let seen = [check.seen_ftp(), check.race_ftp].into_iter().flatten().reduce(f32::max);
                let flagged = (check.trust() == Trust::Flagged).then(|| Flagged { at: now, track: t.id.clone(), name: presence(query, sid).name, why: check.why() });
                if let Some(f) = &flagged { eprintln!("fair play: {} flagged on {}: {}", f.name, f.track, f.why.join(", ")) }
                let mut riders = app.riders.lock().unwrap_or_else(|e| e.into_inner());
                riders.ride_done(pid.unwrap_or(""), seen.filter(|s| *s > r.ftp_now(now)), flagged, now);
                if let Err(e) = riders.save(&app.db, pid.unwrap_or("")) { eprintln!("riders: {e}") }
            }
            if let Some(me) = track.as_ref().and_then(|t| t.riders.lock().unwrap_or_else(|e| e.into_inner()).remove(&sid)).filter(|p| !p.pid.is_empty() && ride.ridden >= 100.0) {
                let mut clubs = app.clubs.lock().unwrap_or_else(|e| e.into_inner());
                if clubs.add_km(&me.pid, &me.name, ride.ridden / 1000.0, club::week(unix_now())) && let Err(e) = clubs.save(&app.db) { eprintln!("clubs: {e}") }
            }
            app.sensors.lock().unwrap_or_else(|e| e.into_inner()).remove(&sid);
            app.paused.lock().unwrap_or_else(|e| e.into_inner()).remove(&sid);
            app.turns.lock().unwrap_or_else(|e| e.into_inner()).remove(&sid);
            app.inbox.lock().unwrap_or_else(|e| e.into_inner()).remove(&sid);
            app.powerup.lock().unwrap_or_else(|e| e.into_inner()).remove(&sid);
            app.owners.lock().unwrap_or_else(|e| e.into_inner()).remove(&sid);
            app.rideons.lock().unwrap_or_else(|e| e.into_inner()).remove(&sid);
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
    others: Vec<Other<'a>>,
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

/// Another rider as a frame carries them: what moves every time, and who they are (name, color, bike, flag) the
/// first time this stream shows them only. The page keeps those by id. Halves the frames on a busy road.
#[derive(Serialize)]
struct Other<'a> {
    id: u64,
    distance: f32,
    speed_kmh: f32,
    power: f32,
    wkg: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pos: Option<[f64; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    emote: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    color: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    country: Option<&'a str>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    bot: bool,
}

impl<'a> Other<'a> {
    fn new(p: &'a Presence, full: bool) -> Self {
        Self {
            id: p.id, distance: p.distance, speed_kmh: p.speed_kmh, power: p.power, wkg: p.wkg, pos: p.pos, emote: p.emote,
            name: full.then_some(p.name.as_str()), color: full.then_some(p.color.as_str()), frame: full.then_some(p.frame), country: p.country.as_deref().filter(|_| full), bot: full && p.bot,
        }
    }
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
    /// The race radio's latest line (src/peloton.rs).
    #[serde(skip_serializing_if = "Option::is_none")]
    radio: Option<String>,
}

#[derive(Serialize)]
struct BoardRow {
    name: String,
    cat: char,
    gap: f32,
    me: bool,
}

/// An event's field by distance (session, distance, name, category) and its group leader's distance: made once
/// per `STANDINGS_TTL` and shared by every rider's frames, not sorted again by each of them (500 riders made
/// that quadratic, under the course's one lock).
struct Standing {
    field: Vec<(u64, f32, String, char)>,
    leader: Option<f32>,
}
const STANDINGS_TTL: Duration = Duration::from_millis(300);

impl Standing {
    fn new(e: &Event, riders: &HashMap<u64, Presence>) -> Self {
        let cats: HashMap<&str, char> = e.entrants.iter().map(|x| (x.pid.as_str(), x.cat)).collect();
        // The field: people, and the server's racers (not a group ride's leader).
        let mut field: Vec<_> = riders.values().filter(|p| (!p.bot || p.pid.starts_with("bot:")) && p.event.as_deref() == Some(&e.id))
            .map(|p| (p.sid, p.ridden, p.name.clone(), cats.get(p.pid.as_str()).copied().unwrap_or('-'))).collect();
        field.sort_by(|a, b| b.1.total_cmp(&a.1));
        let leader = riders.values().find(|p| p.bot && p.pid.is_empty() && p.event.as_deref() == Some(&e.id)).map(|p| p.ridden);
        Self { field, leader }
    }
}

impl App {
    fn standing(&self, e: &Event, riders: &HashMap<u64, Presence>) -> Arc<Standing> {
        let mut all = self.standings.lock().unwrap_or_else(|x| x.into_inner());
        if let Some((at, s)) = all.get(&e.id) && at.elapsed() < STANDINGS_TTL { return s.clone() }
        all.retain(|_, (at, _)| at.elapsed() < Duration::from_secs(60)); // events over
        let s = Arc::new(Standing::new(e, riders));
        all.insert(e.id.clone(), (std::time::Instant::now(), s.clone()));
        s
    }
}

/// The event as this rider sees it now, from its standing.
fn event_live(e: &Event, st: &Standing, sid: u64, ride: &Ride, phase: Option<Phase>, finish: Option<(usize, usize, f32)>, course: &Course) -> EventLive {
    let field = &st.field;
    let rank = field.iter().position(|f| f.0 == sid).map_or(field.len() + 1, |i| i + 1);
    let board = field.iter().enumerate().filter(|&(i, f)| i < 5 || f.0 == sid)
        .map(|(_, f)| BoardRow { name: f.2.clone(), cat: f.3, gap: if f.0 == sid { 0.0 } else { f.1 - ride.ridden }, me: f.0 == sid }).collect();
    let bunch = st.leader.map_or(0, |l| field.iter().filter(|f| (-25.0..200.0).contains(&(l - f.1))).count());
    EventLive {
        id: e.id.clone(), kind: e.kind, name: e.name.clone(), phase: phase.unwrap_or(Phase::Upcoming),
        starts_in: (e.start as f64 - now_f()) as f32, laps: e.laps, race_m: e.distance(course.length(), course.looped), done_m: ride.ridden,
        leader_gap: st.leader.map(|l| l - ride.ridden), bunch, rank, of: field.len(), board, finish, unranked: None, radio: None,
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
            let mut riders = t.riders.lock().unwrap_or_else(|e| e.into_inner());
            let p = riders.entry(id).or_insert_with(|| Presence {
                id, sid: id, emote: None, emote_at: None, name: format!("Meneur · {:.1} W/kg", e.pace).replace('.', ","), color: "#f2cd37".into(), frame: "road",
                country: None, distance: 0.0, speed_kmh: 0.0, power: 0.0, wkg: e.pace, pos: None, bot: true, pid: String::new(), ridden: 0.0, event: Some(e.id.clone()),
            });
            (p.distance, p.speed_kmh, p.power, p.ridden) = (ride.distance, ride.speed_kmh, ride.power, ride.ridden);
        }
    }
    t.riders.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
}

/// The server's riders in a race (src/peloton.rs): they wait in the pen, go at the gun, draft like everyone, and
/// their finishes count in the results. The race radio's lines go to `App::radio` for the riders' frames.
async fn peloton_task(t: Arc<Track>, e: Event, app: Arc<App>) {
    let base = 800_000 + e.id.bytes().fold(0u64, |h, b| (h * 31 + b as u64) % 90_000) * 32;
    let course = &t.course;
    let race_m = e.distance(course.length(), course.looped);
    let mut race = peloton::Race::new(&e, race_m);
    let mut rides: Vec<Ride> = race.bots.iter().map(|b| Ride::new(b.mass + rider::BIKE_KG)).collect();
    let lap = course.looped.then(|| course.length());
    let mut tick = tokio::time::interval(Duration::from_secs_f32(DT));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut n = 0u32;
    loop {
        tick.tick().await;
        let phase = e.phase(unix_now());
        if phase == Phase::Over || (race.all_done() && n > 600) { break }
        n += 1;
        if phase == Phase::Live {
            let t_race = (now_f() - e.start as f64) as f32;
            let views: Vec<View> = rides.iter().map(|r| View { ridden: r.ridden, race_m, grade: r.grade }).collect();
            let human_lead = {
                let riders = t.riders.lock().unwrap_or_else(|x| x.into_inner());
                let human = riders.values().filter(|p| !p.bot && p.event.as_deref() == Some(&e.id)).map(|p| p.ridden).fold(0.0, f32::max);
                human - views.iter().map(|v| v.ridden).fold(0.0, f32::max)
            };
            let powers = race.step(&views, human_lead, DT);
            for (i, r) in rides.iter_mut().enumerate() {
                r.power = powers[i];
                r.tick(course, DT);
                if r.ridden >= race_m && race.bots[i].finished.is_none() {
                    race.finish(i, t_race);
                    let mut events = app.events.lock().unwrap_or_else(|x| x.into_inner());
                    events.finish(&e.id, &race.bots[i].pid, &race.bots[i].name, t_race);
                    if let Err(err) = events.save(&app.db) { eprintln!("events: {err}") }
                }
            }
        }
        if n.is_multiple_of(3) {
            let mut riders = t.riders.lock().unwrap_or_else(|x| x.into_inner());
            for (i, b) in race.bots.iter().enumerate() {
                let id = base + i as u64;
                let r = &mut rides[i];
                r.draft = draft_behind(riders.values().filter(|p| p.sid != id), r.distance, None, lap);
                let p = riders.entry(id).or_insert_with(|| Presence {
                    id, sid: id, emote: None, emote_at: None, name: b.name.clone(), color: b.color().into(), frame: "road", country: Some(b.country.into()),
                    distance: 0.0, speed_kmh: 0.0, power: 0.0, wkg: 0.0, pos: None, bot: true, pid: b.pid.clone(), ridden: 0.0, event: Some(e.id.clone()),
                });
                (p.distance, p.speed_kmh, p.power, p.wkg, p.ridden) = (r.distance, r.speed_kmh, r.power, r.power / b.mass, r.ridden);
            }
            drop(riders);
            if let Some(line) = race.radio.drain(..).next_back() { app.radio.lock().unwrap_or_else(|x| x.into_inner()).insert(e.id.clone(), (line, unix_now())); }
        }
    }
    let mut riders = t.riders.lock().unwrap_or_else(|x| x.into_inner());
    for i in 0..race.bots.len() { riders.remove(&(base + i as u64)); }
    app.radio.lock().unwrap_or_else(|x| x.into_inner()).remove(&e.id);
    app.pelotons.lock().unwrap_or_else(|x| x.into_inner()).remove(&e.id);
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
    // A bot needs no 10 Hz: it ticks at the frame rate it is seen at (a steady power, physics holds at 0.3 s).
    const PACE_DT: f32 = DT * 3.0;
    let (mut tick, mut n, mut rest) = (tokio::time::interval(Duration::from_secs_f32(PACE_DT)), 0u32, 0.0);
    loop {
        tick.tick().await;
        n += 1;
        if let (Some(c), Some(r)) = (city, route.as_mut()) {
            if r.next == Turn::Straight && rand() < 0.012 { r.next = if rand() < 0.5 { Turn::Left } else { Turn::Right } }
            r.extend(c, ride.distance);
            // The route only grows: start a fresh one from here every 50 km so it never weighs much.
            if ride.distance > 50_000.0 {
                let here = r.course.latlon_at(ride.distance);
                *r = Route::start_at(c, here);
                (ride.distance, ride.ridden) = (0.0, 0.0);
            }
        }
        let course = route.as_ref().map_or(&t.course, |r| &r.course);
        ride.power = wkg * 75.0 * (1.0 + 0.04 * (n as f32 * 0.04 + k as f32).sin());
        if ride.finished {
            rest += PACE_DT;
            if rest > 20.0 { (ride, rest) = (Ride::new(83.0), 0.0) } // at the top of a climb: back down to the start
        }
        ride.tick(course, PACE_DT);
        {
            let pos = route.as_ref().map(|r| (r.course.latlon_at(ride.distance), r.course.latlon_at(ride.distance + 5.0)));
            let mut riders = t.riders.lock().unwrap_or_else(|e| e.into_inner());
            ride.draft = draft_behind(riders.values().filter(|p| p.sid != id), ride.distance, pos, t.course.looped.then(|| t.course.length()));
            let p = riders.entry(id).or_insert_with(|| Presence {
                id, sid: id, emote: None, emote_at: None, name: format!("{name} · {wkg:.1} W/kg").replace('.', ","), color: color.into(), frame: "road", country: None,
                distance: 0.0, speed_kmh: 0.0, power: 0.0, wkg, pos: None, bot: true, pid: String::new(), ridden: 0.0, event: None,
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
    app: &Arc<App>,
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
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay); // behind: no burst of catch-up ticks
    let (mut beacon_spawned, mut tick_work) = (false, 0.0f64);
    let (mut n, mut sent) = (0u32, None::<f32>);
    // Riders this stream has introduced (name, color...): the next frames carry only what moves.
    let mut seen: HashSet<u64> = HashSet::new();
    loop {
        tick.tick().await;
        let t0 = std::time::Instant::now();
        let seek = app.demo_seek.lock().unwrap_or_else(|e| e.into_inner()).remove(&sid).filter(|_| event.is_none()); // no shortcuts in an event
        if let (Some(c), Some(r)) = (city, route.as_mut()) {
            if let Some(t) = app.turns.lock().unwrap_or_else(|e| e.into_inner()).remove(&sid) {
                r.next = t;
            }
            r.extend(c, seek.unwrap_or(0.0).max(ride.distance)); // a demo jump ahead: grow the road there first
        }
        let course = route.as_ref().map_or(base, |r| &r.course);
        let sensors = app.sensors.lock().unwrap_or_else(|e| e.into_inner()).get(&sid).copied().unwrap_or_default();
        (ride.power, ride.cadence, ride.hr) = match app.trainer {
            Some(_) if !sensors.live => {
                let (w, cadence) = *app.trainer_sample.lock().unwrap_or_else(|e| e.into_inner());
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
        let paused = app.paused.lock().unwrap_or_else(|e| e.into_inner()).contains(&sid);
        if app.powerup.lock().unwrap_or_else(|e| e.into_inner()).remove(&sid) { ride.use_powerup() }
        // An event: on the start line until the gun.
        let phase = event.as_ref().map(|e| e.phase(unix_now()));
        let held = matches!(phase, Some(Phase::Upcoming | Phase::Pen));
        if held { (ride.distance, ride.speed_kmh) = (0.0, 0.0) }
        if let (Some(e), Some(Phase::Live)) = (&event, phase) && e.kind == EventKind::Group && !beacon_spawned && { beacon_spawned = true; app.beacons.lock().unwrap_or_else(|e| e.into_inner()).insert(e.id.clone()) }
            && let Some(t) = app.track(&e.track)
        {
            tokio::spawn(beacon(t, e.clone()));
        }
        // A race: the server's riders line up in the pen with the first person who shows up.
        if let (Some(e), Some(Phase::Pen | Phase::Live)) = (&event, phase) && e.kind == EventKind::Race && !beacon_spawned && { beacon_spawned = true; e.entrants.iter().any(|x| x.bot) && app.pelotons.lock().unwrap_or_else(|x| x.into_inner()).insert(e.id.clone()) }
            && let Some(t) = app.track(&e.track)
        {
            tokio::spawn(peloton_task(t, e.clone(), app.clone()));
        }
        // Every second of power is checked, warm-up in the pen included: the slider or no profile makes a demo ride.
        if !paused && !caught { check.tick(ride.power, ride.cadence, sensors.live || app.trainer.is_some(), DT) }
        if !paused && !caught && !held {
            ride.tick(course, DT);
            if let (Some(e), Some(Phase::Live), None, None) = (&event, phase, finish, unranked)
                && e.kind == EventKind::Race
                && ride.ridden >= e.distance(base.length(), base.looped)
                && let Some(me) = track.and_then(|t| t.riders.lock().unwrap_or_else(|e| e.into_inner()).get(&sid).cloned())
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
                metrics::inc("le_mollet_race_finishes_total", &[("result", unranked.unwrap_or("ranked"))]);
                if unranked.is_none() {
                    let mut events = app.events.lock().unwrap_or_else(|e| e.into_inner());
                    finish = events.finish(&e.id, &me.pid, &me.name, time).map(|(o, c)| (o, c, time));
                    if let Err(err) = events.save(&app.db) { eprintln!("events: {err}") }
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
            && let Some(me) = track.and_then(|t| t.riders.lock().unwrap_or_else(|e| e.into_inner()).get(&sid).cloned())
        {
            let w = &windows[i];
            // A hash of the profile id: enough to keep one line per rider, and not the id itself (docs/security.md).
            let who = { let mut h = Fnv::default(); h.add(if me.pid.is_empty() { sid.to_string() } else { me.pid }.as_bytes()); h.hex() };
            let mut boards = app.boards.lock().unwrap_or_else(|e| e.into_inner());
            let rank = boards.record(&key(w), Entry { who, name: me.name, color: me.color, time, date: unix_now() });
            if let Err(e) = boards.save(&app.db, &key(w)) { eprintln!("leaderboards: {e}") }
            metrics::inc("le_mollet_segment_efforts_total", &[("kind", if w.kind == Kind::Kom { "kom" } else { "sprint" })]);
            result = Some(SegResult { name: w.name.clone(), kind: w.kind, time, rank });
        }
        // Workout clock = riding time, so it starts with the first pedal stroke.
        let progress = plan.map(|(w, ftp)| w.progress(ride.elapsed, ftp));
        n += 1;
        if n % 3 == 0 {
            // Publish where we are, read where the others are: the nearest ones, they are the ones on screen.
            let mut live = None;
            let near = track.map_or(vec![], |t| {
                let mut riders = t.riders.lock().unwrap_or_else(|e| e.into_inner());
                let me = riders.get_mut(&sid).and_then(|me| {
                    (me.distance, me.speed_kmh, me.power, me.wkg, me.ridden) = (ride.distance, ride.speed_kmh, ride.power, ride.power / (ride.mass - rider::BIKE_KG).max(30.0), ride.ridden);
                    me.pos = city.map(|_| course.latlon_at(ride.distance));
                    me.pos
                });
                let lap = t.course.looped.then(|| t.course.length());
                // Drafting: the rider just ahead cuts the wind for the next ticks.
                ride.draft = draft_behind(riders.values().filter(|p| p.sid != sid), ride.distance, me.map(|a| (a, course.latlon_at(ride.distance + 5.0))), lap);
                live = event.as_ref().map(|e| EventLive { unranked, radio: app.radio.lock().unwrap_or_else(|x| x.into_inner()).get(&e.id).filter(|r| unix_now() < r.1 + 25).map(|r| r.0.clone()), ..event_live(e, &app.standing(e, &riders), sid, ride, phase, finish, base) });
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
                let best = app.boards.lock().unwrap_or_else(|e| e.into_inner()).0.get(&key(w)).and_then(|l| l.first()).map(|e| e.time);
                SegLive { name: w.name.clone(), kind: w.kind, done: ride.distance - w.d0, len: w.d1 - w.d0, time: ride.elapsed - t0, best }
            });
            let new_windows = windows[sent_windows..].to_vec();
            sent_windows = windows.len();
            let others: Vec<Other> = near.iter().map(|p| Other::new(p, seen.insert(p.id))).collect();
            let frame = Frame { fair: Fair { trust: check.trust(), why: check.why() }, event: live, ride, workout: progress, zombie: game.as_ref(), others, route: fresh, junction, windows: new_windows, segment, result: result.take(), inbox: app.inbox.lock().unwrap_or_else(|e| e.into_inner()).remove(&sid).unwrap_or_default() };
            let mut json = serde_json::to_vec(&frame).expect("frame serializes");
            json.splice(0..0, *b"data: ");
            json.extend_from_slice(b"\n\n");
            metrics::add("le_mollet_frame_bytes_total", &[], json.len() as f64);
            // A client that stops taking frames is gone: its rider leaves the road instead of standing there.
            match tokio::time::timeout(WRITE_TIMEOUT, sock.write_all(&json)).await {
                Ok(r) => r?,
                Err(_) => return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "client stopped reading")),
            }
        }
        tick_work += t0.elapsed().as_secs_f64();
        if n.is_multiple_of(10) { metrics::observe("le_mollet_ride_tick_seconds", &[], tick_work / 10.0); tick_work = 0.0 }
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
const SESSION_COOKIE: &str = "lm_session";
const CLEAR_COOKIE: &str = "lm_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0";
const SESSION_TTL: Duration = Duration::from_secs(30 * 86_400);

#[derive(Clone, Serialize, serde::Deserialize)]
struct Session {
    account: u64,
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

async fn redirect(sock: &mut TcpStream, to: &str, set_cookie: Option<&str>) -> std::io::Result<()> {
    metrics::inc("le_mollet_http_responses_total", &[("code", "302")]);
    let cookie = set_cookie.map(|c| format!("Set-Cookie: {c}\r\n")).unwrap_or_default();
    let head = format!("HTTP/1.1 302 Found\r\nLocation: {to}\r\n{cookie}Cache-Control: no-store\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    sock.write_all(head.as_bytes()).await
}

/// A request's route for the metrics: the known paths, everything else as "other" (no unbounded labels).
fn route_label(path: &str) -> &'static str {
    const ROUTES: [&str; 38] = ["/friend", "/friend/ask", "/friend/accept", "/friend/remove", "/friend/view", "/friend/ride", "/friend/unride", "/", "/courses", "/worlds", "/worlds/new", "/course", "/terrain", "/buildings", "/district", "/ortho", "/workouts", "/turn", "/events",
        "/power", "/segments", "/whoami", "/emote", "/seek", "/pause", "/powerup", "/rider", "/metrics", "/club", "/club/create", "/club/join", "/club/leave",
        "/event/list", "/event/create", "/event/join", "/event/leave", "/event/delete", "/admin/flags"];
    ROUTES.iter().find(|r| **r == path).copied().unwrap_or(if path.starts_with("/audio/") { "/audio" } else if path.starts_with("/auth/") { "/auth" } else if path.starts_with("/admin/") { "/admin" } else { "other" })
}

/// Times a request into the metrics when it is answered (dropped).
struct Timed(&'static str, std::time::Instant);

impl Drop for Timed {
    fn drop(&mut self) {
        metrics::observe("le_mollet_http_request_duration_seconds", &[("route", self.0)], self.1.elapsed().as_secs_f64());
    }
}

/// A static file: `tag` as ETag (304 when the browser or a CDN has it), gzipped when the client takes it, and
/// cached for a year when the URL names this very version (`v=<tag>`), else revalidated each time.
async fn reply_static(sock: &mut TcpStream, req: &str, query: &str, ctype: &str, raw: &[u8], gz: Option<Arc<Vec<u8>>>, tag: &str) -> std::io::Result<()> {
    let etag = format!("W/\"{tag}\"");
    let cache = if param(query, "v") == Some(tag) { "public, max-age=31536000, immutable" } else { "public, no-cache" };
    let gz = gz.filter(|_| header(req, "accept-encoding").is_some_and(|a| a.contains("gzip")));
    let fresh = header(req, "if-none-match").is_some_and(|h| h.split(',').any(|t| t.trim() == etag));
    let (status, body, enc) = match (&gz, fresh) {
        (_, true) => ("304 Not Modified", &[][..], ""),
        (Some(z), false) => ("200 OK", &z[..], "Content-Encoding: gzip\r\n"),
        (None, false) => ("200 OK", raw, ""),
    };
    metrics::inc("le_mollet_http_responses_total", &[("code", &status[..3])]);
    let head = format!("HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\n{enc}Content-Length: {}\r\nCache-Control: {cache}\r\nETag: {etag}\r\nVary: Accept-Encoding\r\n{SAFE_HEADERS}Connection: close\r\n\r\n", body.len());
    sock.write_all(head.as_bytes()).await?;
    sock.write_all(body).await
}

/// Headers every response carries: never framed by another site, no content sniffing, no referrer leaks.
const SAFE_HEADERS: &str = "X-Frame-Options: DENY\r\nContent-Security-Policy: frame-ancestors 'none'\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\n";

/// Equal, in time that does not depend on where they differ.
fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn reply(sock: &mut TcpStream, status: &str, ctype: &str, body: &[u8]) -> std::io::Result<()> {
    metrics::inc("le_mollet_http_responses_total", &[("code", status.get(..3).unwrap_or("?"))]);
    let head = format!("HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n{SAFE_HEADERS}Connection: close\r\n\r\n", body.len());
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
        Presence { id, sid: id, emote: None, emote_at: None, name: String::new(), color: String::new(), frame: "road", country: None, distance, speed_kmh: 0.0, power: 0.0, wkg: 0.0, pos: None, bot: false, pid: String::new(), ridden: 0.0, event: None }
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
