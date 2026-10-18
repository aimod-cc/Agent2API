# 安装与部署

本页是三种交付形态的**完整安装与运维说明**：桌面端（Windows / macOS / Linux）、服务端（Windows / Linux，无界面）、Docker。不确定该选哪个，先看下面的对照表。

各形态装完之后**都在浏览器面板里管理**（加账号、建网关 Key、看报表）—— 面板与网关同端口，桌面端则额外有应用窗口。

**简体中文** | [English](./install.en.md) | [繁體中文](./install.zh-Hant.md) | [日本語](./install.ja.md) | [한국어](./install.ko.md) | [Português (BR)](./install.pt-BR.md)

## 目录

- [选哪种形态](#选哪种形态)
- [桌面端（Windows / macOS / Linux）](#桌面端windows--macos--linux)
- [服务端（Windows / Linux，无界面）](#服务端windows--linux无界面)
- [Docker](#docker)
- [验证与客户端接入](#验证与客户端接入)
- [局域网访问](#局域网访问)
- [手机端](#手机端)

## 选哪种形态

| 形态 | 产物 | 界面 | 说明 |
| --- | --- | --- | --- |
| **Windows 桌面端** | `Agent2API_<版本>_x64-setup.exe` | 应用窗口 | NSIS 安装包，默认装到 `C:\Program Files\Agent2API`，安装时需要管理员授权 |
| **macOS 桌面端** | `Agent2API_<版本>_universal.dmg` | 应用窗口 | 通用二进制（Apple Silicon 与 Intel 各编一遍再合并），最低 macOS 10.15 |
| **Linux 桌面端** | `Agent2API_<版本>_amd64.deb` 或 `Agent2API_<版本>_amd64.AppImage` | 应用窗口 | deb 装进系统（依赖 webkit2gtk-4.1 / gtk3）；AppImage 免安装单文件，任意发行版可用 |
| **Windows 服务端**（无 GUI） | `agent2api-server-windows-amd64.zip` | **浏览器面板** | 命令行安装、注册成 Windows 服务，见 [服务端](#服务端windows--linux无界面) |
| **Linux 服务端**（无 GUI） | `agent2api-server-linux-<arch>.tar.gz` | **浏览器面板** | 命令行安装、systemd 托管，同一个包也是容器内更新载荷 |
| **Docker** | `aimodcc/agent2api:latest` | **浏览器面板** | amd64 / arm64 双架构，见 [Docker](#docker) |

> 选哪个：**自己有桌面**（要托盘、开机自启、本机登录态导入）→ 桌面端；
> **服务器上没有图形界面**（Windows Server / Linux 服务器）→ 服务端包；
> **已经跑 Docker** → 镜像。
>
> 桌面端需要图形界面，**装不到无 GUI 的服务器上**。服务端包不链接任何
> WebView / GTK 依赖，一个可执行文件加一个 `ui/` 目录就能跑。

## 桌面端（Windows / macOS / Linux）

**桌面端**（Windows / macOS / Linux）安装后启动即可，**无需安装 Node 或任何其它运行时**；网关跑在应用进程内，托盘常驻。

1. 首次启动即在应用进程内启动本机网关（端口 3065）并打开主窗口；若检测到旧版本的数据目录或数据文件，会弹窗提示迁移，按指引操作即可。
2. 点「账号」页的「添加账号」，选提供商后按弹窗提示完成登录或填写凭证即可（方式以弹窗为准：网页登录 / 手机验证码 / 粘贴凭证 / 导入本机登录态）。
3. 把 OpenAI 客户端的 `base_url` 填成 `http://127.0.0.1:3065/v1`，`api_key` 随便填（例如 `sk-local`，未启用鉴权时服务端不校验）。

关闭窗口默认最小化到托盘，网关继续在后台转发；彻底退出请在托盘图标右键选「退出」。

> **macOS 首次打开**：dmg 里的应用未做 Apple 代码签名与公证（签名需要付费的 Apple Developer 账号，见 `.github/workflows/build.yml` 的说明），Gatekeeper 会拦下首次启动 —— 到「系统设置 → 隐私与安全性」里手动放行一次即可。
>
> **Linux 桌面端**：托盘依赖 `libayatana-appindicator3`（多数桌面环境已自带）；若桌面环境下托盘图标不显示，装 `libayatana-appindicator3-1` 后重开即可。AppImage 首次运行前需要可执行位（`chmod +x`，程序在「打开安装包」时会自动补）。更新方式：deb 走系统安装器（装的时候会问一次密码），AppImage 是换文件即升级。
>
> **桌面端的差异**：AutoClaw 与智谱清言的「导入本机登录态」只在 Windows 可用（读的是 Windows 的 DPAPI 密文，macOS / Linux 上整段收起）；其余家（小浣熊 / CatPaw / Cline）读的是 `HOME` 下的明文文件，三个平台都可用。托盘、开机自启、局域网访问与网页面板托管在 macOS / Linux 上同样支持。

## 服务端（Windows / Linux，无界面）

给**没有图形界面的服务器**用（Windows Server、Linux 服务器、云主机）。产物是同一份
`agent2api-server` 二进制：无窗口、无托盘、不链接 WebView / GTK，运行时不依赖
Node 或任何其它运行时。命令只负责**安装**，装完之后的管理（加账号、建 Key、
看报表）都在浏览器面板里做。

服务以系统服务方式常驻：开机自启、崩溃自动拉起、无控制台后台运行。

### Windows Server

下载 `agent2api-server-windows-amd64.zip`，解压后**以管理员身份**打开 PowerShell：

```powershell
Expand-Archive agent2api-server-windows-amd64.zip -DestinationPath C:\agent2api-pkg
cd C:\agent2api-pkg
.\install-service.ps1
```

脚本会：装程序到 `C:\Agent2API`、数据落到 `C:\ProgramData\Agent2API`、注册名为
`Agent2API` 的 Windows 服务（`LocalSystem`，开机自启）、加防火墙入站规则，然后启动。

```powershell
# 自定义位置与端口
.\install-service.ps1 -InstallDir D:\apps\agent2api -DataDir D:\data\agent2api -Port 8080

# 日常管理
Get-Service Agent2API
Restart-Service Agent2API
Stop-Service Agent2API

# 卸载（保留数据）；加 -Purge 连程序与数据一起删
.\install-service.ps1 -Uninstall
```

**升级**：解压新版本包，重跑一次 `install-service.ps1` 即可（脚本会先停服务、
覆盖程序与界面、再启动；数据与配置不动）。

> **两个 Windows 特有的坑，脚本已经处理好了**：
> 服务进程的工作目录是 `C:\Windows\system32` 而不是 exe 所在目录 —— 界面目录靠
> exe 路径定位，不受影响；数据目录则由脚本显式写入服务注册表的
> `AGENT2API_PROXY_HOME`（不设的话会落到 LocalSystem 的
> `C:\Windows\system32\config\systemprofile` 下，几乎不可能找到）。
> 服务没有控制台，所以日志同时落文件 `C:\ProgramData\Agent2API\logs\server.log`。

### Linux（systemd）

```bash
tar -xzf agent2api-server-linux-amd64.tar.gz
cd agent2api-server-linux-amd64      # 或解包后的目录名
sudo bash install.sh
```

脚本会：建 `agent2api` 系统账号（不允许登录）、装程序到 `/opt/agent2api`、数据落
`/var/lib/agent2api`、配置写到 `/etc/agent2api/agent2api.env`、装 systemd 单元并
`enable + start`。

```bash
# 升级（保留数据与配置，旧二进制备份为 .bak）
sudo bash install.sh --upgrade

# 卸载（保留数据）；加 --purge 连数据与配置一起删
sudo bash install.sh --uninstall

# 日常管理
systemctl status agent2api
systemctl restart agent2api
journalctl -u agent2api -f
```

开机自启、崩溃自动拉起（`Restart=always`）、SIGTERM 优雅停机（等在途请求跑完
再退出，SSE 长连接不会被打断）都已配好。配置改 `/etc/agent2api/agent2api.env`
后 `systemctl restart agent2api` 生效。

> **裸机不要开 `AGENT2API_SELF_UPDATE`**：那是容器内的方案（换文件后退出，靠
> 容器 restart 策略拉起）。裸机上程序文件属 root、服务账号无权替换，单元里已
> 显式设成 `0`。升级走 `install.sh --upgrade`。

### 装完之后

两个平台都一样 —— 打开浏览器：

```
http://<服务器IP>:3065/
```

1. 首次进入会引导**注册管理员账号**（也可以用环境变量预置，见下）
2. 登录后在「网关 Key」页创建一把 API Key
3. 客户端 `base_url` 填 `http://<服务器IP>:3065/v1`，`api_key` 填刚建的那把

> ⚠️ **建 Key 之前 `/v1/*` 会拒绝服务**（fail-closed）。这是刻意的：服务端形态
> 绑 `0.0.0.0`，没配 Key 就把上游额度对全网开放是不可接受的。

### 无人值守部署

管理员和网关 Key 都能用环境变量预置，跳过浏览器里的手动步骤：

| 变量 | 说明 |
| --- | --- |
| `AGENT2API_ADMIN_USER` + `AGENT2API_ADMIN_PASSWORD` | 预置管理员（密码明文填，启动时自动转 bcrypt）；也可用 `AGENT2API_ADMIN_PASSWORD_HASH` 直接给哈希 |
| `AGENT2API_PROXY_API_KEY` | 预置一把网关 Key，跳过「面板里建第一把」 |
| `AGENT2API_PROXY_HOME` | 数据目录 |
| `AGENT2API_HOST` / `AGENT2API_PROXY_PORT` | 监听地址（默认 `0.0.0.0`）/ 端口（默认 `3065`） |
| `AGENT2API_PANEL_PORT` | 面板分端口：管理面单独监听该端口，主端口只留 `/v1/*` |
| `AGENT2API_ALLOW_NO_KEY` | 置 `1` 关闭 fail-closed（仅纯内网） |
| `AGENT2API_LOG_FILE` | 日志文件路径，默认 `{数据目录}/logs/server.log` |
| `AGENT2API_VERBOSE` | 置 `1` 打开 debug 日志 |

Linux 写在 `/etc/agent2api/agent2api.env`；Windows 改服务注册表的
`HKLM\SYSTEM\CurrentControlSet\Services\Agent2API\Environment`（`install-service.ps1`
已经写好基础项，重跑脚本会覆盖）。

> **账号无法预置**：账号需要扫码 / 验证码 / 粘贴凭证，这一步由上游协议决定，
> 绕不过去。批量部署可以把已经配好的**数据目录整个拷过去**（SQLite 库 +
> `account.key` 在同一个目录里），账号随之迁移。

## Docker

```bash
docker run -d --name agent2api --restart unless-stopped \
  -p 3065:3065 -v ./data:/data \
  aimodcc/agent2api:latest
```

浏览器打开 `http://<主机>:3065`，首次进入会引导**注册管理员账号**；登录后在「网关 Key」页建一把 API Key 给客户端用 —— `http://<主机>:3065/v1` 即 OpenAI 兼容端点（未建 Key 前拒绝转发）。状态（SQLite 库 / 配置 / 日志）都在 `./data` 一个卷里。

compose 用户（amd64 / arm64 都有镜像）：

```yaml
services:
  agent2api:
    image: aimodcc/agent2api:latest
    container_name: agent2api
    restart: unless-stopped
    ports:
      - "3065:3065"
    volumes:
      - ./data:/data
```

环境变量（都可选）：

| 变量 | 说明 |
| --- | --- |
| `AGENT2API_ADMIN_USER` + `AGENT2API_ADMIN_PASSWORD` | 预置管理员账号密码（密码明文填，启动时自动转哈希）；不填走面板注册 |
| `AGENT2API_PANEL_PORT` | 面板分端口：设后面板（界面 + `/api/*`）单独监听该端口，公网只映射主端口即可把管理面留在内网（面板端口绑回环，写 `127.0.0.1:3066:3066`） |
| `AGENT2API_HOST` / `AGENT2API_PROXY_PORT` | 监听地址（默认 `0.0.0.0`）/ 端口（默认 `3065`） |
| `AGENT2API_ALLOW_NO_KEY` | 置 `1` 关闭 fail-closed（未配 Key 也放行 `/v1`，仅限纯内网） |
| `AGENT2API_CAPTCHA_ENABLED` | 登录页人机验证：默认 `1` 开启，`0` 关闭 |
| `AGENT2API_SELF_UPDATE` | 容器内的更新入口：默认 `1` 开启（面板可「下载并更新」）；置 `0` 关闭，只走手动升级 |
| `WORKBUDDY_UPDATE_REPO` + `WORKBUDDY_UPDATE_API_BASE` | 进阶：把「检查更新 / 下载更新」换成自建源。前者换仓库（默认跟上游 `Agent2API/agent2api`，fork 后可改），后者换 API 根（默认 `https://api.github.com`，需返回与 GitHub 同形的 release JSON）。配了自建源后，下载白名单只额外放行**该源自己的域名**（按源的 scheme：配 `http://` 就允许明文下载，仅建议本地演练/内网镜像用）|

从源码构建：克隆本仓库后 `docker compose up -d --build`（镜像里只有网关与面板，不含 Rust 工具链）。

**网页面板的功能差异**（源于「没有本机桌面客户端」）：网页登录（WorkBuddy / Qoder / Cline）、手机验证码、粘贴凭证完全可用；需要本机回调的网页登录（AutoClaw / CatPaw / Accio / CodeArts / Trae）与「导入本机登录态」不可用，请改用粘贴凭证。桌面端开着「同时开放网页管理面板」时，用浏览器访问该面板也是这一套差异 —— 同一份界面、同一套判定。

### 容器内更新（可选）

面板的「软件更新」页在容器里能直接「下载并更新」：下载官方 Release 里的 `agent2api-server-linux-<arch>.tar.gz`（校验 `.sha256`）→ 替换可执行文件与 `ui/` → 进程优雅退出、由 `restart` 策略拉起同一个容器。由 `AGENT2API_SELF_UPDATE` 控制（Dockerfile 默认 `1`），置 `0` 则只走手动升级。

同一个 `agent2api-server-linux-<arch>.tar.gz` 也是 [Linux 服务端](#linuxsystemd) 的安装包：包内的 `agent2api-server` 与 `ui/` 是这条更新链按名取的条目，另外还带着 `install.sh` / `agent2api.service` / `agent2api.env.example`。容器更新链只认前两个条目（其余读完即丢，见 `selfupdate::extract_bundle`），所以一份产物同时服务两种用途 —— 容器里解包替换，裸机上解包跑 `install.sh`。

代价要清楚：更新后容器标签与**实际运行的版本会漂移**（`docker inspect` 显示的仍是旧 tag），执行 `docker compose pull && up -d` 重建容器会回到镜像里的版本。想固定版本就别开这个入口。

## 验证与客户端接入

网关起来后，用 curl 确认联通性（`model` 填 `GET /v1/models` 里任一实际存在的名字）：

```bash
curl http://127.0.0.1:3065/health
curl http://127.0.0.1:3065/v1/models

curl http://127.0.0.1:3065/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model":"deepseek-v4.1-flash","messages":[{"role":"user","content":"你好"}],"stream":true}'
```

客户端接入示例（Python SDK）：

```python
from openai import OpenAI

client = OpenAI(base_url="http://127.0.0.1:3065/v1", api_key="sk-local")
resp = client.chat.completions.create(
    model="deepseek-v4.1-flash",     # 名单里有哪家就是哪家，见 GET /v1/models
    messages=[{"role": "user", "content": "你好"}],
)
print(resp.choices[0].message.content)
```

**浏览器页面**（自建 Web UI 等）用 `fetch` 直连会被跨源预检拒绝——网关默认不应答 CORS，预检落到 API Key 校验上得 401。两种解法：① 设置页「安全 → 网关跨域访问」打开（来源 `*`；此时任何网页都能借本机网关打上游，建议同时配「网关 Key」）；② 页面与网关同源——本地静态服务托管页面并把 `/v1` 反代到 `127.0.0.1:3065`。

## 局域网访问

默认只监听 `127.0.0.1`。开启「设置 → 通用 → 局域网访问」后改听所有网卡，局域网设备把 API 地址指向本机 IP 即可共用（界面会给出完整地址）。出于安全，开启前必须注册面板管理员：管理接口要求管理员会话或网关 Key，无可用 Key 时转发接口同样拒绝服务（开启流程会自动补一把「默认」Key）；也可选择同时开放网页面板（默认关闭）。改动重启生效。

桌面端（Windows / macOS）装包里就带着网页面板那一份 `ui/`，打开「同时开放网页管理面板」后局域网设备用浏览器访问同一地址即是管理界面 —— 与 Docker 版是同一份界面；**桌面窗口自己不走网页登录**（没有登录页），它以本机第一把网关 Key 作为凭证，打开这个开关不需要你额外登录桌面端。网页登录页只在浏览器访问面板时出现。

## 手机端

手机端有两种形态，按需要选：

| 形态 | 怎么装 | 适合 |
| --- | --- | --- |
| **浏览器 / PWA** | 手机浏览器打开 `/m`，可「添加到主屏幕」 | 临时看、不想装东西 |
| **Android APK** | 装 `Agent2API-<版本>.apk` | 要图标、要全屏、要管多台服务器 |

两者的界面是同一份代码，区别在「谁提供页面」：PWA 由网关出（所以必须开「同时开放网页管理面板」），APK 把界面打进包里（所以离线也能起界面，且不需要那个开关）。

### 前置条件（按形态）

**桌面端（Windows / macOS / Linux）**：两个开关都要开，缺一不可。

1. 「设置 → 通用 → 局域网访问」—— 允许局域网设备访问网关。开启流程会要求先注册面板管理员，并自动补一把名为「默认」的 Key。
2. 「设置 → 通用 → 局域网访问 → 同时开放网页管理面板」—— 托管 `ui/` 静态目录。**浏览器 / PWA 形态需要它**（`/m` 由网关出，开关关着时 404）；**APK 不需要**（界面在包里），但 APK 仍然要第 1 项，否则网关只监听 `127.0.0.1`，手机连不上。

两项都要重启生效。

**服务端 / Docker**：不用额外开开关 —— 这两种形态默认就监听所有网卡、默认托管 `ui/`（Dockerfile 里 `AGENT2API_UI_DIR=/app/ui` 是写死的）。只要容器或服务的端口对局域网可达即可。若你手工设过 `AGENT2API_HOST=127.0.0.1`，改成 `0.0.0.0`；若 `AGENT2API_UI_DIR` 指向的目录不存在，面板会 404 而 API 照常工作。

### 在手机上打开（浏览器 / PWA）

`http://<网关所在机器的局域网IP>:3065/m`

桌面端就是上一步界面给出的那个地址（形如 `192.168.1.23`），把端口后面的 `/v1` 换成 `/m`。Docker / 服务端用宿主机的局域网 IP 与映射出去的端口。

首次打开会跳到登录页，用面板管理员账号登录后自动回到手机端。没注册过管理员时，登录页会先让你创建一个。

### 安装 APK（Android）

装包后首次打开是一个「服务器」页，流程只有三步：

1. 点「添加」，填网关的 `IP:端口`（如 `192.168.1.23:3065`，不写端口默认 3065）；
2. 点「连接并保存」—— 会先探活，连不上会当场说清楚是地址错还是网关没开局域网；
3. 弹出登录页，输入这台网关的管理员账号密码（没注册过管理员的会变成「创建管理员」）。

**多台服务器**：再点一次「添加」即可。每台的登录状态各自独立保存 —— 切过去不用重新登录，切回来也一样。入口在「设置 → 服务器」，列表里点哪台就切到哪台。

**APK 的已知限制**：

- 只出 Android。iOS 需要 Apple 开发者账号签名，或者自己用 Xcode 装（侧载签名 7 天过期）。
- 连的是 `http://` 局域网地址，明文。风险面是「同一局域网内可嗅探」，网关本身仍然要管理员登录。想消除的话给网关配 TLS，在 App 里填 `https://` 地址（解析逻辑一样，只是不会自动补 http）。
- 界面随 APK 版本走，不随网关升级。网关升级后如果新增了页面，要装新版 APK 才看得到；纯数据变化不受影响。

### 安装到主屏幕（浏览器 / PWA）

可选，装完就是全屏、无地址栏。手机端「设置 → 安装到桌面」里直接给了入口：

- **Android / Chrome**：设置页点「安装到主屏幕」（弹系统安装框）；也可以走右上角菜单 →「添加到主屏幕」/「安装应用」
- **iOS / Safari**：Safari 没有安装接口，设置页会写明步骤 —— 分享 →「添加到主屏幕」

装完从桌面图标进入，直接落到手机端首页。已在独立窗口里运行时，设置页会显示「已安装」。

有新版本时页面底部会浮出一条「重启更新」：网关升级后新版本的界面已经下好，点一下切过去（不点也不影响当前使用，下次打开自然生效）。

> **为什么没有原生 App / 应用商店包**：手机端连的必然是**另一台机器上跑的网关**（网关要读那台机器的登录态、写它的配置文件，手机上跑不了）。它的本质是「远程访问一个 HTTP 服务」，浏览器已经是最合适的载体 —— 同源认证、免安装、跟着网关一起升级。PWA 形态额外给了全屏与主屏幕图标，观感与原生应用一致。
>
> **Service Worker（离线外壳）只在 `https` 或 `localhost` 下生效**：局域网走的是 `http://<IP>`，浏览器不提供 SW，所以断网时手机端打不开。这是浏览器的安全限制，不是功能缺失 —— 正常使用时网关在线，不受影响。

### 能做什么、不能做什么

| 能做 | 不能做（请回桌面端） |
| --- | --- |
| 看报表、账号状态、余额、请求日志、运行日志 | 添加账号（扫码 / 粘贴令牌 / 导入登录态） |
| 启停账号、刷新凭证、删除账号 | 编辑账号的凭证、代理、限流规则 |
| 一键签到、看签到记录 | 逐项领取新手福利与套餐 |
| 启停模型、按提供商筛选 | 改模型映射、思考等级、能力位 |
| 复制网关 Key、启停 Key | 新建 Key、设置可用范围白名单 |
| 启停代理、测试连通性 | 新增代理、订阅管理、内核下载 |
| 启停定时任务、改间隔 | 改端口、路由、重试、超时、数据保留 |
| 助手快捷查询（余额 / 重复模型 / 今日用量 / 限流账号） | 助手的自由对话与工具调用 |
| 查看 Agent 接管状态、还原 | 改写客户端配置（手机读不到本机文件） |

桌面端侧栏底部有「手机端打开」入口，点开即是同一个页面。
