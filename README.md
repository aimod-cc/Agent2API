<div align="center">

# Agent2API

**把多家 AI 桌面客户端的登录态，变成一个本地 OpenAI 兼容 API 网关**

[简体中文](./README.md) | [English](./docs/README.en.md) | [繁體中文](./docs/README.zh-Hant.md) | [日本語](./docs/README.ja.md) | [한국어](./docs/README.ko.md) | [Português (BR)](./docs/README.pt-BR.md)

[![license](https://img.shields.io/badge/license-MIT%20%2B%20附加声明-green)](./LICENSE)
[![release](https://img.shields.io/github/v/release/Agent2API/agent2api?include_prereleases)](https://github.com/Agent2API/agent2api/releases/latest)
[![docker pulls](https://img.shields.io/docker/pulls/aimodcc/agent2api)](https://hub.docker.com/r/aimodcc/agent2api)
[![stars](https://img.shields.io/github/stars/Agent2API/agent2api?style=flat&color=yellow)](https://github.com/Agent2API/agent2api/stargazers)
[![platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux%20%7C%20Docker-blue)](https://github.com/Agent2API/agent2api/releases/latest)

[✨ 核心特性](#-核心特性) • [💬 社区与支持](#-社区与支持) • [🤝 支持的提供商](#-支持的提供商) • [🚀 快速开始](#-快速开始) • [📦 安装与部署](./docs/install.md) • [📸 界面预览](#-界面预览) • [🔧 开发与构建](#-开发与构建) • [📜 使用声明](#-使用声明)

</div>

---

Agent2API 把多家 AI 桌面客户端里已登录的账号接到一起，共用一套 OpenAI 兼容接口。任何支持自定义 `base_url` 的客户端，填上 `http://127.0.0.1:3065/v1` 就能直接调用这些账号的模型额度 —— **不需要上游 API Key，也不需要改客户端**。

## ✨ 核心特性

- 🔌 **OpenAI 兼容** —— 客户端填个 `base_url` 就能用，Chat / Responses / Messages 三种协议都支持
- 🏪 **内置 17 家提供商** —— WorkBuddy、小浣熊、CatPaw、Qoder、Trae 等开箱即用，也支持自定义提供商
- 🔁 **凭证自动续期** —— Token 到期自动换新，临期凭证可批量刷新，模型目录也不会过期
- 🪂 **多账号轮转** —— 按优先级排队，429 自动换账号，限额冷却期内不空等
- 🛡️ **出网代理** —— 每个账号可配独立出口，出站指纹按官方客户端伪装
- 🔑 **网关安全** —— 多把网关 Key 可分权、登录有人机验证，没配 Key 时默认拒绝请求
- 📊 **请求报表** —— 请求量、成功率、Token 消耗一览，还有缓存命中率与 365 天热力图
- ⏰ **定时任务** —— 自动签到、凭证维护、模型目录刷新，开关与间隔在一页里管理
- 💰 **余额与用量** —— 账号行内直接看余额与用量，支持跨账号并发查询
- 🖥️ **三种形态** —— 桌面端（Windows / macOS / Linux，托盘常驻）、服务端（Windows / Linux，无界面，注册成系统服务）、Docker（面板与网关同端口，amd64 / arm64）；都不需要装 Node，界面六语

## ❤️ 赞助商

<a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme"><img src="assets/sponsor-proxylane.png" alt="ProxyLane（赞助位）" width="100%" /></a>

<table>

<tr>
<td width="240"><a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme"><picture><source media="(prefers-color-scheme: dark)" srcset="assets/proxylane-logo-for-dark-bg.png" /><img src="assets/proxylane-logo-for-light-bg.png" alt="ProxyLane" width="220" /></picture></a></td>
<td>感谢 <a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme">ProxyLane</a> 赞助了本项目！ProxyLane 提供 AI 账号专用的原生静态住宅 IP（Static Residential ISP）：IP 来自真实家庭宽带运营商，Scamalytics 低欺诈评分，每个 IP 独享，不限流量，$3.90 起。在「网络代理」页给每个账号配一个出口即可。首次购买使用优惠码 <code>AGENT2API25</code> 享 75 折。</td>
</tr>

</table>

## 💬 社区与支持

- **QQ 群**：Agent2API 交流群 —— **群 1（`1127656296`）即将满员，请优先加群 2（`1130894674`）**；在 QQ 里搜索群号即可加入。
- **Telegram 群**：<https://t.me/+9gM8YPvgRL02NGRl>
- **Discord 群**：<https://discord.gg/z9GzSJMWp>
- **💖 赞赏**：如果 Agent2API 帮到了你，欢迎[请作者喝杯咖啡](docs/SPONSOR.md)；赞赏完全自愿，不解锁任何功能。
- **🤝 赞助 / 商务合作**：想赞助本项目，或想让你的产品出现在这里？发邮件到 [541675139@qq.com](mailto:541675139@qq.com)。

## 🤝 支持的提供商

支持情况一览（✓ 支持 · ✗ 不支持 · — 无此概念）：

| 平台 | LLM 请求 | Token 自动续期 | 模型列表 | 余额查询 | 签到 | 领取类 |
| --- | :--: | :--: | :--: | :--: | :--: | :--: |
| WorkBuddy 国内版 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| WorkBuddy 国际版 | ✓ | ✓ | ✓ | ✓ | ✗ | — |
| 小浣熊 | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| CatPaw | ✓ | ✗ | ✓ | ✓ | ✗ | — |
| AutoClaw（国内版 / 国际版） | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| Qoder | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| Cline（Free / Pass） | ✓ | ✓ | ✓ | ✓ | — | — |
| Accio（国际版 / 国内版） | ✓ | ✓ | ✓ | ✓ | — | — |
| ZCode（国内版 / 国际版） | ✓ | ✗ | ✗ | ✓ | — | ✓ |
| CodeArts | ✓ | ✓ | ✓ | ✓ | — | ✓ |
| Trae | ✓ | ✓ | ✓ | ✓ | — | — |
| Loomy（讯飞） | ✓ | ✗ | ✓ | ✓ | ✓ | — |
| KukuAI（百度文库） | ✓ | ✗ | ✓ | ✓ | ✓ | — |
| MonkeyCode（长亭科技，国内版 / 国际版） | ✓ | ✗ | ✓ | — | — | — |
| Command Code | ✓ | ✗ | ✓ | — | — | — |
| Antigravity（Google，Gemini / Claude / GPT-OSS） | ✓ | ✓ | ✓ | — | — | — |
| OfficeAce（华为云果办 / OfficeClaw） | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| OrcaTerm（腾讯云遨驰终端） | ✓ | ✓ | ✓ | ✓ | — | — |
| 智谱清言 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| 自定义提供商 | ✓ | — | ✓ | — | — | — |

> 注：模型列表的 ✓ 表示支持在线自动拉取，部分平台另有内置静态表兜底；ZCode 仅有内置静态表。Token 续期列 ✗ 表示该平台没有可用的续期接口（账号在有效期内不受影响）。Qoder 签到仅中国版可用。
>
> **智谱清言**是本项目**唯一支持「访客模式」**的一家：不需要登录，点一下就能换到一份可直接对话的凭证（免费额度由上游按设备发放）。代价是访客账号没有积分体系 —— 上表里的「余额查询 / 签到 / 领取类」，以及**任务中心的新手任务与每日任务**，都只对**登录账号**有效（粘贴令牌或导入客户端登录态），访客会被如实跳过。

`/v1/chat/completions`、`/v1/responses`、`/v1/messages`（含 `count_tokens`）与 `/v1/models` 对所有平台一视同仁；模型映射、全局优先级队列、429 降级、出网代理、出站指纹脱敏与请求报表同样全平台通用。

> **本项目仅供学习与交流使用。** 它以非官方客户端形态复用你自己账号的登录态，可能不符合上游服务的使用条款，风险（含账号被风控、封禁）自负；禁止商用或绕过计费。详见[使用声明](#-使用声明)与 [LICENSE](./LICENSE)。
>
> 本项目与各上游厂商及其官方产品均无关（名单见[使用声明](#-使用声明)）；接口形态来自对各家客户端通信的观察，上游随时可能调整。

## 🚀 快速开始

三种形态，按你的环境选一个；**完整的安装、升级、卸载与排错见 [安装与部署](docs/install.md)**。

| 形态 | 产物 | 界面 |
| --- | --- | --- |
| **桌面端**（Windows / macOS / Linux） | `Agent2API_<版本>_x64-setup.exe`、`_universal.dmg`、`_amd64.deb`、`_amd64.AppImage` | 应用窗口 |
| **服务端**（Windows Server / Linux，无 GUI） | `agent2api-server-windows-amd64.zip`、`agent2api-server-linux-<arch>.tar.gz` | 浏览器面板 |
| **Docker** | `aimodcc/agent2api:latest`（amd64 / arm64） | 浏览器面板 |

**桌面端**：装完启动 → 「账号」页添加账号 → 客户端 `base_url` 填 `http://127.0.0.1:3065/v1`（`api_key` 随便填，例如 `sk-local`）。无需安装 Node 或任何其它运行时，网关跑在应用进程内，托盘常驻。

**服务端**（无图形界面，注册成系统服务、开机自启）：

```bash
# Linux
tar -xzf agent2api-server-linux-amd64.tar.gz && cd agent2api-server-linux-amd64 && sudo bash install.sh
```

```powershell
# Windows Server（管理员 PowerShell）
Expand-Archive agent2api-server-windows-amd64.zip -DestinationPath C:\agent2api-pkg
cd C:\agent2api-pkg; .\install-service.ps1
```

**Docker**：

```bash
docker run -d --name agent2api --restart unless-stopped -p 3065:3065 -v ./data:/data aimodcc/agent2api:latest
```

三种形态装完后都打开面板（`http://127.0.0.1:3065/`，服务端与 Docker 换成主机 IP）注册管理员账号，再在「网关 Key」页建一把 Key。连通性自测：`curl http://127.0.0.1:3065/health`

## 📸 界面预览

### 账号

![账号管理页：全局队列、按模型限额冷却、有效期与余额](./assets/screenshots/accounts.png)

添加账号：

![添加账号：选择提供商与版本，然后走网页登录](./assets/screenshots/add-account.png)

### 模型管理

![模型管理页：按提供商查看模型清单、倍率、上下文与映射](./assets/screenshots/models.png)

### 报表

概览：请求数 / 成功率 / Token 总量 / Top 模型与账号、提供商排名，附用量环图与 365 天活跃热力图：

![报表概览：统计卡片、Top 账号 / 提供商、模型与提供商用量环形图](./assets/screenshots/report-overview.png)

趋势：24 小时**缓存命中率**（折线）与 **Token 消耗**（面积）双轴同图，底部按天 Token 柱状图：

![报表趋势：缓存命中率与 Token 消耗双轴趋势、按天 Token 柱状图](./assets/screenshots/report-trends.png)

### 签到中心

![签到中心页：签到总览与每日签到分组](./assets/screenshots/checkin-center.png)

## 🔧 开发与构建

### 环境要求

- Rust >= 1.77 与 Tauri 2 工具链（编译桌面端本体；Windows 上还需 WebView2 运行时）
- Node.js >= 18.17（只用于 `npm run tauri:*` 与前端构建脚本，桌面端运行时不依赖 Node）
- 目标平台：Windows / macOS / Linux 桌面端、Windows / Linux 服务端（无 GUI）、Docker 服务端

### 常用脚本

```bash
npm run tauri:install      # 安装桌面端依赖（等价 npm --prefix desktop-tauri install）
npm run tauri:dev          # 开发模式调起桌面端（自动热重载）
npm run tauri:build        # 构建桌面端安装包

npm run build:icon         # 生成图标源图（改图标设计后执行，再跑 tauri icon）
```

打包产物为 `target/release/bundle/nsis/Agent2API_<版本>_x64-setup.exe`（3.0 版约 8.6 MB；`src-tauri/.cargo/config.toml` 把 cargo 的 `target-dir` 指到了项目根的 `target/`）。

macOS 的 `.dmg` **只能在 macOS 上构建**（`npm --prefix desktop-tauri run build -- --target universal-apple-darwin`），无法从 Windows 交叉编译，所以正式产物由 CI 的 `build` 工作流产（`macos-latest` + 双目标合并）；Docker 镜像与容器内更新用的服务端包由同一工作流的 `linux-server` job 出（复用 Dockerfile 的编译路径，包内可执行文件与镜像同源同工具链）。

## 📜 使用声明

### 仅供学习与交流

本项目是一个用于学习 HTTP 反向代理、SSE 流式透传、多上游协议适配与桌面端打包（Tauri）等技术主题的实践项目，**仅供个人学习与研究使用**。它不是官方产品，与腾讯公司及 WorkBuddy / CodeBuddy、美团及 CatPaw、商汤及小浣熊、智谱及 AutoClaw / autoglm、阿里巴巴及 Qoder / Accio、华为云及 CodeArts / OfficeAce、字节跳动及 Trae、科大讯飞及 Loomy、百度文库及 KukuAI、长亭科技及 MonkeyCode、Command Code、Google 及 Antigravity 均无任何关联，未获得其授权、认可或赞助。

### 关于反向代理行为

本项目实现的是本地反向代理：在你自己的机器上复用你自己账号的登录态，把请求转发到官方上游网关。它不破解、不绕过任何付费或权限校验，使用的额度始终来自你自己账号本就拥有的配额。但需要明确的是，这种「以非官方客户端形态复用登录态」的转发方式，**可能不符合上游服务的用户协议或使用条款**；是否使用、由此产生的一切后果（包括但不限于账号被限流、被风控、被冻结或封禁），均由使用者自行承担。

### 禁止用途

禁止将本项目用于任何商业用途、二次分发牟利、批量账号运营、绕过上游计费或配额限制、或其他违反当地法律法规的活动。如需在生产环境或商业场景中调用相关模型服务，请使用官方渠道与官方 API。

### 凭证与数据风险

本项目会把账号凭证（`accessToken` / `refreshToken` 等）以**明文**形式保存在本机配置目录（默认 `~/.agent2api/`）中，导出功能生成的文件同样包含明文凭证。请自行妥善保管，切勿提交到公开仓库、上传到网盘或分享给他人。因凭证泄露造成的损失由使用者自行承担。

### 无担保与权利通知

本项目按「现状」提供，作者不对其可用性、稳定性、安全性或对特定用途的适用性作任何承诺。上游接口随时可能变更，本项目可能随时失效而不再维护。完整条款见 [LICENSE](./LICENSE)。本项目中的接口形态、协议字段等信息来自对各官方客户端公开网络通信的观察与整理，相关商标与服务的权利归其各自所有者；若权利方认为本项目存在不当之处，请联系作者，将及时调整或删除。

## 📄 许可

本项目基于 [MIT License](./LICENSE)，可自由使用、修改与分发，须保留版权声明。

需要留意的是：LICENSE 正文之后附有一份**使用声明**，其中第 3 条在 MIT 之上**追加了限制**（禁止商业用途、禁止二次分发牟利、禁止批量账号运营）。因此本项目**不是**纯粹的 MIT 项目——**MIT 条款与使用声明共同构成完整的授权与使用约定**，两者对同一行为给出不同结论时以更严格的一方为准。这也是 `Cargo.toml` 用 `license-file` 指向 LICENSE、而不声明 SPDX `"MIT"` 的原因。

## ⭐ Star History

<a href="https://star-history.com/#Agent2API/agent2api&Date">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=Agent2API/agent2api&type=Date&theme=dark" />
    <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=Agent2API/agent2api&type=Date" />
    <img alt="Star History Chart" src="https://api.star-history.com/svg?repos=Agent2API/agent2api&type=Date" />
  </picture>
</a>

<div align="center">

**[下载最新版](https://github.com/Agent2API/agent2api/releases/latest)** • **[问题反馈](https://github.com/Agent2API/agent2api/issues)**

如果这个项目对你有帮助，欢迎点一个 ⭐️ Star！

</div>
