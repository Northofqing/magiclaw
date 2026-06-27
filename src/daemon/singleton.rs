//! Process-level singleton guard for long-running modes (daemon/mcp).
//!
//! We lock a file under the DB directory so another process cannot start
//! another resident runtime against the same workspace state.

use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use fs2::FileExt;

use crate::infrastructure::config::AppConfig;

pub struct SingletonGuard {
    _lock_file: fs::File,
}

pub fn acquire_singleton(
    mode: &str,
    config: &AppConfig,
) -> Result<SingletonGuard, Box<dyn std::error::Error>> {
    let db_path = PathBuf::from(&config.db_path);
    let lock_dir = db_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    fs::create_dir_all(&lock_dir)?;
    let lock_path = lock_dir.join("magiclaw.instance.lock");
    let mut file = OpenOptions::new()
        .create(true)
        // Do NOT truncate on open: that would wipe the running owner's PID/
        // metadata before the advisory lock check, even when this process is
        // refused. We rewrite metadata via set_len(0) only after acquiring the lock.
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)?;

    if let Err(e) = file.try_lock_exclusive() {
        return Err(format!(
            "{} mode refused: another magiclaw instance is already running (lock: {}) ({})",
            mode,
            lock_path.display(),
            e
        )
        .into());
    }

    let _ = file.set_len(0);
    let _ = writeln!(file, "pid={}", std::process::id());
    let _ = writeln!(file, "mode={}", mode);
    let _ = writeln!(file, "cwd={}", std::env::current_dir()?.display());

    Ok(SingletonGuard { _lock_file: file })
}
