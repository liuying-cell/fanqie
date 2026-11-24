import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./ball.css";

type Phase = "focus" | "shortbreak" | "longbreak";
interface Snapshot {
  phase: Phase;
  phaseLabel: string;
  remainingSecs: number;
  totalSecs: number;
  progress: number;
  round: number;
  cycleCount: number;
  running: boolean;
  started: boolean;
  endsAt: number | null;
  roundInCycle: number;
  todayFocusCount: number;
}

const app = document.getElementById("ball-app")!;
app.innerHTML = `
  <div class="ball-root">
    <button class="floating-ball" id="ball" type="button" aria-label="展开或收起悬浮球">
      <span class="ball-content">
        <span class="ball-time" id="ballTime">25</span>
        <span class="ball-unit">MIN</span>
      </span>
      <span class="ball-error-badge" id="ballErrorBadge" hidden aria-label="存储错误">!</span>
    </button>

    <div class="ball-card" id="ballCard" data-open="false">
      <div class="ball-error" id="ballError" hidden></div>
      <div class="ball-card-top">
        <div>
          <span class="ball-status" id="ballStatus">专注中</span>
          <p class="ball-round" id="ballRound">今日第 1 轮</p>
        </div>
        <button class="collapse-card" id="collapseCard" type="button">收起</button>
      </div>
      <div class="ball-time-row">
        <span class="ball-big-time" id="cardTime">25:00</span>
        <span>剩余</span>
      </div>
      <div class="card-progress"><span id="cardProgress"></span></div>
      <div class="ball-actions">
        <button type="button" id="ballToggle">暂停</button>
        <button type="button" id="ballSkip">跳过</button>
        <button type="button" class="open-app" id="openApp">打开</button>
      </div>
    </div>

  </div>
`;

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const ball = $<HTMLButtonElement>("ball");
const ballCard = $("ballCard");
const ballTime = $("ballTime");
const ballStatus = $("ballStatus");
const ballRound = $("ballRound");
const cardTime = $("cardTime");
const cardProgress = $("cardProgress");
const ballToggle = $<HTMLButtonElement>("ballToggle");
const ballError = $("ballError");
const ballErrorBadge = $("ballErrorBadge");

let snap: Snapshot | null = null;
let expanded = false;

/// 持久化失败等错误在球上保持可见：收起态显示角标，展开卡片显示完整消息。
function showBallError(message: string) {
  ballError.textContent = message;
  ballError.hidden = false;
  ballErrorBadge.hidden = false;
}

function fmt(sec: number): string {
  const s = Math.max(0, Math.ceil(sec));
  return `${String(Math.floor(s / 60)).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`;
}

function tick() {
  if (snap) {
    let remain = snap.remainingSecs;
    if (snap.running && snap.endsAt) {
      remain = Math.max(0, Math.ceil((snap.endsAt - Date.now()) / 1000));
    }
    ballTime.textContent = String(Math.max(1, Math.ceil(remain / 60)));
    cardTime.textContent = fmt(remain);
    const p = Math.max(0, Math.min(100, (remain / Math.max(1, snap.totalSecs)) * 100));
    ball.style.setProperty("--ball-progress", p.toFixed(2));
    cardProgress.style.width = `${p.toFixed(2)}%`;
  }
}

function render(s: Snapshot) {
  snap = s;
  document.body.dataset.phase = s.phase;
  ballStatus.textContent = s.running
    ? `${s.phaseLabel} · 进行中`
    : s.started ? `${s.phaseLabel} · 已暂停` : `${s.phaseLabel} · 待开始`;
  // 与主界面同口径：显示本组第 X / Y 轮，不再用 round（大轮次）冒充今日轮数。
  ballRound.textContent = `本组第 ${s.roundInCycle} / ${s.cycleCount} 轮`;
  ballToggle.textContent = s.running ? "暂停" : s.started ? "继续" : "开始";
}

async function refresh() {
  try {
    const s = await invoke<Snapshot>("get_snapshot");
    render(s);
  } catch (e) {
    console.error("ball get_snapshot failed", e);
  }
}

async function setExpanded(next: boolean) {
  expanded = next;
  ballCard.dataset.open = String(next);
  await invoke("set_ball_expanded", { expanded: next });
}

// 单击展开/收起；拖动由 JS 手动触发，避免 drag-region 吞掉 click
let downX = 0;
let downY = 0;
let pressed = false;
let dragged = false;

ball.addEventListener("pointerdown", async (e) => {
  if (e.button !== 0) return;
  downX = e.screenX;
  downY = e.screenY;
  pressed = true;
  dragged = false;
});
ball.addEventListener("pointermove", async (e) => {
  if (!pressed || dragged) return;
  if (Math.abs(e.screenX - downX) > 4 || Math.abs(e.screenY - downY) > 4) {
    dragged = true;
    pressed = false;
    try {
      // startDragging() 的 Promise 在拖动结束后 resolve；此时吸附 + 越界校正 + 保存只做一次。
      await getCurrentWindow().startDragging();
      await invoke("snap_ball_to_work_area");
    } catch (err) {
      // 权限缺失会导致 startDragging 直接 reject，必须打出来才能定位。
      console.error("ball startDragging failed", err);
    }
  }
});
ball.addEventListener("pointerup", () => {
  const wasPressed = pressed && !dragged;
  pressed = false;
  if (wasPressed) void setExpanded(!expanded);
});

$("collapseCard").addEventListener("click", () => void setExpanded(false));
ballToggle.addEventListener("click", async () => {
  try {
    const s = await invoke<Snapshot>("toggle_timer");
    render(s);
  } catch (e) {
    showBallError("操作失败：" + String(e));
  }
});
$("ballSkip").addEventListener("click", async () => {
  try {
    await invoke("skip_phase");
    await refresh();
  } catch (e) {
    showBallError("跳过失败：" + String(e));
  }
});
$("openApp").addEventListener("click", async () => {
  await invoke("show_main");
});

// 右键菜单：调用 Rust 侧原生 Menu.popup_at，不受 82px 透明窗口裁切。
// 菜单项点击由后端全局 on_menu_event 处理（ball-open / ball-hide / ball-quit）。
ball.addEventListener("contextmenu", (e) => {
  e.preventDefault();
  void invoke("popup_ball_menu", { x: Math.round(e.screenX), y: Math.round(e.screenY) });
});

void listen("timer-changed", (e) => render(e.payload as Snapshot));
void listen("phase-complete", () => void refresh());
void listen<{ operation: string; message: string }>("storage-error", (e) => {
  showBallError(`保存失败：${e.payload.message}`);
});

// 主题跟随主窗口
function applyTheme(dark: boolean) {
  document.body.dataset.theme = dark ? "dark" : "light";
}
void invoke<boolean>("get_theme").then(applyTheme);
void listen<boolean>("theme-changed", (e) => applyTheme(e.payload));

// 只保留一条显示循环；render 频繁刷新时不能再次启动它。
window.setInterval(tick, 250);
tick();
void refresh();
