import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./styles.css";

// ---------------- 类型 ----------------
type Phase = "focus" | "shortbreak" | "longbreak";
interface Snapshot {
  phase: Phase;
  phaseLabel: string;
  phaseEn: string;
  hint: string;
  running: boolean;
  started: boolean;
  endsAt: number | null;
  remainingSecs: number;
  totalSecs: number;
  progress: number;
  round: number;
  cycleCount: number;
  todayCompleted: number;
  todayMinutes: number;
  focusDoneInCycle: number;
  roundInCycle: number;
  todayFocusCount: number;
  todayFocusMinutes: number;
}
interface Settings {
  focusMinutes: number;
  shortMinutes: number;
  longMinutes: number;
  cycleCount: number;
  autoStartNext: boolean;
  soundEnabled: boolean;
  ballVisible: boolean;
  autostart: boolean;
  alwaysOnTop: boolean;
}

type NumericSettingKey = "focusMinutes" | "shortMinutes" | "longMinutes" | "cycleCount";
type PresetName = "classic" | "deep" | "custom";
type PresetSettings = Pick<Settings, NumericSettingKey>;

const PRESETS: Record<Exclude<PresetName, "custom">, PresetSettings> = {
  classic: { focusMinutes: 25, shortMinutes: 5, longMinutes: 15, cycleCount: 4 },
  deep: { focusMinutes: 50, shortMinutes: 10, longMinutes: 30, cycleCount: 2 },
};
const NUMERIC_SETTING_KEYS: NumericSettingKey[] = [
  "focusMinutes",
  "shortMinutes",
  "longMinutes",
  "cycleCount",
];

// ---------------- 提示音（WebAudio 合成短促双音，无需外部资源） ----------------
let audioCtx: AudioContext | null = null;

// 在首次用户操作时预热音频上下文，避免阶段结束才创建时被 WebView 自动播放策略拦截。
function primeAudio() {
  try {
    audioCtx = audioCtx ?? new AudioContext();
    if (audioCtx.state === "suspended") void audioCtx.resume();
  } catch {
    /* 音频不可用时静默降级 */
  }
}
document.addEventListener("pointerdown", primeAudio, { capture: true, once: true });
document.addEventListener("keydown", primeAudio, { capture: true, once: true });

function playChime(): boolean {
  try {
    primeAudio();
    if (!audioCtx) return false;
    const ctx = audioCtx;
    const now = ctx.currentTime;
    [880, 1174].forEach((freq, i) => {
      const osc = ctx.createOscillator();
      const gain = ctx.createGain();
      osc.type = "sine";
      osc.frequency.value = freq;
      gain.gain.setValueAtTime(0.0001, now + i * 0.16);
      gain.gain.exponentialRampToValueAtTime(0.22, now + i * 0.16 + 0.02);
      gain.gain.exponentialRampToValueAtTime(0.0001, now + i * 0.16 + 0.3);
      osc.connect(gain).connect(ctx.destination);
      osc.start(now + i * 0.16);
      osc.stop(now + i * 0.16 + 0.32);
    });
    return true;
  } catch {
    /* 音频不可用时静默降级 */
    return false;
  }
}

