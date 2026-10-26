//! Tiny HTTP server: embedded web UI + ride sessions.
//!   GET /                                      page
//!   GET /courses                               {trainer, courses: [summary]}  (real climbs)
//!   GET /worlds                                [summary + relief + riders]   (open worlds)
//!   GET /worlds/new?length=<5|10|20>&relief=<flat|hills|mountains>   generate an open world, {id}
//!   GET /course?id=<id>                        full course JSON (climb or world)
//!   GET /terrain?id=<id>                       binary voxel terrain (see le_mollet::terrain)
//!   GET /workouts                              built-in training sessions
//!   GET /events?course=<id>&mass=<kg>&session=<n>[&name=&color=&frame=&country=]
//!                                              Server-Sent Events, one ride per connection; riders on the
//!                                              same course see each other ("others" in each frame)
//!   GET /events?workout=<id>&ftp=<W>&mass=<kg>&session=<n>   same, workout on a flat road
//!   GET /events?course=<id>&mode=zombie&ftp=<W>&...           same, chased by a zombie horde
//!   GET /power?session=<n>&w=<W>               demo mode: set the rider's power
//!   GET /seek?session=<n>&d=<m>                demo mode: teleport the rider along the course
//!   GET /pause?session=<n>&on=<0|1>            freeze or resume a ride (menu open)
//!   GET /audio/<name>.mp3                      nature ambience loops (web/audio/CREDITS.md)
//!
//! With a trainer, power comes from it and the latest ride drives its grade.
//! Without, each browser tab rides on its own with the power slider (demo mode).

use le_mollet::Trainer;
use le_mollet::course::Course;
use le_mollet::ride::Ride;
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
// ponytail: worlds live in memory, oldest empty ones dropped past this; persist them if people want to keep maps
const MAX_WORLDS: usize = 12;

/// A rider as other riders on the same course see them.
#[derive(Debug, Clone, Serialize)]
struct Presence {
    id: u64,
    name: String,
    color: String,
    frame: &'static str,
    country: Option<String>,
    distance: f32,
    speed_kmh: f32,
    power: f32,
}

/// A rideable road: a real climb loaded from disk or a generated open world.
struct Track {
    id: String,
    course: Course,
    json: String,
    terrain: Option<Vec<u8>>,
    relief: Option<Relief>, // Some = open world
    riders: Mutex<HashMap<u64, Presence>>,
}

impl Track {
    fn new(id: String, course: Course, terrain: Option<Vec<u8>>, relief: Option<Relief>) -> anyhow::Result<Self> {
        let json = serde_json::to_string(&course)?;
        Ok(Self { id, course, json, terrain, relief, riders: Mutex::new(HashMap::new()) })
    }

    fn summary(&self) -> serde_json::Value {
        let c = &self.course;
        // 60-sample elevation profile for the picker thumbnails.
        let profile: Vec<f32> = (0..60).map(|i| c.ele_at(c.length() * i as f32 / 59.0).round()).collect();
        json!({
            "id": self.id, "name": c.name, "length": c.length(), "gain": c.elevation_gain().round(), "profile": profile,
            "terrain": self.terrain.is_some(), "looped": c.looped,
            "relief": self.relief.map(Relief::label), "riders": self.riders.lock().unwrap().len(),
        })
    }
}

fn world_track(seed: u64, length_km: u32, relief: Relief, name: String) -> anyhow::Result<Track> {
    let w = world::generate(seed, length_km as f64 * 1000.0, relief, &name);
    Track::new(format!("world-{seed:x}"), w.course, Some(w.terrain.to_bytes()), Some(relief))
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
    demo_power: Mutex<HashMap<u64, f32>>,
    demo_seek: Mutex<HashMap<u64, f32>>,
    paused: Mutex<HashSet<u64>>,
    /// Session allowed to drive the trainer grade: the most recent one.
    driver: AtomicU64,
}

