//! 计时状态机：以绝对截止时间（epoch 毫秒）为唯一真相源。
//! 不依赖每秒递减累加，休眠/锁屏恢复后仍能算出正确剩余时间与完成事件。

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// 阶段类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Focus,
    ShortBreak,
    LongBreak,
}

impl Phase {
    pub fn as_label(&self) -> &'static str {
        match self {
            Phase::Focus => "专注",
            Phase::ShortBreak => "短休息",
            Phase::LongBreak => "长休息",
        }
    }
    pub fn as_en(&self) -> &'static str {
        match self {
            Phase::Focus => "FOCUS",
            Phase::ShortBreak => "SHORT BREAK",
            Phase::LongBreak => "LONG BREAK",
        }
    }
    pub fn as_hint(&self, short: u32, long: u32) -> String {
        match self {
            Phase::Focus => format!("结束后休息 {} 分钟", short),
            Phase::ShortBreak => "休息结束后继续专注".to_string(),
            Phase::LongBreak => format!("长休息 {} 分钟，好好放松", long),
        }
    }
    pub fn is_break(&self) -> bool {
        matches!(self, Phase::ShortBreak | Phase::LongBreak)
    }
}

/// 可持久化设置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub focus_minutes: u32,
    pub short_minutes: u32,
    pub long_minutes: u32,
    pub cycle_count: u32,
    pub auto_start_next: bool,
    pub sound_enabled: bool,
    pub ball_visible: bool,
    pub autostart: bool,
    pub always_on_top: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            focus_minutes: 25,
            short_minutes: 5,
            long_minutes: 15,
            cycle_count: 4,
            auto_start_next: false,
            sound_enabled: true,
            ball_visible: true,
            autostart: false,
            always_on_top: true,
        }
    }
}

impl Settings {
    /// 兜底校验来自旧文件或 IPC 的数值，避免 0 时长、0 轮次等非法状态进入计时器。
    pub fn sanitized(mut self) -> Self {
        self.focus_minutes = self.focus_minutes.clamp(1, 90);
        self.short_minutes = self.short_minutes.clamp(1, 45);
        self.long_minutes = self.long_minutes.clamp(1, 45);
        self.cycle_count = self.cycle_count.clamp(2, 8);
        self
    }
}

/// 悬浮球位置（持久化）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BallPosition {
    pub x: i32,
    pub y: i32,
}

/// 计时器快照（发给前端）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimerSnapshot {
    pub phase: Phase,
    pub phase_label: String,
    pub phase_en: String,
    pub hint: String,
    pub running: bool,
    /// 当前阶段是否已经启动过；用于区分“待开始”和“已暂停”。
    pub started: bool,
    pub ends_at: Option<u64>,
    pub remaining_secs: u32,
    pub total_secs: u32,
    pub progress: f64,
    /// 兼容旧字段：大轮次，仅长休息结束后 +1；UI 不再用它表示组内轮次。
    pub round: u32,
    pub cycle_count: u32,
    /// 兼容旧字段：今日完成专注数（语义同 today_focus_count）。
    pub today_completed: u32,
    /// 兼容旧字段：今日专注分钟（语义同 today_focus_minutes）。
    pub today_minutes: u32,
    // ---- 本轮新增：正确的组内与今日统计字段 ----
    /// 本组已完成的专注数（0..=cycle_count）。
    pub focus_done_in_cycle: u32,
    /// 当前组内轮次（1..=cycle_count），供主界面与悬浮球统一显示。
    pub round_in_cycle: u32,
    pub today_focus_count: u32,
    pub today_focus_minutes: u32,
}

/// 阶段完成事件负载
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhaseCompletePayload {
    pub finished_phase: Phase,
    pub next_phase: Phase,
    pub next_label: String,
    pub completed_round: u32,
    pub auto_started: bool,
    pub focus_minutes: u32,
}

