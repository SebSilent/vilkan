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

/// Proof-of-concept speed mode (`--dev-fast-mode`, set as
/// `VILKAN_DEV_FAST_MODE`). It is not a fidelity or resolution change:
/// the front end pairs it with `VICTORY_PIPELINE_WORKERS=all`, and here
/// it drops the per-frame black-frame probe readback, the one diagnostics
/// read that runs on every presented frame. A true low-resolution present
/// needs a scaling blit (the present path is a 1:1 `CopyResource`) and is
/// a separate milestone, so this flag never claims to lower resolution.
pub fn dev_fast_mode() -> bool {
    match std::env::var("VILKAN_DEV_FAST_MODE") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "" | "0" | "off" | "false" | "no"
        ),
        Err(_) => false,
    }
}

/// The proof-of-concept render size for [`dev_fast_mode`]. (reasoning elided)
/// The game sizes its render targets from its own window, so both
/// window-creation paths (the title's `CreateWindowEx` in the HLE layer and
/// the host presenter's fallback window) clamp to this when fast mode is on;
/// the game then draws small natively and the 1:1 `CopyResource` in
/// `present` stays valid (no scaling blit needed). Never upscales.
pub const FAST_WIDTH: u32 = 800;
/// See [`FAST_WIDTH`].
pub const FAST_HEIGHT: u32 = 600;

/// `width` x `height` clamped to [`FAST_WIDTH`] x [`FAST_HEIGHT`] when
/// [`dev_fast_mode`] is on (never larger than asked), else unchanged.
pub fn fast_size(width: u32, height: u32) -> (u32, u32) {
    if dev_fast_mode() {
        (width.min(FAST_WIDTH), height.min(FAST_HEIGHT))
    } else {
        (width, height)
    }
}