// ---------------- DOM ----------------
const app = document.getElementById("app")!;
app.innerHTML = `
  <div class="app-window">
    <div class="window-bar" data-tauri-drag-region>
      <div class="brand">
        <span class="brand-glyph" aria-hidden="true">一</span>
        <span class="brand-copy"><strong>一轮</strong><small>番茄钟</small></span>
      </div>
      <div class="window-tools">
        <button class="settings-link" id="openSettings" type="button">设置</button>
        <button class="window-control" id="minBtn" type="button" aria-label="最小化"><span class="minimize-mark"></span></button>
        <button class="window-control close" id="closeBtn" type="button" aria-label="关闭到托盘"><span class="close-mark"></span></button>
      </div>
    </div>

    <div class="window-content">
      <section class="view" id="mainView">
        <div class="phase-line">
          <div>
            <span class="phase-title" id="phaseTitle">专注</span>
            <span class="phase-caption" id="phaseCaption">把注意力放在一件事上</span>
          </div>
          <span class="round-label" id="roundLabel">第 1 / 4 轮</span>
        </div>

        <div class="ring-area">
          <div class="timer-ring" id="timerRing">
            <div class="ring-inner">
              <span class="timer-label" id="timerLabel">FOCUS</span>
              <span class="timer-display" id="timerDisplay">25:00</span>
              <span class="timer-hint" id="timerHint">结束后休息 5 分钟</span>
            </div>
          </div>
        </div>

        <div class="session-strip" id="sessionStrip"></div>

        <button class="primary-button" id="timerToggle" type="button">开始专注</button>
        <div class="secondary-row">
          <button class="text-button" id="resetTimer" type="button" title="只重置当前阶段计时">重置计时</button>
          <span class="divider-dot"></span>
          <button class="text-button" id="skipPhase" type="button">跳过本轮</button>
          <span class="divider-dot"></span>
          <button class="text-button" id="resetCycle" type="button" title="回到第 1 轮，保留今日统计">重置整轮</button>
        </div>

        <div class="day-summary">
          <span>今日已完成 <strong id="todayCount">0 轮</strong></span>
          <span class="duration" id="todayMinutes">0 MIN</span>
        </div>
        <div class="degraded" id="degraded" hidden>系统通知不可用，阶段结束仅通过应用内提示与声音提醒。</div>
      </section>

      <section class="view settings-view" id="settingsView" hidden>
        <div class="settings-head">
          <div>
            <h2>设置</h2>
            <p>选择节奏或手动调整，保存后从下一阶段开始生效。</p>
          </div>
          <div class="settings-head-actions">
            <span class="settings-dirty" id="settingsDirty" role="status" hidden>有未保存的更改</span>
            <button class="back-button" id="closeSettings" type="button">返回</button>
          </div>
        </div>

        <div class="settings-group">
          <h3>计时节奏</h3>
          <div class="preset-grid" role="group" aria-label="计时节奏预设">
            <button class="preset-option" type="button" data-preset="classic" aria-pressed="false"><strong>经典</strong><span>25 / 5 / 15 · 4 轮</span></button>
            <button class="preset-option" type="button" data-preset="deep" aria-pressed="false"><strong>深度</strong><span>50 / 10 / 30 · 2 轮</span></button>
            <button class="preset-option" type="button" data-preset="custom" aria-pressed="false"><strong>自定义</strong><span>手动调整时长</span></button>
          </div>
          <div class="duration-grid">
            ${durationTile("专注时长", "focusMinutes")}
            ${durationTile("短休息", "shortMinutes")}
            ${durationTile("长休息", "longMinutes")}
            ${durationTile("长休息间隔", "cycleCount")}
          </div>
        </div>

        <div class="settings-group">
          <h3>提醒与悬浮球</h3>
          <div class="setting-list">
            ${switchRow("自动开始下一阶段", "关闭时，每轮结束后等待确认", "autoStartNext")}
            ${switchRow("结束提示音", "播放一段简短的本地提示音", "soundEnabled")}
            ${switchRow("显示悬浮球", "在主窗口关闭后继续显示进度", "ballVisible")}
          </div>
        </div>

        <div class="settings-group">
          <h3>窗口行为</h3>
          <div class="setting-list">
            ${switchRow("开机自动启动", "登录 Windows 后自动准备计时", "autostart")}
            ${switchRow("始终置顶", "让悬浮球保持在其他窗口上方", "alwaysOnTop")}
            <div class="setting-row"><span class="setting-label"><strong>深色外观</strong><span>切换到适合夜晚的配色</span></span>
              <label class="switch"><input type="checkbox" id="darkThemeToggle" aria-label="深色外观"><span class="switch-track"></span></label></div>
          </div>
        </div>

        <div class="settings-group">
          <h3>数据与诊断</h3>
          <div class="setting-list">
            <div class="setting-row"><span class="setting-label"><strong>数据目录</strong><span>设置、计时与本地日志保存在此目录</span></span>
              <button class="text-button" id="openDataDir" type="button">打开数据目录</button></div>
          </div>
        </div>

        <p class="settings-note">关闭主窗口后计时继续，应用会留在系统托盘。只有从托盘选择退出才会结束应用。</p>
        <button class="primary-button save-button" id="saveSettings" type="button">保存设置</button>
      </section>
    </div>

    <div class="toast" id="toast" role="status"></div>

    <dialog class="app-dialog" id="resetCycleDialog" aria-labelledby="resetCycleTitle" aria-describedby="resetCycleCopy">
      <form method="dialog" class="dialog-card">
        <h2 class="dialog-title" id="resetCycleTitle">回到第 1 轮？</h2>
        <p class="dialog-copy" id="resetCycleCopy">当前计时会停止，本组进度将清空。今日已完成轮数和专注分钟会保留。</p>
        <div class="dialog-actions">
          <button class="dialog-button dialog-cancel" type="submit" value="cancel" autofocus>继续本轮</button>
          <button class="dialog-button dialog-danger" type="submit" value="confirm">重置整轮</button>
        </div>
      </form>
    </dialog>
  </div>
`;

