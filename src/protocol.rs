//! Pure BLE payload encoding/decoding. No I/O, fully unit-tested.
//!
//! Supports two control protocols:
//! - FTMS (Bluetooth SIG Fitness Machine Service 0x1826), newer KICKR firmware.
//! - Wahoo proprietary trainer characteristic inside Cycling Power Service, used by
//!   KICKR 2 (2016) and older firmware that lack FTMS.

use uuid::Uuid;

const fn sig(short: u16) -> Uuid {
    Uuid::from_u128(((short as u128) << 96) | 0x0000_0000_0000_1000_8000_0080_5F9B_34FB)
}

pub const CYCLING_POWER_SERVICE: Uuid = sig(0x1818);
pub const CYCLING_POWER_MEASUREMENT: Uuid = sig(0x2A63);
pub const FTMS_SERVICE: Uuid = sig(0x1826);
pub const FTMS_INDOOR_BIKE_DATA: Uuid = sig(0x2AD2);
pub const FTMS_CONTROL_POINT: Uuid = sig(0x2AD9);
pub const WAHOO_TRAINER: Uuid = Uuid::from_u128(0xA026E005_0A7D_4AB3_97FA_F1500F9FEB8B);

/// One telemetry sample. `None` = trainer did not send this field.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Sample {
    pub power_w: Option<i16>,
    pub cadence_rpm: Option<f32>,
    pub speed_kmh: Option<f32>,
}

/// Rider/bike params for simulation (grade) mode.
#[derive(Debug, Clone, Copy)]
pub struct SimParams {
    pub weight_kg: f32, // rider + bike
    pub crr: f32,       // rolling resistance coefficient
    pub cw: f32,        // wind resistance coefficient, kg/m (0.5 * rho * CdA)
}

impl Default for SimParams {
    fn default() -> Self {
        // ponytail: generic road-bike defaults, expose in UI once rider profiles exist
        Self { weight_kg: 85.0, crr: 0.004, cw: 0.51 }
    }
}

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

// ---------- FTMS ----------

/// Parse FTMS Indoor Bike Data (0x2AD2).
pub fn parse_indoor_bike_data(b: &[u8]) -> Option<Sample> {
    let flags = u16_at(b, 0)?;
    let mut i = 2;
    let mut s = Sample::default();
    // Bit 0 is inverted: 0 means instantaneous speed IS present.
    if flags & 1 == 0 {
        s.speed_kmh = Some(u16_at(b, i)? as f32 / 100.0);
        i += 2;
    }
    if flags & (1 << 1) != 0 { i += 2 } // average speed
    if flags & (1 << 2) != 0 {
        s.cadence_rpm = Some(u16_at(b, i)? as f32 / 2.0);
        i += 2;
    }
    if flags & (1 << 3) != 0 { i += 2 } // average cadence
    if flags & (1 << 4) != 0 { i += 3 } // total distance (u24)
    if flags & (1 << 5) != 0 { i += 2 } // resistance level
    if flags & (1 << 6) != 0 {
        s.power_w = Some(u16_at(b, i)? as i16);
    }
    Some(s)
}

pub const FTMS_REQUEST_CONTROL: [u8; 1] = [0x00];
pub const FTMS_START: [u8; 1] = [0x07];

pub fn ftms_set_target_power(watts: i16) -> Vec<u8> {
    let mut v = vec![0x05];
    v.extend_from_slice(&watts.to_le_bytes());
    v
}

/// `grade_pct` e.g. 5.0 for 5%.
pub fn ftms_set_sim(grade_pct: f32, p: SimParams) -> Vec<u8> {
    let mut v = vec![0x11];
    v.extend_from_slice(&0i16.to_le_bytes()); // wind speed, 0.001 m/s
    v.extend_from_slice(&((grade_pct * 100.0).round() as i16).to_le_bytes()); // 0.01 %
    v.push((p.crr * 10_000.0).round().clamp(0.0, 255.0) as u8); // 0.0001
    v.push((p.cw * 100.0).round().clamp(0.0, 255.0) as u8); // 0.01 kg/m
    v
}

// ---------- Cycling Power + Wahoo proprietary ----------

/// Cycling Power Measurement (0x2A63) parser. Stateful: cadence and speed are
/// derived from cumulative revolution counters between two notifications.
#[derive(Debug, Default)]
pub struct CpsDecoder {
    pub wheel_circumference_m: f32,
    last_wheel: Option<(u32, u16)>,
    last_crank: Option<(u16, u16)>,
}

impl CpsDecoder {
    pub fn new() -> Self {
        // ponytail: 700x25c default, KICKR reports virtual wheel speed based on it
        Self { wheel_circumference_m: 2.096, ..Default::default() }
    }

    pub fn parse(&mut self, b: &[u8]) -> Option<Sample> {
        let flags = u16_at(b, 0)?;
        let mut s = Sample { power_w: Some(u16_at(b, 2)? as i16), ..Default::default() };
        let mut i = 4;
        if flags & 1 != 0 { i += 1 } // pedal power balance
        if flags & (1 << 2) != 0 { i += 2 } // accumulated torque
        if flags & (1 << 4) != 0 {
            let (revs, t) = (u32_at(b, i)?, u16_at(b, i + 4)?);
            i += 6;
            if let Some((prev_revs, prev_t)) = self.last_wheel.replace((revs, t)) {
                let dt = t.wrapping_sub(prev_t) as f32 / 2048.0;
                let dr = revs.wrapping_sub(prev_revs) as f32;
                // dt == 0: no new wheel event, keep last known speed unknown.
                if dt > 0.0 {
                    s.speed_kmh = Some(dr * self.wheel_circumference_m / dt * 3.6);
                }
            }
        }
        if flags & (1 << 5) != 0 {
            let (revs, t) = (u16_at(b, i)?, u16_at(b, i + 2)?);
            if let Some((prev_revs, prev_t)) = self.last_crank.replace((revs, t)) {
                let dt = t.wrapping_sub(prev_t) as f32 / 1024.0;
                let dr = revs.wrapping_sub(prev_revs) as f32;
                if dt > 0.0 {
                    s.cadence_rpm = Some(dr / dt * 60.0);
                }
            }
        }
        Some(s)
    }
}

