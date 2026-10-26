//! Minimal CLI: connect to a trainer, print telemetry, accept commands on stdin.
//!
//!   le-mollet [name-filter]     e.g. `le-mollet kickr`
//!   > erg 200                   hold 200 W
//!   > grade 4.5                 simulate 4.5 % slope
//!   > quit

use le_mollet::Trainer;
use futures::StreamExt;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter = std::env::args().nth(1);
    eprintln!("scanning...");
    let trainer = Trainer::connect(filter.as_deref(), Duration::from_secs(20)).await?;
    eprintln!("connected to {:?} via {:?}. commands: erg <W> | grade <%> | quit", trainer.name, trainer.protocol);

    let mut samples = trainer.samples().await?;
    tokio::spawn(async move {
        while let Some(s) = samples.next().await {
            let f = |v: Option<f32>| v.map_or("-".into(), |v| format!("{v:.1}"));
            println!("power {:>4} W | cadence {:>5} rpm | speed {:>5} km/h",
                s.power_w.map_or("-".into(), |p| p.to_string()), f(s.cadence_rpm), f(s.speed_kmh));
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
            _ => { eprintln!("? erg <W> | grade <%> | quit"); continue }
        };
        if let Err(e) = res { eprintln!("command failed: {e:#}") }
    }
    trainer.disconnect().await
}
