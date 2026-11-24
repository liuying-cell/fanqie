mod logging;
mod storage;
mod timer;

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};

use timer::{BallPosition, PhaseCompletePayload, Settings, TimerSnapshot, TimerState};

/// 悬浮球右键原生菜单项 id
const BALL_MENU_OPEN: &str = "ball-open";
const BALL_MENU_HIDE: &str = "ball-hide";
const BALL_MENU_QUIT: &str = "ball-quit";

/// 悬浮球窗口逻辑尺寸（与 set_ball_expanded 保持一致）
const BALL_COLLAPSED: (f64, f64) = (82.0, 82.0);
const BALL_EXPANDED: (f64, f64) = (315.0, 300.0);

/// 全局应用状态
pub struct AppState {
    pub timer: Mutex<TimerState>,
    pub settings: Mutex<Settings>,
    pub ball_pos: Mutex<BallPosition>,
    /// 收起态的球锚点：展开时保存，收起时恢复，避免展开把锚点挤到边缘。
    pub ball_anchor: Mutex<BallPosition>,
    /// 串行化状态变更与对应文件写入，避免旧快照后写覆盖新状态。
    pub persist_lock: Mutex<()>,
    pub data_dir: PathBuf,
    /// 通知权限是否被拒（用于 UI 降级提示）
    pub notify_degraded: AtomicBool,
    /// 悬浮球是否处于展开态（展开态的位置由 ball_anchor 管理，不写入 ball_pos）
    pub ball_expanded: AtomicBool,
    /// 本地诊断日志（1MB 轮转，最多两份）
    pub logger: Arc<logging::Logger>,
}

impl AppState {
    fn write_json<T: serde::Serialize>(
        &self,
        app: &AppHandle,
        name: &str,
        value: &T,
    ) -> Result<(), String> {
        match storage::write_json_atomic(&self.data_dir, name, value) {
            Ok(()) => {
                let _ = self.logger.log("INFO", &format!("saved {name}"));
                Ok(())
            }
            Err(err) => {
                self.report_storage_error(app, name, &err);
                Err(err.to_string())
            }
        }
    }

    /// 保存失败统一记账：本地日志 + 前端可见事件，两处都不吞错。
    fn report_storage_error(&self, app: &AppHandle, operation: &str, err: &storage::StorageError) {
        let message = format!("{operation}: {err}");
        let _ = self.logger.log("ERROR", &message);
        let _ = app.emit(
            "storage-error",
            serde_json::json!({ "operation": operation, "message": message }),
        );
    }

    /// 在同一串行区间内完成设置变更与落盘。
    /// 主窗口、悬浮球和托盘可能同时改设置；仅锁内存而不锁写盘顺序时，旧快照可能后写覆盖新值。
    fn with_settings_change<T>(
        &self,
        app: &AppHandle,
        change: impl FnOnce(&mut Settings) -> T,
    ) -> Result<T, String> {
        let _persist = self.persist_lock.lock().map_err(|e| e.to_string())?;
        let (result, persisted) = {
            let mut settings = self.settings.lock().map_err(|e| e.to_string())?;
            let result = change(&mut settings);
            (result, (*settings).clone())
        };
        self.write_json(app, "settings.json", &persisted)?;
        Ok(result)
    }

    /// 在同一串行区间内完成计时变更与落盘，返回变更结果和最新快照。
    fn with_timer_change<T>(
        &self,
        app: &AppHandle,
        change: impl FnOnce(&mut TimerState, &Settings) -> T,
    ) -> Result<(T, TimerSnapshot), String> {
        let _persist = self.persist_lock.lock().map_err(|e| e.to_string())?;
        let (result, snap, persisted) = {
            let mut timer = self.timer.lock().map_err(|e| e.to_string())?;
            let settings = self.settings.lock().map_err(|e| e.to_string())?;
            timer.rollover_day_if_needed();
            let result = change(&mut timer, &settings);
            (result, timer.snapshot(&settings), timer.clone())
        };
        self.save_timer_snapshot(app, &persisted)?;
        Ok((result, snap))
    }

