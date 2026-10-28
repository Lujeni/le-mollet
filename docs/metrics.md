# Metrics (Prometheus)

`GET /metrics` serves usage and performance in the Prometheus text format (`src/metrics.rs`, no dependency).

- **At home** (`--mode local`) it is open, like the rest of the server.
- **On a public server** it answers only with `Authorization: Bearer <LE_MOLLET_METRICS_TOKEN>`; without that
  variable it is `404`.

```yaml
# prometheus.yml
scrape_configs:
  - job_name: le-mollet
    scrape_interval: 15s
    authorization: { credentials: "<LE_MOLLET_METRICS_TOKEN>" }   # cloud only
    static_configs: [{ targets: ["localhost:8080"] }]
```

Labels only take values from fixed sets (routes, kinds, reasons) or track ids, never anything a rider types.

## Usage

| Metric | Type | Labels |
|---|---|---|
| `le_mollet_riders_online` | gauge | `track` (pace partners excluded) |
| `le_mollet_rides_total` | counter | `kind` (climb, loop, world, city, workout, zombie, race, group), `trust` (demo, flagged, device) |
| `le_mollet_ride_seconds_total`, `_ride_km_total`, `_ride_kj_total` | counter | `kind` |
| `le_mollet_fair_flags_total` | counter | `why` (flat, spike, no cadence, beyond W′, ...) |
| `le_mollet_segment_efforts_total` | counter | `kind` (kom, sprint) |
| `le_mollet_race_finishes_total` | counter | `result` (ranked, demo, flagged, above rank) |
| `le_mollet_events_upcoming`, `le_mollet_event_entrants` | gauge | `kind` (race, group) |
| `le_mollet_event_signups_total`, `le_mollet_events_created_total` | counter | `kind`, `by` (organizer, club) |
| `le_mollet_emotes_total` | counter | `e` |
| `le_mollet_powerups_total`, `le_mollet_worlds_generated_total` | counter | |
| `le_mollet_club_actions_total` | counter | `action` (create, join, leave) |
| `le_mollet_signins_total` | counter | `result` (ok, refused, error) |
| `le_mollet_riders_registered`, `_riders_flagged`, `_clubs`, `_club_members`, `_signed_in` | gauge | |

## Performance

| Metric | Type | Labels |
|---|---|---|
| `le_mollet_http_requests_total` | counter | `route` |
| `le_mollet_http_responses_total` | counter | `code` |
| `le_mollet_http_request_duration_seconds` | histogram | `route` (ride streams excluded: they last the ride) |
| `le_mollet_streams_open` | gauge | ride streams (SSE) open |
| `le_mollet_ride_tick_seconds` | histogram | work of one 100 ms tick of a ride, all rides |
| `le_mollet_frame_bytes_total` | counter | bytes of ride frames sent |
| `le_mollet_db_seconds` | histogram | `op` (get, put, delete, replace, all) |
| `le_mollet_db_errors_total` | counter | `op` |
| `le_mollet_rejected_total` | counter | `reason` (cross_site, not_your_ride, profile_owner) |
| `le_mollet_tracks` | gauge | `type` |
| `le_mollet_trainer_connected`, `_uptime_seconds`, `_resident_memory_bytes` | gauge | |

## Useful queries

```promql
sum(le_mollet_riders_online)                                         # riders on the road
sum by (kind) (rate(le_mollet_rides_total[1h])) * 3600               # rides per hour, by kind
sum(rate(le_mollet_ride_km_total[1d])) * 86400                       # km ridden per day
sum by (trust) (rate(le_mollet_rides_total[1d]))                     # share of demo / device / flagged rides
histogram_quantile(0.99, sum by (le) (rate(le_mollet_ride_tick_seconds_bucket[5m])))   # p99 tick: keep well under 0.1 s
histogram_quantile(0.95, sum by (le, route) (rate(le_mollet_http_request_duration_seconds_bucket[5m])))
sum(rate(le_mollet_frame_bytes_total[1m])) * 8                       # bandwidth to pages, bit/s
sum by (code) (rate(le_mollet_http_responses_total[5m]))
```

A ride tick that nears 100 ms means the server falls behind real time: that is the first number to watch as
riders grow.