function durationTile(label: string, id: NumericSettingKey): string {
  const unit = id === "cycleCount" ? "轮" : "分";
  const defaults: Record<string, number> = {
    focusMinutes: 25,
    shortMinutes: 5,
    longMinutes: 15,
    cycleCount: 4,
  };
  return `<div class="duration-tile"><span>${label}</span>
    <div class="stepper" data-target="${id}">
      <button type="button" data-step="-1" aria-label="减少${label}">-</button>
      <output id="${id}">${defaults[id]} ${unit}</output>
      <button type="button" data-step="1" aria-label="增加${label}">+</button>
    </div></div>`;
}
function switchRow(label: string, desc: string, key: keyof Settings): string {
  const preview = key === "soundEnabled"
    ? `<button class="text-button preview-sound" id="previewSound" type="button">试听</button>`
    : "";
  return `<div class="setting-row"><span class="setting-label"><strong>${label}</strong><span>${desc}</span></span>
    <div class="setting-actions">${preview}<label class="switch"><input type="checkbox" data-key="${key}" aria-label="${label}"><span class="switch-track"></span></label></div></div>`;
}

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const timerDisplay = $("timerDisplay");
const timerRing = $("timerRing");
const timerLabel = $("timerLabel");
const timerHint = $("timerHint");
const timerToggle = $<HTMLButtonElement>("timerToggle");
const phaseTitle = $("phaseTitle");
const phaseCaption = $("phaseCaption");
const roundLabel = $("roundLabel");
const sessionStrip = $("sessionStrip");
const todayCount = $("todayCount");
const todayMinutes = $("todayMinutes");
const toast = $("toast");
const mainView = $("mainView");
const settingsView = $("settingsView");
const degraded = $("degraded");
const saveSettingsButton = $<HTMLButtonElement>("saveSettings");
const settingsDirty = $("settingsDirty");

let snap: Snapshot | null = null;
let toastTimer = 0;
let settings: Settings | null = null;
let savedSettings: Settings | null = null;
let presetMode: PresetName = "custom";
let settingsLoading = false;

function showToast(msg: string) {
  toast.textContent = msg;
  toast.classList.add("show");
  window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => toast.classList.remove("show"), 2200);
}

function fmt(sec: number): string {
  const s = Math.max(0, Math.ceil(sec));
  return `${String(Math.floor(s / 60)).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`;
}

function timerActionLabel(s: Snapshot): string {
  const isFocus = s.phase === "focus";
  if (s.running) return isFocus ? "暂停专注" : "暂停休息";
  if (!s.started) return isFocus ? "开始专注" : "开始休息";
  return isFocus ? "继续专注" : "继续休息";
}

function settingsEqual(a: Settings, b: Settings): boolean {
  return (Object.keys(a) as (keyof Settings)[]).every((key) => a[key] === b[key]);
}

function detectPreset(value: Settings): PresetName {
  for (const name of ["classic", "deep"] as const) {
    if (NUMERIC_SETTING_KEYS.every((key) => value[key] === PRESETS[name][key])) return name;
  }
  return "custom";
}

