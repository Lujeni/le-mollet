//! Road cycling physics: power in, speed out.

/// Rider + bike parameters.
#[derive(Debug, Clone, Copy)]
pub struct Rider {
    pub mass_kg: f32, // rider + bike
    pub cda: f32,     // drag area, m²
    pub crr: f32,     // rolling resistance coefficient
}

impl Default for Rider {
    fn default() -> Self {
        // ponytail: hoods position on a road bike, expose per bike once the garage has weights
        Self { mass_kg: 83.0, cda: 0.32, crr: 0.004 }
    }
}

const G: f32 = 9.81;
const DRIVETRAIN_EFFICIENCY: f32 = 0.976;
/// Rotating wheels add inertia, roughly equivalent to this extra mass.
const WHEEL_INERTIA_KG: f32 = 1.4;

/// ISA air density at altitude (15 °C at sea level). ~1.0 kg/m³ at Alpe d'Huez summit.
pub fn air_density(alt_m: f32) -> f32 {
    1.225 * (1.0 - 2.25577e-5 * alt_m).powf(4.2559)
}

/// Advance speed `v` (m/s) by `dt` seconds given rider power and road grade (%).
pub fn step(v: f32, power_w: f32, grade_pct: f32, alt_m: f32, r: &Rider, dt: f32) -> f32 {
    let theta = (grade_pct / 100.0).atan();
    let resist = r.mass_kg * G * (theta.sin() + r.crr * theta.cos()) + 0.5 * air_density(alt_m) * r.cda * v * v;
    // Drive force P/v blows up at standstill; cap it as if moving at 1 m/s.
    let drive = DRIVETRAIN_EFFICIENCY * power_w.max(0.0) / v.max(1.0);
    let a = (drive - resist) / (r.mass_kg + WHEEL_INERTIA_KG);
    (v + a * dt).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steady(power: f32, grade: f32) -> f32 {
        let r = Rider::default();
        let mut v = 0.0;
        for _ in 0..3000 {
            v = step(v, power, grade, 0.0, &r, 0.1);
        }
        v * 3.6
    }

    #[test]
    fn realistic_speeds() {
        let flat = steady(200.0, 0.0);
        assert!((32.0..37.0).contains(&flat), "flat 200 W = {flat} km/h");
        let climb = steady(250.0, 8.0);
        assert!((12.0..13.5).contains(&climb), "8% 250 W = {climb} km/h");
        assert_eq!(steady(0.0, 5.0), 0.0); // no power uphill: stays still
        assert!(steady(0.0, -6.0) > 50.0); // freewheel downhill
    }

    #[test]
    fn thinner_air_is_faster() {
        assert!((air_density(1800.0) - 1.02).abs() < 0.02);
        let r = Rider::default();
        assert!(step(10.0, 200.0, 0.0, 1800.0, &r, 1.0) > step(10.0, 200.0, 0.0, 0.0, &r, 1.0));
    }
}
