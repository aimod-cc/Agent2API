<#
.SYNOPSIS
Agent2API 桌面端本地构建脚本（增量）。

.DESCRIPTION
构建顺序与产物归属：
  1. i18n 词典      desktop-tauri/i18n      → ui/i18n（运行时词典，随 ui/ 打包）
  2. 页面岛         desktop-tauri/ui-islands → ui/islands/ui.js + ui.css + login.js
  3. 手机端 bundle  desktop-tauri/mobile     → ui/m/mobile.js + mobile.css
  4. Rust 程序      desktop-tauri/src-tauri  → 桌面壳与 headless 网关两个可执行文件

增量口径：
  · 第 1、2、3 步对比源码与产物的最新修改时间，产物更新就整步跳过；
  · 第 4 步对比 ui/ 的内容指纹（持久化在 target/ui-stamp.txt）：generate_context!
    在编译期把 ui/ 嵌进桌面壳二进制，指纹变化（前端产物有实质变化）时删除
    桌面壳的产物与 fingerprint 强制重编重嵌，其余依赖全部走缓存；
  · 所有缓存放仓库本地：Rust 用 target/（见 src-tauri/.cargo/config.toml），
    Node 依赖用各包自带的 node_modules/，脚本不碰用户全局目录。
  · Rust 构建前自动停掉「从 target 目录启动」的运行中实例 —— 运行中的程序
    锁住产物文件会让链接失败；只认 target 目录下的进程，不碰其它安装。

词典型增量说明：build.mjs 只编译 i18n-src 里已有的键。新增 t() 文案后
需要先跑一次键扫描把新键登记进 i18n-src（-ScanI18n），否则新文案在
非简体语言回落中文原文。

.EXAMPLE
powershell scripts/build-desktop.ps1                  # 增量构建可执行文件
powershell scripts/build-desktop.ps1 -DebugBuild      # 调试档，编译更快，便于本机测试
powershell scripts/build-desktop.ps1 -Test            # 构建后跑 cargo test
powershell scripts/build-desktop.ps1 -FrontendOnly    # 只重建前端产物
powershell scripts/build-desktop.ps1 -ScanI18n        # 新增界面文案后先扫描词典键
powershell scripts/build-desktop.ps1 -Force           # 忽略时间戳全部重建
powershell scripts/build-desktop.ps1 -Bundle          # 追加 NSIS 安装包（需要 cargo tauri）
#>
[CmdletBinding()]
param(
  # 用调试档编译 Rust（编译快、带调试信息，适合本机测试）
  [switch]$DebugBuild,
  # 只构建前端产物，不编译 Rust
  [switch]$FrontendOnly,
  # 跳过前端，只编译 Rust（前端产物已就绪时用）
  [switch]$SkipFrontend,
  # 忽略产物时间戳，前端全部重建
  [switch]$Force,
  # 构建完成后跑 cargo test（工作区全部测试）
  [switch]$Test,
  # 构建词典前先跑 scan.mjs，把源码里新增的 t() 键登记进 i18n-src
  [switch]$ScanI18n,
  # 追加 NSIS 安装包（需要 cargo tauri 子命令，本地未装时脚本会给出提示）
  [switch]$Bundle,
  # 首次拉取 crates.io 依赖时如需走本地代理，传 http://127.0.0.1:7890
  [string]$Proxy = '',
  # mingw64 工具链目录（GNU 工具链链接时需要 dlltool 等工具）
  [string]$MingwBin = 'D:\msys64\mingw64\bin'
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$desktop = Join-Path $repo 'desktop-tauri'
$ui = Join-Path $desktop 'ui'
$profileName = if ($DebugBuild) { 'debug' } else { 'release' }
$targetDir = Join-Path $repo 'target'
$desktopExe = Join-Path $targetDir "$profileName\agent2api.exe"
$serverExe = Join-Path $targetDir "$profileName\agent2api-server.exe"
# 上次构建嵌入桌面壳时的 ui/ 指纹（持久化在本地构建目录里）：
# 与当前指纹不一致说明打包树有实质变化，需要清桌面壳强制重嵌
$stampFile = Join-Path $targetDir 'ui-stamp.txt'

if ($Proxy) {
  $env:HTTPS_PROXY = $Proxy
  $env:HTTP_PROXY = $Proxy
}
if ($MingwBin -and (Test-Path $MingwBin)) {
  $env:PATH = "$MingwBin;$env:PATH"
}

if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
  throw 'node not found in PATH; install Node.js 18+ first'
}
if (-not $FrontendOnly -and -not (Get-Command cargo -ErrorAction SilentlyContinue)) {
  throw 'cargo not found in PATH; install the Rust toolchain first'
}