function renderPresetSelection() {
  document.querySelectorAll<HTMLButtonElement>(".preset-option").forEach((button) => {
    const active = button.dataset.preset === presetMode;
    button.classList.toggle("active", active);
    button.setAttribute("aria-pressed", String(active));
  });
}

function renderDurationControls(value: Settings) {
  NUMERIC_SETTING_KEYS.forEach((key) => {
    const el = document.getElementById(key);
    if (el) el.textContent = `${value[key]} ${key === "cycleCount" ? "轮" : "分"}`;
  });
}

function renderSwitchControls(value: Settings) {
  document.querySelectorAll<HTMLInputElement>(".switch input[data-key]").forEach((input) => {
    input.checked = Boolean((value as unknown as Record<string, boolean>)[input.dataset.key!]);
  });
}

function updateSettingsDirtyState() {
  if (settingsLoading) {
    settingsDirty.hidden = true;
    saveSettingsButton.disabled = true;
    saveSettingsButton.textContent = "读取设置…";
    return;
  }
  if (!settings) {
    settingsDirty.hidden = true;
    saveSettingsButton.disabled = true;
    saveSettingsButton.textContent = "设置不可用";
    return;
  }
  const dirty = Boolean(savedSettings && !settingsEqual(settings, savedSettings));
  settingsDirty.hidden = !dirty;
  saveSettingsButton.disabled = !dirty;
  saveSettingsButton.textContent = dirty ? "保存设置" : "已保存";
}

function renderSettingsDraft() {
  if (!settings) return;
  renderDurationControls(settings);
  renderSwitchControls(settings);
  renderPresetSelection();
  updateSettingsDirtyState();
}

function applyPreset(name: PresetName) {
  if (!settings) return;
  presetMode = name;
  if (name !== "custom") Object.assign(settings, PRESETS[name]);
  renderSettingsDraft();
}

function renderSessionStrip(s: Snapshot) {
  const n = Math.max(1, s.cycleCount);
  // 直接使用后端给出的组内完成数，不再从 round 推导。
  const doneInCycle = Math.min(n, s.focusDoneInCycle);
  let html = "";
  for (let i = 0; i < n; i++) {
    let cls = "session-segment";
    if (i < doneInCycle) cls += " done";
    else if (i === doneInCycle && s.phase === "focus") cls += " current";
    html += `<span class="${cls}"></span>`;
  }
  sessionStrip.innerHTML = html;
  sessionStrip.style.gridTemplateColumns = `repeat(${n}, 1fr)`;
}

// 本地即时递减显示：基于 endsAt 计算，避免依赖轮询频率
function tickDisplay() {
  if (snap) {
    let remain = snap.remainingSecs;
    if (snap.running && snap.endsAt) {
      remain = Math.max(0, Math.ceil((snap.endsAt - Date.now()) / 1000));
    }
    const time = fmt(remain);
    if (timerDisplay.textContent !== time) {
      timerDisplay.textContent = time;
    }
    const total = Math.max(1, snap.totalSecs);
    const progress = (remain / total) * 100;
    timerRing.style.setProperty("--progress", progress.toFixed(2));
  }
  window.setTimeout(tickDisplay, 250);
}

function render(s: Snapshot) {
  const isFocus = s.phase === "focus";
  document.body.dataset.theme = document.body.dataset.theme || "light";
  document.body.dataset.phase = s.phase;
  phaseTitle.textContent = s.phaseLabel;
  phaseCaption.textContent = isFocus
    ? "把注意力放在一件事上"
    : s.phase === "shortbreak" ? "短暂放松，恢复精力" : "好好休息，再出发";
  timerLabel.textContent = s.phaseEn;
  timerHint.textContent = s.hint;
  // 组内轮次用后端快照字段（专注中=已完成+1，休息中=已完成），主界面/悬浮球同口径。
  roundLabel.textContent = `第 ${s.roundInCycle} / ${s.cycleCount} 轮`;
  timerToggle.textContent = timerActionLabel(s);
  // 今日统计只使用后端提供的专注统计（排除休息阶段）。
  todayCount.textContent = `${s.todayFocusCount} 轮`;
  todayMinutes.textContent = `${s.todayFocusMinutes} MIN`;
  renderSessionStrip(s);
  snap = s;
  tickDisplayOnce();
}

