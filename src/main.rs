mod web;

use anyhow::Context;
use clap::{Parser, Subcommand};
use futures::StreamExt;
use le_mollet::Trainer;
use le_mollet::course::Course;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

#[derive(Parser)]
#[command(about = "le-mollet: open-source indoor cycling")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Connect to a trainer, print telemetry, type commands: erg <W> | grade <%> | quit
    Trainer {
        /// Name substring, e.g. "kickr"
        filter: Option<String>,
    },
    /// Everything a Bluetooth device says, to support new hardware: lists devices, or maps one and logs it (docs/hardware.md)
    Probe {
        /// Name substring or address of the device to map; none: list the devices around
        target: Option<String>,
        /// How long to log notifications, s
        #[arg(long, default_value_t = 60)]
        seconds: u64,
        /// Send once subscribed, repeatable: <characteristic uuid>=<hex> or =text:<ascii>, e.g. a handshake
        #[arg(long)]
        write: Vec<String>,
    },
    /// Print the validation figures of a course file (see courses/AGENTS.md)
    Stats {
        /// Course JSON file
        course: String,
    },
    /// Cap a course at its published steepest ramp, % over 100 m, where the ~30 m elevation outside France
    /// squeezes it into a fake wall; rewrites the course and its terrain, see docs/courses.md
    Cap {
        /// Course JSON file
        course: String,
        /// Steepest grade, %
        max: f32,
    },
    /// (Re)build the 3D terrain of an existing course file
    Terrain {
        /// Course JSON file
        course: String,
    },
    /// Web UI: profiles, garage, courses. Demo mode (power slider) unless --trainer is given
    Serve {
        #[arg(long, default_value = "127.0.0.1:8080")]
        addr: String,
        /// Ride for real: trainer name substring, e.g. "kickr"
        #[arg(long)]
        trainer: Option<String>,
        /// Share of the real grade the trainer applies (Zwift-like trainer difficulty)
        #[arg(long, default_value_t = 0.5)]
        difficulty: f32,
        /// Directory of course JSON files
        #[arg(long, default_value = "courses")]
        courses: String,
        /// The database: riders, events, clubs, leaderboards, roles, sign-ins (one SQLite file, see src/db.rs)
        #[arg(long, default_value = "le-mollet.db")]
        db: String,
        /// local: no account needed, this PC's browser is admin. cloud: a Le Mollet account required (docs/rbac.md)
        #[arg(long, value_enum, default_value_t = web::Mode::Local)]
        mode: web::Mode,
        /// Local mode: open the server to the local network (listen on 0.0.0.0, same port); guests ride as Riders
        #[arg(long)]
        lan: bool,
    },
    /// Give roles to accounts: set <name> admin|organizer|rider [--note X], remove <name>, list
    Role {
        /// set, remove or list
        action: String,
        /// Account name (as signed up in the page)
        account: Option<String>,
        /// admin, organizer or rider
        role: Option<String>,
        /// A label for the list
        #[arg(long, default_value = "")]
        note: String,
        /// The server's database
        #[arg(long, default_value = "le-mollet.db")]
        db: String,
    },
    /// Build an open city (every street inside an OSM boundary) and its terrain, see docs/city.md
    City {
        /// Display name, e.g. "Paris"
        name: String,
        /// Output file, e.g. courses/paris.city.json
        #[arg(long, short)]
        out: String,
        /// OpenStreetMap boundary relation id, e.g. 7444 for Paris
        #[arg(long)]
        relation: u64,
        /// "lat,lon" where riders start
        #[arg(long)]
        start: String,
    },
    /// Build a hand-made island from its map file, see docs/island.md
    Island {
        /// Map file, e.g. courses/ile-des-briques.map.json; writes x.city.json and x.city.terrain.bin
        map: String,
    },
    /// (Re)fetch the real buildings (IGN BD TOPO) of an open city, over its terrain box
    Buildings {
        /// City file, e.g. courses/paris.city.json (its terrain file must exist)
        city: String,
    },
    /// Fetch a realistic district of an open city (aerial photo, 1 m elevation, roofs, trees), see docs/city.md
    District {
        /// City file, e.g. courses/paris.city.json
        city: String,
        /// "south,west,north,east" box, at most ~4 km a side
        #[arg(long)]
        bbox: String,
    },
    /// Decor from the real place: aerial photo mosaic and trees, OpenStreetMap benches, lamps, fences... (docs/courses.md)
    Decor {
        /// Course file whose terrain file exists, e.g. courses/longchamp.json
        course: String,
    },
    /// Fetch the IGN aerial photo over a course or city terrain, for the realistic render
    Ortho {
        /// Course or city file whose terrain file exists, e.g. courses/alpe-dhuez.json
        course: String,
    },
    /// Build a course file from waypoints, or a loop from an OSM route relation, see docs/courses.md
    Course {
        /// Display name, e.g. "Alpe d'Huez"
        name: String,
        /// Output JSON file
        #[arg(long, short)]
        out: String,
        /// Road ref to stick to, repeatable, e.g. --road "D 211" --road "D 211F"
        #[arg(long)]
        road: Vec<String>,
        /// "lat,lon" waypoints: start, optional vias, finish; with --relation, just the start (optional)
        waypoints: Vec<String>,
        /// Loop from an OpenStreetMap route relation id instead, e.g. 163270 (Anneau cyclable de Longchamp)
        #[arg(long)]
        relation: Option<u64>,
        /// Terrain cell size, m (default 5; 2 for small bricks on a short loop)
        #[arg(long)]
        cell: Option<f64>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().cmd {
        Cmd::Trainer { filter } => trainer_console(filter).await,
        Cmd::Probe { target, seconds, write } => {
            let writes = write.iter().map(|w| le_mollet::probe::parse_write(w)).collect::<anyhow::Result<Vec<_>>>()?;
            le_mollet::probe::run(target.as_deref(), seconds, &writes).await
        }
        Cmd::Stats { course } => {
            print_course_stats(&course, &Course::load(&course)?);
            Ok(())
        }
        Cmd::Cap { course, max } => {
            let mut c = Course::load(&course)?;
            c.cap_grade(max);
            std::fs::write(&course, serde_json::to_string(&c)?)?;
            let path = course.clone();
            tokio::task::spawn_blocking(move || build_terrain(&path)).await??;
            print_course_stats(&course, &c);
            Ok(())
        }
        Cmd::Terrain { course } => tokio::task::spawn_blocking(move || build_terrain(&course)).await?,
        Cmd::Serve { addr, trainer, difficulty, courses, db, mode, lan } => {
            let addr = if lan { format!("0.0.0.0:{}", addr.rsplit(':').next().unwrap_or("8080")) } else { addr };
            serve(&addr, trainer, difficulty, &courses, &db, mode).await
        }
        Cmd::Role { action, account, role, note, db } => {
            use le_mollet::auth::{Grant, Role, Roles};
            let db = le_mollet::db::Db::open(&db, &[db_dir(&db)])?;
            let mut r = Roles::load(&db)?;
            let find = |name: &str| le_mollet::account::find(&db, name).with_context(|| format!("no account named {name}"));
            match (action.as_str(), account, role) {
                ("set", Some(name), Some(role)) => {
                    let a = find(&name)?;
                    let role = Role::parse(&role).context("role: admin, organizer or rider")?;
                    r.grants.insert(a.id, Grant { role, note });
                    r.save(&db)?;
                    println!("{} is now {role:?} on this server (effective at once).", a.name);
                }
                ("remove", Some(name), _) => {
                    let a = find(&name)?;
                    anyhow::ensure!(r.grants.remove(&a.id).is_some(), "{} has no role", a.name);
                    r.save(&db)?;
                    println!("{} is a rider again.", a.name);
                }
                ("list", _, _) => r.grants.iter().for_each(|(id, g)| {
                    let name = le_mollet::account::by_id(&db, *id).map_or_else(|| format!("#{id}"), |a| a.name);
                    println!("{name:<24} {:<10} {}", format!("{:?}", g.role), g.note)
                }),
                _ => anyhow::bail!("usage: le-mollet role set <account> admin|organizer|rider [--note X] | remove <account> | list"),
            }
            Ok(())
        }
        Cmd::District { city, bbox } => {
            let v: Vec<f64> = bbox.split(',').map(|x| x.trim().parse()).collect::<Result<_, _>>().context("bbox: expected south,west,north,east")?;
            let bbox: [f64; 4] = v.try_into().map_err(|_| anyhow::anyhow!("bbox: expected 4 numbers"))?;
            let dir = district_dir(&city);
            tokio::task::spawn_blocking(move || le_mollet::district::build(&dir, bbox)).await?
        }
        Cmd::Ortho { course } => tokio::task::spawn_blocking(move || {
            let head = std::fs::read(terrain_path(&course)).with_context(|| format!("{course}: build its terrain first"))?;
            let f = |o: usize| f64::from_le_bytes(head[o..o + 8].try_into().unwrap());
            let u = |o: usize| u32::from_le_bytes(head[o..o + 4].try_into().unwrap()) as usize;
            le_mollet::ortho::build(&ortho_dir(&course), [f(8), f(16), f(24), f(32)], u(0), u(4))
        }).await?,
        Cmd::Island { map } => {
            let out = format!("{}.city.json", map.strip_suffix(".map.json").context("map file must end in .map.json")?);
            let i = le_mollet::island::build(&map)?;
            std::fs::write(&out, serde_json::to_string(&i.city)?)?;
            std::fs::write(terrain_path(&out), i.terrain.to_bytes())?;
            println!("{out}: {} roads, {} decor pieces; terrain {}x{} cells", i.city.streets.len(), i.city.props.len(), i.terrain.w, i.terrain.h);
            Ok(())
        }
        Cmd::Decor { course } => tokio::task::spawn_blocking(move || build_decor(&course)).await?,
        Cmd::Buildings { city } => tokio::task::spawn_blocking(move || build_buildings(&city)).await?,
        Cmd::City { name, out, relation, start } => {
            let start = parse_latlon(&start)?;
            let (mut city, terrain) = tokio::task::spawn_blocking(move || le_mollet::city::build(&name, relation, start)).await??;
            // Hand-picked start places survive a rebuild.
            if let Ok(old) = le_mollet::city::City::load(&out) { city.starts = old.starts }
            std::fs::write(&out, serde_json::to_string(&city)?)?;
            std::fs::write(terrain_path(&out), terrain.to_bytes())?;
            let city_path = out.clone();
            tokio::task::spawn_blocking(move || build_buildings(&city_path)).await??;
            let km: f64 = city.streets.iter().map(|s| s.pts.windows(2).map(|w| ((w[1][0] - w[0][0]) * 110_540.0).hypot((w[1][1] - w[0][1]) * 73_000.0)).sum::<f64>()).sum::<f64>() / 1000.0;
            println!("{out}: {} streets, ~{km:.0} km; terrain {}x{} cells", city.streets.len(), terrain.w, terrain.h);
            Ok(())
        }
        Cmd::Course { name, out, road, waypoints, relation, cell } => {
            let wps = waypoints.iter().map(|w| parse_latlon(w)).collect::<anyhow::Result<Vec<[f64; 2]>>>()?;
            if relation.is_none() && wps.len() < 2 { anyhow::bail!("need a start and a finish waypoint, or --relation") }
            let mut c = tokio::task::spawn_blocking(move || match relation {
                Some(id) => Course::build_ring(&name, id, wps.first().copied()),
                None => Course::build(&name, &wps, &road),
            })
            .await??;
            c.cell_m = cell;
            std::fs::write(&out, serde_json::to_string(&c)?)?;
            let course_path = out.clone();
            tokio::task::spawn_blocking(move || build_terrain(&course_path)).await??;
            print_course_stats(&out, &c);
            Ok(())
        }
    }
}