    /// 持久化一份已经取好的 TimerState 快照。
    /// 关键：绝不在持有 timer MutexGuard 时调用本函数——save_timer 曾经内部再锁 timer，
    /// 而 toggle/reset/tray 调用点都还持有同一个 Guard，形成自锁导致应用无响应。
    fn save_timer_snapshot(&self, app: &AppHandle, timer: &TimerState) -> Result<(), String> {
        self.write_json(app, "timer.json", timer)
    }

    fn save_ball_pos(&self, app: &AppHandle) -> Result<(), String> {
        let position = self.ball_pos.lock().map_err(|e| e.to_string())?.clone();
        self.write_json(app, "ball-position.json", &position)
    }
}

// ---------------- Tauri Commands ----------------

#[tauri::command]
fn get_snapshot(state: State<'_, AppState>) -> Result<TimerSnapshot, String> {
    let mut timer = state.timer.lock().map_err(|e| e.to_string())?;
    let settings = state.settings.lock().map_err(|e| e.to_string())?;
    timer.rollover_day_if_needed();
    Ok(timer.snapshot(&settings))
}

#[tauri::command]
fn get_settings(state: State<'_, AppState>) -> Result<Settings, String> {
    Ok(state.settings.lock().map_err(|e| e.to_string())?.clone())
}

#[tauri::command]
fn toggle_timer(app: AppHandle, state: State<'_, AppState>) -> Result<TimerSnapshot, String> {
    let (_, snap) = state.with_timer_change(&app, |timer, _| timer.toggle())?;
    let _ = app.emit("timer-changed", &snap);
    Ok(snap)
}

#[tauri::command]
fn reset_timer(app: AppHandle, state: State<'_, AppState>) -> Result<TimerSnapshot, String> {
    let (_, snap) = state.with_timer_change(&app, |timer, settings| timer.reset(settings))?;
    let _ = app.emit("timer-changed", &snap);
    Ok(snap)
}

#[tauri::command]
fn reset_cycle(app: AppHandle, state: State<'_, AppState>) -> Result<TimerSnapshot, String> {
    let (_, snap) = state.with_timer_change(&app, |timer, settings| timer.reset_cycle(settings))?;
    let _ = app.emit("timer-changed", &snap);
    Ok(snap)
}

#[tauri::command]
fn skip_phase(app: AppHandle, state: State<'_, AppState>) -> Result<TimerSnapshot, String> {
    let (payload, snap) = state.with_timer_change(&app, |timer, settings| timer.skip(settings))?;
    let _ = app.emit("timer-changed", &snap);
    let _ = app.emit("phase-complete", &payload);
    Ok(snap)
}

#[tauri::command]
fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<(), String> {
    let settings = settings.sanitized();
    let saved = state.with_settings_change(&app, move |guard| {
        *guard = settings;
        guard.clone()
    })?;

    // 开机启动：失败写诊断日志，不让它吞掉已经成功的设置保存
    use tauri_plugin_autostart::ManagerExt;
    let autostart_result = if saved.autostart {
        app.autolaunch().enable()
    } else {
        app.autolaunch().disable()
    };
    if let Err(err) = autostart_result {
        let _ = state
            .logger
            .log("ERROR", &format!("autostart sync failed: {err}"));
    }

    // 悬浮球可见性
    sync_ball_window(&app, saved.ball_visible);

    // 置顶
    if let Some(ball) = app.get_webview_window("ball") {
        let _ = ball.set_always_on_top(saved.always_on_top);
    }

    Ok(())
}

#[tauri::command]
fn show_main(app: AppHandle) -> Result<(), String> {
    focus_main_window(&app);
    Ok(())
}

#[tauri::command]
fn set_ball_visible(
    app: AppHandle,
    state: State<'_, AppState>,
    visible: bool,
) -> Result<(), String> {
    state.with_settings_change(&app, |s| s.ball_visible = visible)?;
    sync_ball_window(&app, visible);
    Ok(())
}

#[tauri::command]
fn is_ball_visible(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state
        .settings
        .lock()
        .map_err(|e| e.to_string())?
        .ball_visible)
}

