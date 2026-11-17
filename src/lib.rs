//! PIARU — Pixelated Image Abstraction in Rust.
pub mod color;
pub mod pia;
pub mod pixeloe;
pub mod saliency;

pub use color::{Space, F};
pub use pia::{run, Input, Output, Params, Stats};
pub use saliency::spectral_residual;

/// Peak resident set size of this process in KiB (Linux /proc), if available.
pub fn peak_rss_kib() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    s.lines()
        .find(|l| l.starts_with("VmHWM:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}