fn parse_latlon(w: &str) -> anyhow::Result<[f64; 2]> {
    let (lat, lon) = w.split_once(',').with_context(|| format!("{w:?}: expected lat,lon"))?;
    Ok([lat.trim().parse()?, lon.trim().parse()?])
}

async fn trainer_console(filter: Option<String>) -> anyhow::Result<()> {
    eprintln!("scanning...");
    let trainer = Trainer::connect(filter.as_deref(), Duration::from_secs(20)).await?;
    eprintln!("connected to {:?} via {:?}. commands: erg <W> | grade <%> | quit", trainer.name, trainer.protocol);

    let mut samples = trainer.samples().await?;
    tokio::spawn(async move {
        while let Some(s) = samples.next().await {
            let f = |v: Option<f32>| v.map_or("-".into(), |v| format!("{v:.1}"));
            println!(
                "power {:>4} W | cadence {:>5} rpm | speed {:>5} km/h",
                s.power_w.map_or("-".into(), |p| p.to_string()),
                f(s.cadence_rpm),
                f(s.speed_kmh)
            );
        }
    });

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = lines.next_line().await? {
        let mut it = line.split_whitespace();
        let res = match (it.next(), it.next().map(str::parse::<f32>)) {
            (Some("erg"), Some(Ok(w))) if w >= 0.0 => trainer.set_erg(w as u16).await,
            (Some("grade"), Some(Ok(g))) => trainer.set_grade(g).await,
            (Some("quit"), _) => break,
            (None, _) => continue,
            _ => {
                eprintln!("? erg <W> | grade <%> | quit");
                continue;
            }
        };
        if let Err(e) = res {
            eprintln!("command failed: {e:#}")
        }
    }
    trainer.disconnect().await
}