#[tauri::command]
fn notify_status(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.notify_degraded.load(Ordering::SeqCst))
}

/// 主题以独立文件持久化，避免污染 Settings 结构；写入后广播给所有窗口。
#[tauri::command]
fn set_theme(app: AppHandle, state: State<'_, AppState>, dark: bool) -> Result<(), String> {
    let text = if dark { "dark" } else { "light" };
    if let Err(err) = storage::write_text_atomic(&state.data_dir, "theme.json", text) {
        state.report_storage_error(&app, "theme.json", &err);
        return Err(err.to_string());
    }
    let _ = state.logger.log("INFO", "saved theme.json");
    let _ = app.emit("theme-changed", dark);
    Ok(())
}

#[tauri::command]
fn get_theme(state: State<'_, AppState>) -> Result<bool, String> {
    let loaded = storage::load_text(&state.data_dir, "theme.json").map_err(|e| e.to_string())?;
    Ok(loaded.into_option().as_deref() == Some("dark"))
}

/// 在资源管理器中打开当前数据目录（设置页「数据与诊断」）。
#[tauri::command]
fn open_data_dir(state: State<'_, AppState>) -> Result<(), String> {
    let dir = &state.data_dir;
    if !dir.is_dir() {
        return Err(format!("数据目录不存在：{}", dir.display()));
    }
    #[cfg(windows)]
    {
        std::process::Command::new("explorer.exe")
            .arg(dir)
            .spawn()
            .map_err(|e| format!("无法启动资源管理器：{e}"))?;
    }
    #[cfg(not(windows))]
    {
        return Err("打开数据目录仅支持 Windows".to_string());
    }
    let _ = state
        .logger
        .log("INFO", &format!("open_data_dir: {}", dir.display()));
    Ok(())
}

#[tauri::command]
fn get_ball_position(state: State<'_, AppState>) -> Result<BallPosition, String> {
    Ok(state.ball_pos.lock().map_err(|e| e.to_string())?.clone())
}

/// 找到包含窗口中心点的显示器；找不到则归到主显示器。
fn monitor_for_window(app: &AppHandle, window: &tauri::WebviewWindow) -> Option<tauri::Monitor> {
    let monitors = app.available_monitors().ok()?;
    if monitors.is_empty() {
        return None;
    }
    let pos = window.outer_position().ok()?;
    let size = window.outer_size().ok()?;
    let cx = pos.x + size.width as i32 / 2;
    let cy = pos.y + size.height as i32 / 2;
    for m in &monitors {
        let mp = m.position();
        let ms = m.size();
        if cx >= mp.x && cx <= mp.x + ms.width as i32 && cy >= mp.y && cy <= mp.y + ms.height as i32
        {
            return Some(m.clone());
        }
    }
    Some(monitors[0].clone())
}

/// 把窗口位置限制在指定显示器的工作区内（不压任务栏）。
fn clamp_to_work_area(
    window: &tauri::WebviewWindow,
    monitor: &tauri::Monitor,
) -> Option<(i32, i32)> {
    let area = monitor.work_area();
    let size = window.outer_size().ok()?;
    let cur = window.outer_position().ok()?;
    let min_x = area.position.x;
    let min_y = area.position.y;
    let max_x = area.position.x + area.size.width as i32 - size.width as i32;
    let max_y = area.position.y + area.size.height as i32 - size.height as i32;
    let nx = cur.x.clamp(min_x, max_x.max(min_x));
    let ny = cur.y.clamp(min_y, max_y.max(min_y));
    if nx != cur.x || ny != cur.y {
        let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition {
            x: nx,
            y: ny,
        }));
    }
    Some((nx, ny))
}

