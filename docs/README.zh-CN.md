<div align="center">

# Agent2API

**AI 路由网关 + Agent 客户端接管，统一管理模型接口与本机客户端配置**

[English](../README.md) | **简体中文** | [繁體中文](./README.zh-Hant.md) | [日本語](./README.ja.md) | [한국어](./README.ko.md) | [Português (BR)](./README.pt-BR.md)

[![license](https://img.shields.io/badge/license-AGPL--3.0-green)](../LICENSE)
[![release](https://img.shields.io/github/v/release/aimod-cc/Agent2API?include_prereleases)](https://github.com/aimod-cc/Agent2API/releases/latest)
[![docker pulls](https://img.shields.io/docker/pulls/aimodcc/agent2api)](https://hub.docker.com/r/aimodcc/agent2api)
[![stars](https://img.shields.io/github/stars/aimod-cc/Agent2API?style=flat&color=yellow)](https://github.com/aimod-cc/Agent2API/stargazers)
[![platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux%20%7C%20Docker-blue)](https://github.com/aimod-cc/Agent2API/releases/latest)

[✨ 核心特性](#-核心特性) • [💬 社区与支持](#-社区与支持) • [🚀 快速开始](#-快速开始) • [📦 安装与部署](./install.md) • [📸 界面预览](#-界面预览) • [🔧 开发与构建](#-开发与构建) • [📜 使用声明](#-使用声明)

</div>

---

Agent2API 是一个 **AI 路由网关 + Agent 客户端接管**项目。它将自定义 AI 提供商接入统一的模型接口，集中管理账号选路、模型映射、网关密钥和请求报表；同时管理本机 Agent 客户端的连接配置，支持配置导入、切换与写入预览。

## ✨ 核心特性

- 🔌 **统一模型接口** —— 支持 OpenAI Chat / Responses / Images 与 Anthropic Messages 协议，以及统一的模型列表；小浣熊、Loomy、Antigravity、ModelScope、WorkBuddy、豆包工作 提供文生图，小浣熊、Loomy、Antigravity、ModelScope 与 WorkBuddy 还支持参考图编辑；生图与编辑也可由 AI 助手工具调用
- 🧭 **AI 路由** —— 模型映射、账号优先级、负载均衡、会话粘性与失败切换集中配置
- 🤖 **Agent 客户端接管** —— 管理本机客户端的多套连接配置，导入现有配置、预览写入内容并切换网关或自定义服务
- 🧩 **插件扩展** —— 额外提供商实现独立安装和更新，不随主程序打包；自定义提供商无需插件
- 🛡️ **出网代理** —— 账号与更新任务可选择代理池中的出口
- 🔑 **网关安全** —— 网关 Key 可分权、面板登录支持人机验证，未配置 Key 时默认拒绝模型请求
- 📊 **请求报表** —— 请求量、成功率、Token 消耗、缓存命中率与 365 天热力图
- ⏰ **后台维护** —— 管理模型目录刷新、凭证维护与软件更新检查；每日签到按各家自己的窗口时刻执行（Qoder 10:01、ZCode 19:01，其余跟随条目时刻），另有一轮补签只处理当天还没签上的账号；额外提供商专属任务由插件扩展
- 🖥️ **三种形态** —— 桌面端（Windows / macOS / Linux，托盘常驻）、服务端（Windows / Linux，无界面）、Docker（amd64 / arm64）；无需 Node 运行时，界面支持六种语言

## ❤️ 赞助商

<a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme"><img src="../assets/sponsor-proxylane.png" alt="ProxyLane（赞助位）" width="100%" /></a>

<table>

<tr>
<td width="240"><a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme"><picture><source media="(prefers-color-scheme: dark)" srcset="../assets/proxylane-logo-for-dark-bg.png" /><img src="../assets/proxylane-logo-for-light-bg.png" alt="ProxyLane" width="220" /></picture></a></td>
<td>感谢 <a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme">ProxyLane</a> 赞助了本项目！ProxyLane 提供 AI 账号专用的原生静态住宅 IP（Static Residential ISP）：IP 来自真实家庭宽带运营商，Scamalytics 低欺诈评分，每个 IP 独享，不限流量，$3.90 起。在「网络代理」页给每个账号配一个出口即可。首次购买使用优惠码 <code>AGENT2API25</code> 享 75 折。</td>
</tr>

</table>

## 💬 社区与支持

- **QQ 群**：Agent2API 交流群 —— **群 1（`1127656296`）与群 2（`1130894674`）都即将满员，请加群 3（`1108276647`）**；在 QQ 里搜索群号即可加入。
- **Telegram 群**：<https://t.me/+9gM8YPvgRL02NGRl>
- **💖 赞赏**：如果 Agent2API 帮到了你，欢迎[请作者喝杯咖啡](./SPONSOR.md)；赞赏完全自愿，不解锁任何功能。
- **🤝 赞助 / 商务合作**：想赞助本项目，或想让你的产品出现在这里？发邮件到 [541675139@qq.com](mailto:541675139@qq.com)。

## 🚀 快速开始

三种形态，按你的环境选一个；**完整的安装、升级、卸载与排错见 [安装与部署](./install.md)**。

| 形态 | 产物 | 界面 |
| --- | --- | --- |
| **桌面端**（Windows / macOS / Linux） | `Agent2API_<版本>_x64-setup.exe`、`_universal.dmg`、`_amd64.deb`、`_amd64.AppImage` | 应用窗口 |
| **服务端**（Windows Server / Linux，无 GUI） | `agent2api-server-windows-amd64.zip`、`agent2api-server-linux-<arch>.tar.gz` | 浏览器面板 |
| **Docker** | `aimodcc/agent2api:latest`（amd64 / arm64） | 浏览器面板 |

**桌面端**：启动后添加自定义提供商与账号，创建网关 Key，再在「Agent」页应用连接配置，或在客户端填入 `http://127.0.0.1:3065/v1` 和该 Key。额外提供商能力需先安装对应插件。

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

**手机端**：浏览器打开 `http://<网关IP>:3065/m` 即可（可「添加到主屏幕」变成全屏 PWA）；想要独立图标就装 Release 里的 `Agent2API-<版本>.apk`（Android）或 `Agent2API-<版本>.ipa`（iOS，未签名，需自行重签）。细节见[安装与部署 → 手机端](./install.md#手机端)。

## 📸 界面预览

### 账号

![账号管理页：全局队列、按模型限额冷却、有效期与余额](../assets/screenshots/accounts.png)

添加账号：

![添加账号：选择提供商与版本，然后走网页登录](../assets/screenshots/add-account.png)

### 模型管理

![模型管理页：按提供商查看模型清单、倍率、上下文与映射](../assets/screenshots/models.png)

### 报表

概览：请求数 / 成功率 / Token 总量 / Top 模型与账号、提供商排名，附用量环图与 365 天活跃热力图：

![报表概览：统计卡片、Top 账号 / 提供商、模型与提供商用量环形图](../assets/screenshots/report-overview.png)

趋势：24 小时**缓存命中率**（折线）与 **Token 消耗**（面积）双轴同图，底部按天 Token 柱状图：

![报表趋势：缓存命中率与 Token 消耗双轴趋势、按天 Token 柱状图](../assets/screenshots/report-trends.png)

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

### 凭证与数据安全

账号凭证在本地数据库中加密保存，但导出文件、备份及密钥文件仍可能包含敏感数据。请妥善保管，不要上传到公开仓库、网盘或分享给他人。

### 无担保

本项目按「现状」提供，不保证可用性、稳定性、安全性或特定用途适用性。相关商标与服务的权利归各自所有者；完整许可与免责条款见 [LICENSE](../LICENSE)。

## 📄 许可

本项目与提供商插件仓库一致，采用 [GNU Affero General Public License v3.0（AGPL-3.0）](../LICENSE)。可依许可使用、修改与分发（包括商业用途），须保留版权及许可声明并履行相应源码提供义务；通过网络向用户提供修改版服务时，须向这些用户提供该版本的对应源码。

## ⭐ Star History

<a href="https://star-history.com/#aimod-cc/Agent2API&Date">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=aimod-cc/Agent2API&type=Date&theme=dark" />
    <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=aimod-cc/Agent2API&type=Date" />
    <img alt="Star History Chart" src="https://api.star-history.com/svg?repos=aimod-cc/Agent2API&type=Date" />
  </picture>
</a>

<div align="center">

**[下载最新版](https://github.com/aimod-cc/Agent2API/releases/latest)** • **[问题反馈](https://github.com/aimod-cc/Agent2API/issues)**

如果这个项目对你有帮助，欢迎点一个 ⭐️ Star！

</div>