/// 状态机本体。仅内存持有；持久化由 AppState 负责。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimerState {
    pub phase: Phase,
    pub round: u32,
    pub running: bool,
    /// 当前阶段是否已经启动过；暂停后仍为 true，重置/进入新阶段后为 false。
    #[serde(default)]
    pub started: bool,
    /// 当前阶段截止时刻（epoch ms）。running 时有效。
    pub ends_at: Option<u64>,
    /// 暂停时的剩余毫秒（用于恢复）。
    pub remaining_ms: u64,
    /// 今日完成的专注轮数（仅专注阶段结束才累加）。
    pub today_completed: u32,
    /// 记录的本地日期（YYYY-MM-DD），用于跨日清零。
    pub day_stamp: String,
    /// 本组已完成的专注数（决定短/长休息）。
    #[serde(default)]
    pub focus_done_in_cycle: u32,
    /// 去重的完成事件标记：上一个已发出的完成事件对应的 ends_at。
    #[serde(default)]
    pub last_fired_ends_at: Option<u64>,
    /// 进入当前阶段时固定的总时长：当前阶段内不随设置改变。
    #[serde(default)]
    pub phase_total_ms: u64,
    /// 今日累计专注分钟（按每个专注阶段实际总时长累加）。
    #[serde(default)]
    pub today_focus_minutes: u32,
}

impl TimerState {
    pub fn new(settings: &Settings) -> Self {
        let total = Self::phase_ms(settings, Phase::Focus);
        Self {
            phase: Phase::Focus,
            round: 1,
            running: false,
            started: false,
            ends_at: None,
            remaining_ms: total,
            today_completed: 0,
            day_stamp: today_stamp(),
            focus_done_in_cycle: 0,
            last_fired_ends_at: None,
            phase_total_ms: total,
            today_focus_minutes: 0,
        }
    }

    fn phase_ms(settings: &Settings, phase: Phase) -> u64 {
        let minutes = match phase {
            Phase::Focus => settings.focus_minutes,
            Phase::ShortBreak => settings.short_minutes,
            Phase::LongBreak => settings.long_minutes,
        };
        minutes as u64 * 60_000
    }

    /// 当前阶段总时长：返回进入阶段时固定的值，不从当前设置重算。
    /// 这样保存新设置后当前阶段不会跳时，新时长只在下一阶段生效。
    pub fn total_ms(&self, _settings: &Settings) -> u64 {
        self.phase_total_ms
    }

    /// 旧 timer.json 可能缺少 phase_total_ms（为 0）；加载后调用本方法修复。
    pub fn ensure_phase_total(&mut self, settings: &Settings) {
        if self.phase_total_ms == 0 {
            let fallback = Self::phase_ms(settings, self.phase);
            self.phase_total_ms = fallback.max(self.remaining_ms).max(1);
        }
    }