#[tauri::command]
fn set_ball_expanded(
    app: AppHandle,
    state: State<'_, AppState>,
    expanded: bool,
) -> Result<(), String> {
    let Some(ball) = app.get_webview_window("ball") else {
        return Ok(());
    };
    let (width, height) = if expanded {
        BALL_EXPANDED
    } else {
        BALL_COLLAPSED
    };
    // 告知 Moved 落盘线程：展开态的位置不算「收起态落盘位置」。
    state.ball_expanded.store(expanded, Ordering::SeqCst);

    if expanded {
        // 展开前记下当前收起态锚点（仅第一次展开时记，避免嵌套覆盖）。
        if let Ok(mut anchor) = state.ball_anchor.lock() {
            if anchor.x < 0 {
                if let Ok(p) = ball.outer_position() {
                    anchor.x = p.x;
                    anchor.y = p.y;
                }
            }
        }
    }

    // Windows 下不可调整大小的窗口会忽略 set_size；放开后须延迟恢复，
    // 否则同一帧内重新锁定会吞掉这次尺寸变更。
    let _ = ball.set_resizable(true);
    let _ = ball.set_size(tauri::Size::Logical(tauri::LogicalSize { width, height }));

    if expanded {
        // 展开后立即按当前显示器工作区限位，保证三个按钮不出屏、不压任务栏。
        if let Some(monitor) = monitor_for_window(&app, &ball) {
            clamp_to_work_area(&ball, &monitor);
        }
    } else {
        // 收起后恢复到展开前的球锚点。
        let anchor = state
            .ball_anchor
            .lock()
            .map(|a| (*a).clone())
            .unwrap_or(BallPosition { x: -1, y: -1 });
        if anchor.x >= 0 {
            let _ = ball.set_position(tauri::Position::Physical(tauri::PhysicalPosition {
                x: anchor.x,
                y: anchor.y,
            }));
        }
        if let Ok(mut a) = state.ball_anchor.lock() {
            a.x = -1;
            a.y = -1;
        }
    }

    let ball2 = ball.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(140));
        let _ = ball2.set_resizable(false);
    });
    Ok(())
}

/// 拖动结束后调用一次：用工作区吸附并保存位置。
#[tauri::command]
fn snap_ball_to_work_area(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let Some(ball) = app.get_webview_window("ball") else {
        return Ok(());
    };
    snap_ball_position(&app, &state);
    // 更新锚点为吸附后的位置，保证下次展开从新位置起算。
    if let Ok(p) = ball.outer_position() {
        if let Ok(mut a) = state.ball_anchor.lock() {
            a.x = p.x;
            a.y = p.y;
        }
    }
    Ok(())
}

