//! `le-mollet probe`: everything a Bluetooth device says, to support new hardware (docs/hardware.md). Without a
//! name, lists the devices around; with one, connects, maps its services and characteristics, reads what can be
//! read (model, firmware...), optionally writes a handshake, then logs every notification for a while. The output
//! is plain text, made to be pasted into an issue.

use anyhow::{Context, Result, bail};
use btleplug::api::{Central, CharPropFlags, Manager as _, Peripheral as _, ScanFilter, WriteType};
use btleplug::platform::{Manager, Peripheral};
use futures::StreamExt;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Names for the UUIDs that matter here: standard fitness ones and the known proprietary ones.
fn known(u: &Uuid) -> &'static str {
    let s = u.to_string();
    let short = s.strip_suffix("-0000-1000-8000-00805f9b34fb").and_then(|h| h.strip_prefix("0000"));
    match short.unwrap_or(&s) {
        "1826" => "Fitness Machine (FTMS)",
        "2acc" => "FTMS feature",
        "2ad2" => "Indoor Bike Data",
        "2ad3" => "Training Status",
        "2ad6" => "Supported Resistance Range",
        "2ad8" => "Supported Power Range",
        "2ad9" => "FTMS Control Point",
        "2ada" => "Fitness Machine Status",
        "1818" => "Cycling Power",
        "2a63" => "Cycling Power Measurement",
        "2a65" => "Cycling Power Feature",
        "2a66" => "Cycling Power Control Point",
        "1816" => "Cycling Speed and Cadence",
        "2a5b" => "CSC Measurement",
        "180d" => "Heart Rate",
        "2a37" => "Heart Rate Measurement",
        "180f" => "Battery",
        "2a19" => "Battery Level",
        "180a" => "Device Information",
        "2a29" => "Manufacturer Name",
        "2a24" => "Model Number",
        "2a25" => "Serial Number",
        "2a26" => "Firmware Revision",
        "2a27" => "Hardware Revision",
        "2a28" => "Software Revision",
        "fc82" => "Zwift Ride / Click",
        "a026e005-0a7d-4ab3-97fa-f1500f9feb8b" => "Wahoo trainer control",
        "00000001-19ca-4651-86e5-fa29dcdd09d1" => "Zwift controller service",
        "00000002-19ca-4651-86e5-fa29dcdd09d1" => "Zwift async (notify)",
        "00000003-19ca-4651-86e5-fa29dcdd09d1" => "Zwift sync RX (write)",
        "00000004-19ca-4651-86e5-fa29dcdd09d1" => "Zwift sync TX (indicate)",
        _ => "",
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")
}

/// Printable text, when the bytes are some (a model name, a firmware version).
fn text(b: &[u8]) -> Option<String> {
    let s = std::str::from_utf8(b).ok()?.trim_end_matches('\0');
    (s.len() >= 2 && s.chars().all(|c| !c.is_control())).then(|| format!("  \"{s}\""))
}

fn props(p: CharPropFlags) -> String {
    let all = [(CharPropFlags::READ, "read"), (CharPropFlags::WRITE, "write"), (CharPropFlags::WRITE_WITHOUT_RESPONSE, "write-no-resp"), (CharPropFlags::NOTIFY, "notify"), (CharPropFlags::INDICATE, "indicate")];
    all.iter().filter(|(f, _)| p.contains(*f)).map(|(_, n)| *n).collect::<Vec<_>>().join(",")
}

/// A write to send once subscribed, from `<characteristic uuid or short>=<hex|text:...>`.
pub fn parse_write(w: &str) -> Result<(String, Vec<u8>)> {
    let (uuid, data) = w.split_once('=').context("--write: expected <uuid>=<hex> or <uuid>=text:<ascii>")?;
    let bytes = match data.strip_prefix("text:") {
        Some(t) => t.as_bytes().to_vec(),
        None => (0..data.replace(' ', "").len()).step_by(2).map(|i| u8::from_str_radix(&data.replace(' ', "")[i..i + 2], 16)).collect::<Result<_, _>>().context("--write: bad hex")?,
    };
    Ok((uuid.to_lowercase(), bytes))
}

async fn scan(secs: u64) -> Result<Vec<Peripheral>> {
    let adapter = Manager::new().await?.adapters().await?.into_iter().next().context("no Bluetooth adapter")?;
    adapter.start_scan(ScanFilter::default()).await.context("start scan")?;
    tokio::time::sleep(Duration::from_secs(secs)).await;
    adapter.stop_scan().await.ok();
    Ok(adapter.peripherals().await?)
}

