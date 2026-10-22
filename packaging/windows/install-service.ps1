<#
.SYNOPSIS
    Agent2API 服务端安装脚本（Windows Server / 无界面形态）。

.DESCRIPTION
    把 agent2api-server.exe 注册成 Windows 服务（由 SCM 托管，开机自启、
    崩溃自动拉起）。脚本假定它与 agent2api-server.exe、ui\ 在同一个目录里。

.PARAMETER InstallDir
    程序安装目录，默认 C:\Agent2API

.PARAMETER DataDir
    数据目录（SQLite 库、配置、日志），默认 C:\ProgramData\Agent2API

.PARAMETER Port
    监听端口，默认 3065

.PARAMETER Uninstall
    卸载服务（默认保留程序、数据与配置）

.PARAMETER Purge
    配合 -Uninstall 使用，连程序目录与数据目录一起删

.EXAMPLE
    .\install-service.ps1
    默认安装在 C:\Agent2API，数据在 C:\ProgramData\Agent2API

.EXAMPLE
    .\install-service.ps1 -InstallDir D:\apps\agent2api -Port 8080

.NOTES
    需要管理员权限（注册服务、写 ProgramData）。
#>
[CmdletBinding()]
param(
    [string]$InstallDir = "C:\Agent2API",
    [string]$DataDir = "C:\ProgramData\Agent2API",
    [int]$Port = 3065,
    [switch]$Uninstall,
    [switch]$Purge
)

$ErrorActionPreference = "Stop"

$ServiceName = "Agent2API"
$DisplayName = "Agent2API 网关"
$Description = "Agent2API 多提供商本地网关：OpenAI 兼容接口、账号管理与请求报表（无界面服务形态）"
$ExeName = "agent2api-server.exe"

# ── 管理员检查 ──────────────────────────────────────────────
# 注册服务与写 ProgramData 都需要提权；不检查的话失败信息会很难懂
# （sc.exe 报 5 = 拒绝访问，用户看不出是权限问题）。
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Error "需要管理员权限。请右键 PowerShell 选「以管理员身份运行」，再执行本脚本。"
    exit 1
}

function Test-ServiceExists {
    param([string]$Name)
    # Get-Service 在服务不存在时抛错，用 -ErrorAction 静默后判空
    $svc = Get-Service -Name $Name -ErrorAction SilentlyContinue
    return $null -ne $svc
}

function Stop-ExistingService {
    if (-not (Test-ServiceExists -Name $ServiceName)) { return }
    $svc = Get-Service -Name $ServiceName
    if ($svc.Status -ne "Stopped") {
        Write-Host "→ 停止现有服务"
        # SCM 的停止是异步的：等它真停稳再往下走，否则后续删服务 / 覆盖
        # exe 会因为文件占用失败
        Stop-Service -Name $ServiceName -Force
        $deadline = (Get-Date).AddSeconds(30)
        while ((Get-Service -Name $ServiceName).Status -ne "Stopped" -and (Get-Date) -lt $deadline) {
            Start-Sleep -Milliseconds 500
        }
    }
}

# ── 卸载 ────────────────────────────────────────────────────
if ($Uninstall) {
    Write-Host "→ 卸载服务 $ServiceName"
    if (Test-ServiceExists -Name $ServiceName) {
        Stop-ExistingService
        # sc.exe delete 而不是 Remove-Service：后者要 PowerShell 6+，
        # Server 2016/2019 自带的 5.1 没有。sc.exe 全版本可用。
        & sc.exe delete $ServiceName | Out-Null
        if ($LASTEXITCODE -ne 0) {
            Write-Warning "sc.exe delete 返回 $LASTEXITCODE（服务可能已被其它进程占用）"
        }
    } else {
        Write-Host "   服务不存在，跳过"
    }

    if ($Purge) {
        Write-Host "→ 删除程序目录 $InstallDir"
        Remove-Item -Recurse -Force $InstallDir -ErrorAction SilentlyContinue
        Write-Host "→ 删除数据目录 $DataDir"
        Remove-Item -Recurse -Force $DataDir -ErrorAction SilentlyContinue
        Write-Host "✅ 已卸载并清理"
    } else {
        Write-Host "→ 程序与数据已保留：$InstallDir / $DataDir"
        Write-Host "   （连它们一起删用 -Uninstall -Purge）"
        Write-Host "✅ 已卸载服务"
    }
    exit 0
}

