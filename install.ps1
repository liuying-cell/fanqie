#Requires -Version 5.1
<#
.SYNOPSIS
  「一轮番茄钟」便携部署脚本（T001 · ASCII 部署基线）。
.DESCRIPTION
  不需管理员权限。把最终 release exe 部署到纯 ASCII 路径
  %LOCALAPPDATA%\PomodoroApp，创建中文开始菜单快捷方式，并设置与通知身份一致的
  AppUserModelID (com.future.pomodoro)，使 Windows 通知能归属到本应用。
  只写应用目录、开始菜单快捷方式（与可选桌面快捷方式），不删除任何用户文件。
.PARAMETER AlsoDesktop
  额外创建桌面快捷方式（默认不创建）。
.PARAMETER NoLaunch
  只部署不启动（默认会启动）。
.PARAMETER 环境变量 POMODORO_INSTALL_DIR / POMODORO_START_MENU_DIR / POMODORO_DESKTOP_DIR
  仅用于隔离测试：把安装目录与快捷方式目录指向临时路径；未提供时使用默认位置。
.EXAMPLE
  powershell -ExecutionPolicy Bypass -File install.ps1 -NoLaunch
#>
param(
    [switch]$AlsoDesktop,
    [switch]$NoLaunch
)

$ErrorActionPreference = 'Stop'

# ---- 固定常量 ----
$AppId       = 'com.future.pomodoro'           # 与 tauri.conf.json identifier / 通知 AUMID 一致
$DeployName  = 'PomodoroApp'                   # 纯 ASCII 目录名
$ExeName     = 'pomodoro-app.exe'
$LinkName    = '一轮番茄钟'                     # 中文显示名
$RepoRoot    = Split-Path -Parent $PSScriptRoot # pomodoro-app 的上级（合作目录）
$SourceExe   = Join-Path $PSScriptRoot 'release\一轮番茄钟.exe'

# ---- 解析路径：优先用脚本自身定位源 exe，兼容从任意 cwd 调用 ----
if (-not (Test-Path -LiteralPath $SourceExe)) {
    # 清理构建缓存后，正式交付使用 release 目录；开发构建时可回退到 target。
    $SourceExe = Join-Path $PSScriptRoot 'src-tauri\target\release\一轮番茄钟.exe'
}
if (-not (Test-Path -LiteralPath $SourceExe)) {
    throw "找不到 release exe：$SourceExe 。请先运行 npm run tauri build。"
}

function Resolve-InstallPath {
    param([string]$Name, [string]$DefaultPath)
    $override = [Environment]::GetEnvironmentVariable($Name)
    if ([string]::IsNullOrWhiteSpace($override)) { return $DefaultPath }
    return [IO.Path]::GetFullPath($override)
}

$DeployDir   = Resolve-InstallPath 'POMODORO_INSTALL_DIR' (Join-Path $env:LOCALAPPDATA $DeployName)
$DeployExe   = Join-Path $DeployDir $ExeName
$StartMenu   = Resolve-InstallPath 'POMODORO_START_MENU_DIR' (Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs')
$LinkDir     = Join-Path $StartMenu '一轮番茄钟'
$LinkPath    = Join-Path $LinkDir "$LinkName.lnk"

Write-Host "=== 一轮番茄钟 · 便携部署 ===" -ForegroundColor Cyan
Write-Host "源 exe   : $SourceExe"
Write-Host "部署目录 : $DeployDir"

# ---- 运行中保护：目标 exe 正在运行则明确失败，绝不强制结束进程 ----
$running = @(Get-Process -Name 'pomodoro-app' -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and ([IO.Path]::GetFullPath($_.Path) -ieq [IO.Path]::GetFullPath($DeployExe)) })
if ($running.Count -gt 0) {
    throw "目标程序正在运行，请先手动关闭后再安装：$DeployExe"
}

# ---- 1. 创建 ASCII 部署目录（不删除已有文件）----
if (-not (Test-Path -LiteralPath $DeployDir)) {
    New-Item -ItemType Directory -Path $DeployDir -Force | Out-Null
    Write-Host "[1/5] 已创建部署目录" -ForegroundColor Green
} else {
    Write-Host "[1/5] 部署目录已存在（覆盖 exe，不动其他文件）" -ForegroundColor Green
}

# ---- 2. 临时副本校验后替换；旧 exe 留作 pomodoro-app.previous.exe ----
$tmpExe   = Join-Path $DeployDir 'pomodoro-app.new.exe'
$prevExe  = Join-Path $DeployDir 'pomodoro-app.previous.exe'
$srcHash  = (Get-FileHash -LiteralPath $SourceExe -Algorithm SHA256).Hash
Copy-Item -LiteralPath $SourceExe -Destination $tmpExe -Force
$tmpHash  = (Get-FileHash -LiteralPath $tmpExe -Algorithm SHA256).Hash
if ($tmpHash -ine $srcHash) {
    Remove-Item -LiteralPath $tmpExe -Force
    throw "临时副本校验失败：源 $srcHash / 副本 $tmpHash；未替换已安装 exe。"
}
if (Test-Path -LiteralPath $DeployExe) {
    if (Test-Path -LiteralPath $prevExe) { Remove-Item -LiteralPath $prevExe -Force }
    Move-Item -LiteralPath $DeployExe -Destination $prevExe -Force
}
Move-Item -LiteralPath $tmpExe -Destination $DeployExe -Force
$hash = (Get-FileHash -LiteralPath $DeployExe -Algorithm SHA256).Hash
$size = (Get-Item -LiteralPath $DeployExe).Length
Write-Host "[2/5] 已校验并替换 exe  大小=$size B  SHA256=$hash  上一版=$prevExe" -ForegroundColor Green

