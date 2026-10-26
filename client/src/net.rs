//! Talking to the le-mollet server: the same HTTP API as the web page. Blocking calls run on
//! threads; ride frames arrive over Server-Sent Events into a channel the game polls each frame.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::io::{BufRead, BufReader, Read};
use std::sync::mpsc::{Receiver, channel};

#[derive(Clone)]
pub struct Server {
    base: String,
}

/// One server tick of the ride, as streamed by `/events` (fields the client uses).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Frame {
    pub power: f32,
    pub speed_kmh: f32,
    pub distance: f32,
    pub elapsed: f32,
    pub grade: f32,
    pub ele: f32,
}

impl Server {
    pub fn new(base: &str) -> Self {
        Self { base: base.trim_end_matches('/').to_owned() }
    }

    pub fn bytes(&self, path: &str) -> Result<Vec<u8>> {
        let r = ureq::get(&format!("{}{path}", self.base)).call().with_context(|| format!("GET {path}"))?;
        let mut b = vec![];
        r.into_reader().read_to_end(&mut b)?;
        Ok(b)
    }

    pub fn json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T> {
        Ok(serde_json::from_slice(&self.bytes(path)?).with_context(|| format!("parse {path}"))?)
    }

    /// Open the ride stream; frames come out of the returned channel until the server closes it.
    pub fn ride(&self, course: &str, session: u64, name: &str, mass: f32) -> Result<Receiver<Frame>> {
        let url = format!("{}/events?course={course}&mass={mass}&session={session}&name={}", self.base, name.replace(' ', "+"));
        let r = ureq::get(&url).call().context("open ride stream")?;
        if r.status() != 200 { bail!("ride stream: HTTP {}", r.status()) }
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(r.into_reader()).lines().map_while(Result::ok) {
                let Some(json) = line.strip_prefix("data: ") else { continue };
                if let Ok(f) = serde_json::from_str::<Frame>(json)
                    && tx.send(f).is_err()
                {
                    break; // game closed
                }
            }
        });
        Ok(rx)
    }

    /// Fire and forget: demo-mode power, or a teleport along the course.
    pub fn send(&self, path: String) {
        let url = format!("{}{path}", self.base);
        std::thread::spawn(move || {
            let _ = ureq::get(&url).call();
        });
    }
}
