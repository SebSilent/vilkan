//! Attribution toggles for our stacked mitigations.
//!
//! Each mitigation defaults ON. Setting its environment variable to
//! `0`/`off`/`false`/`no` (any case) disables it; any other value, or an
//! unset variable, leaves it ON. All-on is behaviourally identical to the
//! un-toggleable code — the switches only select which mitigation runs,
//! they add no new semantics. Their purpose is attribution: an isolation
//! sequence can turn one mitigation off at a time and measure its effect.
//!
//! Toggles (env var -> mitigation):
//! * `VILKAN_MIT_CLOSE_ABORT`  close_lane completes pending statuses E_ABORT

use std::sync::OnceLock;

/// A default-ON toggle: absent => on; `0`/`off`/`false`/`no` (any case) => off.
fn on(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "off" | "false" | "no"
        ),
        Err(_) => true,
    }
}

/// The resolved mitigation set, read once from the environment.
pub struct Mitigations {
    /// close_lane completes pending statuses with E_ABORT. OFF => the
    /// pre-fix behaviour (statuses cleared while still E_PENDING).
    pub close_abort: bool,
}

static SET: OnceLock<Mitigations> = OnceLock::new();

/// The process-wide mitigation set (read once, on first use).
pub fn get() -> &'static Mitigations {
    SET.get_or_init(|| Mitigations {
        close_abort: on("VILKAN_MIT_CLOSE_ABORT"),
    })
}

/// A one-line description of the active mitigation set, for the run log.
pub fn active_set() -> String {
    let m = get();
    format!("close_abort={}", m.close_abort)
}