# ---- 3. 创建开始菜单快捷方式，并设置 AppUserModelID ----
if (-not (Test-Path -LiteralPath $LinkDir)) {
    New-Item -ItemType Directory -Path $LinkDir -Force | Out-Null
}

# 设置 AUMID 需要的 COM 接口（IShellLink + IPropertyStore）
$sig = @'
using System;
using System.Runtime.InteropServices;
public static class ShortcutAumid {
    [ComImport, Guid("00021401-0000-0000-C000-000000000046")]
    private class ShellLink {}
    [ComImport, InterfaceType(ComInterfaceType.InterfaceIsIUnknown), Guid("000214F9-0000-0000-C000-000000000046")]
    private interface IShellLinkW {
        void GetPath([Out, MarshalAs(UnmanagedType.LPWStr)] System.Text.StringBuilder pszFile, int cchMaxPath, out IntPtr pfd, int fFlags);
        void GetIDList(out IntPtr ppidl); void SetIDList(IntPtr pidl);
        void GetDescription([Out, MarshalAs(UnmanagedType.LPWStr)] System.Text.StringBuilder pszName, int cchMaxName);
        void SetDescription([MarshalAs(UnmanagedType.LPWStr)] string pszName);
        void GetWorkingDirectory([Out, MarshalAs(UnmanagedType.LPWStr)] System.Text.StringBuilder pszDir, int cchMaxPath);
        void SetWorkingDirectory([MarshalAs(UnmanagedType.LPWStr)] string pszDir);
        void GetArguments([Out, MarshalAs(UnmanagedType.LPWStr)] System.Text.StringBuilder pszArgs, int cchMaxPath);
        void SetArguments([MarshalAs(UnmanagedType.LPWStr)] string pszArgs);
        void GetHotkey(out short pwHotkey); void SetHotkey(short wHotkey);
        void GetShowCmd(out int piShowCmd); void SetShowCmd(int iShowCmd);
        void GetIconLocation([Out, MarshalAs(UnmanagedType.LPWStr)] System.Text.StringBuilder pszIconPath, int cchIconPath, out int piIcon);
        void SetIconLocation([MarshalAs(UnmanagedType.LPWStr)] string pszIconPath, int iIcon);
        void SetRelativePath([MarshalAs(UnmanagedType.LPWStr)] string pszPathRel, int dwReserved);
        void Resolve(IntPtr hwnd, int fFlags);
        void SetPath([MarshalAs(UnmanagedType.LPWStr)] string pszFile);
    }
    [ComImport, InterfaceType(ComInterfaceType.InterfaceIsIUnknown), Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99")]
    private interface IPropertyStore {
        void GetCount(out uint cProps);
        void GetAt(uint iProp, out PROPERTYKEY pkey);
        void GetValue(ref PROPERTYKEY key, out PROPVARIANT pv);
        void SetValue(ref PROPERTYKEY key, ref PROPVARIANT pv);
        void Commit();
    }
    [StructLayout(LayoutKind.Sequential)] private struct PROPERTYKEY { public Guid fmtid; public uint pid; }
    [StructLayout(LayoutKind.Sequential)] private struct PROPVARIANT {
        public ushort vt; public ushort wReserved1; public ushort wReserved2; public ushort wReserved3;
        public IntPtr pwszVal;
    }
    [ComImport, Guid("0000010B-0000-0000-C000-000000000046"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IPersistFile {
        void GetClassID(out Guid pClassID);
        void IsDirty();
        void Load([MarshalAs(UnmanagedType.LPWStr)] string pszFileName, uint dwMode);
        void Save([MarshalAs(UnmanagedType.LPWStr)] string pszFileName, [MarshalAs(UnmanagedType.Bool)] bool fRemember);
        void SaveCompleted([MarshalAs(UnmanagedType.LPWStr)] string pszFileName);
        void GetCurFile([Out, MarshalAs(UnmanagedType.LPWStr)] System.Text.StringBuilder pszFileName);
    }
    // 正确值来自 Windows SDK propkey.h: PKEY_AppUserModel_ID
    // DEFINE_PROPERTYKEY(PKEY_AppUserModel_ID, 0x9F4C2855,0x9F79,0x4B39,
    //   0xA8,0xD0,0xE1,0xD4,0x2D,0xE1,0xD5,0xF3, 5)
    private static readonly Guid PKEY_AppUserModel_ID = new Guid("9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3");
    private static PROPVARIANT MakeVariant(string s) {
        var v = new PROPVARIANT();
        v.vt = 31; // VT_LPWSTR
        v.pwszVal = Marshal.StringToCoTaskMemUni(s);
        return v;
    }
    public static void SetAumid(string lnkPath, string exePath, string aumid) {
        object link = new ShellLink();
        var sl = (IShellLinkW)link;
        sl.SetPath(exePath);
        sl.SetDescription("一轮 · 番茄钟");
        sl.SetIconLocation(exePath, 0);
        var ps = (IPropertyStore)link;
        var key = new PROPERTYKEY { fmtid = PKEY_AppUserModel_ID, pid = 5 };
        var val = MakeVariant(aumid);
        ps.SetValue(ref key, ref val);
        ps.Commit();
        ((IPersistFile)link).Save(lnkPath, true);
        Marshal.FreeCoTaskMem(val.pwszVal);
    }
}
'@
$aumidOk = $false
$actualAumid = ''
try {
    Add-Type -TypeDefinition $sig -ErrorAction Stop
    [ShortcutAumid]::SetAumid($LinkPath, $DeployExe, $AppId)
} catch {
    # 兜底：创建普通快捷方式（不带 AUMID）。下面统一读回校验，失败不会被误报成功。
    $ws = New-Object -ComObject WScript.Shell
    $sc = $ws.CreateShortcut($LinkPath)
    $sc.TargetPath = $DeployExe
    $sc.WorkingDirectory = $DeployDir
    $sc.IconLocation = "$DeployExe,0"
    $sc.Description = '一轮 · 番茄钟'
    $sc.Save()
    Write-Host "[3/5] AUMID 写入异常，已回退普通快捷方式：$($_.Exception.Message)" -ForegroundColor Yellow
}

# ---- 按 lead 要求：用 Shell Property System 读回校验，不通过即报错 ----
try {
    $shellApp = New-Object -ComObject Shell.Application
    $folderObj = $shellApp.Namespace((Split-Path -Parent $LinkPath))
    $linkItem = $folderObj.ParseName((Split-Path -Leaf $LinkPath))
    $actualAumid = [string]$linkItem.ExtendedProperty('System.AppUserModel.ID')
    $aumidOk = ($actualAumid -eq $AppId)
} catch {
    Write-Host "[3/5] 读回 AUMID 异常：$($_.Exception.Message)" -ForegroundColor Red
}

# 证据文件：T001-aumid-verified.txt（不覆盖上一轮 18-aumid-verified.txt）
$artifactsDir = Join-Path $PSScriptRoot 'artifacts'
if (-not (Test-Path -LiteralPath $artifactsDir)) { New-Item -ItemType Directory -Path $artifactsDir -Force | Out-Null }
@(
    'T001 round4 - shortcut AUMID verification',
    "shortcut_path: $LinkPath",
    "expected_aumid: $AppId",
    "actual_aumid: $actualAumid",
    "match: $aumidOk",
    "pkey_guid: 9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3 (pid=5)",
    "target: $DeployExe"
) | Out-File -FilePath (Join-Path $artifactsDir 'T001-aumid-verified.txt') -Encoding utf8

if (-not $aumidOk) {
    throw "AUMID write failed: actual='$actualAumid' expected='$AppId'"
}
Write-Host "[3/5] 已创建开始菜单快捷方式，AUMID 读回校验通过 = $actualAumid" -ForegroundColor Green

# 可选桌面快捷方式
if ($AlsoDesktop) {
    $deskDir = Resolve-InstallPath 'POMODORO_DESKTOP_DIR' ([Environment]::GetFolderPath('Desktop'))
    $desk = Join-Path $deskDir "$LinkName.lnk"
    $ws2 = New-Object -ComObject WScript.Shell
    $sc2 = $ws2.CreateShortcut($desk)
    $sc2.TargetPath = $DeployExe
    $sc2.IconLocation = "$DeployExe,0"
    $sc2.Save()
    Write-Host "      已创建桌面快捷方式" -ForegroundColor Green
}

# ---- 4. 启动 ASCII 路径 exe ----
$pidStarted = 0
if (-not $NoLaunch) {
    $proc = Start-Process -FilePath $DeployExe -PassThru
    Start-Sleep -Seconds 3
    $pidStarted = $proc.Id
    Write-Host "[4/5] 已启动部署 exe  PID=$pidStarted" -ForegroundColor Green
} else {
    Write-Host "[4/5] 按 -NoLaunch 跳过启动" -ForegroundColor Yellow
}

# ---- 5. 输出汇总（供证据脚本读取）----
Write-Host "[5/5] 部署汇总" -ForegroundColor Green
Write-Host "INSTALL_PATH=$DeployExe"
Write-Host "PREVIOUS_EXE=$prevExe"
Write-Host "INSTALL_SIZE=$size"
Write-Host "INSTALL_SHA256=$hash"
Write-Host "LAUNCH_PID=$pidStarted"
Write-Host "SHORTCUT=$LinkPath"
