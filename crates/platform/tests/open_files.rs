//! #67: a process started with launchd's soft limit of 256 descriptors can open more files than
//! that once `raise_open_file_limit` has run. Its own test binary: it lowers the process limit.

#[cfg(unix)]
#[test]
fn more_open_files_than_the_launchd_default_after_raising() {
    use rustix::process::{Resource, getrlimit, setrlimit};

    let original = getrlimit(Resource::Nofile);
    let mut low = original;
    low.current = Some(256);
    setrlimit(Resource::Nofile, low).unwrap();

    let dir = std::env::temp_dir().join(format!("filmcraft-open-files-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let opened = |n: usize| -> std::io::Result<Vec<std::fs::File>> {
        (0..n).map(|i| std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join(format!("{i}")))).collect()
    };
    // The launchd default really is too small for 400 sources.
    assert!(opened(400).is_err());

    let limit = filmcraft_platform::raise_open_file_limit().unwrap();
    assert!(limit >= 1024, "limit {limit}");
    let files = opened(400).unwrap();
    assert_eq!(files.len(), 400);
    // Raising again is a no-op, never a decrease.
    assert_eq!(filmcraft_platform::raise_open_file_limit().unwrap(), limit);

    drop(files);
    let _ = std::fs::remove_dir_all(&dir);
    setrlimit(Resource::Nofile, original).unwrap();
}