# 构建前停掉从 target 目录跑起来的旧实例：运行中的程序锁住产物文件，链接会失败
if (-not $FrontendOnly) {
  Get-Process -ErrorAction SilentlyContinue | Where-Object {
    $_.Path -and $_.Path.StartsWith($targetDir, [StringComparison]::OrdinalIgnoreCase)
  } | ForEach-Object {
    Write-Host "==> stopping running instance ($($_.ProcessName), pid $($_.Id))"
    Stop-Process -Id $_.Id -Force
  }
}

function Invoke-Checked([string]$Step, [scriptblock]$Block) {
  Write-Host "==> $Step"
  & $Block
  if ($LASTEXITCODE -ne 0) {
    throw "$Step failed with exit code $LASTEXITCODE"
  }
}

# 一组根目录下（跳过 node_modules）最新的文件修改时间
function Get-NewestWrite([string[]]$Roots) {
  $newest = [datetime]::MinValue
  foreach ($root in $Roots) {
    if (-not (Test-Path -LiteralPath $root)) { continue }
    $files = Get-ChildItem -LiteralPath $root -Recurse -File -ErrorAction SilentlyContinue |
      Where-Object { $_.FullName -notmatch '\\node_modules\\' }
    foreach ($file in $files) {
      if ($file.LastWriteTimeUtc -gt $newest) { $newest = $file.LastWriteTimeUtc }
    }
  }
  $newest
}

# 任一产物缺失，或源码比产物新，都算需要重建
function Test-Stale([string[]]$Outputs, [string[]]$Roots) {
  foreach ($output in $Outputs) {
    if (-not (Test-Path -LiteralPath $output)) { return $true }
  }
  (Get-NewestWrite $Roots) -gt (Get-NewestWrite $Outputs)
}

function Assert-NodePackage([string]$Dir) {
  if (-not (Test-Path (Join-Path $Dir 'node_modules'))) {
    throw "node_modules missing under $Dir; run 'npm ci' there first"
  }
}

# ui/ 打包树的内容指纹（相对路径 + 每个文件的内容哈希）：
# 词典这类「重写但内容不变」的产物不会改变指纹，避免无谓的重嵌重链
function Get-UiStamp {
  $sha = [System.Security.Cryptography.SHA256]::Create()
  try {
    $parts = [System.Collections.Generic.List[string]]::new()
    $files = Get-ChildItem -LiteralPath $ui -Recurse -File -ErrorAction SilentlyContinue | Sort-Object FullName
    foreach ($file in $files) {
      $hash = $sha.ComputeHash([IO.File]::ReadAllBytes($file.FullName))
      $parts.Add($file.FullName.Substring($ui.Length) + ':' + [BitConverter]::ToString($hash).Replace('-', ''))
    }
    [string]::Join("`n", $parts)
  } finally { $sha.Dispose() }
}

$uiStampCurrent = Get-UiStamp
$uiStampLast = ''
if (Test-Path -LiteralPath $stampFile) {
  $uiStampLast = [IO.File]::ReadAllText($stampFile).Trim()
}

if (-not $SkipFrontend) {
  # ── 1. i18n 词典 ──
  $i18nDir = Join-Path $desktop 'i18n'
  Assert-NodePackage $i18nDir
  $i18nOut = @((Join-Path $ui 'i18n\manifest.js'))
  $i18nSrc = @(
    (Join-Path $i18nDir 'i18n-src'),
    (Join-Path $i18nDir 'zh-hant-glossary.json'),
    (Join-Path $i18nDir 'scan.mjs'),
    (Join-Path $i18nDir 'build.mjs')
  )
  if ($Force -or $ScanI18n -or (Test-Stale $i18nOut $i18nSrc)) {
    if ($ScanI18n -or $Force) {
      Invoke-Checked 'i18n scan' { Push-Location $i18nDir; try { node scan.mjs } finally { Pop-Location } }
    }
    Invoke-Checked 'i18n build' { Push-Location $i18nDir; try { node build.mjs } finally { Pop-Location } }
  } else {
    Write-Host '==> i18n up to date, skipped'
  }

  # ── 2. 页面岛（含 ui-shared / ui-kit 源码）──
  $islandsDir = Join-Path $desktop 'ui-islands'
  Assert-NodePackage $islandsDir
  $islandsOut = @(
    (Join-Path $ui 'islands\ui.js'),
    (Join-Path $ui 'islands\ui.css'),
    (Join-Path $ui 'islands\login.js')
  )
  $islandsSrc = @(
    (Join-Path $islandsDir 'src'),
    (Join-Path $desktop 'ui-shared'),
    (Join-Path $desktop 'ui-kit\src'),
    (Join-Path $desktop 'ui-kit\styles')
  )
  if ($Force -or (Test-Stale $islandsOut $islandsSrc)) {
    Invoke-Checked 'islands typecheck' { Push-Location $islandsDir; try { node node_modules\typescript\bin\tsc --noEmit } finally { Pop-Location } }
    Invoke-Checked 'islands vite build' { Push-Location $islandsDir; try { node node_modules\vite\bin\vite.js build } finally { Pop-Location } }
    Invoke-Checked 'islands login build' { Push-Location $islandsDir; try { node node_modules\vite\bin\vite.js build --config vite.login.config.ts } finally { Pop-Location } }
  } else {
    Write-Host '==> islands up to date, skipped'
  }

  # ── 3. 手机端 bundle ──
  $mobileDir = Join-Path $desktop 'mobile'
  Assert-NodePackage $mobileDir
  $mobileOut = @(
    (Join-Path $ui 'm\mobile.js'),
    (Join-Path $ui 'm\mobile.css')
  )
  $mobileSrc = @(
    (Join-Path $mobileDir 'src'),
    (Join-Path $desktop 'ui-shared'),
    (Join-Path $desktop 'ui-kit\src'),
    (Join-Path $desktop 'ui-kit\styles')
  )
  if ($Force -or (Test-Stale $mobileOut $mobileSrc)) {
    Invoke-Checked 'mobile typecheck' { Push-Location $mobileDir; try { node node_modules\typescript\bin\tsc --noEmit } finally { Pop-Location } }
    Invoke-Checked 'mobile vite build' { Push-Location $mobileDir; try { node node_modules\vite\bin\vite.js build } finally { Pop-Location } }
  } else {
    Write-Host '==> mobile up to date, skipped'
  }
}

