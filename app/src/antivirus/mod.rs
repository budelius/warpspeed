//! Module containing utilities to query the currently running antivirus / EDR software on the
//! user's machine.

mod telemetry;
#[cfg(windows)]
mod windows;

use warpui::{Entity, ModelContext, SingletonEntity};

/// Singleton model that reports the currently running antivirus software.
#[derive(Debug, Clone)]
pub struct AntivirusInfo(Option<String>);

impl AntivirusInfo {
    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        #[cfg(windows)]
        _ctx.spawn(async move { Self::scan().await }, Self::on_scan_complete);

        Self(None)
    }

    /// Test-only: an inert instance that never scans.
    ///
    /// `new` kicks off a real background antivirus scan on Windows, which a
    /// unit test has no business doing -- but the terminal's shell-death path
    /// reads this singleton for telemetry, so a test that lets a shell die
    /// needs it registered or the effect panics.
    #[cfg(any(test, feature = "test-util"))]
    pub fn new_for_test() -> Self {
        Self(None)
    }

    /// Returns the currently running antivirus software if any.
    /// If called before the antivirus is computed (i.e. before
    /// [`AntivirusInfoEvent::ScannedComplete`] is emitted), this function returns [`None`].
    ///
    /// ## Platform-specific
    /// This function always returns `None` on non-Windows platforms.
    pub fn get(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

pub enum AntivirusInfoEvent {
    #[allow(dead_code)]
    ScannedComplete,
}

impl Entity for AntivirusInfo {
    type Event = AntivirusInfoEvent;
}

impl SingletonEntity for AntivirusInfo {}