let displayTicking = false;
function tickDisplayOnce() {
  if (displayTicking) return;
  displayTicking = true;
  tickDisplay();
}

async function refresh() {
  try {
    const s = await invoke<Snapshot>("get_snapshot");
    render(s);
  } catch (e) {
    console.error("get_snapshot failed", e);
  }
}

// 跨窗口兜底同步：悬浮球触发的启停之前只靠 timer-changed 事件下发，
// 而主窗口在后台时该事件可能被 WebView 节流，导致按钮/状态滞后。
// 这里在窗口可见时每秒拉一次快照，并在重新可见/获得焦点时立即同步。
window.setInterval(() => {
  if (!document.hidden) void refresh();
}, 1000);
window.addEventListener("focus", () => void refresh());
document.addEventListener("visibilitychange", () => {
  if (!document.hidden) void refresh();
});

// ---------------- 事件绑定 ----------------
$("openSettings").addEventListener("click", () => {
  mainView.hidden = true;
  settingsView.hidden = false;
  // 每次进入都重新读取，避免托盘/悬浮球修改设置后，本页仍用旧对象覆盖回去。
  void loadSettingsToUI();
});
$("closeSettings").addEventListener("click", () => {
  if (settings && savedSettings && !settingsEqual(settings, savedSettings)) {
    if (!window.confirm("有未保存的设置，确定放弃这些更改吗？")) return;
  }
  settingsView.hidden = true;
  mainView.hidden = false;
});
$("saveSettings").addEventListener("click", async () => {
  if (!settings) {
    showToast("设置尚未加载完成");
    return;
  }
  try {
    await invoke("save_settings", { settings });
    savedSettings = { ...settings };
    updateSettingsDirtyState();
    showToast("设置已保存，下一阶段生效");
    settingsView.hidden = true;
    mainView.hidden = false;
  } catch (e) {
    showToast("保存失败：" + String(e));
  }
});
$("previewSound").addEventListener("click", () => {
  showToast(playChime() ? "已播放提示音" : "当前环境无法播放提示音");
});

timerToggle.addEventListener("click", async () => {
  try {
    const s = await invoke<Snapshot>("toggle_timer");
    render(s);
  } catch (e) {
    showToast("操作失败：" + String(e));
  }
});
$("resetTimer").addEventListener("click", async () => {
  try {
    const s = await invoke<Snapshot>("reset_timer");
    render(s);
    showToast("本轮计时已重置");
  } catch (e) {
    showToast("重置失败：" + String(e));
  }
});
const resetCycleDialog = $<HTMLDialogElement>("resetCycleDialog");
let resetCycleBusy = false;

async function executeResetCycle() {
  if (resetCycleBusy) return;
  resetCycleBusy = true;
  try {
    const s = await invoke<Snapshot>("reset_cycle");
    render(s);
    showToast("已重置整轮，回到第 1 轮");
  } catch (e) {
    showToast("重置整轮失败：" + String(e));
  } finally {
    resetCycleBusy = false;
  }
}

$("resetCycle").addEventListener("click", () => {
  if (!snap || snap.focusDoneInCycle === 0) {
    void executeResetCycle();
    return;
  }
  resetCycleDialog.returnValue = "";
  resetCycleDialog.showModal();
});
resetCycleDialog.addEventListener("click", (e) => {
  if (e.target === resetCycleDialog) resetCycleDialog.close("cancel");
});
resetCycleDialog.addEventListener("close", () => {
  if (resetCycleDialog.returnValue === "confirm") void executeResetCycle();
});
$("skipPhase").addEventListener("click", async () => {
  try {
    await invoke("skip_phase");
    await refresh();
    showToast("已跳过本轮");
  } catch (e) {
    showToast("跳过失败：" + String(e));
  }
});
$("openDataDir").addEventListener("click", async () => {
  try {
    await invoke("open_data_dir");
    showToast("已打开数据目录");
  } catch (e) {
    showToast("打开数据目录失败：" + String(e));
  }
});
$("minBtn").addEventListener("click", async () => {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().minimize();
});
$("closeBtn").addEventListener("click", async () => {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  await getCurrentWindow().hide();
});