impl App {
    /// Load every `*.json` course in `dir` (the id is the file stem) and open today's world.
    pub fn new(dir: &str, trainer: Option<Trainer>, difficulty: f32) -> anyhow::Result<Self> {
        let mut paths: Vec<_> = std::fs::read_dir(dir)
            .map(|rd| rd.filter_map(|e| Some(e.ok()?.path())).filter(|p| p.extension().is_some_and(|x| x == "json")).collect())
            .unwrap_or_default();
        paths.sort();
        let mut tracks = vec![];
        for p in paths {
            let path = p.to_string_lossy();
            let id = p.file_stem().unwrap().to_string_lossy().into_owned();
            tracks.push(Arc::new(Track::new(id, Course::load(&path)?, std::fs::read(crate::terrain_path(&path)).ok(), None)?));
        }
        let summaries: Vec<_> = tracks.iter().map(|t| t.summary()).collect();
        let list_json = json!({ "trainer": trainer.as_ref().map(|t| &t.name), "courses": summaries }).to_string();
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
            demo_power: Mutex::new(HashMap::new()),
            demo_seek: Mutex::new(HashMap::new()),
            paused: Mutex::new(HashSet::new()),
            driver: AtomicU64::new(0),
        })
    }

    pub fn course_count(&self) -> usize {
        self.tracks.read().unwrap().iter().filter(|t| t.relief.is_none()).count()
    }

    fn track(&self, id: &str) -> Option<Arc<Track>> {
        self.tracks.read().unwrap().iter().find(|t| t.id == id).cloned()
    }

    fn worlds_json(&self) -> String {
        json!(self.tracks.read().unwrap().iter().filter(|t| t.relief.is_some()).map(|t| t.summary()).collect::<Vec<_>>()).to_string()
    }

    fn add_world(&self, track: Track) -> String {
        let id = track.id.clone();
        let mut tracks = self.tracks.write().unwrap();
        tracks.push(Arc::new(track));
        while tracks.iter().filter(|t| t.relief.is_some()).count() > MAX_WORLDS {
            // Worlds are kept in creation order after the climbs: drop the oldest empty one.
            match tracks.iter().position(|t| t.relief.is_some() && t.riders.lock().unwrap().is_empty()) {
                Some(i) => drop(tracks.remove(i)),
                None => break,
            }
        }
        id
    }
}

pub async fn serve(addr: &str, app: Arc<App>) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
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

/// Who this session shows to the others, from untrusted query values.
fn presence(query: &str, sid: u64) -> Presence {
    let name: String = param(query, "name").map(decode).unwrap_or_default().chars().filter(|c| !c.is_control()).take(24).collect();
    let color = param(query, "color").map(decode).filter(|c| c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|h| h.is_ascii_hexdigit()));
    let frame = ["road", "aero", "gravel", "vintage"].into_iter().find(|f| param(query, "frame") == Some(*f)).unwrap_or("road");
    let country = param(query, "country").filter(|c| c.len() == 2 && c.bytes().all(|b| b.is_ascii_uppercase())).map(str::to_owned);
    Presence {
        id: sid,
        name: if name.trim().is_empty() { "Cycliste".into() } else { name.trim().to_owned() },
        color: color.unwrap_or_else(|| "#c91a09".into()),
        frame,
        country,
        distance: 0.0,
        speed_kmh: 0.0,
        power: 0.0,
    }
}