    /// 为旧 timer.json 补齐 started 语义：运行中或剩余时间已减少，都说明阶段启动过。
    pub fn normalize_loaded(&mut self) {
        if self.running || self.remaining_ms < self.phase_total_ms {
            self.started = true;
        }
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    /// 供外部（启动补发逻辑）读取当前 epoch 毫秒
    pub fn now_ms_public() -> u64 {
        Self::now_ms()
    }

    /// 剩余毫秒（截止时间真相源）
    pub fn remaining_ms(&self) -> u64 {
        if !self.running {
            return self.remaining_ms;
        }
        match self.ends_at {
            Some(end) => end.saturating_sub(Self::now_ms()),
            None => self.remaining_ms,
        }
    }

    pub fn start(&mut self) {
        if self.running {
            return;
        }
        // 若之前已耗尽，重置为整段再开始
        if self.remaining_ms == 0 {
            self.remaining_ms = self.phase_total_ms.max(1);
        }
        self.ends_at = Some(Self::now_ms() + self.remaining_ms);
        self.running = true;
        self.started = true;
    }

    pub fn pause(&mut self) {
        if !self.running {
            return;
        }
        self.remaining_ms = self.remaining_ms();
        self.ends_at = None;
        self.running = false;
    }

    pub fn toggle(&mut self) {
        if self.running {
            self.pause();
        } else {
            self.start();
        }
    }

    /// 重置当前阶段到满时长（不清今日轮数）
    pub fn reset(&mut self, _settings: &Settings) {
        self.running = false;
        self.ends_at = None;
        // 用进入阶段时固定的总时长，而非当前设置，避免跳时。
        self.remaining_ms = self.phase_total_ms;
        self.started = false;
        self.last_fired_ends_at = None;
    }

    /// Reset the whole cycle to the first focus round; keep today's stats.
    pub fn reset_cycle(&mut self, settings: &Settings) {
        self.phase = Phase::Focus;
        self.round = 1;
        self.focus_done_in_cycle = 0;
        self.phase_total_ms = Self::phase_ms(settings, Phase::Focus);
        self.remaining_ms = self.phase_total_ms;
        self.running = false;
        self.started = false;
        self.ends_at = None;
        self.last_fired_ends_at = None;
    }

    /// 跳过当前阶段：不增加今日完成数（跳过不算完成），切到下一阶段。
    pub fn skip(&mut self, settings: &Settings) -> PhaseCompletePayload {
        self.advance_phase(settings, false, false)
    }

    /// 当截止时间到达时消费一次完成事件，切换到下一阶段。
    /// 返回 Some 表示本次确实发生了完成（去重由 last_fired_ends_at 保证）。
    pub fn consume_completion(&mut self, settings: &Settings) -> Option<PhaseCompletePayload> {
        if !self.running {
            return None;
        }
        let end = self.ends_at?;
        if Self::now_ms() < end {
            return None;
        }
        // 去重：同一 ends_at 只发一次
        if self.last_fired_ends_at == Some(end) {
            return None;
        }
        self.last_fired_ends_at = Some(end);
        // 统计移入 advance_phase：只有真正完成专注阶段才累加。
        Some(self.advance_phase(settings, true, settings.auto_start_next))
    }

    /// 推进到下一阶段。
    /// count_focus=true 表示"阶段正常完成"，false 表示"跳过"。
    fn advance_phase(
        &mut self,
        settings: &Settings,
        count_focus: bool,
        auto_start: bool,
    ) -> PhaseCompletePayload {
        let finished = self.phase;
        let completed_round = self.round;
        let finished_total_ms = self.phase_total_ms;

        // 只有"完成一个专注阶段"才算一次专注；短休/长休结束或跳过都不算。
        if count_focus && finished == Phase::Focus {
            self.focus_done_in_cycle += 1;
            self.today_completed += 1;
            self.today_focus_minutes += (finished_total_ms / 60_000) as u32;
        }

        let next = match finished {
            Phase::Focus => {
                // 第 4 次专注结束进入长休，但此处不清零——长按显示"第 4 轮已完成"。
                if self.focus_done_in_cycle >= settings.cycle_count.max(1) {
                    Phase::LongBreak
                } else {
                    Phase::ShortBreak
                }
            }
            _ => {
                // 长休息结束或跳过长休后，清零组内计数，下一专注显示第 1 轮。
                if finished == Phase::LongBreak {
                    self.focus_done_in_cycle = 0;
                    self.round += 1;
                }
                Phase::Focus
            }
        };

        // 进入新阶段：固定新阶段总时长（用当前设置），下一阶段才采用新时长。
        self.phase = next;
        self.phase_total_ms = Self::phase_ms(settings, next);
        self.remaining_ms = self.phase_total_ms;
        self.ends_at = None;
        self.running = false;
        self.started = false;

        if auto_start {
            self.start();
        }

        PhaseCompletePayload {
            finished_phase: finished,
            next_phase: next,
            next_label: next.as_label().to_string(),
            completed_round,
            auto_started: auto_start,
            focus_minutes: settings.focus_minutes,
        }
    }

    /// 当前组内轮次：专注中=已完成数+1；休息中=已完成数（至少 1）。
    pub fn round_in_cycle(&self) -> u32 {
        match self.phase {
            Phase::Focus => self.focus_done_in_cycle + 1,
            _ => self.focus_done_in_cycle.max(1),
        }
    }

    /// 跨日清零今日轮数与今日分钟
    pub fn rollover_day_if_needed(&mut self) {
        let stamp = today_stamp();
        if stamp != self.day_stamp {
            self.day_stamp = stamp;
            self.today_completed = 0;
            self.today_focus_minutes = 0;
        }
    }

    pub fn snapshot(&self, settings: &Settings) -> TimerSnapshot {
        let total_ms = self.total_ms(settings).max(1);
        let remain_ms = self.remaining_ms();
        let remain_secs = remain_ms.div_ceil(1000) as u32;
        let total_secs = (total_ms / 1000) as u32;
        let progress = (remain_ms as f64 / total_ms as f64 * 100.0).clamp(0.0, 100.0);
        TimerSnapshot {
            phase: self.phase,
            phase_label: self.phase.as_label().to_string(),
            phase_en: self.phase.as_en().to_string(),
            hint: self
                .phase
                .as_hint(settings.short_minutes, settings.long_minutes),
            running: self.running,
            started: self.started,
            ends_at: self.ends_at,
            remaining_secs: remain_secs,
            total_secs,
            progress,
            round: self.round,
            cycle_count: settings.cycle_count,
            today_completed: self.today_completed,
            today_minutes: self.today_focus_minutes,
            focus_done_in_cycle: self.focus_done_in_cycle,
            round_in_cycle: self.round_in_cycle(),
            today_focus_count: self.today_completed,
            today_focus_minutes: self.today_focus_minutes,
        }
    }
}

/// 本地日期 YYYY-MM-DD（无第三方依赖，走 UTC+8 近似）
fn today_stamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let local = secs + 8 * 3600; // 北京时间
    let days = local.div_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{:04}-{:02}-{:02}", y, m, d)
}

