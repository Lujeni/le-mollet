//! Prometheus metrics, usage and performance (docs/metrics.md): counters, gauges and histograms in one small
//! registry, rendered in the text format at `/metrics`. Labels only ever take values from fixed sets or track
//! ids, never anything a rider types.
//! ponytail: one mutex for every series; per-series atomics if a profile ever shows it

use std::collections::BTreeMap;
use std::fmt::Write;
use std::sync::{LazyLock, Mutex};

/// Every metric: name, type, help. A series of a name not listed here is a bug, caught by the test below.
pub const DEFS: &[(&str, &str, &str)] = &[
    // HTTP and the ride loop
    ("le_mollet_http_requests_total", "counter", "HTTP requests by route"),
    ("le_mollet_http_responses_total", "counter", "HTTP responses by status code"),
    ("le_mollet_http_request_duration_seconds", "histogram", "Time to answer a request, by route (ride streams excluded)"),
    ("le_mollet_streams_open", "gauge", "Ride streams (SSE) open now"),
    ("le_mollet_ride_tick_seconds", "histogram", "Work of one ride tick (100 ms of simulation), all rides"),
    ("le_mollet_frame_bytes_total", "counter", "Bytes of ride frames sent to pages"),
    ("le_mollet_db_seconds", "histogram", "Database calls, by operation"),
    ("le_mollet_db_errors_total", "counter", "Database calls that failed, by operation"),
    ("le_mollet_rejected_total", "counter", "Requests refused by the fair-play and security gates, by reason"),
    // Riding
    ("le_mollet_rides_total", "counter", "Rides ended, by kind and trust"),
    ("le_mollet_ride_seconds_total", "counter", "Time ridden, by kind"),
    ("le_mollet_ride_km_total", "counter", "Distance ridden, by kind"),
    ("le_mollet_ride_kj_total", "counter", "Work done by riders, kJ, by kind"),
    ("le_mollet_fair_flags_total", "counter", "Rides flagged by the power checks, by reason"),
    ("le_mollet_segment_efforts_total", "counter", "Segment efforts that reached a leaderboard, by kind"),
    ("le_mollet_race_finishes_total", "counter", "Race finishes, ranked or why not"),
    ("le_mollet_event_signups_total", "counter", "Event sign-ups, by kind"),
    ("le_mollet_events_created_total", "counter", "Events scheduled, by kind and by whom"),
    ("le_mollet_emotes_total", "counter", "Emotes sent, by emote"),
    ("le_mollet_powerups_total", "counter", "Road bonuses used"),
    ("le_mollet_club_actions_total", "counter", "Club creations, joins and departures"),
    ("le_mollet_worlds_generated_total", "counter", "Open worlds generated on request"),
    ("le_mollet_signins_total", "counter", "Sign-ups and sign-ins, by result (signup, ok, link, refused, locked)"),
    ("le_mollet_mails_total", "counter", "Sign-in links mailed, by result (sent, failed)"),
    ("le_mollet_feedback_total", "counter", "What riders told us from the page, by kind (bug, idea, feedback, hardware)"),
    // Read at scrape time
    ("le_mollet_riders_online", "gauge", "Riders on the road now, by track (pace partners excluded)"),
    ("le_mollet_events_upcoming", "gauge", "Events not over yet, by kind"),
    ("le_mollet_event_entrants", "gauge", "Riders signed up to events not over yet, by kind"),
    ("le_mollet_riders_registered", "gauge", "Profiles the server knows"),
    ("le_mollet_riders_flagged", "gauge", "Profiles with at least one flagged ride"),
    ("le_mollet_clubs", "gauge", "Clubs"),
    ("le_mollet_club_members", "gauge", "Club members"),
    ("le_mollet_signed_in", "gauge", "Signed-in browser sessions"),
    ("le_mollet_tracks", "gauge", "Tracks loaded, by type"),
    ("le_mollet_trainer_connected", "gauge", "1 when the server drives a trainer of its own"),
    ("le_mollet_uptime_seconds", "gauge", "Seconds since the server started"),
    ("le_mollet_resident_memory_bytes", "gauge", "Resident memory of the server"),
];

const HTTP_BUCKETS: &[f64] = &[0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0];
const TICK_BUCKETS: &[f64] = &[0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05];
const DB_BUCKETS: &[f64] = &[0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.25];

fn buckets(name: &str) -> &'static [f64] {
    match name {
        "le_mollet_ride_tick_seconds" => TICK_BUCKETS,
        "le_mollet_db_seconds" => DB_BUCKETS,
        _ => HTTP_BUCKETS,
    }
}

#[derive(Default)]
struct Hist {
    counts: Vec<u64>,
    sum: f64,
    n: u64,
}

