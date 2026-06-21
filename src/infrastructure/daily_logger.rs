use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use chrono::Local;

/// Daily rotating logger for daemon events (token refresh, ret=-2, session errors, etc.).
pub struct DailyLogger {
    log_dir: PathBuf,
    current_date: Mutex<String>,
    file: Mutex<Option<File>>,
}

impl DailyLogger {
    pub fn new(log_dir: impl AsRef<Path>) -> Result<Self, Box<dyn std::error::Error>> {
        let log_dir = log_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&log_dir)?;
        Ok(Self {
            log_dir,
            current_date: Mutex::new(Local::now().format("%Y-%m-%d").to_string()),
            file: Mutex::new(None),
        })
    }

    fn get_file(&self) -> Result<std::sync::MutexGuard<'_, Option<File>>, String> {
        let mut file_guard = self.file.lock().map_err(|e| e.to_string())?;
        let today = Local::now().format("%Y-%m-%d").to_string();
        let mut date_guard = self.current_date.lock().map_err(|e| e.to_string())?;

        if today != *date_guard {
            *date_guard = today.clone();
            *file_guard = None; // Close old file
        }

        if file_guard.is_none() {
            let log_path = self.log_dir.join(format!("magiclaw-{}.log", today));
            let f = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .map_err(|e| format!("failed to open log file: {}", e))?;
            *file_guard = Some(f);
        }

        drop(date_guard); // Release date lock before returning file lock
        Ok(file_guard)
    }

    pub fn log_token_refresh(
        &self,
        peer_id: &str,
        old_age_secs: u64,
        source: &str, // "long-poll", "probe", "send"
    ) {
        let ts = Local::now().format("%H:%M:%S%.3f");
        let msg = format!(
            "[{}] TOKEN_REFRESH peer_id={} old_age_secs={} source={}\n",
            ts, peer_id, old_age_secs, source
        );

        if let Ok(mut file_guard) = self.get_file() {
            if let Some(ref mut f) = *file_guard {
                let _ = f.write_all(msg.as_bytes());
                let _ = f.flush();
            }
        }
    }

    pub fn log_send_failure(
        &self,
        peer_id: &str,
        error_code: i32,
        error_msg: &str,
    ) {
        let ts = Local::now().format("%H:%M:%S%.3f");
        let msg = format!(
            "[{}] SEND_FAILURE peer_id={} errcode={} errmsg={}\n",
            ts, peer_id, error_code, error_msg
        );

        if let Ok(mut file_guard) = self.get_file() {
            if let Some(ref mut f) = *file_guard {
                let _ = f.write_all(msg.as_bytes());
                let _ = f.flush();
            }
        }
    }

    pub fn log_session_expired(
        &self,
        session_retries: u32,
        max_retries: u32,
    ) {
        let ts = Local::now().format("%H:%M:%S%.3f");
        let msg = format!(
            "[{}] SESSION_EXPIRED retries={}/{}\n",
            ts, session_retries, max_retries
        );

        if let Ok(mut file_guard) = self.get_file() {
            if let Some(ref mut f) = *file_guard {
                let _ = f.write_all(msg.as_bytes());
                let _ = f.flush();
            }
        }
    }

    pub fn log_probe_error(
        &self,
        error: &str,
    ) {
        let ts = Local::now().format("%H:%M:%S%.3f");
        let msg = format!(
            "[{}] PROBE_ERROR error={}\n",
            ts, error
        );

        if let Ok(mut file_guard) = self.get_file() {
            if let Some(ref mut f) = *file_guard {
                let _ = f.write_all(msg.as_bytes());
                let _ = f.flush();
            }
        }
    }
}