/// Where the database sits: an older roles.json is looked for there too.
fn db_dir(db: &str) -> &str {
    std::path::Path::new(db).parent().and_then(|p| p.to_str()).filter(|p| !p.is_empty()).unwrap_or(".")
}

async fn serve(addr: &str, filter: Option<String>, difficulty: f32, courses: &str, db: &str, mode: web::Mode) -> anyhow::Result<()> {
    let trainer = match filter {
        Some(f) => {
            eprintln!("scanning...");
            let t = Trainer::connect(Some(&f), Duration::from_secs(20)).await?;
            eprintln!("connected to {:?} via {:?}, difficulty {:.0} %", t.name, t.protocol, difficulty * 100.0);
            Some(t)
        }
        None => None,
    };
    let db = le_mollet::db::Db::open(db, &[courses, db_dir(db)])?;
    let app = Arc::new(web::App::new(courses, trainer, difficulty, db, mode)?);
    if mode == web::Mode::Local && addr.starts_with("0.0.0.0:") {
        // The address other PCs on the network use; connecting a UDP socket sends nothing.
        let ip = std::net::UdpSocket::bind("0.0.0.0:0").and_then(|u| u.connect("192.0.2.1:9").and(u.local_addr()));
        if let Ok(ip) = ip { eprintln!("LAN: other riders open http://{}:{}", ip.ip(), addr.rsplit(':').next().unwrap_or("8080")) }
    }
    eprintln!("{} course(s) from {courses}/, {}", app.course_count(), if app.trainer.is_some() { "trainer mode" } else { "demo mode" });
    if let Some(t) = &app.trainer {
        let mut samples = t.samples().await?;
        let a = app.clone();
        tokio::spawn(async move {
            while let Some(s) = samples.next().await {
                let mut last = a.trainer_sample.lock().unwrap();
                *last = (s.power_w.map_or(last.0, |p| p as f32), s.cadence_rpm);
            }
        });
    }
    tokio::select! {
        r = web::serve(addr, app.clone()) => r?,
        _ = tokio::signal::ctrl_c() => {}
    }
    if let Some(t) = &app.trainer {
        let _ = t.set_grade(0.0).await;
        t.disconnect().await?;
    }
    Ok(())
}