/// 悬浮球右键：在鼠标位置弹出原生菜单（不受 82px 透明窗口裁切）。
#[tauri::command]
fn popup_ball_menu(app: AppHandle, x: i32, y: i32) -> Result<(), String> {
    let Some(ball) = app.get_webview_window("ball") else {
        return Ok(());
    };
    let open = MenuItem::with_id(&app, BALL_MENU_OPEN, "显示主窗口", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let hide = MenuItem::with_id(&app, BALL_MENU_HIDE, "隐藏悬浮球", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let quit = MenuItem::with_id(&app, BALL_MENU_QUIT, "退出", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let menu = Menu::with_items(&app, &[&open, &hide, &quit]).map_err(|e| e.to_string())?;
    ball.popup_menu_at(
        &menu,
        tauri::Position::Physical(tauri::PhysicalPosition { x, y }),
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

// ---------------- 辅助函数 ----------------

fn focus_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn sync_ball_window(app: &AppHandle, visible: bool) {
    if let Some(w) = app.get_webview_window("ball") {
        if visible {
            let _ = w.show();
        } else {
            let _ = w.hide();
        }
    }
}

/// 发送系统通知（失败时写降级标志，不崩溃）
fn send_notification(app: &AppHandle, title: &str, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    match app.notification().builder().title(title).body(body).show() {
        Ok(_) => {}
        Err(err) => {
            if let Some(state) = app.try_state::<AppState>() {
                state.notify_degraded.store(true, Ordering::SeqCst);
                let _ = state
                    .logger
                    .log("ERROR", &format!("notification failed: {err}"));
            }
            let _ = app.emit("notify-denied", ());
        }
    }
}

/// 播放提示音：通知前端（由 WebView 播放嵌入的音频），避免依赖系统 API
fn play_chime(app: &AppHandle) {
    let _ = app.emit("play-chime", ());
}

fn announce_phase_complete(app: &AppHandle, payload: &PhaseCompletePayload) {
    let title = match payload.finished_phase {
        timer::Phase::Focus => "专注完成",
        _ => "休息结束",
    };
    send_notification(app, title, &format!("下一阶段：{}", payload.next_label));
    let sound_enabled = app
        .try_state::<AppState>()
        .map(|s| {
            s.settings
                .lock()
                .map(|settings| settings.sound_enabled)
                .unwrap_or(false)
        })
        .unwrap_or(false);
    if sound_enabled {
        play_chime(app);
    }
    let _ = app.emit("phase-complete", payload);
}

/// 定时器后台线程：每 500ms 检查一次截止时间，触发一次性完成事件
fn spawn_ticker(app: AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(500));
        let state = match app.try_state::<AppState>() {
            Some(s) => s,
            None => continue,
        };

        let payload = {
            let _persist = state
                .persist_lock
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let (payload, persisted) = {
                let mut timer = match state.timer.lock() {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                let settings = match state.settings.lock() {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                timer.rollover_day_if_needed();
                let payload = timer.consume_completion(&settings);
                (payload, timer.clone())
            };

            if payload.is_some() {
                // 后台保存失败已由 AppState 写入日志并发出 storage-error，这里不阻塞计时。
                let _ = state.save_timer_snapshot(&app, &persisted);
            }
            payload
        };

        if let Some(p) = payload {
            announce_phase_complete(&app, &p);
        }
    });
}

/// 悬浮球边缘吸附 + 越界校正
fn snap_ball_position(app: &AppHandle, state: &AppState) {
    let window = match app.get_webview_window("ball") {
        Some(w) => w,
        None => return,
    };
    let pos = match window.outer_position() {
        Ok(p) => p,
        Err(_) => return,
    };
    let size = match window.outer_size() {
        Ok(s) => s,
        Err(_) => return,
    };

    let monitors = app.available_monitors().unwrap_or_default();
    if monitors.is_empty() {
        return;
    }
    // 找包含窗口中心点的显示器；若无，归到主显示器
    let cx = pos.x + size.width as i32 / 2;
    let cy = pos.y + size.height as i32 / 2;
    let mut target = monitors[0].clone();
    for m in &monitors {
        let mp = m.position();
        let ms = m.size();
        if cx >= mp.x && cx <= mp.x + ms.width as i32 && cy >= mp.y && cy <= mp.y + ms.height as i32
        {
            target = m.clone();
            break;
        }
    }
    // 使用显示器工作区（排除任务栏），而非完整屏幕尺寸。
    let area = target.work_area();
    let min_x = area.position.x;
    let min_y = area.position.y;
    let max_x = area.position.x + area.size.width as i32 - size.width as i32;
    let max_y = area.position.y + area.size.height as i32 - size.height as i32;

    let mut nx = pos.x.clamp(min_x, max_x.max(min_x));
    let mut ny = pos.y.clamp(min_y, max_y.max(min_y));
    // 边缘吸附：距边 < 40px 则贴边
    if nx - min_x < 40 {
        nx = min_x;
    }
    if max_x - nx < 40 {
        nx = max_x;
    }
    if ny - min_y < 40 {
        ny = min_y;
    }
    if max_y - ny < 40 {
        ny = max_y;
    }

    if nx != pos.x || ny != pos.y {
        let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition {
            x: nx,
            y: ny,
        }));
    }
    if let Ok(mut p) = state.ball_pos.lock() {
        p.x = nx;
        p.y = ny;
    }
    let _ = state.save_ball_pos(app);
}

/// 构建托盘菜单及事件
fn setup_tray(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let toggle = MenuItem::with_id(app, "toggle", "开始 / 暂停", true, None::<&str>)?;
    let ball = MenuItem::with_id(app, "ball", "显示 / 隐藏悬浮球", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &toggle, &ball, &quit])?;

    let mut builder = TrayIconBuilder::new()
        .menu(&menu)
        .show_menu_on_left_click(false);
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    let _tray = builder
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "show" => {
                focus_main_window(app);
            }
            "toggle" => {
                if let Some(state) = app.try_state::<AppState>() {
                    if let Ok((_, snap)) = state.with_timer_change(app, |timer, _| timer.toggle()) {
                        let _ = app.emit("timer-changed", &snap);
                    }
                }
            }
            "ball" => {
                if let Some(state) = app.try_state::<AppState>() {
                    let visible = state.with_settings_change(app, |s| {
                        s.ball_visible = !s.ball_visible;
                        s.ball_visible
                    });
                    if let Ok(visible) = visible {
                        sync_ball_window(app, visible);
                    }
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                focus_main_window(app);
            }
        })
        .build(app)?;
    Ok(())
}