pub async fn run(target: Option<&str>, secs: u64, writes: &[(String, Vec<u8>)]) -> Result<()> {
    println!("le-mollet probe {} · {}", env!("CARGO_PKG_VERSION"), std::env::consts::OS);
    println!("scanning 8 s...");
    let found = scan(8).await?;
    let mut picked = None;
    println!("\n== devices around ({})", found.len());
    for p in &found {
        let Some(pr) = p.properties().await? else { continue };
        let name = pr.local_name.clone().unwrap_or_default();
        let services: Vec<String> = pr.services.iter().map(|u| format!("{u} {}", known(u)).trim().to_owned()).collect();
        println!("- {:<24} {}  rssi {}", if name.is_empty() { "(no name)" } else { &name }, pr.address, pr.rssi.map_or("?".into(), |r| r.to_string()));
        for s in services { println!("    service  {s}") }
        for (id, data) in &pr.manufacturer_data { println!("    maker    0x{id:04x}: {}", hex(data)) }
        for (u, data) in &pr.service_data { println!("    svc data {u}: {}", hex(data)) }
        if let Some(t) = target {
            let t = t.to_lowercase();
            if picked.is_none() && (name.to_lowercase().contains(&t) || pr.address.to_string().to_lowercase() == t) { picked = Some(p.clone()) }
        }
    }
    let Some(t) = target else {
        println!("\nNext: le-mollet probe \"<name or address>\" to map one device and log what it sends.");
        return Ok(());
    };
    let Some(p) = picked else { bail!("no device matching {t:?} (is it awake, and not connected to another app?)") };

    println!("\n== connecting to {t}");
    // BlueZ often drops the first try (a sleeping device, a stale link): three tries, like the trainer's.
    let mut tries = 1;
    while let Err(e) = async { p.connect().await.context("connect")?; p.discover_services().await.context("discover services") }.await {
        if tries == 3 { return Err(e.context("press a button to wake it, and close any other app using it")) }
        println!("try {tries} failed: {e:#}, again...");
        p.disconnect().await.ok();
        tokio::time::sleep(Duration::from_secs(2)).await;
        tries += 1;
    }
    let mut listen = vec![];
    for s in p.services() {
        println!("service {} {}", s.uuid, known(&s.uuid));
        for c in &s.characteristics {
            print!("  char {} [{}] {}", c.uuid, props(c.properties), known(&c.uuid));
            if c.properties.contains(CharPropFlags::READ) {
                match p.read(c).await {
                    Ok(v) => print!("\n      = {}{}", hex(&v), text(&v).unwrap_or_default()),
                    Err(e) => print!("\n      read failed: {e}"),
                }
            }
            println!();
            if c.properties.intersects(CharPropFlags::NOTIFY | CharPropFlags::INDICATE) { listen.push(c.clone()) }
        }
    }
    for c in &listen {
        if let Err(e) = p.subscribe(c).await { println!("subscribe {} failed: {e}", c.uuid) }
    }
    let mut notes = p.notifications().await?;
    let chars: Vec<_> = p.characteristics().into_iter().collect();
    for (u, data) in writes {
        let Some(c) = chars.iter().find(|c| c.uuid.to_string() == *u || c.uuid.to_string().starts_with(&format!("0000{u}-"))) else { println!("write: no characteristic {u}"); continue };
        let kind = if c.properties.contains(CharPropFlags::WRITE_WITHOUT_RESPONSE) { WriteType::WithoutResponse } else { WriteType::WithResponse };
        match p.write(c, data, kind).await {
            Ok(()) => println!("wrote {} <- {}", c.uuid, hex(data)),
            Err(e) => println!("write {} failed: {e}", c.uuid),
        }
    }
    println!("\n== listening {secs} s: pedal, press each button one at a time, shift up and down (Ctrl-C to stop)");
    let start = Instant::now();
    let end = tokio::time::sleep(Duration::from_secs(secs));
    tokio::pin!(end);
    loop {
        tokio::select! {
            _ = &mut end => break,
            _ = tokio::signal::ctrl_c() => break,
            n = notes.next() => match n {
                Some(n) => println!("{:>8.3}s {} {}  {}", start.elapsed().as_secs_f32(), n.uuid, known(&n.uuid), hex(&n.value)),
                None => break,
            },
        }
    }
    p.disconnect().await.ok();
    println!("== done");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_names() {
        assert_eq!(parse_write("00000003-19ca-4651-86e5-fa29dcdd09d1=text:RideOn").unwrap().1, b"RideOn");
        assert_eq!(parse_write("2ad9=00 07").unwrap(), ("2ad9".into(), vec![0, 7]));
        assert!(parse_write("2ad9=zz").is_err() && parse_write("2ad9").is_err());
        assert_eq!(known(&Uuid::parse_str("00002ad2-0000-1000-8000-00805f9b34fb").unwrap()), "Indoor Bike Data");
        assert_eq!(text(b"1.2.0\0"), Some("  \"1.2.0\"".into()));
        assert_eq!(text(&[1, 2]), None);
    }
}