# ── 安装 ────────────────────────────────────────────────────
$sourceExe = Join-Path $PSScriptRoot $ExeName
if (-not (Test-Path $sourceExe)) {
    Write-Error "找不到 $sourceExe —— 请在解包后的发行目录里运行本脚本"
    exit 1
}
$sourceUi = Join-Path $PSScriptRoot "ui"
if (-not (Test-Path $sourceUi)) {
    Write-Warning "同目录下没有 ui\ 目录 —— 装完后管理面板不可用（纯 API 部署可忽略）"
}

# 正在运行的旧服务要先停，否则 exe 被占用无法覆盖
$hadService = Test-ServiceExists -Name $ServiceName
if ($hadService) { Stop-ExistingService }

Write-Host "→ 安装程序到 $InstallDir"
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
Copy-Item -Force $sourceExe (Join-Path $InstallDir $ExeName)
if (Test-Path $sourceUi) {
    $targetUi = Join-Path $InstallDir "ui"
    Remove-Item -Recurse -Force $targetUi -ErrorAction SilentlyContinue
    Copy-Item -Recurse -Force $sourceUi $targetUi
}

Write-Host "→ 准备数据目录 $DataDir"
New-Item -ItemType Directory -Force -Path $DataDir | Out-Null
$logDir = Join-Path $DataDir "logs"
New-Item -ItemType Directory -Force -Path $logDir | Out-Null

# ── 服务账号 ────────────────────────────────────────────────
# 默认用 LocalSystem：它对本机 data 目录有完全控制权，且不需要额外凭据。
# （专用虚拟账号要授予目录权限、还要考虑网络凭据，对单机部署收益不大。）
$account = "LocalSystem"

# ── 注册服务 ────────────────────────────────────────────────
$exePath = Join-Path $InstallDir $ExeName

# 环境变量随 ImagePath 一起写进注册表：服务进程不继承安装者的环境，
# 只能靠 SCM 从这里读。带空格的路径必须用引号包住整条 exe 路径。
# AGENT2API_PROXY_HOME 是**必须**的：不设的话服务会以 LocalSystem 的
# USERPROFILE（C:\Windows\system32\config\systemprofile）为家目录，
# 数据会写进系统目录里，排查时极难找到。
$binPath = "`"$exePath`" --service"

if ($hadService) {
    Write-Host "→ 更新现有服务的启动参数"
    & sc.exe config $ServiceName binPath= $binPath start= auto | Out-Null
} else {
    Write-Host "→ 注册服务 $ServiceName"
    # binPath= 后面必须有空格：sc.exe 的语法要求「选项=值」，等号两侧的空格
    # 是分隔符的一部分，漏了会报「参数无效」
    & sc.exe create $ServiceName binPath= $binPath start= auto DisplayName= $DisplayName | Out-Null
    if ($LASTEXITCODE -ne 0) {
        Write-Error "sc.exe create 失败（退出码 $LASTEXITCODE）"
        exit 1
    }
}

& sc.exe description $ServiceName $Description | Out-Null

# 崩溃自动重启：3 次失败后间隔递增，之后每天重置计数。
# 与 Linux 侧 Restart=always + StartLimitBurst 的作用对齐 —— Rust 是
# panic=abort，任何未捕获 panic 都会直接带走进程。
& sc.exe failure $ServiceName reset= 86400 actions= restart/5000/restart/10000/restart/30000 | Out-Null

# ── 环境变量写进服务注册表 ──────────────────────────────────
# sc.exe 没有「设环境变量」的选项，只能直接写注册表
# HKLM\SYSTEM\CurrentControlSet\Services\<name>\Environment（REG_MULTI_SZ）。
$regPath = "HKLM:\SYSTEM\CurrentControlSet\Services\$ServiceName"
$envValues = @(
    "AGENT2API_PROXY_HOME=$DataDir",
    "AGENT2API_UI_DIR=$(Join-Path $InstallDir 'ui')",
    "AGENT2API_HOST=0.0.0.0",
    "AGENT2API_PROXY_PORT=$Port",
    "AGENT2API_LOG_FILE=$(Join-Path $DataDir 'logs\server.log')",
    # 裸机不要开容器内自更新：它靠「换文件后退出、由外部拉起」，
    # 而且替换的是 Program Files 下的文件，服务账号通常无权写。
    # 升级请重新运行本脚本（会 stop → 覆盖 → start）。
    "AGENT2API_SELF_UPDATE=0"
)
New-ItemProperty -Path $regPath -Name "Environment" -PropertyType MultiString `
    -Value $envValues -Force | Out-Null

