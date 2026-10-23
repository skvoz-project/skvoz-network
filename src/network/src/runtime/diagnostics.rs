//! One fixed-size embedding snapshot; no packet, profile or wire data is retained.
use super::RuntimeFailure;
use serde::Serialize;
use std::{
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

pub(crate) const RESERVATION: usize = 1024;
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct RuntimeDiagnostics {
    pub enabled: bool,
    pub collection: u64,
    pub sample_age_ms: Option<u64>,
    pub samples: u64,
    pub elapsed_ms: u64,
    pub turns: u64,
    pub native_us: u64,
    pub drive_us: u64,
    pub read_full: u64,
    pub write_full: u64,
    pub read_block: u64,
    pub write_block: u64,
    pub read_paused: u64,
    pub core_turns: u64,
    pub core_turn_us: u64,
    pub core_progress: u64,
    pub core_idle_count: u64,
    pub core_idle_us: u64,
    pub core_output_us: u64,
}
pub(crate) struct Mailbox {
    control: AtomicU64,
    latest: Mutex<Option<(u64, Instant, RuntimeDiagnostics)>>,
}
impl Mailbox {
    pub(crate) fn new() -> Self {
        Self {
            control: AtomicU64::new(0),
            latest: Mutex::new(None),
        }
    }
    pub(crate) fn control(&self) -> u64 {
        self.control.load(Ordering::Acquire)
    }
    pub(crate) fn read(&self, enabled: bool) -> Result<RuntimeDiagnostics, RuntimeFailure> {
        // Readers serialize only this control path, never packet processing.
        let mut latest = self.latest.lock().map_err(|_| RuntimeFailure::Internal)?;
        let mut old = self.control();
        let control = loop {
            if (old & 1 != 0) == enabled {
                break old;
            }
            let epoch = (old >> 1)
                .checked_add(1)
                .filter(|value| *value <= u64::MAX >> 1)
                .ok_or(RuntimeFailure::Internal)?;
            let value = epoch << 1 | u64::from(enabled);
            match self
                .control
                .compare_exchange(old, value, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => break value,
                Err(current) => old = current,
            }
        };
        if latest.as_ref().is_some_and(|sample| sample.0 != control) {
            *latest = None;
        }
        if enabled
            && let Some((epoch, time, value)) = latest.as_ref().filter(|sample| sample.0 == control)
        {
            let mut snapshot = *value;
            snapshot.collection = epoch >> 1;
            snapshot.sample_age_ms =
                Some(u64::try_from(time.elapsed().as_millis()).unwrap_or(u64::MAX));
            return Ok(snapshot);
        }
        Ok(RuntimeDiagnostics {
            enabled,
            collection: control >> 1,
            ..RuntimeDiagnostics::default()
        })
    }
    pub(crate) fn publish(
        &self,
        control: u64,
        value: RuntimeDiagnostics,
    ) -> Result<(), RuntimeFailure> {
        let mut latest = self.latest.lock().map_err(|_| RuntimeFailure::Internal)?;
        if control & 1 != 0 && self.control() == control {
            *latest = Some((control, Instant::now(), value));
        }
        Ok(())
    }
}
#[cfg(test)]
#[path = "diagnostics/tests.rs"]
mod tests;