/// Terrain file lives next to the course: `x.json` -> `x.terrain.bin`.
pub fn terrain_path(course: &str) -> String {
    format!("{}.terrain.bin", course.strip_suffix(".json").unwrap_or(course))
}

fn build_terrain(course_path: &str) -> anyhow::Result<()> {
    let c = Course::load(course_path)?;
    let t = le_mollet::terrain::Terrain::build(&c)?;
    let out = terrain_path(course_path);
    std::fs::write(&out, t.to_bytes())?;
    println!("{out}: {}x{} cells of {} m", t.w, t.h, c.cell_m.unwrap_or(le_mollet::terrain::CELL_M));
    Ok(())
}

/// Paint the course's terrain from the aerial photo and place its decor, in place.
fn build_decor(course_path: &str) -> anyhow::Result<()> {
    let mut c = Course::load(course_path)?;
    let tp = terrain_path(course_path);
    let mut t = le_mollet::terrain::Terrain::from_bytes(&std::fs::read(&tp).with_context(|| format!("{course_path}: build its terrain first"))?)?;
    le_mollet::decor::build(&mut c, &mut t)?;
    std::fs::write(&tp, t.to_bytes())?;
    std::fs::write(course_path, serde_json::to_string(&c)?)?;
    let mut kinds = std::collections::BTreeMap::new();
    for p in &c.props { *kinds.entry(p.k.as_str()).or_insert(0) += 1 }
    println!("{course_path}: {} props {kinds:?}", c.props.len());
    Ok(())
}

