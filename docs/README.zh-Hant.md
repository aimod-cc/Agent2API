<div align="center">

# Agent2API

**把多家 AI 桌面客戶端的登入狀態，變成一個本地 OpenAI 相容 API 閘道**

[简体中文](../README.md) | [English](./README.en.md) | **繁體中文** | [日本語](./README.ja.md) | [한국어](./README.ko.md) | [Português (BR)](./README.pt-BR.md)

[![license](https://img.shields.io/badge/license-MIT%20%2B%20附加聲明-green)](../LICENSE)
[![release](https://img.shields.io/github/v/release/Agent2API/agent2api?include_prereleases)](https://github.com/Agent2API/agent2api/releases/latest)
[![docker pulls](https://img.shields.io/docker/pulls/aimodcc/agent2api)](https://hub.docker.com/r/aimodcc/agent2api)
[![stars](https://img.shields.io/github/stars/Agent2API/agent2api?style=flat&color=yellow)](https://github.com/Agent2API/agent2api/stargazers)
[![platform](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux%20%7C%20Docker-blue)](https://github.com/Agent2API/agent2api/releases/latest)

[✨ 核心特性](#-核心特性) • [💬 社群與支援](#-社群與支援) • [🤝 支援的供應商](#-支援的供應商) • [🚀 快速開始](#-快速開始) • [📦 安裝與部署](./install.zh-Hant.md) • [📸 介面預覽](#-介面預覽) • [🔧 開發與建置](#-開發與建置) • [📜 使用聲明](#-使用聲明)

</div>

---

Agent2API 把多家 AI 桌面客戶端裡已登入的帳號接到一起，共用一套 OpenAI 相容介面。任何支援自訂 `base_url` 的客戶端，填上 `http://127.0.0.1:3065/v1` 就能直接呼叫這些帳號的模型額度 —— **不需要上游 API Key，也不需要改客戶端**。

## ✨ 核心特性

- 🔌 **OpenAI 相容** —— 客戶端填上 `base_url` 就能用，Chat / Responses / Messages 三種協定都支援
- 🏪 **內建 17 家供應商** —— WorkBuddy、小浣熊、CatPaw、Qoder、Trae 等開箱即用，也支援自訂供應商
- 🔁 **憑證自動續期** —— Token 到期自動換新，臨期憑證可批次刷新，模型目錄也不會過期
- 🪂 **多帳號輪轉** —— 依優先順序排隊，429 自動換帳號，限額冷卻期內不空等
- 🛡️ **出口代理** —— 每個帳號可配獨立出口，出站指紋按官方客戶端偽裝
- 🔑 **閘道安全** —— 多把閘道 Key 可分權、登入有人機驗證，沒配 Key 時預設拒絕請求
- 📊 **請求報表** —— 請求量、成功率、Token 消耗一覽，還有快取命中率與 365 天熱力圖
- ⏰ **排程任務** —— 自動簽到、憑證維護、模型目錄重新整理，開關與間隔在同一頁管理
- 💰 **餘額與用量** —— 帳號列內直接看餘額與用量，支援跨帳號並行查詢
- 🖥️ **四種形態** —— Windows / macOS / Linux 桌面端常駐系統匣、免裝 Node；Docker 版面板與閘道同連接埠（amd64 / arm64），介面六語

## ❤️ 贊助商

<a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme"><img src="../assets/sponsor-proxylane.png" alt="ProxyLane（贊助位）" width="100%" /></a>

<table>

<tr>
<td width="240"><a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme"><picture><source media="(prefers-color-scheme: dark)" srcset="../assets/proxylane-logo-for-dark-bg.png" /><img src="../assets/proxylane-logo-for-light-bg.png" alt="ProxyLane" width="220" /></picture></a></td>
<td>感謝 <a href="https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=github_readme">ProxyLane</a> 贊助了本專案！ProxyLane 提供 AI 帳號專用的原生靜態住宅 IP（Static Residential ISP）：IP 來自真實家庭寬頻營運商，Scamalytics 低詐欺評分，每個 IP 獨享，不限流量，$3.90 起。在「網路代理」頁給每個帳號配一個出口即可。首次購買使用優惠碼 <code>AGENT2API25</code> 享 75 折。</td>
</tr>

</table>

## 💬 社群與支援

- **QQ 群**：Agent2API 交流群 —— **群 1（`1127656296`）即將滿員，請優先加群 2（`1130894674`）**；在 QQ 裡搜尋群號即可加入。
- **Telegram 群**：<https://t.me/+9gM8YPvgRL02NGRl>
- **Discord 群**：<https://discord.gg/z9GzSJMWp>
- **💖 贊助**：如果 Agent2API 幫到了你，歡迎[請作者喝杯咖啡](./SPONSOR.md)；贊助完全自願，不解鎖任何功能。
- **🤝 贊助 / 商務合作**：想贊助本專案，或想讓你的產品出現在這裡？寄信到 [541675139@qq.com](mailto:541675139@qq.com)。

## 🤝 支援的供應商

支援情況一覽（✓ 支援 · ✗ 不支援 · — 無此概念）：

| 平台 | LLM 請求 | Token 自動續期 | 模型清單 | 餘額查詢 | 簽到 | 領取類 |
| --- | :--: | :--: | :--: | :--: | :--: | :--: |
| WorkBuddy 中國版 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| WorkBuddy 國際版 | ✓ | ✓ | ✓ | ✓ | ✗ | — |
| 小浣熊 | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| CatPaw | ✓ | ✗ | ✓ | ✓ | ✗ | — |
| AutoClaw（中國版 / 國際版） | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| Qoder | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| Cline（Free / Pass） | ✓ | ✓ | ✓ | ✓ | — | — |
| Accio（國際版 / 中國版） | ✓ | ✓ | ✓ | ✓ | — | — |
| ZCode（中國版 / 國際版） | ✓ | ✗ | ✗ | ✓ | — | ✓ |
| CodeArts | ✓ | ✓ | ✓ | ✓ | — | ✓ |
| Trae | ✓ | ✓ | ✓ | ✓ | — | — |
| Loomy（訊飛） | ✓ | ✗ | ✓ | ✓ | ✓ | — |
| KukuAI（百度文庫） | ✓ | ✗ | ✓ | ✓ | ✓ | — |
| MonkeyCode（長亭科技，中國版 / 國際版） | ✓ | ✗ | ✓ | — | — | — |
| Command Code | ✓ | ✗ | ✓ | — | — | — |
| Antigravity（Google，Gemini / Claude / GPT-OSS） | ✓ | ✓ | ✓ | — | — | — |
| OfficeAce（華為雲果辦 / OfficeClaw） | ✓ | ✓ | ✓ | ✓ | ✓ | — |
| OrcaTerm（騰訊雲遨馳終端） | ✓ | ✓ | ✓ | ✓ | — | — |
| 智譜清言 | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| 自訂供應商 | ✓ | — | ✓ | — | — | — |

> 註：模型清單的 ✓ 表示支援遠端自動拉取，部分平台另有內建靜態清單備援；ZCode 僅有內建靜態清單。Token 續期欄 ✗ 表示該平台沒有可用的續期介面（帳號在有效期內不受影響）。Qoder 簽到僅中國版可用。
>
> **智譜清言**是本專案**唯一支援「訪客模式」**的一家：不需要登入，點一下就能換到一份可直接對話的憑證（免費額度由上游按裝置發放）。代價是訪客帳號沒有積分體系 —— 上表裡的「餘額查詢 / 簽到 / 領取類」，以及**任務中心的新手任務與每日任務**，都只對**登入帳號**有效（貼上權杖或匯入客戶端登入態），訪客會被如實跳過。

`/v1/chat/completions`、`/v1/responses`、`/v1/messages`（含 `count_tokens`）與 `/v1/models` 對所有平台一視同仁；模型對應、全域優先順序佇列、429 降級、出口代理、出站指紋脫敏與請求報表同樣對全平台通用。

> **本專案僅供學習與交流使用。** 它以非官方客戶端形態重用你自己帳號的登入狀態，可能不符合上游服務的使用條款，風險（含帳號被風控、封鎖）自負；禁止商用或繞過計費。詳見[使用聲明](#-使用聲明)與 [LICENSE](../LICENSE)。
>
> 本專案與各上游廠商及其官方產品均無關（名單見[使用聲明](#-使用聲明)）；介面形態來自對各家客戶端通訊的觀察，上游隨時可能調整。

## 🚀 快速開始

三種形態，依你的環境挑一個；**完整的安裝、升級、解除安裝與排錯見 [安裝與部署](./install.zh-Hant.md)**。

| 形態 | 產物 | 介面 |
| --- | --- | --- |
| **桌面端**（Windows / macOS / Linux） | `Agent2API_<版本>_x64-setup.exe`、`_universal.dmg`、`_amd64.deb`、`_amd64.AppImage` | 應用程式視窗 |
| **伺服器端**（Windows Server / Linux，無 GUI） | `agent2api-server-windows-amd64.zip`、`agent2api-server-linux-<arch>.tar.gz` | 瀏覽器面板 |
| **Docker** | `aimodcc/agent2api:latest`（amd64 / arm64） | 瀏覽器面板 |

**桌面端**：裝完啟動 → 「帳號」頁新增帳號 → 客戶端 `base_url` 填 `http://127.0.0.1:3065/v1`（`api_key` 隨便填，例如 `sk-local`）。無需安裝 Node 或任何其它執行環境，閘道跑在應用程式行程內，常駐系統匣。

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

### 簽到中心

![簽到中心頁：簽到總覽與每日簽到分組](../assets/screenshots/checkin-center.png)

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

### 僅供學習與交流

本專案是一個用於學習 HTTP 反向代理、SSE 串流透傳、多上游協定適配與桌面端打包（Tauri）等技術主題的實踐專案，**僅供個人學習與研究使用**。它不是官方產品，與騰訊公司及 WorkBuddy / CodeBuddy、美團及 CatPaw、商湯及小浣熊、智譜及 AutoClaw / autoglm、阿里巴巴及 Qoder / Accio、華為雲及 CodeArts / OfficeAce、字節跳動及 Trae、科大訊飛及 Loomy、百度文庫及 KukuAI、長亭科技及 MonkeyCode、Command Code、Google 及 Antigravity 均無任何關聯，未獲得其授權、認可或贊助。

### 關於反向代理行為

本專案實作的是本地反向代理：在你自己的機器上重用你自己帳號的登入狀態，把請求轉發到官方上游閘道。它不破解、不繞過任何付費或權限校驗，使用的額度始終來自你自己帳號本就擁有的配額。但需要明確的是，這種「以非官方客戶端形態重用登入狀態」的轉發方式，**可能不符合上游服務的使用者條款或使用約定**；是否使用、由此產生的一切後果（包括但不限於帳號被限流、被風控、被凍結或封鎖），均由使用者自行承擔。

### 禁止用途

禁止將本專案用於任何商業用途、二次分發牟利、批次帳號營運、繞過上游計費或配額限制、或其他違反當地法律法規的活動。如需在生產環境或商業場景中呼叫相關模型服務，請使用官方管道與官方 API。

### 憑證與資料風險

本專案會把帳號憑證（`accessToken` / `refreshToken` 等）以**明文**形式保存在本機設定目錄（預設 `~/.agent2api/`）中，匯出功能產生的檔案同樣包含明文憑證。請自行妥善保管，切勿提交到公開倉庫、上傳到雲端硬碟或分享給他人。因憑證洩露造成的損失由使用者自行承擔。

### 無擔保與權利通知

本專案依「現狀」提供，作者不對其可用性、穩定性、安全性或對特定用途的適用性作任何承諾。上游介面隨時可能變更，本專案可能隨時失效而不再維護。完整條款見 [LICENSE](../LICENSE)。本專案中的介面形態、協定欄位等資訊來自對各官方客戶端公開網路通訊的觀察與整理，相關商標與服務的權利歸其各自所有者；若權利方認為本專案存在不當之處，請聯絡作者，將及時調整或刪除。

## 📄 授權

本專案基於 [MIT License](../LICENSE)，可自由使用、修改與分發，須保留版權聲明。

需要留意的是：LICENSE 正文之後附有一份**使用聲明**，其中第 3 條在 MIT 之上**追加了限制**（禁止商業用途、禁止二次分發牟利、禁止批次帳號營運）。因此本專案**不是**純粹的 MIT 專案——**MIT 條款與使用聲明共同構成完整的授權與使用約定**，兩者對同一行為給出不同結論時以更嚴格的一方為準。這也是 `Cargo.toml` 用 `license-file` 指向 LICENSE、而不宣告 SPDX `"MIT"` 的原因。

## ⭐ Star History

<a href="https://star-history.com/#Agent2API/agent2api&Date">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=Agent2API/agent2api&type=Date&theme=dark" />
    <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=Agent2API/agent2api&type=Date" />
    <img alt="Star History Chart" src="https://api.star-history.com/svg?repos=Agent2API/agent2api&type=Date" />
  </picture>
</a>

<div align="center">

**[下載最新版](https://github.com/Agent2API/agent2api/releases/latest)** • **[問題回饋](https://github.com/Agent2API/agent2api/issues)**

如果這個專案對你有幫助，歡迎點一個 ⭐️ Star！

</div>