if (-not $FrontendOnly) {
  # ── 4. Rust（桌面壳 + headless 网关）──
  # ui/ 在编译期被 generate_context! 嵌进桌面壳二进制：当前指纹与上次构建
  # 记录的指纹不同（前端产物有实质变化）时，删掉桌面 lib 的产物与 fingerprint
  # 强制重编重嵌（ui 嵌在 lib 编译单元里，bin 随依赖变化自动重链），
  # server 库与其余依赖继续走缓存。
  # 不用 `cargo clean -p`：它按「当次选择的单元」清理，会漏掉整包构建留下的产物。
  if ((Test-Path $desktopExe) -and $uiStampCurrent -ne $uiStampLast) {
    Write-Host '==> ui changed, removing desktop crate artifacts for asset re-embed'
    Remove-Item -LiteralPath $desktopExe -Force -ErrorAction SilentlyContinue
    Remove-Item -Path (Join-Path $targetDir "$profileName\deps\workbuddy_proxy_desktop_lib*") -Force -ErrorAction SilentlyContinue
    Remove-Item -Path (Join-Path $targetDir "$profileName\.fingerprint\workbuddy_proxy_desktop_lib-*") -Recurse -Force -ErrorAction SilentlyContinue
  }
  Invoke-Checked "cargo build ($profileName)" {
    Push-Location (Join-Path $desktop 'src-tauri')
    try {
      # 桌面 lib 的 crate-type 已收敛为 rlib（见 Cargo.toml 的说明），
      # 整包构建一次产出桌面壳与 headless 网关，且不触发 cdylib 链接
      if ($DebugBuild) { cargo build } else { cargo build --release }
    } finally { Pop-Location }
  }

  if ($Test) {
    Invoke-Checked 'cargo test' {
      Push-Location (Join-Path $desktop 'src-tauri')
      try {
        # 只跑 server crate：桌面壳的测试二进制在 windows-gnu 下加载阶段
        # 就会因 PATH 里 mingw 运行时 DLL 冲突报 STATUS_ENTRYPOINT_NOT_FOUND
        # （该 crate 没有任何测试，失败与代码无关），--workspace 会被它卡住
        cargo test -p agent2api-server
      } finally { Pop-Location }
    }
  }

  # 构建成功才记录指纹：失败时保留旧值，下次重试仍会走重嵌
  [IO.File]::WriteAllText($stampFile, $uiStampCurrent, [Text.UTF8Encoding]::new($false))

  if ($Bundle) {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw 'cargo not found' }
    $hasTauriCli = $false
    cargo tauri --version 2>$null | Out-Null
    if ($LASTEXITCODE -eq 0) { $hasTauriCli = $true }
    if (-not $hasTauriCli) {
      throw 'cargo tauri CLI not installed; run: cargo install tauri-cli --locked'
    }
    Invoke-Checked 'tauri bundle (nsis)' {
      Push-Location (Join-Path $desktop 'src-tauri')
      try { cargo tauri build } finally { Pop-Location }
    }
  }

  Write-Host ''
  Write-Host "Desktop app : $desktopExe"
  Write-Host "Headless gw : $serverExe"
} else {
  Write-Host ''
  Write-Host "Frontend artifacts under: $ui"
}
