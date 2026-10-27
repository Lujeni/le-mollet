//! le-mollet core: smart trainer engine over Bluetooth LE.

pub mod buildings;
pub mod city;
pub mod course;
pub mod district;
pub mod island;
pub mod ortho;
pub mod physics;
pub mod protocol;
pub mod ride;
pub mod terrain;
pub mod workout;
pub mod world;
pub mod zombie;

use anyhow::{Context, Result, bail};
use btleplug::api::{Central, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType};
use btleplug::platform::{Manager, Peripheral};
use futures::{Stream, StreamExt, future};
use protocol::*;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Protocol {
    /// Bluetooth SIG Fitness Machine Service.
    Ftms,
    /// Wahoo proprietary extension of Cycling Power (KICKR 2 and older firmware).
    Wahoo,
}

pub struct Trainer {
    peripheral: Peripheral,
    pub name: String,
    pub protocol: Protocol,
    telemetry: Characteristic,
    control: Characteristic,
    pub sim: SimParams,
}

impl Trainer {
    /// Scan for a trainer advertising FTMS or Cycling Power, optionally filtered by
    /// name substring (case-insensitive), connect and prepare it for control.
    pub async fn connect(name_filter: Option<&str>, timeout: Duration) -> Result<Self> {
        let manager = Manager::new().await?;
        let adapter = manager.adapters().await?.into_iter().next().context("no Bluetooth adapter")?;
        adapter
            .start_scan(ScanFilter { services: vec![FTMS_SERVICE, CYCLING_POWER_SERVICE] })
            .await
            .context("start scan")?;

        let filter = name_filter.map(str::to_lowercase);
        let deadline = tokio::time::Instant::now() + timeout;
        let (peripheral, name) = 'scan: loop {
            if tokio::time::Instant::now() > deadline {
                bail!("no trainer found within {timeout:?}");
            }
            for p in adapter.peripherals().await? {
                let Some(props) = p.properties().await? else { continue };
                let name = props.local_name.unwrap_or_default();
                let is_trainer = props.services.iter().any(|s| *s == FTMS_SERVICE || *s == CYCLING_POWER_SERVICE);
                let name_ok = filter.as_ref().is_none_or(|f| name.to_lowercase().contains(f));
                if is_trainer && name_ok {
                    break 'scan (p, name);
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        };
        adapter.stop_scan().await.context("stop scan")?;

        // ponytail: fixed 3 tries, BlueZ often times out the first BLE service discovery
        let mut attempt = 1;
        while let Err(e) = async {
            peripheral.connect().await.context("connect")?;
            peripheral.discover_services().await.context("discover services")
        }
        .await
        {
            if attempt == 3 {
                return Err(e);
            }
            eprintln!("connection attempt {attempt} failed: {e:#}, retrying");
            let _ = peripheral.disconnect().await;
            attempt += 1;
        }
        let find = |uuid| peripheral.characteristics().into_iter().find(|c| c.uuid == uuid);

        // Prefer standard FTMS; fall back to Wahoo proprietary control.
        let (protocol, telemetry, control) = match (find(FTMS_INDOOR_BIKE_DATA), find(FTMS_CONTROL_POINT)) {
            (Some(t), Some(c)) => (Protocol::Ftms, t, c),
            _ => (
                Protocol::Wahoo,
                find(CYCLING_POWER_MEASUREMENT).context("no Cycling Power Measurement characteristic")?,
                find(WAHOO_TRAINER).context("no FTMS nor Wahoo trainer control characteristic")?,
            ),
        };
        // Control points answer via indications; they must be enabled before writing.
        peripheral.subscribe(&telemetry).await?;
        peripheral.subscribe(&control).await?;
        // Debug: with FTMS, also dump Cycling Power to cross-check power readings.
        if protocol == Protocol::Ftms
            && std::env::var_os("LE_MOLLET_RAW").is_some()
            && let Some(cp) = find(CYCLING_POWER_MEASUREMENT)
        {
            peripheral.subscribe(&cp).await?;
        }

        let t = Self { peripheral, name, protocol, telemetry, control, sim: SimParams::default() };
        match protocol {
            Protocol::Ftms => {
                t.write(&FTMS_REQUEST_CONTROL).await?;
                t.write(&FTMS_START).await?;
            }
            Protocol::Wahoo => {
                t.write(&WAHOO_UNLOCK).await?;
                t.write(&wahoo_set_sim(t.sim)).await?;
            }
        }
        Ok(t)
    }

    async fn write(&self, data: &[u8]) -> Result<()> {
        self.peripheral.write(&self.control, data, WriteType::WithResponse).await?;
        Ok(())
    }

    /// Live telemetry stream (power, cadence, speed).
    pub async fn samples(&self) -> Result<impl Stream<Item = Sample> + Send + use<>> {
        let (uuid, protocol) = (self.telemetry.uuid, self.protocol);
        let mut cps = CpsDecoder::new();
        let raw = std::env::var_os("LE_MOLLET_RAW").is_some();
        Ok(self.peripheral.notifications().await?.filter_map(move |n| {
            if raw {
                let hex: Vec<_> = n.value.iter().map(|b| format!("{b:02x}")).collect();
                eprintln!("raw {} {}", n.uuid, hex.join(" "));
            }
            future::ready(match (n.uuid == uuid, protocol) {
                (false, _) => None, // control point acks: visible with LE_MOLLET_RAW=1
                (true, Protocol::Ftms) => parse_indoor_bike_data(&n.value),
                (true, Protocol::Wahoo) => cps.parse(&n.value),
            })
        }))
    }

    /// ERG mode: trainer holds target power regardless of cadence.
    pub async fn set_erg(&self, watts: u16) -> Result<()> {
        match self.protocol {
            Protocol::Ftms => self.write(&ftms_set_target_power(watts.min(i16::MAX as u16) as i16)).await,
            Protocol::Wahoo => self.write(&wahoo_set_erg(watts)).await,
        }
    }

    /// Simulation mode: trainer resists like a road at `grade_pct` %.
    pub async fn set_grade(&self, grade_pct: f32) -> Result<()> {
        match self.protocol {
            Protocol::Ftms => self.write(&ftms_set_sim(grade_pct, self.sim)).await,
            Protocol::Wahoo => {
                // Leaving ERG requires re-entering sim mode before grade applies.
                self.write(&wahoo_set_sim(self.sim)).await?;
                self.write(&wahoo_set_grade(grade_pct)).await
            }
        }
    }

    pub async fn disconnect(&self) -> Result<()> {
        Ok(self.peripheral.disconnect().await?)
    }
}
