# 一轮番茄钟

Windows 桌面版番茄钟。正式运行入口：

`C:\Users\<user>\AppData\Local\PomodoroApp\pomodoro-app.exe`

开始菜单快捷方式名称为“一轮番茄钟”。

## 重新安装

项目目录已保留最终可执行文件和安装包：

- `release\pomodoro-app.exe`
- `release\PomodoroApp_0.1.0_x64-setup.exe`

无管理员权限部署：

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

脚本会部署到 `%LOCALAPPDATA%\PomodoroApp`，并创建带正确 AUMID 的开始菜单快捷方式。

覆盖升级使用同一个命令：先把新 exe 复制到临时副本并校验 SHA256，再把当前
`pomodoro-app.exe` 留作 `pomodoro-app.previous.exe` 后替换；用户数据不会被动到。
若目标程序正在运行，脚本会明确报错退出，不强制结束进程——请先手动关闭应用。

## 数据位置

- 主数据目录：`%APPDATA%\com.future.pomodoro`（不可写时回退 `%LOCALAPPDATA%\PomodoroAppData`，不会使用 exe 同级 `data` 或临时目录）
- 配置文件：`settings.json`、`timer.json`、`ball-position.json`、`theme.json`
- 原子写入产生同名 `.tmp`（事务中）与 `.bak`（上一次的值）；主文件损坏时自动从 `.bak` 读取
- 诊断日志：`%APPDATA%\com.future.pomodoro\logs\pomodoro.log`，超过 1 MB 轮转为 `pomodoro.log.1`，最多两份
- 设置页「数据与诊断 → 打开数据目录」可在资源管理器中打开当前数据目录

## 卸载

默认卸载只删除程序目录与快捷方式，**保留全部用户数据**：

```powershell
powershell -ExecutionPolicy Bypass -File .\uninstall.ps1
```

若旧安装曾在程序目录留下 `data\`，卸载时会先迁移到
`%APPDATA%\com.future.pomodoro\legacy-install-data`，再删除程序目录。

需要彻底删除用户数据时显式加参数：

```powershell
powershell -ExecutionPolicy Bypass -File .\uninstall.ps1 -RemoveData
```

该参数会删除 `%APPDATA%\com.future.pomodoro` 与 `%LOCALAPPDATA%\PomodoroAppData`，不可撤销。

## 开发

源码保留在 `src\` 和 `src-tauri\src\`。从全新环境构建前，需要准备 Rustup、Visual Studio C++ Build Tools 和 WebView2：

```powershell
npm ci
npm run build
npm run tauri build
```

生产构建必须使用 `npm run tauri build`，不要用单独的 `cargo build --release` 代替正式部署产物。

## 质量检查

一次执行前端构建、Rust 格式、单元测试与 Clippy：

```powershell
npm run check
```

也可单独执行：

```powershell
npm run build
cargo fmt --manifest-path .\src-tauri\Cargo.toml --all -- --check
cargo test --manifest-path .\src-tauri\Cargo.toml
cargo clippy --manifest-path .\src-tauri\Cargo.toml --all-targets -- -D warnings
```

Windows 下若当前终端找不到 MSVC，请先进入 `x64 Native Tools Command Prompt for VS` 再执行 Rust 命令。
