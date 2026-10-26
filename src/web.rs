//! Tiny HTTP server: embedded web UI + ride sessions.
//!   GET /                                      page
//!   GET /courses                               {trainer, courses: [summary]}
//!   GET /course?id=<id>                        full course JSON
//!   GET /terrain?id=<id>                       binary voxel terrain (see le_mollet::terrain)
//!   GET /events?course=<id>&mass=<kg>&session=<n>   Server-Sent Events, one ride per connection
//!   GET /power?session=<n>&w=<W>               demo mode: set the rider's power
//!   GET /seek?session=<n>&d=<m>                demo mode: teleport the rider along the course
//!   GET /audio/<name>.mp3                      nature ambience loops (web/audio/CREDITS.md)
//!
//! With a trainer, power comes from it and the latest ride drives its grade.
//! Without, each browser tab rides on its own with the power slider (demo mode).

use le_mollet::Trainer;
use le_mollet::course::Course;
use le_mollet::ride::Ride;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use std::sync::Arc;
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

pub struct App {
    courses: Vec<(String, Course, String)>, // id, course, JSON
    terrains: HashMap<String, Vec<u8>>,
    list_json: String,
    pub trainer: Option<Trainer>,
    /// Latest (power, cadence) from the trainer.
    pub trainer_sample: Mutex<(f32, Option<f32>)>,
    difficulty: f32,
    demo_power: Mutex<HashMap<u64, f32>>,
    demo_seek: Mutex<HashMap<u64, f32>>,
    /// Session allowed to drive the trainer grade: the most recent one.
    driver: AtomicU64,
}

impl App {
    /// Load every `*.json` course in `dir`; the id is the file stem.
    pub fn new(dir: &str, trainer: Option<Trainer>, difficulty: f32) -> anyhow::Result<Self> {
        let mut paths: Vec<_> = std::fs::read_dir(dir)
            .map(|rd| rd.filter_map(|e| Some(e.ok()?.path())).filter(|p| p.extension().is_some_and(|x| x == "json")).collect())
            .unwrap_or_default();
        paths.sort();
        let (mut courses, mut terrains) = (vec![], HashMap::new());
        for p in paths {
            let c = Course::load(&p.to_string_lossy())?;
            let id = p.file_stem().unwrap().to_string_lossy().into_owned();
            let json = serde_json::to_string(&c)?;
            if let Ok(bin) = std::fs::read(crate::terrain_path(&p.to_string_lossy())) {
                terrains.insert(id.clone(), bin);
            }
            courses.push((id, c, json));
        }
        let summaries: Vec<_> = courses
            .iter()
            .map(|(id, c, _)| {
                // 60-sample elevation profile for the course picker thumbnails.
                let profile: Vec<f32> = (0..60).map(|i| c.ele_at(c.length() * i as f32 / 59.0).round()).collect();
                json!({ "id": id, "name": c.name, "length": c.length(), "gain": c.elevation_gain().round(), "profile": profile, "terrain": terrains.contains_key(id) })
            })
            .collect();
        let list_json = json!({ "trainer": trainer.as_ref().map(|t| &t.name), "courses": summaries }).to_string();
        Ok(Self {
            courses,
            terrains,
            list_json,
            trainer,
            trainer_sample: Mutex::new((0.0, None)),
            difficulty,
            demo_power: Mutex::new(HashMap::new()),
            demo_seek: Mutex::new(HashMap::new()),
            driver: AtomicU64::new(0),
        })
    }

    pub fn course_count(&self) -> usize {
        self.courses.len()
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

async fn handle(mut sock: TcpStream, app: Arc<App>) -> std::io::Result<()> {
    let mut buf = [0u8; 2048];
    let n = sock.read(&mut buf).await?;
    let req = String::from_utf8_lossy(&buf[..n]);
    let target = req.split_whitespace().nth(1).unwrap_or("/");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let course = param(query, "id").or(param(query, "course")).and_then(|id| app.courses.iter().find(|c| c.0 == id));
    let session: Option<u64> = param(query, "session").and_then(|s| s.parse().ok());
    match (path, course) {
        ("/", _) => reply(&mut sock, "200 OK", "text/html; charset=utf-8", PAGE.as_bytes()).await,
        (p, _) if p.starts_with("/audio/") => match AUDIO.iter().find(|a| a.0 == p) {
            Some((_, bytes)) => reply(&mut sock, "200 OK", "audio/mpeg", bytes).await,
            None => reply(&mut sock, "404 Not Found", "text/plain", b"not found").await,
        },
        ("/courses", _) => reply(&mut sock, "200 OK", "application/json", app.list_json.as_bytes()).await,
        ("/course", Some(c)) => reply(&mut sock, "200 OK", "application/json", c.2.as_bytes()).await,
        ("/terrain", Some(c)) if app.terrains.contains_key(&c.0) => {
            reply(&mut sock, "200 OK", "application/octet-stream", &app.terrains[&c.0]).await
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
        ("/events", Some((_, course, _))) => {
            let Some(sid) = session else { return reply(&mut sock, "400 Bad Request", "text/plain", b"need session").await };
            let mass = param(query, "mass").and_then(|m| m.parse::<f32>().ok()).filter(|m| (30.0..=250.0).contains(m));
            sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\r\n").await?;
            app.driver.store(sid, Ordering::Relaxed);
            let res = ride_session(&mut sock, &app, course, sid, mass.unwrap_or(83.0)).await;
            app.demo_power.lock().unwrap().remove(&sid);
            res
        }
        _ => reply(&mut sock, "404 Not Found", "text/plain", b"not found").await,
    }
}

async fn ride_session(sock: &mut TcpStream, app: &App, course: &Course, sid: u64, mass: f32) -> std::io::Result<()> {
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
        ride.tick(course, DT);
        n += 1;
        if n % 3 == 0 {
            let json = serde_json::to_string(&ride).expect("ride serializes");
            sock.write_all(format!("data: {json}\n\n").as_bytes()).await?;
        }
        // ponytail: 0.2 % deadband at 1 Hz keeps BLE writes low, tune if the road feels steppy
        if let Some(t) = &app.trainer
            && n % 10 == 0
            && app.driver.load(Ordering::Relaxed) == sid
        {
            let target = if ride.finished { 0.0 } else { (ride.grade * app.difficulty * 10.0).round() / 10.0 };
            if sent.is_none_or(|g| (g - target).abs() >= 0.2) {
                match t.set_grade(target).await {
                    Ok(()) => sent = Some(target),
                    Err(e) => eprintln!("set grade failed: {e:#}"),
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
