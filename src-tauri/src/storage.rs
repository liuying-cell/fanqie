//! JSON / 文本原子持久化。
//!
//! 写入路径：同名 `.tmp` → `sync_all` → 旧主文件转 `.bak` → `.tmp` 重命名为正式文件。
//! 读取时主文件损坏或不可读，自动回退到 `.bak`。所有失败都以 `Result` 返回，
//! 调用方必须显式处理，不允许再用 `let _ = fs::write(...)` 吞错。

use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum StorageError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    /// 主文件与备份都不可用，明确报出两个路径。
    Corrupt {
        path: PathBuf,
        backup: PathBuf,
        detail: String,
    },
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "IO error on {}: {source}", path.display()),
            Self::Json { path, source } => {
                write!(f, "invalid JSON in {}: {source}", path.display())
            }
            Self::Corrupt {
                path,
                backup,
                detail,
            } => write!(
                f,
                "primary {} and backup {} are both unusable: {detail}",
                path.display(),
                backup.display()
            ),
        }
    }
}

impl std::error::Error for StorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json { source, .. } => Some(source),
            Self::Corrupt { .. } => None,
        }
    }
}

/// 读取结果：区分「首次运行没有文件」与「从备份恢复」。
#[derive(Debug, PartialEq, Eq)]
pub enum LoadStatus<T> {
    Missing,
    Loaded(T),
    Recovered(T),
}

impl<T> LoadStatus<T> {
    pub fn into_option(self) -> Option<T> {
        match self {
            Self::Missing => None,
            Self::Loaded(value) | Self::Recovered(value) => Some(value),
        }
    }

    pub fn is_recovered(&self) -> bool {
        matches!(self, Self::Recovered(_))
    }
}

fn tmp_name(name: &str) -> String {
    format!("{name}.tmp")
}

fn bak_name(name: &str) -> String {
    format!("{name}.bak")
}