/// 仅供测试：模拟阶段已耗尽后消费完成
#[cfg(test)]
impl TimerState {
    fn consume_after_manual_elapse(&mut self, settings: &Settings) -> PhaseCompletePayload {
        self.running = true;
        self.ends_at = Some(TimerState::now_ms() - 1);
        self.last_fired_ends_at = None;
        self.consume_completion(settings)
            .expect("completion should fire")
    }
}

/// Howard Hinnant 的 days-from-civil 逆变换
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    y += if m <= 2 { 1 } else { 0 };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn s() -> Settings {
        Settings::default()
    }

    #[test]
    fn default_params() {
        let st = TimerState::new(&s());
        assert_eq!(st.phase, Phase::Focus);
        assert_eq!(st.round, 1);
        assert_eq!(st.total_ms(&s()), 25 * 60_000);
        assert!(!st.running);
        assert!(!st.started);
    }

    #[test]
    fn focus_then_rest_then_focus() {
        let settings = s();
        let mut st = TimerState::new(&settings);
        // 第 1 轮专注完成 -> 短休，round 仍 1
        let p = st.consume_after_manual_elapse(&settings);
        assert_eq!(p.next_phase, Phase::ShortBreak);
        assert_eq!(p.completed_round, 1);
        assert_eq!(st.today_completed, 1, "仅专注完成才计入今日");
        assert_eq!(st.today_focus_minutes, 25, "按实际专注总时长累加");
        // 休息完成 -> 专注（今日统计不变）
        let p = st.consume_after_manual_elapse(&settings);
        assert_eq!(p.next_phase, Phase::Focus);
        assert_eq!(st.round, 1);
        assert_eq!(st.today_completed, 1, "短休完成不得增加今日专注数");
        assert_eq!(st.today_focus_minutes, 25);
    }

    #[test]
    fn long_break_increments_round_after_rest() {
        let settings = s();
        let mut st = TimerState::new(&settings);
        // 走完 4 个完整专注（每个专注后接休息），第 4 个专注进长休
        for i in 0..4 {
            let p = st.consume_after_manual_elapse(&settings); // 专注完成
            if i < 3 {
                assert_eq!(p.next_phase, Phase::ShortBreak);
                st.consume_after_manual_elapse(&settings); // 短休完成 -> 专注
            } else {
                assert_eq!(p.next_phase, Phase::LongBreak);
            }
        }
        assert_eq!(st.phase, Phase::LongBreak);
        // 第 4 次专注结束进入长休时仍显示第 4 轮，不清零
        assert_eq!(st.focus_done_in_cycle, 4);
        assert_eq!(st.round_in_cycle(), 4);
        // 长休完成 -> 专注，round 变为 2，组内计数清零
        st.consume_after_manual_elapse(&settings);
        assert_eq!(st.phase, Phase::Focus);
        assert_eq!(st.round, 2);
        assert_eq!(st.focus_done_in_cycle, 0);
        assert_eq!(st.round_in_cycle(), 1);
    }

    #[test]
    fn long_break_every_four() {
        let settings = s();
        let mut st = TimerState::new(&settings);
        // 连续 4 个专注完成，中间不插休息完成，用于验证长休触发
        for i in 0..4 {
            // 手动把阶段设回专注以隔离测试
            st.phase = Phase::Focus;
            let p = st.consume_after_manual_elapse(&settings);
            if i < 3 {
                assert_eq!(p.next_phase, Phase::ShortBreak);
            } else {
                assert_eq!(p.next_phase, Phase::LongBreak);
            }
        }
        assert_eq!(st.today_completed, 4);
    }

    #[test]
    fn four_focus_round_sequence_is_1234() {
        let settings = s();
        let mut st = TimerState::new(&settings);
        let mut seq = Vec::new();
        for i in 0..4 {
            // 专注进行中显示轮次
            seq.push(st.round_in_cycle());
            st.consume_after_manual_elapse(&settings); // 专注完成
            if i < 3 {
                // 短休中仍显示刚完成的那一轮
                assert_eq!(st.round_in_cycle(), (i + 1) as u32);
                st.consume_after_manual_elapse(&settings); // 短休完成
            }
        }
        assert_eq!(seq, vec![1, 2, 3, 4]);
        assert_eq!(st.today_completed, 4);
        assert_eq!(st.today_focus_minutes, 100);
    }

    #[test]
    fn skip_focus_does_not_count() {
        let settings = s();
        let mut st = TimerState::new(&settings);
        let _ = st.skip(&settings);
        assert_eq!(st.today_completed, 0, "跳过专注不增加今日统计");
        assert_eq!(st.today_focus_minutes, 0);
        assert_eq!(st.focus_done_in_cycle, 0);
    }

    #[test]
    fn settings_change_keeps_current_phase_total() {
        let settings = s();
        let mut st = TimerState::new(&settings);
        assert_eq!(st.total_ms(&settings), 25 * 60_000);
        // 保存新设置：当前专注阶段总时长与进度不变
        let mut changed = s();
        changed.focus_minutes = 40;
        assert_eq!(st.total_ms(&changed), 25 * 60_000, "当前阶段不得跳时");
        st.reset(&changed);
        assert_eq!(st.remaining_ms, 25 * 60_000, "重置也用固定的阶段总时长");
        // 下一阶段才采用新时长
        st.consume_after_manual_elapse(&changed); // 专注完成 -> 短休
        assert_eq!(st.phase_total_ms, 5 * 60_000);
        st.consume_after_manual_elapse(&changed); // 短休完成 -> 专注
        assert_eq!(st.phase_total_ms, 40 * 60_000, "下一专注采用新时长");
    }

    #[test]
    fn reset_cycle_returns_to_first_focus_and_keeps_today_stats() {
        let settings = s();
        let mut st = TimerState::new(&settings);
        st.consume_after_manual_elapse(&settings);
        st.consume_after_manual_elapse(&settings);
        st.consume_after_manual_elapse(&settings);
        assert_eq!(st.today_completed, 2);
        assert_eq!(st.round_in_cycle(), 2);

        st.reset_cycle(&settings);

        assert_eq!(st.phase, Phase::Focus);
        assert_eq!(st.round, 1);
        assert_eq!(st.focus_done_in_cycle, 0);
        assert_eq!(st.round_in_cycle(), 1);
        assert_eq!(st.remaining_ms, 25 * 60_000);
        assert_eq!(st.today_completed, 2);
        assert!(!st.running && !st.started);
        assert_eq!(st.ends_at, None);
    }

    #[test]
    fn old_json_without_new_fields_loads() {
        let old = r#"{
            "phase":"focus","round":1,"running":false,"endsAt":null,
            "remainingMs":1500000,"todayCompleted":3,"dayStamp":"2026-11-19"
        }"#;
        let mut st: TimerState = serde_json::from_str(old).expect("old timer.json must load");
        assert_eq!(st.today_completed, 3);
        assert_eq!(st.focus_done_in_cycle, 0);
        assert_eq!(st.today_focus_minutes, 0);
        st.ensure_phase_total(&s());
        assert_eq!(st.phase_total_ms, 25 * 60_000);
    }

    #[test]
    fn invalid_and_partial_settings_are_safe() {
        let partial: Settings = serde_json::from_str(r#"{"focusMinutes":0,"cycleCount":0}"#)
            .expect("missing fields should use defaults");
        let safe = partial.sanitized();
        assert_eq!(safe.focus_minutes, 1);
        assert_eq!(safe.cycle_count, 2);
        assert_eq!(safe.short_minutes, 5);
    }

    #[test]
    fn pause_resume_uses_deadline() {
        let settings = s();
        let mut st = TimerState::new(&settings);
        st.start();
        std::thread::sleep(Duration::from_millis(30));
        st.pause();
        let rem = st.remaining_ms();
        assert!(
            rem < 25 * 60_000 && rem > 0,
            "pause should keep partial remaining: {}",
            rem
        );
        st.start();
        assert!(st.ends_at.is_some());
        assert!(st.started);
    }

    #[test]
    fn started_distinguishes_fresh_pause_reset_and_auto_start() {
        let settings = s();
        let mut st = TimerState::new(&settings);
        assert!(!st.started, "new phase is waiting, not paused");

        st.toggle();
        assert!(st.started && st.running);
        st.toggle();
        assert!(st.started && !st.running, "paused phase remains started");

        st.reset(&settings);
        assert!(!st.started, "reset returns to the waiting state");

        let mut auto = settings.clone();
        auto.auto_start_next = true;
        st.start();
        let payload = st.consume_after_manual_elapse(&auto);
        assert_eq!(payload.next_phase, Phase::ShortBreak);
        assert!(
            st.started && st.running,
            "auto-started next phase is running"
        );

        st.pause();
        assert!(st.started && !st.running);
        let payload = st.skip(&auto);
        assert_eq!(payload.next_phase, Phase::Focus);
        assert!(
            !st.started && !st.running,
            "newly entered phase waits to start"
        );
    }

    #[test]
    fn legacy_partial_pause_is_normalized_as_started() {
        let mut st = TimerState::new(&s());
        st.running = false;
        st.remaining_ms = st.phase_total_ms - 1000;
        st.started = false;
        st.normalize_loaded();
        assert!(st.started);
    }

    #[test]
    fn remaining_never_negative_after_sleep() {
        let settings = s();
        let mut st = TimerState::new(&settings);
        st.start();
        // 模拟休眠：直接把 ends_at 拨到过去
        st.ends_at = Some(TimerState::now_ms() - 1000);
        assert_eq!(st.remaining_ms(), 0);
        let p = st.consume_completion(&settings).unwrap();
        assert_eq!(p.finished_phase, Phase::Focus);
        // 去重：第二次不再发
        assert!(st.consume_completion(&settings).is_none());
    }
}
