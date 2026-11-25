# Windows 代码签名与发布

本项目使用 [SignPath Foundation](https://signpath.org/) 为公开开源版本提供 Windows Authenticode 代码签名。签名只在 GitHub Actions 中完成，私钥不会保存到仓库或本地开发机。

`v0.1.0` 及更早的 Release 产物未签名，无法事后补签。首个完成签名的版本应使用新的版本号发布，例如 `v0.1.1`。

## 一次性申请

1. 在 SignPath Foundation 的 [Apply](https://signpath.org/apply) 页面提交申请。
2. 项目地址填写 `https://github.com/liuying-cell/fanqie`。
3. 说明项目使用 MIT License，发布物是 Windows x64 的 Tauri 安装程序与便携版 EXE。
4. 审核通过后，安装 SignPath GitHub App，并允许它访问本仓库。
5. 在 SignPath 中创建项目，并连接预定义的 `GitHub.com` Trusted Build System。
6. 创建签名策略，建议 slug 使用 `release-signing`。
7. 上传本仓库的 `.signpath/artifact-configuration.xml`，建议 slug 使用 `windows-release`。

签名工作流会在临时目录中把发布文件转换为两个稳定的 ASCII 文件名后提交 SignPath：

- `PomodoroApp_release_x64-setup.exe`
- `pomodoro-app.exe`

签名完成并验证后，再发布为用户可见的中文文件名：

- `一轮番茄钟_<版本>_x64-setup.exe`
- `一轮番茄钟.exe`
- `一轮番茄钟_<版本>_windows-x64-portable.zip`

SignPath Foundation 的开源签名要求参与签名请求的 GitHub Actions 任务全部运行在 GitHub 托管运行器上。本仓库的发布工作流固定使用 `windows-latest`。

## 申请资料参考

可直接按下面的信息填写申请表：

```text
Project name: PomodoroApp / 一轮番茄钟
Repository: https://github.com/liuying-cell/fanqie
License: MIT
Platform: Windows 10/11 x64
Framework: Tauri 2, Rust, TypeScript
Description: An offline desktop Pomodoro timer with configurable focus/break
sessions, a floating timer ball, tray controls, notifications and local-only data.
Build: GitHub Actions on windows-latest, npm ci, npm run check,
npm run tauri -- build --bundles nsis --ci
Artifacts: NSIS installer and portable EXE
Why signing is needed: Prevent Windows SmartScreen from showing an unknown publisher
for public GitHub Releases and establish a verifiable link between the repository
and released binaries.
```

## GitHub 配置

在仓库的 **Settings → Secrets and variables → Actions** 中添加：

Secret：

- `SIGNPATH_API_TOKEN`：在 SignPath 中创建的 API Token，需要项目提交者权限。

Repository variables：

- `SIGNPATH_ORGANIZATION_ID`：SignPath 组织 ID。
- `SIGNPATH_PROJECT_SLUG`：SignPath 项目 slug。
- `SIGNPATH_SIGNING_POLICY_SLUG`：`release-signing`，或你在 SignPath 中实际使用的 slug。
- `SIGNPATH_ARTIFACT_CONFIGURATION_SLUG`：`windows-release`，或你在 SignPath 中实际使用的 slug。

不要提交 API Token、签名私钥或任何签名凭据到仓库。

## 测试签名流程

1. 打开 **Actions → Sign and release Windows build**。
2. 选择 **Run workflow**。
3. 保持 `publish` 为 `false`。
4. 工作流完成构建、提交 SignPath、下载签名产物并验证证书。
5. 从本次运行下载名称以 `一轮番茄钟-<版本>-signed-` 开头的工作流产物。
6. 检查两个 EXE 的签名状态均为 `Valid`，并且存在时间戳。

本地复核命令：

```powershell
Get-AuthenticodeSignature .\一轮番茄钟_0.1.1_x64-setup.exe |
  Format-List Status, StatusMessage, SignerCertificate, TimeStamperCertificate

Get-AuthenticodeSignature .\一轮番茄钟.exe |
  Format-List Status, StatusMessage, SignerCertificate, TimeStamperCertificate
```

## 正式发布

版本号必须同时存在于以下文件：

- `package.json`
- `src-tauri/tauri.conf.json`

发布工作流会校验两者一致，并使用 `v<版本号>` 作为 Release Tag。

推荐流程：

1. 将版本号提升到 `0.1.1`。
2. 提交并推送 `main`。
3. 创建并推送 Tag：`v0.1.1`。
4. 工作流自动构建、签名、验证、生成 `SHA256SUMS.txt` 并创建 GitHub Release。
5. 签名或验证失败时，工作流会在发布前终止，不会上传未签名的发布资产。

也可以在手动运行工作流时把 `publish` 设为 `true`，由工作流创建 Tag 对应的 GitHub Release。

## 常见失败

- `Missing SignPath configuration`：检查上面的 Secret 和 Repository variables。
- Artifact Configuration 不匹配：确认上传 ZIP 中只有安装包和便携版两个 EXE，文件名符合 `.signpath/artifact-configuration.xml`。
- SignPath 请求一直等待：查看 Action 输出中的 signing request URL，并在 SignPath 控制台确认是否需要人工审批。
- `Status` 不是 `Valid`：不要发布该产物，检查 SignPath 的签名策略、时间戳和产物是否在签名后又被改动。

签名只证明发布者和文件完整性，不代表 SmartScreen 下载信誉会立即对所有版本生效。公开签名版本、持续发布并积累下载量后，浏览器和 SmartScreen 的提示会逐步减少。