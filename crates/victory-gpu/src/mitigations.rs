//! Attribution toggles for the stacked, still-unmeasured mitigations.
//!
//! Each mitigation defaults ON. Setting its environment variable to
//! `0`/`off`/`false`/`no` (any case) disables it; any other value, or an
//! unset variable, leaves it ON. **All-on is behaviourally identical to the
//! un-toggleable code** — the switches only select which mitigation runs,
//! they add no new semantics. Their purpose is attribution: after the GPU
//! ban lifts, an isolation sequence can turn one mitigation off at a time
//! and measure its effect on the 1a crash / geometry / counters, instead of
//! guessing which of the stacked changes mattered.
//!
//! The active set is logged once at startup (see the app's run path) next
//! to the backend label, so every run log self-identifies and can never be
//! misattributed to the wrong backend or the wrong mitigation set.
//!
//! Toggles (env var -> mitigation, commit):
//! * `VILKAN_MIT_POOL`           background compile pool (4530bd6 / 574e42c)
//! * `VILKAN_MIT_CLOSE_ABORT`    close_lane completes pending statuses E_ABORT (491a5ff)
//! * `VILKAN_MIT_SHADOW_SEEK`    shadow O(log n) BTree predecessor seek (8e21c2f / fb8ba97)
//! * `VILKAN_MIT_DISPATCH_SINGLE` console type-105 single-pass dispatch mapping
//! * `VILKAN_MIT_ARG_CLAMP`      indirect argument GPU clamp (E5005 guard)

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
    /// Background compile pool. OFF => synchronous inline compilation at
    /// first use (the pre-pool path), no background threads.
    pub pool: bool,
    /// close_lane completes pending statuses with E_ABORT. OFF => the
    /// pre-fix behaviour (statuses cleared while still E_PENDING).
    pub close_abort: bool,
    /// Shadow lookup uses the O(log n) BTree predecessor seek. OFF => the
    /// original capped scan (take 64), which missed large containers.
    pub shadow_seek: bool,
    /// Console type-105 single-pass dispatch maps to a plain dispatch.
    /// OFF => type 105 is not remapped (falls through to the drop path).
    pub dispatch_single: bool,
    /// Indirect argument GPU clamp (the E5005 driver-reset guard). OFF =>
    /// ExecuteIndirect runs on the raw argument counts, unclamped.
    pub arg_clamp: bool,
}

static SET: OnceLock<Mitigations> = OnceLock::new();

/// The process-wide mitigation set (read once, on first use).
pub fn get() -> &'static Mitigations {
    SET.get_or_init(|| Mitigations {
        pool: on("VILKAN_MIT_POOL"),
        close_abort: on("VILKAN_MIT_CLOSE_ABORT"),
        shadow_seek: on("VILKAN_MIT_SHADOW_SEEK"),
        dispatch_single: on("VILKAN_MIT_DISPATCH_SINGLE"),
        arg_clamp: on("VILKAN_MIT_ARG_CLAMP"),
    })
}

/// One-line active set for the startup log, e.g.
/// `pool=on close-abort=on shadow-seek=on dispatch-single=on arg-clamp=on`.
pub fn active_set() -> String {
    let m = get();
    let s = |b: bool| if b { "on" } else { "off" };
    format!(
        "pool={} close-abort={} shadow-seek={} dispatch-single={} arg-clamp={}",
        s(m.pool),
        s(m.close_abort),
        s(m.shadow_seek),
        s(m.dispatch_single),
        s(m.arg_clamp),
    )
}

#[cfg(test)]
mod tests {
    use super::on;

    #[test]
    fn default_on_when_unset() {
        // A name nothing sets: absent => ON.
        assert!(on("VILKAN_MIT_TEST_UNSET_XYZ"));
    }

    #[test]
    fn off_only_for_explicit_negatives() {
        // The parser logic, exercised on a private name we control.
        let name = "VILKAN_MIT_TEST_PARSE_XYZ";
        for v in ["0", "off", "false", "no", "OFF", "False", " no "] {
            // SAFETY: a private test-only variable; no other thread reads it.
            unsafe { std::env::set_var(name, v) };
            assert!(!on(name), "{v:?} should be off");
        }
        for v in ["1", "on", "true", "yes", "", "anything"] {
            // SAFETY: a private test-only variable; no other thread reads it.
            unsafe { std::env::set_var(name, v) };
            assert!(on(name), "{v:?} should be on");
        }
        // SAFETY: a private test-only variable; no other thread reads it.
        unsafe { std::env::remove_var(name) };
        assert!(on(name));
    }
}