#[derive(Default)]
struct Registry {
    /// Counters and gauges, by name then labels (`a="x",b="y"`).
    values: BTreeMap<&'static str, BTreeMap<String, f64>>,
    hists: BTreeMap<&'static str, BTreeMap<String, Hist>>,
}

static REG: LazyLock<Mutex<Registry>> = LazyLock::new(Mutex::default);

fn labels(l: &[(&str, &str)]) -> String {
    l.iter().map(|(k, v)| format!("{k}=\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n"))).collect::<Vec<_>>().join(",")
}

/// Add `v` to a counter (or a gauge kept up to date, like open streams).
pub fn add(name: &'static str, l: &[(&str, &str)], v: f64) {
    *REG.lock().unwrap().values.entry(name).or_default().entry(labels(l)).or_default() += v;
}

pub fn inc(name: &'static str, l: &[(&str, &str)]) {
    add(name, l, 1.0)
}

pub fn observe(name: &'static str, l: &[(&str, &str)], v: f64) {
    let b = buckets(name);
    let mut reg = REG.lock().unwrap();
    let h = reg.hists.entry(name).or_default().entry(labels(l)).or_insert_with(|| Hist { counts: vec![0; b.len()], ..Default::default() });
    for (i, le) in b.iter().enumerate() {
        if v <= *le {
            h.counts[i] += 1
        }
    }
    (h.sum, h.n) = (h.sum + v, h.n + 1);
}

/// Time a call into a histogram.
pub fn time<T>(name: &'static str, l: &[(&str, &str)], f: impl FnOnce() -> T) -> T {
    let t = std::time::Instant::now();
    let out = f();
    observe(name, l, t.elapsed().as_secs_f64());
    out
}

/// The text exposition: the registry, then `gauges` read by the caller at scrape time (name, labels, value).
/// A value read at scrape time: name, labels, value.
pub type Gauge = (&'static str, Vec<(&'static str, String)>, f64);

pub fn render(gauges: &[Gauge]) -> String {
    let reg = REG.lock().unwrap();
    let mut out = String::new();
    for (name, kind, help) in DEFS {
        let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} {kind}");
        for (l, v) in reg.values.get(name).into_iter().flatten() {
            let _ = writeln!(out, "{name}{} {v}", if l.is_empty() { String::new() } else { format!("{{{l}}}") });
        }
        for (l, h) in reg.hists.get(name).into_iter().flatten() {
            let sep = if l.is_empty() { "" } else { "," };
            for (le, c) in buckets(name).iter().zip(&h.counts) {
                let _ = writeln!(out, "{name}_bucket{{{l}{sep}le=\"{le}\"}} {c}");
            }
            let _ = writeln!(out, "{name}_bucket{{{l}{sep}le=\"+Inf\"}} {}", h.n);
            let braces = if l.is_empty() { String::new() } else { format!("{{{l}}}") };
            let _ = writeln!(out, "{name}_sum{braces} {}\n{name}_count{braces} {}", h.sum, h.n);
        }
        for (_, l, v) in gauges.iter().filter(|g| g.0 == *name) {
            let l: Vec<(&str, &str)> = l.iter().map(|(k, v)| (*k, v.as_str())).collect();
            let l = labels(&l);
            let _ = writeln!(out, "{name}{} {v}", if l.is_empty() { String::new() } else { format!("{{{l}}}") });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_exposition() {
        inc("le_mollet_emotes_total", &[("e", "rideon")]);
        add("le_mollet_emotes_total", &[("e", "rideon")], 2.0);
        observe("le_mollet_db_seconds", &[("op", "test")], 0.0003);
        observe("le_mollet_db_seconds", &[("op", "test")], 0.02);
        let out = render(&[("le_mollet_clubs", vec![], 3.0), ("le_mollet_riders_online", vec![("track", "a\"b".into())], 2.0)]);
        assert!(out.contains("# TYPE le_mollet_emotes_total counter\nle_mollet_emotes_total{e=\"rideon\"} 3\n"), "{out}");
        assert!(out.contains("le_mollet_db_seconds_bucket{op=\"test\",le=\"0.0005\"} 1\n"));
        assert!(out.contains("le_mollet_db_seconds_bucket{op=\"test\",le=\"+Inf\"} 2\n") && out.contains("le_mollet_db_seconds_count{op=\"test\"} 2\n"), "{out}");
        assert!(out.contains("le_mollet_clubs 3\n") && out.contains("le_mollet_riders_online{track=\"a\\\"b\"} 2\n"));
        // Every series has its HELP: nothing recorded under a name missing from DEFS.
        let reg = REG.lock().unwrap();
        for name in reg.values.keys().chain(reg.hists.keys()) {
            assert!(DEFS.iter().any(|d| d.0 == *name), "{name} not in DEFS")
        }
    }
}