/// Must be written once before the KICKR accepts any other Wahoo command.
pub const WAHOO_UNLOCK: [u8; 3] = [0x20, 0xEE, 0xFC];

pub fn wahoo_set_erg(watts: u16) -> Vec<u8> {
    let mut v = vec![0x42];
    v.extend_from_slice(&watts.to_le_bytes());
    v
}

pub fn wahoo_set_sim(p: SimParams) -> Vec<u8> {
    let mut v = vec![0x43];
    v.extend_from_slice(&((p.weight_kg * 100.0).round() as u16).to_le_bytes());
    v.extend_from_slice(&((p.crr * 10_000.0).round() as u16).to_le_bytes());
    v.extend_from_slice(&((p.cw * 1000.0).round() as u16).to_le_bytes());
    v
}

/// `grade_pct` e.g. 5.0 for 5%. Encoded as (fraction + 1) * 32768.
pub fn wahoo_set_grade(grade_pct: f32) -> Vec<u8> {
    let g = (grade_pct / 100.0).clamp(-1.0, 1.0);
    let raw = ((g + 1.0) * 32768.0).round().min(u16::MAX as f32) as u16;
    let mut v = vec![0x46];
    v.extend_from_slice(&raw.to_le_bytes());
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuids() {
        assert_eq!(FTMS_SERVICE.to_string(), "00001826-0000-1000-8000-00805f9b34fb");
        assert_eq!(WAHOO_TRAINER.to_string(), "a026e005-0a7d-4ab3-97fa-f1500f9feb8b");
    }

    #[test]
    fn indoor_bike_data() {
        // flags: speed present (bit0=0), cadence (bit2), power (bit6)
        let flags: u16 = (1 << 2) | (1 << 6);
        let mut b = flags.to_le_bytes().to_vec();
        b.extend_from_slice(&3050u16.to_le_bytes()); // 30.50 km/h
        b.extend_from_slice(&180u16.to_le_bytes()); // 90 rpm
        b.extend_from_slice(&250i16.to_le_bytes()); // 250 W
        let s = parse_indoor_bike_data(&b).unwrap();
        assert_eq!(s, Sample { power_w: Some(250), cadence_rpm: Some(90.0), speed_kmh: Some(30.5) });
        assert_eq!(parse_indoor_bike_data(&b[..5]), None); // truncated
    }

    #[test]
    fn cps_power_wheel_crank() {
        let mut d = CpsDecoder::new();
        let pkt = |watts: i16, wrevs: u32, wt: u16, crevs: u16, ct: u16| {
            let mut b = ((1u16 << 4) | (1 << 5)).to_le_bytes().to_vec();
            b.extend_from_slice(&watts.to_le_bytes());
            b.extend_from_slice(&wrevs.to_le_bytes());
            b.extend_from_slice(&wt.to_le_bytes());
            b.extend_from_slice(&crevs.to_le_bytes());
            b.extend_from_slice(&ct.to_le_bytes());
            b
        };
        let first = d.parse(&pkt(200, 100, 0, 10, 0)).unwrap();
        assert_eq!(first, Sample { power_w: Some(200), ..Default::default() });
        // 4 wheel revs in 1 s (2048 ticks), 1 crank rev in 1 s (1024 ticks) = 60 rpm
        let s = d.parse(&pkt(210, 104, 2048, 11, 1024)).unwrap();
        assert_eq!(s.power_w, Some(210));
        assert!((s.speed_kmh.unwrap() - 4.0 * 2.096 * 3.6).abs() < 0.01);
        assert!((s.cadence_rpm.unwrap() - 60.0).abs() < 0.01);
        // u16 time wrap-around
        let s = d.parse(&pkt(210, 108, 2048u16.wrapping_add(2048), 12, 1024u16.wrapping_add(1024))).unwrap();
        assert!((s.cadence_rpm.unwrap() - 60.0).abs() < 0.01);
    }

    #[test]
    fn commands() {
        assert_eq!(ftms_set_target_power(200), [0x05, 0xC8, 0x00]);
        assert_eq!(ftms_set_sim(5.0, SimParams::default()), [0x11, 0, 0, 0xF4, 0x01, 40, 51]);
        assert_eq!(wahoo_set_erg(300), [0x42, 0x2C, 0x01]);
        assert_eq!(wahoo_set_grade(0.0), [0x46, 0x00, 0x80]);
        assert_eq!(wahoo_set_grade(-100.0), [0x46, 0x00, 0x00]);
        assert_eq!(wahoo_set_grade(100.0), [0x46, 0xFF, 0xFF]);
        assert_eq!(wahoo_set_sim(SimParams::default()), [0x43, 0x34, 0x21, 40, 0, 0xFE, 0x01]);
    }
}
