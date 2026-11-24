//! Diagnostics: per-method call counts for the console D3D12/DXGI dispatch,
//! to specify the API surface a boot-to-menu actually exercises. Enabled only
//! when `VILKAN_API_COUNT` is set; the default path is unchanged (one relaxed
//! atomic load per call when off). Counts are dumped to `VILKAN_API_COUNT`
//! (a file path) at present and at exit.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Mutex;

/// (label, count) in first-seen order; labels are `module::handler`.
static COUNTS: Mutex<Vec<(&'static str, AtomicU64)>> = Mutex::new(Vec::new());

fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VILKAN_API_COUNT").is_some())
}

/// Record one call of `label`. Cheap when disabled.
#[inline]
pub(crate) fn tally(label: &'static str) {
    if !enabled() {
        return;
    }
    let mut g = COUNTS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, c)) = g.iter().find(|(l, _)| *l == label) {
        c.fetch_add(1, Relaxed);
        return;
    }
    g.push((label, AtomicU64::new(1)));
}

/// Write the counts (descending) to the path in `VILKAN_API_COUNT`.
pub(crate) fn dump() {
    let Some(path) = std::env::var_os("VILKAN_API_COUNT") else { return };
    let g = COUNTS.lock().unwrap_or_else(|e| e.into_inner());
    let mut rows: Vec<(&str, u64)> = g.iter().map(|(l, c)| (*l, c.load(Relaxed))).collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let total: u64 = rows.iter().map(|(_, n)| n).sum();
    let mut out = format!("# D3D12/DXGI dispatch call counts (total {total})\n");
    for (l, n) in &rows {
        out.push_str(&format!("{n:>10}  {l}\n"));
    }
    let _ = std::fs::write(&path, out);
}