# ── 防火墙 ──────────────────────────────────────────────────
# 不开入站规则的话，别的机器访问不到（本机 curl 能通，容易误判成服务没起）。
$ruleName = "Agent2API 网关 (TCP $Port)"
if (-not (Get-NetFirewallRule -DisplayName $ruleName -ErrorAction SilentlyContinue)) {
    Write-Host "→ 添加入站防火墙规则（TCP $Port）"
    New-NetFirewallRule -DisplayName $ruleName -Direction Inbound -Protocol TCP `
        -LocalPort $Port -Action Allow -Profile Domain,Private | Out-Null
} else {
    Write-Host "→ 防火墙规则已存在，跳过"
}

# ── 启动 ────────────────────────────────────────────────────
Write-Host "→ 启动服务"
Start-Service -Name $ServiceName

# 等它真起来（Start-Service 返回不代表端口已监听）
$deadline = (Get-Date).AddSeconds(30)
$running = $false
while ((Get-Date) -lt $deadline) {
    if ((Get-Service -Name $ServiceName).Status -eq "Running") {
        $running = $true
        break
    }
    Start-Sleep -Milliseconds 500
}

if ($running) {
    $logFile = Join-Path $DataDir "logs\server.log"
    Write-Host ""
    Write-Host "✅ 安装完成" -ForegroundColor Green
    Write-Host ""
    Write-Host "   面板     http://<本机IP>:$Port/"
    Write-Host "   网关     http://<本机IP>:$Port/v1"
    Write-Host ""
    Write-Host "   程序目录 $InstallDir"
    Write-Host "   数据目录 $DataDir"
    Write-Host "   日志文件 $logFile"
    Write-Host ""
    Write-Host "   服务管理："
    Write-Host "     Get-Service $ServiceName"
    Write-Host "     Restart-Service $ServiceName"
    Write-Host "     Stop-Service $ServiceName"
    Write-Host ""
    Write-Host "接下来："
    Write-Host "   1. 浏览器打开面板地址，首次进入会引导注册管理员账号"
    Write-Host "   2. 登录后在「网关 Key」页创建一把 API Key"
    Write-Host "   3. 客户端 base_url 填 http://<本机IP>:$Port/v1，api_key 填刚建的那把"
    Write-Host ""
    Write-Host "   ⚠️  未创建 API Key 之前 /v1/* 会拒绝服务（fail-closed），这是预期行为。" -ForegroundColor Yellow
} else {
    Write-Host "❌ 服务没能启动。排查：" -ForegroundColor Red
    Write-Host "   Get-Service $ServiceName"
    Write-Host "   Get-Content `"$(Join-Path $DataDir 'logs\server.log')`" -Tail 50"
    Write-Host "   事件查看器 → Windows 日志 → 系统（来源 Service Control Manager）"
    exit 1
}
