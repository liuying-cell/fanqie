//! 本地诊断日志（仅标准库）。
//!
//! 追加写入 `<data_dir>\logs\pomodoro.log`；单文件超过 `max_bytes`（默认 1 MiB）
//! 时先把旧文件轮转为 `pomodoro.log.1`（最多保留两份），再写新文件。

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_MAX_BYTES: u64 = 1024 * 1024;

pub struct Logger {
    dir: PathBuf,
    max_bytes: u64,
    lock: Mutex<()>,
}

impl Logger {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join("logs"),
            max_bytes: DEFAULT_MAX_BYTES,
            lock: Mutex::new(()),
        }
    }

    #[cfg(test)]
    fn with_max_bytes(data_dir: &Path, max_bytes: u64) -> Self {
        Self {
            dir: data_dir.join("logs"),
            max_bytes,
            lock: Mutex::new(()),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn init(&self) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        self.log("INFO", "startup")
    }

    pub fn log(&self, level: &str, message: &str) -> io::Result<()> {
        // 进程内多写入者（ticker / UI 命令 / 托盘事件）串行化，避免行交错。
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let path = self.dir.join("pomodoro.log");
        let line = format!("{} [{}] {}\n", timestamp(), level, message);
        if let Ok(meta) = fs::metadata(&path) {
            if meta.len() + line.len() as u64 > self.max_bytes {
                self.rotate()?;
            }
        }
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        file.write_all(line.as_bytes())?;
        file.sync_all()
    }

    fn rotate(&self) -> io::Result<()> {
        let active = self.dir.join("pomodoro.log");
        let rotated = self.dir.join("pomodoro.log.1");
        if rotated.exists() {
            fs::remove_file(&rotated)?;
        }
        fs::rename(&active, &rotated)?;
        Ok(())
    }
}

fn timestamp() -> String {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => format!("{}ms", d.as_millis()),
        Err(e) => format!("-{}ms", e.duration().as_millis()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-temp")
            .join(format!("pomodoro-log-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    #[test]
    fn appends_startup_line() {
        let dir = temp_dir("startup");
        let logger = Logger::with_max_bytes(&dir, DEFAULT_MAX_BYTES);
        logger.init().expect("init");
        let text = fs::read_to_string(dir.join("logs").join("pomodoro.log")).expect("read log");
        assert!(text.contains("[INFO] startup"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotates_at_limit_and_keeps_two_files() {
        let dir = temp_dir("rotate");
        let logger = Logger::with_max_bytes(&dir, 120);
        logger.init().expect("init");
        for i in 0..6 {
            logger
                .log(
                    "WARN",
                    &format!("message {i} with enough bytes to exceed limit"),
                )
                .expect("append");
        }
        let logs = dir.join("logs");
        assert!(logs.join("pomodoro.log").exists());
        assert!(logs.join("pomodoro.log.1").exists());
        // 只保留两份：不再产生 .2 / .3
        assert!(!logs.join("pomodoro.log.2").exists());
        let active = fs::metadata(logs.join("pomodoro.log")).expect("active meta");
        assert!(active.len() <= 160 + 80);
        let _ = fs::remove_dir_all(&dir);
    }
}
