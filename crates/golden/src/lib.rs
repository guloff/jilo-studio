//! Golden-image tests for Jilo Studio's renderer (test-only crate; the library is empty).
//!
//! `tests/golden.rs` builds small procedural projects (no media files), renders one frame of each
//! through the CPU reference compositor (`jilo-render`) and compares it with a committed PNG
//! in `goldens/` using `jilo_testkit::golden`. The same scenes are composited on the GPU
//! (`jilo-gpu`) when an adapter is available and compared with the CPU result.
//!
//! Regenerate references with `JILO_BLESS=1 cargo test -p jilo-golden`.
