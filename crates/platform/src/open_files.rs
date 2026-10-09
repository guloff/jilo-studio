//! The per-process limit on open file descriptors.
//!
//! Each media source keeps its file open for positional reads (#41), and a project can hold hundreds
//! of sources. Apps started from Finder or the Dock inherit launchd's soft limit of 256 descriptors
//! on macOS (1024 on most Linux systems), so a large project ran out and its media showed as
//! failed or offline (#67). [`raise_open_file_limit`] lifts the soft limit towards the hard limit
//! once at startup. It never lowers it.
//!
//! Safe code only: `rustix` wraps `getrlimit` / `setrlimit`.

/// The soft limit FilmCraft asks for. macOS caps the soft limit at `OPEN_MAX` (10240) for
/// `setrlimit`, whatever `kern.maxfilesperproc` says; Linux allows up to the hard limit.
#[cfg(target_os = "macos")]
pub const WANTED: u64 = 10_240;
/// The soft limit FilmCraft asks for.
#[cfg(not(target_os = "macos"))]
pub const WANTED: u64 = 65_536;

/// Raise the soft open-file limit to [`WANTED`] (or the hard limit, if lower). Returns the soft
/// limit in effect afterwards; an error says why it could not be read or raised, and the process
/// keeps its old limit.
#[cfg(unix)]
pub fn raise_open_file_limit() -> Result<u64, String> {
    use rustix::process::{Resource, getrlimit, setrlimit};

    let current = getrlimit(Resource::Nofile);
    // `None` is RLIM_INFINITY.
    let soft = current.current.unwrap_or(u64::MAX);
    let target = current.maximum.map_or(WANTED, |hard| hard.min(WANTED));
    if soft >= target {
        return Ok(soft);
    }
    let mut wanted = current;
    wanted.current = Some(target);
    setrlimit(Resource::Nofile, wanted).map_err(|e| format!("open-file limit stays at {soft}: setrlimit({target}) failed: {e}"))?;
    Ok(getrlimit(Resource::Nofile).current.unwrap_or(target))
}

/// Windows has no descriptor limit of this kind (the C runtime's is separate and not used for
/// media reads): nothing to raise.
#[cfg(not(unix))]
pub fn raise_open_file_limit() -> Result<u64, String> {
    Ok(u64::MAX)
}