// 设置控件事件（事件委托）
document.querySelectorAll<HTMLButtonElement>(".preset-option").forEach((button) => {
  button.addEventListener("click", () => applyPreset(button.dataset.preset as PresetName));
});

document.querySelectorAll<HTMLElement>(".stepper").forEach((stepper) => {
  stepper.querySelectorAll<HTMLButtonElement>("button").forEach((btn) => {
    btn.addEventListener("click", () => {
      if (!settings) return;
      const target = stepper.dataset.target as NumericSettingKey;
      const isCycle = target === "cycleCount";
      const min = isCycle ? 2 : 1;
      const max = isCycle ? 8 : target === "focusMinutes" ? 90 : 45;
      const value = Math.max(min, Math.min(max, settings[target] + Number(btn.dataset.step)));
      settings[target] = value;
      presetMode = "custom";
      renderSettingsDraft();
    });
  });
});
// 只绑定带 data-key 的设置开关；深色外观开关无 data-key，由 get_theme/theme-changed 单独同步，
// 避免打开设置页时把已生效的深色主题错误重置。
document.querySelectorAll<HTMLInputElement>(".switch input[data-key]").forEach((input) => {
  input.addEventListener("change", () => {
    if (!settings) return;
    (settings as unknown as Record<string, boolean>)[input.dataset.key!] = input.checked;
    updateSettingsDirtyState();
  });
});

async function loadSettingsToUI() {
  settingsLoading = true;
  settings = null;
  savedSettings = null;
  presetMode = "custom";
  updateSettingsDirtyState();
  try {
    const loaded = await invoke<Settings>("get_settings");
    settings = { ...loaded };
    savedSettings = { ...loaded };
    presetMode = detectPreset(loaded);
    renderSettingsDraft();
  } catch (e) {
    showToast("读取设置失败：" + String(e));
  } finally {
    settingsLoading = false;
    updateSettingsDirtyState();
  }
}

// ---------------- 主题 ----------------
function applyTheme(dark: boolean) {
  themeDark = dark;
  document.body.dataset.theme = dark ? "dark" : "light";
  const toggle = document.getElementById("darkThemeToggle") as HTMLInputElement | null;
  if (toggle) toggle.checked = dark;
}
const darkThemeToggle = document.getElementById("darkThemeToggle") as HTMLInputElement;
darkThemeToggle.addEventListener("change", () => {
  void invoke("set_theme", { dark: darkThemeToggle.checked });
});
// F8：快速切换深浅主题（便于键盘化验证，不依赖设置页滚动位置）
let themeDark = false;
window.addEventListener("keydown", (e) => {
  if (e.key === "F8") {
    e.preventDefault();
    themeDark = !themeDark;
    void invoke("set_theme", { dark: themeDark });
  }
});
void invoke<boolean>("get_theme").then(applyTheme);
void listen<boolean>("theme-changed", (e) => applyTheme(e.payload));

// ---------------- 后端事件监听 ----------------
void listen("timer-changed", (e) => render(e.payload as Snapshot));
void listen<PhaseCompletePayload>("phase-complete", (e) => {
  const p = e.payload;
  void refresh();
  showToast(p.autoStarted ? `进入${p.nextLabel}（自动开始）` : `进入${p.nextLabel}`);
});
void listen("notify-denied", () => { degraded.hidden = false; });
void listen("play-chime", () => playChime());
void listen<{ operation: string; message: string }>("storage-error", (e) => {
  showToast(`保存失败：${e.payload.message}`);
});

interface PhaseCompletePayload {
  nextLabel: string;
  autoStarted: boolean;
}

// 启动时检查通知降级状态
void invoke<boolean>("notify_status").then((d) => { if (d) degraded.hidden = false; });

void refresh();