/// Aerial photo tiles next to a course: `x.json` -> `x.ortho/`.
pub fn ortho_dir(course: &str) -> String {
    format!("{}.ortho", course.strip_suffix(".json").unwrap_or(course))
}

/// Realistic district next to a city: `x.city.json` -> `x.city.district/`.
pub fn district_dir(city: &str) -> String {
    format!("{}.district", city.strip_suffix(".json").unwrap_or(city))
}

/// Buildings file next to a city: `x.city.json` -> `x.city.buildings.bin`.
pub fn buildings_path(city: &str) -> String {
    format!("{}.buildings.bin", city.strip_suffix(".json").unwrap_or(city))
}

/// Fetch the buildings over the box of the city's terrain file (its 40-byte header).
fn build_buildings(city_path: &str) -> anyhow::Result<()> {
    let head = std::fs::read(terrain_path(city_path)).with_context(|| format!("{city_path}: build its terrain first"))?;
    let f = |o: usize| f64::from_le_bytes(head[o..o + 8].try_into().unwrap());
    let (west, south, east, north) = (f(8), f(16), f(24), f(32));
    let b = le_mollet::buildings::fetch(west, south, east, north)?;
    let out = buildings_path(city_path);
    std::fs::write(&out, le_mollet::buildings::to_bytes(&b, west, north))?;
    println!("{out}: {} buildings", b.len());
    Ok(())
}

/// Figures to compare with the climb's reference profile (courses/AGENTS.md checklist).
fn print_course_stats(path: &str, c: &Course) {
    let (first, last) = (c.points[0].ele, c.points.last().unwrap().ele);
    let (steep, down) = c.grade_extremes(100.0);
    let kms: Vec<String> = c.km_grades().iter().map(|g| format!("{g:.1}")).collect();
    println!("{path}: {}", c.name);
    println!("  length    {:.2} km", c.length() / 1000.0);
    println!("  altitude  {first:.0} m -> {last:.0} m (net {:+.0} m, gain +{:.0} m)", last - first, c.elevation_gain());
    println!("  average   {:.1} %", (last - first) / c.length() * 100.0);
    println!("  100 m     max {steep:.1} %, min {down:.1} %");
    println!("  per km    {}", kms.join(" / "));
}