async fn handle(mut sock: TcpStream, app: Arc<App>) -> std::io::Result<()> {
    let mut buf = [0u8; 4096];
    let n = sock.read(&mut buf).await?;
    let req = String::from_utf8_lossy(&buf[..n]);
    let target = req.split_whitespace().nth(1).unwrap_or("/");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let track = param(query, "id").or(param(query, "course")).and_then(|id| app.track(id));
    let session: Option<u64> = param(query, "session").and_then(|s| s.parse().ok());
    match (path, track) {
        ("/", _) => reply(&mut sock, "200 OK", "text/html; charset=utf-8", PAGE.as_bytes()).await,
        (p, _) if p.starts_with("/audio/") => match AUDIO.iter().find(|a| a.0 == p) {
            Some((_, bytes)) => reply(&mut sock, "200 OK", "audio/mpeg", bytes).await,
            None => reply(&mut sock, "404 Not Found", "text/plain", b"not found").await,
        },
        ("/workouts", _) => reply(&mut sock, "200 OK", "application/json", app.workouts_json.as_bytes()).await,
        ("/courses", _) => reply(&mut sock, "200 OK", "application/json", app.list_json.as_bytes()).await,
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
        ("/power", _) => {
            let w = param(query, "w").and_then(|w| w.parse::<f32>().ok()).filter(|w| (0.0..=2000.0).contains(w));
            match (session, w) {
                (Some(s), Some(w)) => {
                    app.demo_power.lock().unwrap().insert(s, w);
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
            let workout = param(query, "workout").and_then(|id| app.workouts.iter().find(|w| w.id == id));
            let ftp = param(query, "ftp").and_then(|f| f.parse::<f32>().ok()).filter(|f| (30.0..=1000.0).contains(f));
            if track.is_none() && (workout.is_none() || ftp.is_none()) {
                return reply(&mut sock, "400 Bad Request", "text/plain", b"need course=<id>, or workout=<id>&ftp=<W>").await;
            }
            let Some(sid) = session else { return reply(&mut sock, "400 Bad Request", "text/plain", b"need session").await };
            let mass = param(query, "mass").and_then(|m| m.parse::<f32>().ok()).filter(|m| (30.0..=250.0).contains(m));
            sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\r\n").await?;
            app.driver.store(sid, Ordering::Relaxed);
            let plan = workout.zip(ftp).filter(|_| track.is_none());
            let game = ftp.filter(|_| track.is_some() && param(query, "mode") == Some("zombie")).map(Game::new);
            if let Some(t) = &track {
                t.riders.lock().unwrap().insert(sid, presence(query, sid));
            }
            let res = ride_session(&mut sock, &app, track.as_deref(), plan, game, sid, mass.unwrap_or(83.0)).await;
            if let Some(t) = &track {
                t.riders.lock().unwrap().remove(&sid);
            }
            app.demo_power.lock().unwrap().remove(&sid);
            app.paused.lock().unwrap().remove(&sid);
            res
        }
        _ => reply(&mut sock, "404 Not Found", "text/plain", b"not found").await,
    }
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
}

async fn ride_session(
    sock: &mut TcpStream,
    app: &App,
    track: Option<&Track>,
    plan: Option<(&Workout, f32)>,
    mut game: Option<Game>,
    sid: u64,
    mass: f32,
) -> std::io::Result<()> {
    let course = track.map_or(&app.flat, |t| &t.course);
    let mut ride = Ride::new(mass);
    let mut tick = tokio::time::interval(Duration::from_secs_f32(DT));
    let (mut n, mut sent) = (0u32, None::<f32>);
    loop {
        tick.tick().await;
        (ride.power, ride.cadence) = match app.trainer {
            Some(_) => *app.trainer_sample.lock().unwrap(),
            None => (app.demo_power.lock().unwrap().get(&sid).copied().unwrap_or(0.0), None),
        };
        if let Some(d) = app.demo_seek.lock().unwrap().remove(&sid) {
            ride.distance = d.min(course.length());
            ride.finished = false;
        }
        // Paused: time, distance and totals freeze; frames keep flowing so the page stays live.
        // Caught by the zombies: the ride freezes like a finish line.
        let caught = game.as_ref().is_some_and(|g| g.over);
        let paused = app.paused.lock().unwrap().contains(&sid);
        if !paused && !caught {
            ride.tick(course, DT);
            // The horde waits for the first pedal stroke, like the ride clock.
            if let Some(g) = game.as_mut().filter(|_| ride.elapsed > 0.0) {
                g.tick(&ride, DT);
            }
        }
        // Workout clock = riding time, so it starts with the first pedal stroke.
        let progress = plan.map(|(w, ftp)| w.progress(ride.elapsed, ftp));
        n += 1;
        if n % 3 == 0 {
            // Publish where we are, read where the others are.
            // ponytail: everyone sees everyone on a course; add proximity culling past a few dozen riders
            let others = track.map_or(vec![], |t| {
                let mut riders = t.riders.lock().unwrap();
                if let Some(me) = riders.get_mut(&sid) {
                    (me.distance, me.speed_kmh, me.power) = (ride.distance, ride.speed_kmh, ride.power);
                }
                riders.values().filter(|p| p.id != sid).take(50).cloned().collect()
            });
            let frame = Frame { ride: &ride, workout: progress, zombie: game.as_ref(), others };
            let json = serde_json::to_string(&frame).expect("frame serializes");
            sock.write_all(format!("data: {json}\n\n").as_bytes()).await?;
        }
        // Workout: ERG holds the target watts. Course: grade scaled by difficulty.
        // ponytail: 1 W / 0.2 % deadbands at 1 Hz keep BLE writes low, tune if it feels steppy
        if let Some(t) = &app.trainer
            && !paused
            && n % 10 == 0
            && app.driver.load(Ordering::Relaxed) == sid
        {
            let (value, erg) = match progress {
                Some(p) => (p.target, true),
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
}
