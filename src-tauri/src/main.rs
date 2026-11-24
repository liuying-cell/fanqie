// Prevents an additional console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// WebView2 在 exe 路径含非 ASCII 字符时无法创建其运行环境（实测 os error 5 / 0x800700AA）。
/// 若当前 exe 路径含非 ASCII 字符，先将其复制到一个纯 ASCII 缓存目录，再从那里重启自身。
///
/// 关键修复（承接 lead review 第 6 项）：
/// - 逐个尝试所有候选目录，只有"复制成功且镜像可启动"才算成功；一个目录失败必须继续下一个。
/// - 使用唯一临时文件名规避目标被占用导致的 `os error 5`（拒绝访问）。
/// - 全部候选失败时写清错误并以非零码退出，绝不继续在中文路径下创建 WebView2。
#[cfg(windows)]
fn ensure_ascii_exe_path() -> ! {
    use std::path::PathBuf;
    use std::process::Command;

    let Ok(current) = std::env::current_exe() else {
        eprintln!("[bootstrap] cannot determine current_exe");
        std::process::exit(101);
    };
    let path_str = current.to_string_lossy();
    if path_str.is_ascii() {
        // 本函数只在非 ASCII 路径下被调用；走到这里说明路径已是 ASCII，
        // 直接返回继续运行（不会出口，故用特殊处理——见 main 的调用条件）。
        // 实际 main 只在检测到非 ASCII 时才调用，此处兜底直接退出运行原 exe 不现实，
        // 因此交由 main 判定；本分支不会到达。
        std::process::exit(0);
    }

    // 纯 ASCII 缓存目录候选；某些受限环境下 LOCALAPPDATA 子目录不可写，故逐个尝试。
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join("PomodoroRuntime"));
    }
    candidates.push(PathBuf::from(r"C:\pomo"));
    candidates.push(PathBuf::from(r"C:\Users\Public\PomodoroRuntime"));
    candidates.push(PathBuf::from(r"C:\lt"));
    candidates.push(std::env::temp_dir().join("PomodoroRuntime"));

    // 唯一文件名：进程号 + 毫秒时间戳，规避旧镜像文件被占用（os error 5）。
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let mirror_name = format!("pomodoro-app-{}-{}.exe", std::process::id(), stamp);

    let mut failures: Vec<String> = Vec::new();
    for dir in &candidates {
        // 目录必须可创建/可写
        if !dir.exists() && std::fs::create_dir_all(dir).is_err() {
            failures.push(format!("{}: cannot create dir", dir.display()));
            continue;
        }
        let mirror = dir.join(&mirror_name);
        match std::fs::copy(&current, &mirror) {
            Ok(_) => {
                // 以"能否 spawn"作为可执行性验证：spawn 成功即认为该镜像可用。
                let mut cmd = Command::new(&mirror);
                for arg in std::env::args().skip(1) {
                    cmd.arg(arg);
                }
                cmd.env("POMODORO_BOOTSTRAPPED", "1");
                match cmd.spawn() {
                    Ok(_) => std::process::exit(0),
                    Err(e) => {
                        failures.push(format!("{}: spawn failed: {}", mirror.display(), e));
                        let _ = std::fs::remove_file(&mirror);
                    }
                }
            }
            Err(e) => {
                failures.push(format!("{}: copy failed: {}", mirror.display(), e));
            }
        }
    }

    // 所有候选都失败：绝不返回去创建中文路径下的 WebView2。
    eprintln!("[bootstrap] all ASCII candidates failed:");
    for f in &failures {
        eprintln!("[bootstrap]   {}", f);
    }
    std::process::exit(101);
}

/// 启动自举镜像后，清除同一目录中上次留下的旧镜像，避免每次启动堆积约 10 MB。
#[cfg(windows)]
fn cleanup_old_mirrors() {
    let Ok(current) = std::env::current_exe() else {
        return;
    };
    let Some(dir) = current.parent() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path == current || path.extension().and_then(|e| e.to_str()) != Some("exe") {
            continue;
        }
        let is_mirror = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("pomodoro-app-"));
        if is_mirror {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(not(windows))]
fn ensure_ascii_exe_path() {}

/// 进程级显式设置 AppUserModelID（须在创建任何窗口前调用）。
/// 这样即使程序不是从开始菜单快捷方式启动，系统通知身份也与
/// com.future.pomodoro 一致，通知能正确归属到本应用。
/// 失败只写 stderr，绝不阻止应用启动。
#[cfg(windows)]
fn ensure_app_user_model_id() {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    const APP_ID: &str = "com.future.pomodoro";

    unsafe extern "system" {
        fn SetCurrentProcessExplicitAppUserModelID(app_id: *const u16) -> i32;
    }

    let wide: Vec<u16> = OsStr::new(APP_ID)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let hr = unsafe { SetCurrentProcessExplicitAppUserModelID(wide.as_ptr()) };
    if hr < 0 {
        eprintln!(
            "[aumid] SetCurrentProcessExplicitAppUserModelID failed: 0x{:08X}",
            hr as u32
        );
    } else {
        eprintln!("[aumid] SetCurrentProcessExplicitAppUserModelID ok: {APP_ID}");
    }
}

#[cfg(not(windows))]
fn ensure_app_user_model_id() {}

fn main() {
    // 仅当：Windows + 未被标记已自举 + 当前路径含非 ASCII 时才进入自举。
    #[cfg(windows)]
    {
        if std::env::var("POMODORO_BOOTSTRAPPED").is_err() {
            let non_ascii = std::env::current_exe()
                .map(|p| !p.to_string_lossy().is_ascii())
                .unwrap_or(false);
            if non_ascii {
                ensure_ascii_exe_path();
            }
        }
    }
    #[cfg(not(windows))]
    {
        if std::env::var("POMODORO_BOOTSTRAPPED").is_err() {
            ensure_ascii_exe_path();
        }
    }
    #[cfg(windows)]
    if std::env::var("POMODORO_BOOTSTRAPPED").is_ok() {
        cleanup_old_mirrors();
    }
    // Tauri 创建任何窗口前设置进程 AUMID（自举会 exit 并重启，新进程同样会执行到此处）。
    ensure_app_user_model_id();
    pomodoro_app_lib::run()
}
