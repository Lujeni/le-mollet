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
    /// Print the validation figures of a course file (see courses/AGENTS.md)
    Stats {
        /// Course JSON file
        course: String,
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
    /// Build a course file from waypoints, see docs/courses.md
    Course {
        /// Display name, e.g. "Alpe d'Huez"
        name: String,
        /// Output JSON file
        #[arg(long, short)]
        out: String,
        /// Road ref to stick to, repeatable, e.g. --road "D 211" --road "D 211F"
        #[arg(long)]
        road: Vec<String>,
        /// "lat,lon" waypoints: start, optional vias, finish
        #[arg(required = true, num_args = 2..)]
        waypoints: Vec<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().cmd {
        Cmd::Trainer { filter } => trainer_console(filter).await,
        Cmd::Stats { course } => {
            print_course_stats(&course, &Course::load(&course)?);
            Ok(())
        }
        Cmd::Terrain { course } => tokio::task::spawn_blocking(move || build_terrain(&course)).await?,
        Cmd::Serve { addr, trainer, difficulty, courses } => serve(&addr, trainer, difficulty, &courses).await,
        Cmd::City { name, out, relation, start } => {
            let start = parse_latlon(&start)?;
            let (city, terrain) = tokio::task::spawn_blocking(move || le_mollet::city::build(&name, relation, start)).await??;
            std::fs::write(&out, serde_json::to_string(&city)?)?;
            std::fs::write(terrain_path(&out), terrain.to_bytes())?;
            let km: f64 = city.streets.iter().map(|s| s.pts.windows(2).map(|w| ((w[1][0] - w[0][0]) * 110_540.0).hypot((w[1][1] - w[0][1]) * 73_000.0)).sum::<f64>()).sum::<f64>() / 1000.0;
            println!("{out}: {} streets, ~{km:.0} km; terrain {}x{} cells", city.streets.len(), terrain.w, terrain.h);
            Ok(())
        }
        Cmd::Course { name, out, road, waypoints } => {
            let wps = waypoints.iter().map(|w| parse_latlon(w)).collect::<anyhow::Result<Vec<[f64; 2]>>>()?;
            let c = tokio::task::spawn_blocking(move || Course::build(&name, &wps, &road)).await??;
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

async fn serve(addr: &str, filter: Option<String>, difficulty: f32, courses: &str) -> anyhow::Result<()> {
    let trainer = match filter {
        Some(f) => {
            eprintln!("scanning...");
            let t = Trainer::connect(Some(&f), Duration::from_secs(20)).await?;
            eprintln!("connected to {:?} via {:?}, difficulty {:.0} %", t.name, t.protocol, difficulty * 100.0);
            Some(t)
        }
        None => None,
    };
    let app = Arc::new(web::App::new(courses, trainer, difficulty)?);
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
    println!("{out}: {}x{} cells of {} m", t.w, t.h, le_mollet::terrain::CELL_M);
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
