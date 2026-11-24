#Requires -Version 5.1
<#
.SYNOPSIS
  「一轮番茄钟」卸载脚本：默认只删程序目录与快捷方式，保留所有用户数据。
.DESCRIPTION
  - 旧 `exe\data` 若存在，先复制到 `%APPDATA%\com.future.pomodoro\legacy-install-data` 再删除程序目录。
  - 仅带 -RemoveData 时删除 `%APPDATA%\com.future.pomodoro` 与 `%LOCALAPPDATA%\PomodoroAppData`。
  - 所有递归删除前均校验解析后的绝对路径与固定目标完全一致，并拒绝链接/挂载点。
.PARAMETER RemoveData
  同时使用彻底删除用户数据（默认不启用）。
.PARAMETER 环境变量 POMODORO_INSTALL_DIR / POMODORO_START_MENU_DIR / POMODORO_DESKTOP_DIR /
  POMODORO_DATA_DIR / POMODORO_LOCAL_DATA_DIR
  隔离测试用：把各目标指向临时目录；未提供时使用默认位置。
.EXAMPLE
  powershell -ExecutionPolicy Bypass -File .\uninstall.ps1
#>
param([switch]$RemoveData)

$ErrorActionPreference = 'Stop'
$LinkName = '一轮番茄钟'

function Resolve-TargetPath {
    param([string]$Name, [string]$DefaultPath)
    $override = [Environment]::GetEnvironmentVariable($Name)
    $raw = if ([string]::IsNullOrWhiteSpace($override)) { $DefaultPath } else { $override }
    $full = [IO.Path]::GetFullPath($raw).TrimEnd([char]'\')
    $root = [IO.Path]::GetPathRoot($full).TrimEnd([char]'\')
    if ([string]::IsNullOrWhiteSpace($root) -or $full -ieq $root) {
        throw "拒绝操作：目标路径是卷根或无效路径 '$full'"
    }
    return $full
}

function Assert-SafeTarget {
    param([string]$Path, [string]$Expected, [string]$Label)
    $full = [IO.Path]::GetFullPath($Path).TrimEnd([char]'\')
    $expected = [IO.Path]::GetFullPath($Expected).TrimEnd([char]'\')
    if ($full -ine $expected) {
        throw "拒绝删除：$Label 解析结果 '$full' 与固定目标 '$expected' 不一致"
    }
    if (Test-Path -LiteralPath $full) {
        $item = Get-Item -LiteralPath $full -Force
        if ($item.LinkType) {
            throw "拒绝删除：$Label 是链接/挂载点 $full -> $($item.Target)"
        }
        $resolved = (Resolve-Path -LiteralPath $full).ProviderPath.TrimEnd([char]'\')
        if ($resolved -ine $expected) {
            throw "拒绝删除：$Label 解析后的真实路径 '$resolved' 与固定目标 '$expected' 不一致"
        }
    }
}

$installDir  = Resolve-TargetPath 'POMODORO_INSTALL_DIR' (Join-Path $env:LOCALAPPDATA 'PomodoroApp')
$linkRoot    = Resolve-TargetPath 'POMODORO_START_MENU_DIR' (Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs')
$linkDir     = Join-Path $linkRoot $LinkName
$desktopDir  = Resolve-TargetPath 'POMODORO_DESKTOP_DIR' ([Environment]::GetFolderPath('Desktop'))
$dataDir     = Resolve-TargetPath 'POMODORO_DATA_DIR' (Join-Path $env:APPDATA 'com.future.pomodoro')
$localData   = Resolve-TargetPath 'POMODORO_LOCAL_DATA_DIR' (Join-Path $env:LOCALAPPDATA 'PomodoroAppData')

Write-Host "=== 一轮番茄钟 · 卸载 ===" -ForegroundColor Cyan
Write-Host "程序目录 : $installDir"
Write-Host "用户数据 : $dataDir"
Write-Host "用户数据 : $localData"

# ---- 1. 旧 exe\data 迁移到用户数据目录（只迁不删用户数据）----
$legacy = Join-Path $installDir 'data'
$legacyMigrated = 'NO'
if (Test-Path -LiteralPath $legacy) {
    Assert-SafeTarget -Path $legacy -Expected $legacy -Label 'legacy data'
    $dest = Join-Path $dataDir 'legacy-install-data'
    if (-not (Test-Path -LiteralPath $dataDir)) {
        New-Item -ItemType Directory -Path $dataDir -Force | Out-Null
    }
    New-Item -ItemType Directory -Path $dest -Force | Out-Null
    foreach ($child in (Get-ChildItem -LiteralPath $legacy -Force)) {
        Copy-Item -LiteralPath $child.FullName -Destination $dest -Recurse -Force
    }
    $sourceCount = @(Get-ChildItem -LiteralPath $legacy -Recurse -File -Force).Count
    $destCount = @(Get-ChildItem -LiteralPath $dest -Recurse -File -Force).Count
    if ($destCount -lt $sourceCount) {
        throw "旧数据迁移校验失败：源 $sourceCount 个文件，目标仅 $destCount 个；程序目录未删除。"
    }
    Remove-Item -LiteralPath $legacy -Recurse -Force
    $legacyMigrated = "YES -> $dest ($destCount files)"
    Write-Host "[1/3] 旧 data 已迁移并清除：$dest" -ForegroundColor Green
} else {
    Write-Host "[1/3] 无旧 data 目录需要迁移" -ForegroundColor Green
}

# ---- 2. 删除程序目录与快捷方式，保留用户数据 ----
Assert-SafeTarget -Path $installDir -Expected $installDir -Label 'install dir'
if (Test-Path -LiteralPath $installDir) {
    Remove-Item -LiteralPath $installDir -Recurse -Force
}
Assert-SafeTarget -Path $linkDir -Expected $linkDir -Label 'start menu shortcut dir'
if (Test-Path -LiteralPath $linkDir) {
    Remove-Item -LiteralPath $linkDir -Recurse -Force
}
$desktopLink = Join-Path $desktopDir "$LinkName.lnk"
if (Test-Path -LiteralPath $desktopLink) {
    Assert-SafeTarget -Path $desktopLink -Expected $desktopLink -Label 'desktop shortcut'
    Remove-Item -LiteralPath $desktopLink -Force
}
Write-Host "[2/3] 已删除程序目录与快捷方式；用户数据保留" -ForegroundColor Green

# ---- 3. 仅在 -RemoveData 时彻底删除数据 ----
$dataRemoved = 'NO'
if ($RemoveData) {
    foreach ($target in @($dataDir, $localData)) {
        Assert-SafeTarget -Path $target -Expected $target -Label 'user data'
        if (Test-Path -LiteralPath $target) {
            Remove-Item -LiteralPath $target -Recurse -Force
            $dataRemoved = "YES ($target)"
        }
    }
    Write-Host "[3/3] 已按 -RemoveData 删除用户数据" -ForegroundColor Yellow
} else {
    Write-Host "[3/3] 未指定 -RemoveData，用户数据未删除" -ForegroundColor Green
}

Write-Host "UNINSTALL_DIR=$installDir"
Write-Host "LEGACY_MIGRATED=$legacyMigrated"
Write-Host "DATA_REMOVED=$dataRemoved"