fn io_err(path: &Path, source: io::Error) -> StorageError {
    StorageError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// 原子写入字节：临时文件落盘并 sync，成功后旧文件转备份，最后临时文件转正。
fn write_bytes_atomic(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), StorageError> {
    fs::create_dir_all(dir).map_err(|e| io_err(dir, e))?;
    let primary = dir.join(name);
    let tmp = dir.join(tmp_name(name));
    let backup = dir.join(bak_name(name));

    {
        let mut file = File::create(&tmp).map_err(|e| io_err(&tmp, e))?;
        file.write_all(bytes).map_err(|e| io_err(&tmp, e))?;
        file.sync_all().map_err(|e| io_err(&tmp, e))?;
    }

    let committed = (|| -> Result<(), StorageError> {
        if primary.exists() {
            if backup.exists() {
                fs::remove_file(&backup).map_err(|e| io_err(&backup, e))?;
            }
            fs::rename(&primary, &backup).map_err(|e| io_err(&primary, e))?;
        }
        fs::rename(&tmp, &primary).map_err(|e| io_err(&primary, e))?;
        Ok(())
    })();

    if committed.is_err() {
        // 清理未完成事务，保证下次启动不会误读半截临时文件。
        let _ = fs::remove_file(&tmp);
    }
    committed
}

pub fn write_json_atomic<T: Serialize>(
    dir: &Path,
    name: &str,
    value: &T,
) -> Result<(), StorageError> {
    let path = dir.join(name);
    let bytes =
        serde_json::to_vec_pretty(value).map_err(|source| StorageError::Json { path, source })?;
    write_bytes_atomic(dir, name, &bytes)
}

pub fn write_text_atomic(dir: &Path, name: &str, text: &str) -> Result<(), StorageError> {
    write_bytes_atomic(dir, name, text.as_bytes())
}

fn read_backup<T: DeserializeOwned>(
    backup: &Path,
    primary: &Path,
    detail: impl Into<String>,
) -> Result<T, StorageError> {
    let text = fs::read_to_string(backup).map_err(|source| StorageError::Corrupt {
        path: primary.to_path_buf(),
        backup: backup.to_path_buf(),
        detail: format!("backup unreadable: {source}"),
    })?;
    serde_json::from_str(&text).map_err(|source| StorageError::Corrupt {
        path: primary.to_path_buf(),
        backup: backup.to_path_buf(),
        detail: format!("backup invalid: {source}; primary: {}", detail.into()),
    })
}

/// 读取 JSON：主文件优先；主文件损坏/不可读时回退到 `.bak`。
pub fn load_json<T: DeserializeOwned>(
    dir: &Path,
    name: &str,
) -> Result<LoadStatus<T>, StorageError> {
    let primary = dir.join(name);
    let backup = dir.join(bak_name(name));
    match fs::read_to_string(&primary) {
        Ok(text) => match serde_json::from_str::<T>(&text) {
            Ok(value) => Ok(LoadStatus::Loaded(value)),
            Err(err) => Ok(LoadStatus::Recovered(read_backup(
                &backup,
                &primary,
                format!("primary invalid: {err}"),
            )?)),
        },
        Err(primary_err) if primary_err.kind() == io::ErrorKind::NotFound => {
            match fs::read_to_string(&backup) {
                Ok(text) => serde_json::from_str::<T>(&text)
                    .map(LoadStatus::Recovered)
                    .map_err(|err| StorageError::Corrupt {
                        path: primary,
                        backup,
                        detail: format!("backup invalid: {err}"),
                    }),
                Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(LoadStatus::Missing),
                Err(err) => Err(StorageError::Corrupt {
                    path: primary,
                    backup,
                    detail: format!("backup unreadable: {err}"),
                }),
            }
        }
        Err(err) => Ok(LoadStatus::Recovered(read_backup(
            &backup,
            &primary,
            format!("primary unreadable: {err}"),
        )?)),
    }
}

/// 读取纯文本（theme.json 使用）；不存在时返回 Missing。
pub fn load_text(dir: &Path, name: &str) -> Result<LoadStatus<String>, StorageError> {
    let primary = dir.join(name);
    let backup = dir.join(bak_name(name));
    match fs::read_to_string(&primary) {
        Ok(text) => Ok(LoadStatus::Loaded(text)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => match fs::read_to_string(&backup) {
            Ok(text) => Ok(LoadStatus::Recovered(text)),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(LoadStatus::Missing),
            Err(err) => Err(StorageError::Corrupt {
                path: primary,
                backup,
                detail: format!("backup unreadable: {err}"),
            }),
        },
        Err(primary_err) => match fs::read_to_string(&backup) {
            Ok(text) => Ok(LoadStatus::Recovered(text)),
            Err(_) => Err(StorageError::Corrupt {
                path: primary,
                backup,
                detail: format!("primary unreadable: {primary_err}"),
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn temp_dir(tag: &str) -> PathBuf {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-temp")
            .join(format!("pomodoro-storage-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create temp dir");
        path
    }

    #[test]
    fn second_write_keeps_old_value_in_bak() {
        let dir = temp_dir("bak");
        write_json_atomic(&dir, "timer.json", &json!({"n": 1})).expect("first write");
        write_json_atomic(&dir, "timer.json", &json!({"n": 2})).expect("second write");
        let backup: Value = load_json(&dir, "timer.json.bak")
            .expect("read bak")
            .into_option()
            .unwrap();
        let current: Value = load_json(&dir, "timer.json")
            .expect("read main")
            .into_option()
            .unwrap();
        assert_eq!(backup["n"], 1);
        assert_eq!(current["n"], 2);
        assert!(!dir.join("timer.json.tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_primary_recovers_from_backup() {
        let dir = temp_dir("recover");
        write_json_atomic(&dir, "settings.json", &json!({"theme": "good"})).expect("first write");
        write_json_atomic(&dir, "settings.json", &json!({"theme": "better"}))
            .expect("second write");
        fs::write(dir.join("settings.json"), b"{ not json").expect("corrupt primary");
        let loaded = load_json::<Value>(&dir, "settings.json").expect("recover");
        assert!(loaded.is_recovered());
        // .bak 保存的是「上一次写入前」的值：当前主文件是第二次写入，备份即第一次的值。
        assert_eq!(loaded.into_option().unwrap()["theme"], "good");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn invalid_target_returns_error() {
        let dir = temp_dir("invalid");
        let blocker = dir.join("not-a-directory");
        fs::write(&blocker, b"occupied").expect("create blocker");
        // dir 参数是普通文件而非目录，create_dir_all 必须返回 Err，不得静默成功。
        assert!(write_json_atomic(&blocker, "settings.json", &json!({"a": 1})).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_json_roundtrip() {
        let dir = temp_dir("roundtrip");
        let value = json!({"phase": "focus", "running": false});
        write_json_atomic(&dir, "timer.json", &value).expect("write");
        let loaded: Value = load_json(&dir, "timer.json")
            .expect("read")
            .into_option()
            .unwrap();
        assert_eq!(loaded, value);
        let _ = fs::remove_dir_all(&dir);
    }
}
