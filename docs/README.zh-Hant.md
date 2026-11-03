<div align="center">

# Agent2API

**AI 路由閘道 + Agent 客戶端接管，統一管理模型介面與本機客戶端設定**

[English](../README.md) | [简体中文](./README.zh-CN.md) | **繁體中文** | [日本語](./README.ja.md) | [한국어](./README.ko.md) | [Português (BR)](./README.pt-BR.md)

[![license](https://img.shields.io/badge/license-AGPL--3.0-green)](../LICENSE)
[![release](https://img.shields.io/github/v/release/aimod-cc/Agent2API?include_prereleases)](https://github.com/aimod-cc/Agent2API/releases/latest)
[![docker pulls](https://img.shields.io/docker/pulls/aimodcc/agent2api)](https://hub.docker.com/r/aimodcc/agent2api)
[![stars](https://img.shields.io/github/stars/aimod-cc/Agent2API?style=flat&color=yellow)](https://github.com/aimod-cc/Agent2API/stargazers)
[![platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux%20%7C%20Docker-blue)](https://github.com/aimod-cc/Agent2API/releases/latest)

[✨ 核心特性](#-核心特性) • [💬 社群與支援](#-社群與支援) • [🚀 快速開始](#-快速開始) • [📦 安裝與部署](./install.zh-Hant.md) • [📸 介面預覽](#-介面預覽) • [🔧 開發與建置](#-開發與建置) • [📜 使用聲明](#-使用聲明)

</div>

---

Agent2API 是一個 **AI 路由閘道 + Agent 客戶端接管**專案。它將自訂 AI 供應商接入統一的模型介面，集中管理帳號選路、模型對應、閘道金鑰與請求報表；同時管理本機 Agent 客戶端的連線設定，支援設定匯入、切換與寫入預覽。

## ✨ 核心特性

- 🔌 **統一模型介面** —— 支援 OpenAI Chat / Responses / Images 與 Anthropic Messages 協定，以及統一模型清單；小浣熊、Loomy、Antigravity、ModelScope、WorkBuddy、豆包工作 提供文字生圖，小浣熊、Loomy、Antigravity、ModelScope 與 WorkBuddy 亦支援參考圖編輯；生圖與編輯也可由 AI 助手工具呼叫
- 🧭 **AI 路由** —— 集中設定模型對應、帳號優先順序、負載平衡、工作階段黏性與失敗切換
- 🤖 **Agent 客戶端接管** —— 管理本機客戶端的多套連線設定，匯入既有設定、預覽寫入內容並切換閘道或自訂服務
- 🧩 **插件擴充** —— 額外供應商實作獨立安裝與更新，不隨主程式打包；自訂供應商無需插件
- 🛡️ **出口代理** —— 帳號與更新任務可選擇代理池中的出口
- 🔑 **閘道安全** —— 閘道金鑰可分權、面板登入支援人機驗證，未設定金鑰時預設拒絕模型請求
- 📊 **請求報表** —— 請求量、成功率、Token 消耗、快取命中率與 365 天熱力圖
- ⏰ **背景維護** —— 管理模型清單更新、憑證維護與軟體更新檢查；每日簽到按各家自己的視窗時刻執行（Qoder 10:01、ZCode 19:01，其餘跟隨條目時刻），另有一輪補簽只處理當天還沒簽上的帳號；額外供應商專屬任務由插件擴充
- 🖥️ **三種形態** —— Windows / macOS / Linux 桌面端、Windows / Linux 無介面伺服器與 Docker（amd64 / arm64）；無需 Node 執行環境，介面支援六種語言

## ❤️ 贊助商

<a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme"><img src="../assets/sponsor-proxylane.png" alt="ProxyLane（贊助位）" width="100%" /></a>

<table>

<tr>
<td width="240"><a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme"><picture><source media="(prefers-color-scheme: dark)" srcset="../assets/proxylane-logo-for-dark-bg.png" /><img src="../assets/proxylane-logo-for-light-bg.png" alt="ProxyLane" width="220" /></picture></a></td>
<td>感謝 <a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme">ProxyLane</a> 贊助了本專案！ProxyLane 提供 AI 帳號專用的原生靜態住宅 IP（Static Residential ISP）：IP 來自真實家庭寬頻營運商，Scamalytics 低詐欺評分，每個 IP 獨享，不限流量，$3.90 起。在「網路代理」頁給每個帳號配一個出口即可。首次購買使用優惠碼 <code>AGENT2API25</code> 享 75 折。</td>
</tr>

</table>

## 💬 社群與支援

- **QQ 群**：Agent2API 交流群 —— **群 1（`1127656296`）與群 2（`1130894674`）都即將滿員，請加群 3（`1108276647`）**；在 QQ 裡搜尋群號即可加入。
- **Telegram 群**：<https://t.me/+9gM8YPvgRL02NGRl>
- **💖 贊助**：如果 Agent2API 幫到了你，歡迎[請作者喝杯咖啡](./SPONSOR.md)；贊助完全自願，不解鎖任何功能。
- **🤝 贊助 / 商務合作**：想贊助本專案，或想讓你的產品出現在這裡？寄信到 [541675139@qq.com](mailto:541675139@qq.com)。

## 🚀 快速開始

三種形態，依你的環境挑一個；**完整的安裝、升級、解除安裝與排錯見 [安裝與部署](./install.zh-Hant.md)**。

| 形態 | 產物 | 介面 |
| --- | --- | --- |
| **桌面端**（Windows / macOS / Linux） | `Agent2API_<版本>_x64-setup.exe`、`_universal.dmg`、`_amd64.deb`、`_amd64.AppImage` | 應用程式視窗 |
| **伺服器端**（Windows Server / Linux，無 GUI） | `agent2api-server-windows-amd64.zip`、`agent2api-server-linux-<arch>.tar.gz` | 瀏覽器面板 |
| **Docker** | `aimodcc/agent2api:latest`（amd64 / arm64） | 瀏覽器面板 |

**桌面端**：啟動後新增自訂供應商與帳號，建立閘道金鑰，再於「Agent」頁套用連線設定，或在客戶端填入 `http://127.0.0.1:3065/v1` 與該金鑰。額外供應商能力需先安裝對應插件。

**伺服器端**（無圖形介面，註冊成系統服務、開機自動啟動）：

```bash
# Linux
tar -xzf agent2api-server-linux-amd64.tar.gz && cd agent2api-server-linux-amd64 && sudo bash install.sh
```

```powershell
# Windows Server（管理員 PowerShell）
Expand-Archive agent2api-server-windows-amd64.zip -DestinationPath C:\agent2api-pkg
cd C:\agent2api-pkg; .\install-service.ps1
```

**Docker**：

```bash
docker run -d --name agent2api --restart unless-stopped -p 3065:3065 -v ./data:/data aimodcc/agent2api:latest
```

三種形態裝完後都開啟面板（`http://127.0.0.1:3065/`，伺服器端與 Docker 換成主機 IP）註冊管理員帳號，再在「閘道 Key」頁建一把 Key。連線性自測：`curl http://127.0.0.1:3065/health`

**手機端**：瀏覽器開啟 `http://<閘道IP>:3065/m` 即可（可「加入主畫面」變成全螢幕 PWA）；想要獨立圖示就裝 Release 裡的 `Agent2API-<版本>.apk`（Android）或 `Agent2API-<版本>.ipa`（iOS，未簽名，需自行重簽）。細節見[安裝與部署 → 手機端](install.zh-Hant.md#手機端)。

## 📸 介面預覽

### 帳號

![帳號管理頁：全域佇列、依模型限額冷卻、有效期與餘額](../assets/screenshots/accounts.png)

新增帳號：

![新增帳號：選擇供應商與版本，然後走網頁登入](../assets/screenshots/add-account.png)

### 模型管理

![模型管理頁：依供應商檢視模型清單、倍率、上下文與模型對應](../assets/screenshots/models.png)

### 報表

概覽：請求數 / 成功率 / Token 總量 / Top 模型與帳號、供應商排名，附用量環圈圖與 365 天活躍熱力圖：

![報表概覽：統計卡片、Top 帳號 / 供應商、模型與供應商用量環圈圖](../assets/screenshots/report-overview.png)

趨勢：24 小時**快取命中率**（折線）與 **Token 消耗**（面積）雙軸同圖，底部依天 Token 長條圖：

![報表趨勢：快取命中率與 Token 消耗雙軸趨勢、依天 Token 長條圖](../assets/screenshots/report-trends.png)

## 🔧 開發與建置

### 環境需求

- Rust >= 1.77 與 Tauri 2 工具鏈（編譯桌面端本體；Windows 上還需 WebView2 執行環境）
- Node.js >= 18.17（僅用來執行 `npm run tauri:*` 與前端建置腳本，桌面端執行時不依賴 Node）
- 目標平台：Windows / macOS / Linux 桌面端（各平台 bundle：Windows NSIS、macOS app+dmg、Linux deb+AppImage）與 Docker 伺服器

### 常用腳本

```bash
npm run tauri:install      # 安裝桌面端依賴（等價 npm --prefix desktop-tauri install）
npm run tauri:dev          # 開發模式調起桌面端（自動熱重載）
npm run tauri:build        # 建置桌面端安裝包

npm run build:icon         # 產生圖示來源圖（改圖示設計後執行，再跑 tauri icon）
```

打包產物為 `target/release/bundle/nsis/Agent2API_<版本>_x64-setup.exe`（3.0 版約 8.6 MB；`src-tauri/.cargo/config.toml` 把 cargo 的 `target-dir` 指到了專案根的 `target/`）。

macOS 的 `.dmg` **只能在 macOS 上建置**（`npm --prefix desktop-tauri run build -- --target universal-apple-darwin`），無法從 Windows 交叉編譯，因此正式產物由 CI 的 `build` 工作流產出（`macos-latest` + 雙目標合併）；Linux 桌面端包由同一工作流的 `linux-desktop` job 產出（deb + AppImage）；Docker 映像與容器內更新用的伺服器包由 `linux-server` job 產出（沿用 Dockerfile 的編譯路徑，包內執行檔與映像同源同工具鏈）。

## 📜 使用聲明

### 憑證與資料安全

帳號憑證在本機資料庫中加密保存，但匯出檔案、備份及密鑰檔仍可能包含敏感資料。請妥善保管，勿上傳公開倉庫、雲端硬碟或分享給他人。

### 無擔保

本專案依「現狀」提供，不保證可用性、穩定性、安全性或特定用途適用性。相關商標與服務的權利歸各自所有者；完整授權與免責條款見 [LICENSE](../LICENSE)。

## 📄 授權

本專案與供應商插件倉庫一致，採用 [GNU Affero General Public License v3.0（AGPL-3.0）](../LICENSE)。可依授權使用、修改與分發（包括商業用途），須保留版權及授權聲明並履行相應原始碼提供義務；透過網路向使用者提供修改版服務時，須向這些使用者提供該版本的對應原始碼。

## ⭐ Star History

<a href="https://star-history.com/#aimod-cc/Agent2API&Date">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=aimod-cc/Agent2API&type=Date&theme=dark" />
    <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=aimod-cc/Agent2API&type=Date" />
    <img alt="Star History Chart" src="https://api.star-history.com/svg?repos=aimod-cc/Agent2API&type=Date" />
  </picture>
</a>

<div align="center">

**[下載最新版](https://github.com/aimod-cc/Agent2API/releases/latest)** • **[問題回饋](https://github.com/aimod-cc/Agent2API/issues)**

如果這個專案對你有幫助，歡迎點一個 ⭐️ Star！

</div>