/// 启动追踪日志（诊断用，输出到 stderr）
fn trace(msg: &str) {
    eprintln!("[TRACE] {}", msg);
}

/// 解析可写的数据目录。
/// 首选 `%APPDATA%\com.future.pomodoro`；不可写时回退到稳定的
/// `%LOCALAPPDATA%\PomodoroAppData`。不再使用 exe 同级 `data` 或临时目录，
/// 两者都不可写时直接报错，不静默换位置。
fn resolve_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(base) = app.path().app_data_dir() {
        candidates.push(base);
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local).join("PomodoroAppData"));
    }
    for base in candidates {
        if fs::create_dir_all(&base).is_err() {
            continue;
        }
        let probe = base.join(".write-probe");
        let writable = fs::write(&probe, b"1").is_ok();
        let _ = fs::remove_file(&probe);
        if writable {
            return Ok(base);
        }
        eprintln!("[TRACE] data dir not writable: {base:?}");
    }
    Err("没有可写的数据目录：%APPDATA%\\com.future.pomodoro 与 %LOCALAPPDATA%\\PomodoroAppData 均不可写".to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    trace("run-start");
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            focus_main_window(app);
        }))
        .setup(|app| {
            trace("setup-enter");
            let data_dir = resolve_data_dir(app.handle()).map_err(|e| {
                trace(&format!("resolve_data_dir FAIL: {e}"));
                std::io::Error::new(std::io::ErrorKind::PermissionDenied, e)
            })?;
            trace(&format!("data_dir={:?}", data_dir));

            let logger = Arc::new(logging::Logger::new(&data_dir));
            if let Err(e) = logger.init() {
                eprintln!("[log] init failed: {e}");
            }
            let _ = logger.log("INFO", &format!("startup data_dir={}", data_dir.display()));

            let settings: Settings = match storage::load_json::<Settings>(&data_dir, "settings.json") {
                Ok(status) => {
                    if status.is_recovered() {
                        let _ = logger.log("WARN", "settings.json recovered from .bak");
                    }
                    status.into_option().unwrap_or_default().sanitized()
                }
                Err(e) => {
                    let _ = logger.log("ERROR", &format!("settings.json load failed: {e}"));
                    Settings::default()
                }
            };
            let mut timer: TimerState = match storage::load_json::<TimerState>(&data_dir, "timer.json") {
                Ok(status) => {
                    if status.is_recovered() {
                        let _ = logger.log("WARN", "timer.json recovered from .bak");
                    }
                    status.into_option().unwrap_or_else(|| TimerState::new(&settings))
                }
                Err(e) => {
                    let _ = logger.log("ERROR", &format!("timer.json load failed: {e}"));
                    TimerState::new(&settings)
                }
            };
            let ball_pos: BallPosition = match storage::load_json::<BallPosition>(&data_dir, "ball-position.json") {
                Ok(status) => {
                    if status.is_recovered() {
                        let _ = logger.log("WARN", "ball-position.json recovered from .bak");
                    }
                    status.into_option().unwrap_or(BallPosition { x: -1, y: -1 })
                }
                Err(e) => {
                    let _ = logger.log("ERROR", &format!("ball-position.json load failed: {e}"));
                    BallPosition { x: -1, y: -1 }
                }
            };
            timer.rollover_day_if_needed();
            // 旧 timer.json 可能缺少 phase_total_ms；用当前阶段应有的时长修复，避免进度环除零。
            timer.ensure_phase_total(&settings);
            timer.normalize_loaded();

            // 处理关机时未消费的完成事件：若已越过截止时间，补发一次
            let had_elapsed = timer.running && timer.ends_at.map(|e| timer::TimerState::now_ms_public() >= e).unwrap_or(false);
            let pending_payload = if had_elapsed {
                timer.consume_completion(&settings)
            } else {
                None
            };

            app.manage(AppState {
                timer: Mutex::new(timer),
                settings: Mutex::new(settings.clone()),
                ball_pos: Mutex::new(ball_pos.clone()),
                ball_anchor: Mutex::new(ball_pos),
                persist_lock: Mutex::new(()),
                data_dir,
                notify_degraded: AtomicBool::new(false),
                ball_expanded: AtomicBool::new(false),
                logger,
            });

            // 恢复悬浮球位置与可见性
            if let Some(ball) = app.get_webview_window("ball") {
                let state = app.state::<AppState>();
                let p = state.ball_pos.lock().unwrap().clone();
                if p.x >= 0 && p.y >= 0 {
                    let _ = ball.set_position(tauri::Position::Physical(tauri::PhysicalPosition { x: p.x, y: p.y }));
                }
                let _ = ball.set_always_on_top(settings.always_on_top);
                if settings.ball_visible {
                    let _ = ball.show();
                }
            }

            // 主窗口关闭按钮 → 隐藏到托盘（计时由后端状态机继续）
            if let Some(main) = app.get_webview_window("main") {
                let main_clone = main.clone();
                main.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = main_clone.hide();
                    }
                });
            }

            // 悬浮球拖动持久化（lead-08 必做项 1）：
            // 系统 drag loop 期间，JS 端 startDragging() 的 Promise 在本机不会 resolve
            // （实测 wait 3s 仍不 resolve，snap_ball_to_work_area 从未被调用，ball-position.json 不更新）。
            // 因此改由 Rust 侧静默跟踪 ball 窗口的 Moved：只记录坐标，静默约 360ms 后落盘。
            // 全程不调用 set_position，所以不会重现旧版「在 Moved 里吸附导致拖动抖动」的问题。
            if let Some(ball) = app.get_webview_window("ball") {
                let mx = std::sync::Arc::new(std::sync::atomic::AtomicI32::new(i32::MIN));
                let my = std::sync::Arc::new(std::sync::atomic::AtomicI32::new(i32::MIN));
                let seq = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
                {
                    let (mx, my, seq) = (mx.clone(), my.clone(), seq.clone());
                    ball.on_window_event(move |event| {
                        if let WindowEvent::Moved(pos) = event {
                            mx.store(pos.x, Ordering::SeqCst);
                            my.store(pos.y, Ordering::SeqCst);
                            seq.fetch_add(1, Ordering::SeqCst);
                        }
                    });
                }
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    let mut seen: u64 = 0;
                    let mut saved: u64 = 0;
                    let mut quiet: u32 = 0;
                    loop {
                        std::thread::sleep(Duration::from_millis(120));
                        let cur = seq.load(Ordering::SeqCst);
                        if cur == 0 || cur != seen {
                            seen = cur;
                            quiet = 0;
                            continue;
                        }
                        quiet += 1;
                        if quiet < 3 || seen == saved {
                            continue;
                        }
                        // 只持久化「收起态」的位置：展开态（315x300）由 set_ball_expanded
                        // 用 ball_anchor 管理，若把它也写进 ball_pos，收起后位置会逐次漂移。
                        let collapsed = handle
                            .try_state::<AppState>()
                            .map(|s| !s.ball_expanded.load(Ordering::SeqCst))
                            .unwrap_or(false);
                        if !collapsed {
                            saved = seen;
                            continue;
                        }
                        let (x, y) = (mx.load(Ordering::SeqCst), my.load(Ordering::SeqCst));
                        if x == i32::MIN {
                            continue;
                        }
                        if let Some(state) = handle.try_state::<AppState>() {
                            let changed = {
                                let p = state.ball_pos.lock().unwrap();
                                p.x != x || p.y != y
                            };
                            if changed {
                                if let Ok(mut p) = state.ball_pos.lock() {
                                    p.x = x;
                                    p.y = y;
                                }
                                // 锚点同步：收起态位置即锚点。否则拖完再展开/收起，
                                // 会跳回启动时的旧锚点，并把刚拖到的位置重新覆盖掉。
                                if let Ok(mut a) = state.ball_anchor.lock() {
                                    a.x = x;
                                    a.y = y;
                                }
                                let _ = state.save_ball_pos(&handle);
                            }
                        }
                        saved = seen;
                    }
                });
            }

            trace("before-setup_tray");
            setup_tray(app.handle()).map_err(|e| { trace(&format!("setup_tray FAIL: {e}")); e })?;
            trace("setup_tray-ok");
            spawn_ticker(app.handle().clone());

            // 补发关机期间错过的完成事件
            if let Some(p) = pending_payload {
                if let Some(state) = app.try_state::<AppState>() {
                    let _persist = state
                        .persist_lock
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    let persisted = state.timer.lock().unwrap().clone();
                    let _ = state.save_timer_snapshot(app.handle(), &persisted);
                }
                let app_handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(1200));
                    announce_phase_complete(&app_handle, &p);
                });
            }

            // 开机启动同步
            {
                trace("before-autostart");
                use tauri_plugin_autostart::ManagerExt;
                let handle = app.handle();
                if settings.autostart {
                    let _ = handle.autolaunch().enable();
                } else {
                    let _ = handle.autolaunch().disable();
                }
                trace("autostart-done");
            }

            trace("setup-return-Ok");
            Ok(())
        })
        .on_menu_event(|app, event| {
            // 悬浮球运行时弹窗菜单（popup_ball_menu 动态创建）走全局 handler。
            // 托盘菜单由 TrayIconBuilder.on_menu_event 独占处理，三个 id 不重叠，互不干扰。
            match event.id().as_ref() {
                BALL_MENU_OPEN => {
                    focus_main_window(app);
                }
                BALL_MENU_HIDE => {
                    if let Some(state) = app.try_state::<AppState>() {
                        if state.with_settings_change(app, |s| s.ball_visible = false).is_ok() {
                            sync_ball_window(app, false);
                        }
                    }
                }
                BALL_MENU_QUIT => app.exit(0),
                _ => {}
            }
        })
        .on_page_load(|webview, payload| {
            // 页面加载完成时按 theme.json 应用主题。
            // 注意：此回调可能早于 setup 中的 AppState 注册，故直接走文件系统而非 try_state。
            if let tauri::webview::PageLoadEvent::Finished = payload.event() {
                let app = webview.app_handle();
                let dark = match resolve_data_dir(app) {
                    Ok(dir) => match storage::load_text(&dir, "theme.json") {
                        Ok(status) => status.into_option().as_deref() == Some("dark"),
                        Err(e) => {
                            eprintln!("[theme] load failed: {e}");
                            false
                        }
                    },
                    Err(e) => {
                        eprintln!("[theme] data dir unavailable: {e}");
                        false
                    }
                };
                let js = format!(
                    "document.body.dataset.theme = '{}'; var t=document.getElementById('darkThemeToggle'); if(t) t.checked={};",
                    if dark { "dark" } else { "light" },
                    dark
                );
                eprintln!("[theme] dark={}", dark);
                let _ = webview.eval(&js);
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            get_settings,
            save_settings,
            toggle_timer,
            reset_timer,
            reset_cycle,
            skip_phase,
            show_main,
            set_ball_visible,
            is_ball_visible,
            notify_status,
            open_data_dir,
            set_theme,
            get_theme,
            get_ball_position,
            set_ball_expanded,
            snap_ball_to_work_area,
            popup_ball_menu,
            quit_app,
        ])
        .run(tauri::generate_context!())
        .expect("error while running pomodoro app");
}
