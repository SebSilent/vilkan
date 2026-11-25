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

/// The proof-of-concept size for [`dev_fast_mode`]. (reasoning elided)
/// Reported as the display mode under fast mode (see
/// `crate::dxgi::display_size` via `VICTORY_DISPLAY_MODE`) and used for
/// window geometry only behind [`window_clamp_enabled`].
///
/// Neither makes the title render smaller: proof5  reported
/// 800x600 in the mode list and the title still presented 3840x2160, so its
/// back buffer size comes from somewhere else. The open question is *which*
/// API hands it 4K — the mode list and the window are both ruled out.
pub const FAST_WIDTH: u32 = 800;
/// See [`FAST_WIDTH`].
pub const FAST_HEIGHT: u32 = 600;

/// Window geometry is NOT a resolution lever. Proof4 (`cf2ce4a`) clamped the
/// title's top-level window to [`FAST_WIDTH`]x[`FAST_HEIGHT`] and the title
/// still presented 3840x2160, so the clamp buys no resolution. It is not
/// measurably harmful either: proof4 (clamp on) medianed 0.4 fps and proof5
/// (clamp off) medianed 0.3 fps at the same presented size — an earlier
/// "18.4 fps" reading for proof5 was taken from a partial log during an easy
/// startup phase and is retracted. Off by default because it achieves
/// nothing; opt in only to study window behaviour.
pub fn window_clamp_enabled() -> bool {
    dev_fast_mode()
        && matches!(
            std::env::var("VILKAN_FAST_CLAMP").as_deref(),
            Ok("1") | Ok("true") | Ok("yes")
        )
}

/// `width` x `height` clamped to [`FAST_WIDTH`] x [`FAST_HEIGHT`] when
/// [`dev_fast_mode`] is on **and** [`window_clamp_enabled`] opts in (never
/// larger than asked), else unchanged.
pub fn fast_size(width: u32, height: u32) -> (u32, u32) {
    if window_clamp_enabled() {
        (width.min(FAST_WIDTH), height.min(FAST_HEIGHT))
    } else {
        (width, height)
    }
}