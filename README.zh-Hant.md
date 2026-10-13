# Agent2API · 多供應商本地閘道

[简体中文](./README.md) | [English](./README.en.md) | **繁體中文** | [日本語](./README.ja.md) | [한국어](./README.ko.md) | [Português (BR)](./README.pt-BR.md)

把多家 AI 桌面客戶端的登入狀態包裝成本地 **OpenAI 相容 API 閘道**，統一暴露一個 `base_url`，附帶多供應商帳號管理、模型管理（啟停 / 刪除 / 對應）、出站指紋脫敏、出口代理與請求報表，並提供一個開箱即用的 Tauri 桌面端。任何支援自訂 `base_url` 的 OpenAI 客戶端都能以 `http://127.0.0.1:3065/v1` 為端點呼叫這幾家的模型額度——不需要 API Key，不需要改客戶端原始碼。

各平台的反向代理能力一覽（✓ 支援 · ✗ 不支援 · — 無此概念或不適用）：

| 平台 | LLM 請求 | Token 自動續期 | 模型清單（遠端重新整理） | 餘額查詢 | 簽到 | 領取類 |
| --- | :--: | :--: | :--: | :--: | :--: | :--: |
| WorkBuddy 中國版 | ✓ | ✓ | ✓ 遠端 + 靜態備援 | ✓ | ✓ 每日簽到 | — |
| WorkBuddy 國際版 | ✓ | ✓ | ✓ 遠端 + 靜態備援 | ✓ | ✗ 無簽到活動 | — |
| 小浣熊 | ✓ | ✓ | ✓ 遠端 + 靜態備援 | ✓ | ✓ 桌面登入積分 | — |
| CatPaw | ✓ | ✗ 無續期機制 | ✓ 遠端 + 靜態備援 | ✓ | ✗ | — |
| AutoClaw（中國版 / 國際版） | ✓ | ✓ | ✓ 遠端 + 靜態備援 | ✓ | ✓ 每日簽到 | — |
| Qoder | ✓ | ✓ | ✓ 遠端（依地區）+ 靜態備援 | ✓ | ✓ 僅中國版 | — |
| Cline（Free / Pass） | ✓ | ✓ | ✓ 遠端 + 靜態備援 | ✓ | — | — |
| Accio（國際版 / 中國版） | ✓ | ✓ | ✓ 遠端 + 靜態備援 | ✓ 用量百分比 | — | — |
| ZCode（中國版 / 國際版） | ✓ | ✗ | ✗ 靜態清單 | ✓ 套餐餘額 | — | ✓ 限時套餐（手動） |
| CodeArts | ✓ | ✓ 一次性輪換 | ✓ 遠端（三來源合併） | ✓ 兩份帳 | — | ✓ 每日福利（手動） |
| Trae | ✓ | ✓ 一次一換 | ✓ 僅遠端 | ✓ 兩份帳 | — | — |
| Loomy（訊飛） | ✓ | ✗ 無續期介面 | ✓ 僅遠端 | ✓ 兩份積分帳 | ✓ 每日贈送積分重新整理 | — |
| KukuAI（百度文庫） | ✓ | ✗ 無續期介面 | ✓ 遠端 + 靜態備援 | ✓ 積分餘額 | ✓ 每日簽到（免費積分） | — |
| MonkeyCode（長亭科技，中國版 / 國際版） | ✓ | ✗ 無續期介面 | ✓ 僅遠端 | — | — | — |
| Command Code | ✓ | ✗ 靜態 API Key | ✓ 遠端 + 靜態備援 | — | — | — |
| Antigravity（Google，Gemini） | ✓ | ✓ | ✓ 遠端 + 靜態備援 | — | — | — |
| OfficeAce（華為雲果辦 / OfficeClaw） | ✓ | ✓ 控制面自動續期 | ✓ 僅遠端 | ✓ 積分餘額 | ✓ 每日獎勵（簽到 + 新人禮） | — |
| 自訂供應商 | ✓ Chat 透傳 / Responses / Anthropic | — | ✓ 手動登記 + 伺服器端拉取 | — | — | — |

三條對話協定入口（`/v1/chat/completions`、`/v1/responses`、`/v1/messages`，另含 `/v1/messages/count_tokens`）與 `/v1/models` 對所有平台一視同仁，差異只在各家上游能不能做到表裡那些事；模型對應、全域優先順序佇列、429 降級、出口代理、出站指紋脫敏與請求報表同樣對全平台通用。

> **本專案僅供學習與交流使用。** 它以非官方客戶端形態重用你自己帳號的登入狀態，可能不符合上游服務的使用條款，風險（含帳號被風控、封鎖）自負；禁止商用或繞過計費。詳見[使用聲明](#使用聲明)與 [LICENSE](./LICENSE)。
>
> 本專案與各上游廠商及其官方產品均無關（名單見[使用聲明](#使用聲明)）；介面形態來自對各家客戶端通訊的觀察，上游隨時可能調整。

---

## 目錄

- [快速開始](#快速開始)
- [Docker 部署](#docker-部署)
- [介面預覽](#介面預覽)
- [專案結構](#專案結構)
- [開發與建置](#開發與建置)
- [使用聲明](#使用聲明)
- [授權](#授權)

---

## 快速開始

從 Releases 下載安裝包（NSIS，簡體中文，預設裝到 `C:\Program Files\Agent2API`，安裝時需要管理員授權），安裝後啟動即可，**無需安裝 Node 或任何其它執行環境**。

1. 首次啟動即在應用程式行程內啟動本機閘道（連接埠 3065）並開啟主視窗；若偵測到舊版本的資料目錄或資料檔案，會彈窗提示遷移，依指引操作即可。
2. 點「帳號」頁的「新增帳號」，選供應商後依彈窗提示完成登入或填寫憑證即可（方式以彈窗為準：網頁登入 / 手機驗證碼 / 貼上憑證 / 匯入本機登入狀態）。
3. 把 OpenAI 客戶端的 `base_url` 填成 `http://127.0.0.1:3065/v1`，`api_key` 隨便填（例如 `sk-local`，未啟用鑑權時伺服器端不校驗）。

關閉視窗預設最小化到系統匣，閘道繼續在背景轉發；徹底結束請在系統匣圖示按右鍵選「結束」。

### 驗證

閘道起來後，用 curl 確認連線性（`model` 填 `GET /v1/models` 裡任一實際存在的名字）：

```bash
curl http://127.0.0.1:3065/health
curl http://127.0.0.1:3065/v1/models

curl http://127.0.0.1:3065/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model":"deepseek-v4.1-flash","messages":[{"role":"user","content":"你好"}],"stream":true}'
```

客戶端接入範例（Python SDK）：

```python
from openai import OpenAI

client = OpenAI(base_url="http://127.0.0.1:3065/v1", api_key="sk-local")
resp = client.chat.completions.create(
    model="deepseek-v4.1-flash",     # 清單裡有哪家就是哪家，見 GET /v1/models
    messages=[{"role": "user", "content": "你好"}],
)
print(resp.choices[0].message.content)
```

**瀏覽器裡的頁面**（自建 Web UI 等）用 `fetch` 直連會被跨來源預檢拒絕——閘道預設不回應 CORS，預檢落到 API Key 校驗上得 401。兩種解法：① 設定頁「安全 → 閘道跨來源存取」開啟（來源 `*`；此時任何網頁都能借本機閘道打上游，建議同時設定「閘道 Key」）；② 頁面與閘道同來源——本地靜態服務託管頁面並把 `/v1` 反向代理到 `127.0.0.1:3065`。

### 區域網路存取

預設只監聽 `127.0.0.1`。開啟「設定 → 一般 → 區域網路存取」後改聽所有網路介面卡，區域網路裝置把 API 位址指向本機 IP 即可共用（介面會給出完整位址）。出於安全，開啟前必須註冊面板管理員：管理介面要求管理員工作階段或閘道 Key，無可用 Key 時轉發介面同樣拒絕服務（開啟流程會自動補一把「預設」Key）；也可選擇同時開放網頁面板（預設關閉）。變更重新啟動後生效。

---

## Docker 部署

```bash
docker run -d --name agent2api --restart unless-stopped \
  -p 3065:3065 -v ./data:/data \
  aimodcc/agent2api:latest
```

瀏覽器開啟 `http://<主機>:3065`，首次進入會引導**註冊管理員帳號**（後續登入用它）；登入後在「閘道 Key」頁建立一把 API Key 給客戶端用 —— `http://<主機>:3065/v1` 即 OpenAI 相容端點，未建立 Key 前拒絕轉發，建立第一把後自動恢復。所有狀態（SQLite 資料庫 / 設定 / 日誌）都落在 `./data` 一個卷裡。

compose 使用者（amd64 / arm64 都有映像）：

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

環境變數（都可選，不需要預置任何東西）：

| 變數 | 說明 |
| --- | --- |
| `AGENT2API_ADMIN_USER` + `AGENT2API_ADMIN_PASSWORD` | 預置管理員帳號密碼（密碼明文填，啟動時自動轉雜湊）；不填走面板註冊 |
| `AGENT2API_PANEL_PORT` | 面板分連接埠：設定後面板（介面 + `/api/*`）單獨監聽該連接埠，公網只映射主連接埠即可把管理面留在內網（面板連接埠綁定迴環位址，寫 `127.0.0.1:3066:3066`） |
| `AGENT2API_HOST` / `AGENT2API_PROXY_PORT` | 監聽位址（預設 `0.0.0.0`）/ 連接埠（預設 `3065`） |
| `AGENT2API_ALLOW_NO_KEY` | 設 `1` 關閉 fail-closed（未設 Key 也放行 `/v1`，僅限純內網） |
| `AGENT2API_CAPTCHA_ENABLED` | 登入頁人機驗證元件環境變數：預設為 `1` 開啟，`0` 為關閉 |

從原始碼建置：複製本專案後 `docker compose up -d --build`（映像裡只有閘道與面板，不含 Rust 工具鏈）。

**網頁端功能差異**（源於「沒有本機桌面客戶端」）：網頁登入（WorkBuddy / Qoder / Cline）、手機驗證碼、貼上憑證完全可用；需要本機回呼的網頁登入（AutoClaw / CatPaw / Accio / CodeArts / Trae）與「匯入本機登入狀態」不可用，請改用貼上憑證。

---

## 介面預覽

### 帳號

全部帳號排在同一條佇列裡（左起第二欄是優先順序），可逐條啟停；限額冷卻、有效期與餘額都在列內顯示，餘額不足預設依閾值跳過。

![帳號管理頁：全域佇列、依模型限額冷卻、有效期與餘額](./assets/screenshots/accounts.png)

新增帳號先選供應商再登入；同一家可保存多版帳號（例如 WorkBuddy 中國版 / 國際版）：

![新增帳號：選擇供應商與版本，然後走網頁登入](./assets/screenshots/add-account.png)

### 報表

概覽：請求數 / 成功率 / Token 總量 / Top 模型與帳號、供應商排名，附用量環圈圖與 365 天活躍熱力圖：

![報表概覽：統計卡片、Top 帳號 / 供應商、模型與供應商用量環圈圖](./assets/screenshots/report-overview.png)

趨勢：24 小時**快取命中率**（折線）與 **Token 消耗**（面積）雙軸同圖，底部依天 Token 長條圖：

![報表趨勢：快取命中率與 Token 消耗雙軸趨勢、依天 Token 長條圖](./assets/screenshots/report-trends.png)

### 排程任務

背景任務統一在這一頁（開關 / 間隔 / 上次結果 / 立即執行）；清單保存在 `~/.agent2api/config.json` 的 `scheduledTasks`，變更即時生效。

![排程任務頁：自動簽到、憑證維護、模型目錄重新整理等背景任務的開關與間隔](./assets/screenshots/scheduled-tasks.png)

---

## 專案結構

閘道與桌面端都在 `desktop-tauri/`：後端是 `src-tauri/` 下的 Rust 行程內 HTTP 伺服器，前端是 `ui/` 下的原生 HTML/CSS/JS。

```
agent2api/
├─ desktop-tauri/
│  ├─ src-tauri/
│  │  ├─ server/                 閘道本體 crate（agent2api-server，獨立編譯：
│  │  │                          桌面端與 headless 二進位共用；src/server/ 下
│  │  │                          的實作與 bin/agent2api-server.rs 無 GUI 依賴）
│  │  │  ├─ mod.rs               服務組裝：ServerState、啟動、停機、啟動遷移
│  │  │  ├─ http.rs              路由表、CORS、API Key 中介層、body 限制、headless 靜態託管
│  │  │  ├─ config.rs / logging.rs / logs_store.rs / errors.rs
│  │  │  ├─ config_migration.rs  1.x 設定目錄遷移（~/.workbuddy-proxy → ~/.agent2api，啟動第一步）
│  │  │  ├─ request_stats.rs + request_stats/   統計的時鐘視窗、寫入、彙總與裁剪
│  │  │  ├─ core/
│  │  │  │  ├─ providers/        ★ 多供應商層（本改造的核心）
│  │  │  │  │  ├─ mod.rs        ProviderKind（內建各家；Cline 依額度池拆兩池、Accio 與
│  │  │  │  │  │                ZCode 依地區拆兩地）+ PROVIDERS 註冊表 + id 互查
│  │  │  │  │  ├─ adapter.rs    ProviderAdapter trait + adapter_for + implemented_kinds
│  │  │  │  │  ├─ router.rs     模型名 → 候選 provider 集合（彙總目錄）
│  │  │  │  │  ├─ catalog.rs    彙總模型目錄（清單合併 / 同名去重 / 可用性判定）
│  │  │  │  │  ├─ catalog_cache.rs  各家遠端清單的持久化快取（重啟後讀回，不再回落到內建清單）
│  │  │  │  │  ├─ refresh_flight.rs  憑證重新整理的去重單飛
│  │  │  │  │  ├─ workbuddy.rs  WorkBuddy 適配器（標頭集合 / system 注入 / 6004 / 11-128）
│  │  │  │  │  ├─ raccoon/      小浣熊：mod / models / credentials / jwt / oauth / balance
│  │  │  │  │  ├─ catpaw/       CatPaw：adapter（is_stateful）/ conversation（輪次狀態機）/
│  │  │  │  │  │                turn_executor / prepare / decision（輪次判定）/ fingerprint /
│  │  │  │  │  │                registry/（工作階段註冊表：表與句柄 / 帳號身分 / 作廢）/
│  │  │  │  │  │                messages / blocks / tools / openai（翻譯層）/
│  │  │  │  │  │                upstream_http / image_compress / models / credentials / balance
│  │  │  │  │  ├─ autoclaw/     智譜 autoglm（中國版 + 國際版兩家）：region（兩地域名與身分）/
│  │  │  │  │  │                adapter / credentials / refresh / crypto / models /
│  │  │  │  │  │                balance / login（手機驗證碼，僅中國版）/
│  │  │  │  │  │                oauth（Zai / Google 網頁登入，僅國際版）/ checkin（每日簽到任務）
│  │  │  │  │  ├─ qoder/        Qoder：adapter / endpoints（兩站位址）/ oauth（裝置授權）/
│  │  │  │  │  │                auth / cosy（COSY 簽名與 body 編碼）/ protocol（信封解碼）/
│  │  │  │  │  │                chat（工作階段式轉發）/ stream / machine（PKCE 與機器識別）/
│  │  │  │  │  │                credentials / refresh / models / balance
│  │  │  │  │  ├─ cline/        Cline：adapter（Bearer + 產品面標頭）/ credentials（workos: 前綴
│  │  │  │  │  │                + 桌面端登入狀態 + 姓名解析）/ login（WorkOS 裝置授權）/
│  │  │  │  │  │                refresh（單飛續期）/ models（兩額度池 + 預設對應種子）/
│  │  │  │  │  │                balance（credit 餘額，微 credit ÷1e6）
│  │  │  │  │  ├─ accio/        Accio（國際版 + 中國版兩家）：endpoints（兩地區與端點）/
│  │  │  │  │  │                credentials / auth / refresh（單飛續期）/
│  │  │  │  │  │                oauth（PKCE 網頁登入 + loopback 回呼）/
│  │  │  │  │  │                models（靜態備援 + /api/llm/config/v2）/
│  │  │  │  │  │                protocol（OpenAI ↔ ADK 的 Gemini 風格信封）/
│  │  │  │  │  │                chat（工作階段式轉發）/ stream（ADK SSE 解包）/ balance
│  │  │  │  │  ├─ zcode/        ZCode（智譜 Z.AI，中國版 + 國際版兩家）：region（兩地域名與身分）/
│  │  │  │  │  │                adapter（無狀態、依地區參數化）/ credentials（權杖 + 套餐 JWT）/
│  │  │  │  │  │                oauth（CLI 輪詢登入）/ coding_key（換推理用 API Key）/ models（靜態表）/
│  │  │  │  │  │                balance（套餐餘額）/ plan + claim（活動套餐通道與限時領取）/
│  │  │  │  │  │                captcha（人機驗證權杖池）/ reasoning（GLM-5.3 思考預算）/
│  │  │  │  │  │                zcode_system.json（系統提示詞）
│  │  │  │  │  ├─ codearts/     CodeArts（華為雲碼道）：signer（華為雲 SDK-HMAC-SHA256，
│  │  │  │  │  │                與參考實作逐位元組對帳）/ credentials / dpop（ES256 DPoP proof）/
│  │  │  │  │  │                oauth（PKCE 網頁登入 + loopback 回呼）/ refresh（單飛續期）/
│  │  │  │  │  │                session（chat-session 心跳與每帳號並行准入）/ chat（工作階段式轉發）/
│  │  │  │  │  │                stream_fault（HTTP 200 的串流內錯誤信封）/ redact（錯誤體脫敏）/
│  │  │  │  │  │                models（agent / builtin / 福利閘道三源合併）/
│  │  │  │  │  │                balance（訂閱統計 + 福利閘道兩份帳）/
│  │  │  │  │  │                welfare（每日福利領取：冪等鍵先寫入磁碟、回讀二次確認）
│  │  │  │  │  ├─ trae/         Trae（字節 AI IDE SOLO 通道）：credentials / device（裝置金鑰對）/
│  │  │  │  │  │                login + oauth（PKCE 網頁登入 + 換證候選）/ callback_server
│  │  │  │  │  │                （本機隨機連接埠回呼與雜訊過濾）/ refresh（單飛續期）/
│  │  │  │  │  │                payload（SOLO 信封白名單重建）/ headers（SOLO 標頭集合）/
│  │  │  │  │  │                stream（SSE→chunk 翻譯）/ forward（有狀態轉發）/
│  │  │  │  │  │                errors（錯誤分類與死設定名單）/ models（get_detail_param 目錄）/
│  │  │  │  │  │                usage（權益包 + 套餐 quota 兩份帳）/ profile（身分解析）
│  │  │  │  │  ├─ loomy/        Loomy（訊飛）：login（手機驗證碼）/ credentials（session 14 天、無續期介面）/
│  │  │  │  │  │                sign（複刻客戶端 HMAC-SHA1 簽名標頭）/ endpoints / client（整合閘道）/
│  │  │  │  │  │                models（/api/v1/models 遠端目錄，上游無內建備援清單）/
│  │  │  │  │  │                balance（永久積分 + 每日贈送兩份帳）/ checkin（每日首次登入重新整理贈送積分）
│  │  │  │  │  ├─ kuku/         KukuAI（百度文庫「庫庫 AI / GenFlowPro」，kuku.baidu.com）：
│  │  │  │  │  │                adapter（is_stateful）/ session（bdstoken/uinfo/uk 三件套快取）/
│  │  │  │  │  │                engine（STOKEN 換發）/ http / login（主站網頁登入 + 殼側收 Cookie）/
│  │  │  │  │  │                credentials（BDUSS Cookie）/ models（靜態備援 + 遠端重新整理）/
│  │  │  │  │  │                chat（建工作階段 → 分配算力 → SSE）/ balance（積分餘量）/
│  │  │  │  │  │                checkin（每日免費積分）
│  │  │  │  │  ├─ monkeycode/   MonkeyCode（長亭科技，中國版 + 國際版兩家）：region（兩站位址與身分）/
│  │  │  │  │  │                adapter / endpoints（路徑 / cookie 名 / interface_type→CLI 對應）/
│  │  │  │  │  │                client / credentials（session + imageId）/ login（貼上 session 與自動探索）/
│  │  │  │  │  │                models（兩站各一格清單，僅遠端）/ task（建任務）/ stream（WS 任務串流）/
│  │  │  │  │  │                translate（ACP 事件 → chat 幀，含工具自動批准與提問自動應答）
│  │  │  │  │  ├─ commandcode/  Command Code：adapter / endpoints / credentials / login /
│  │  │  │  │  │                fingerprint（決定性裝置指紋與 8h 回報）/ models /
│  │  │  │  │  │                plan（8 鍵信封與 params 重寫、session id 派生）
│  │  │  │  │  ├─ antigravity/  Antigravity（Google，Gemini）：oauth（refresh token 續期 + 單飛）/
│  │  │  │  │                   project（loadCodeAssist → onboardUser 探索）/ credentials /
│  │  │  │  │                   endpoints（三環境基址與標頭集合）/ login /
│  │  │  │  │                   models（fetchAvailableModels + 內建備援 + 上游真名對應）/ adapter
│  │  │  │  │  └─ officeace/    OfficeAce（華為雲果辦 / OfficeClaw）：oauth（PKCE + P-256 DPoP 自助登入）/
│  │  │  │  │                   signer（SDK-HMAC 與 V11-HMAC 兩套簽章 + 向量 fixtures）/ credentials（閘道器 Basic 與控制面臨時 AK/SK 兩層）/
│  │  │  │  │                   chat（無狀態 OpenAI 相容轉發）/ models（遠端目錄 + 每模型倍率）/
│  │  │  │  │                   probe（模型可用性探測，打不通的預設隱藏）/ subscription（訂閱 / 額度 / 獎勵）/
│  │  │  │  │                   onboarding / checkin / balance
│  │  │  │  ├─ protocol/        協定轉換層（各家 wire ↔ 標準 chat SSE 的請求/回應翻譯：
│  │  │  │  │                    Anthropic Messages / Responses / NDJSON（Command Code）/
│  │  │  │  │                    Gemini 信封（Antigravity），以及工具 plan 與歷史修復）
│  │  │  │  ├─ upstream/        轉發編排：全域帳號佇列迴圈（provider_loop）+ 傳送體處理
│  │  │  │  │                    （payload）+ SSE 透傳/聚合 + usage 旁路提取 + 非 chat 協定的
│  │  │  │  │                    翻譯流接入（translate：Anthropic / NDJSON / Gemini 三條並列）
│  │  │  │  ├─ account_store/   帳號儲存（全域優先順序、限額冷卻、各家新增與匯入）
│  │  │  │  ├─ models/          模型目錄底層（workbuddy 內建清單 + /v3/config 重新整理）
│  │  │  │  ├─ model_rules.rs   模型管理規則（停用 / 隱藏 / 對應 alias）
│  │  │  │  ├─ api_keys.rs      閘道 Key 清單（多把 Key，任一啟用即通過）
│  │  │  │  ├─ auth.rs / auth_http.rs / login.rs   工作階段、出口傳輸、無頭登入
│  │  │  │  │                    （login/ 下是各家特有的登入流程：catpaw 的
│  │  │  │  │                    loopback 回呼、qoder 的裝置授權）
│  │  │  │  ├─ routing.rs / billing/   帳號選路（全域優先順序 + 限額冷卻）/ 積分簽到營運
│  │  │  │  ├─ proxies.rs / clash.rs / egress.rs   出口代理與依出口快取 Client
│  │  │  │  ├─ sanitize.rs      出站指紋脫敏（硬編碼規則集：標頭剝離 + 模板句最小改寫）
│  │  │  │  ├─ prompt.rs        閘道自有系統提示詞（透傳 / 取代 / 追加三模式）
│  │  │  │  ├─ degrade.rs       內容攔截降級狀態機（撞上審核誤報後到次日 00:00 用中性提示詞）
│  │  │  │  ├─ credential_maintenance.rs  已過期 / 臨期憑證的批次重新整理
│  │  │  │  ├─ usage_query.rs     餘額 / 積分查詢（跨帳號並行 + 定時那一輪的快照）
│  │  │  │  ├─ scheduled_tasks.rs  間隔型排程任務註冊表與調度迴圈（開關 / 間隔 /
│  │  │  │  │                      上次結果；設定在 config.json 的 scheduledTasks）
│  │  │  │  └─ account_transfer.rs + account_transfer/ / auto_checkin.rs / update/
│  │  │  │                       匯入匯出（含身分歸一）/ 定時簽到 / 軟體更新
│  │  │  └─ api/                 各路由 handler（health/session/accounts/accounts_usage/
│  │  │                          chat/models/keys/model_manage/stats/logs/billing/
│  │  │                          sanitize/prompt/auto-checkin/scheduled-tasks/update/…）
│  │  ├─ lib.rs                 應用程式入口（設定目錄遷移 → 設定 → 系統匣 → 主視窗 → 啟動後端）
│  │  ├─ backend.rs              行程內伺服器生命週期
│  │  ├─ legacy_install.rs       舊「目前使用者」安裝的清理（目錄 / 捷徑 / 解除安裝項目 / 開機自啟；僅 release）
│  │  ├─ gateway.rs              殼側存取管理 API 的 HTTP 客戶端
│  │  ├─ login.rs / commands.rs  登入視窗與輪詢、暴露給前端的 invoke 命令
│  │  ├─ login_profile.rs        每次網頁登入獨享一個臨時 WebView2 資料目錄（登完即刪）
│  │  ├─ bridge.rs               注入 window.workbuddyDesktop 的橋接腳本
│  │  └─ update.rs / settings.rs / state.rs / tray.rs
│  ├─ ui/                        前端（原生 HTML/CSS/JS，無框架）
│  └─ src-tauri/tauri.conf.json  打包設定（NSIS）
├─ build/make-icon.mjs           產生應用程式圖示來源圖
├─ assets/screenshots/           README 配圖（介面截圖）
├─ Dockerfile / .dockerignore    headless 映像（多階段建置，只含閘道與面板）
├─ docker-compose.yml / .env.example   部署（單容器：面板 + 閘道同連接埠）
└─ package.json                  建置腳本入口（tauri:dev / tauri:build / build:icon）
```

---

## 開發與建置

### 環境需求

- Rust >= 1.77 與 Tauri 2 工具鏈（編譯桌面端本體；Windows 上還需 WebView2 執行環境）
- Node.js >= 18.17（僅用來執行 `npm run tauri:*` 與 `build/make-icon.mjs` 這些前端建置腳本，桌面端執行時不依賴 Node，也不會打包任何 Node 產物）

### 常用腳本

```bash
npm run tauri:install      # 安裝桌面端依賴（等價 npm --prefix desktop-tauri install）
npm run tauri:dev          # 開發模式調起桌面端（自動熱重載）
npm run tauri:build        # 建置桌面端安裝包

npm run build:icon         # 產生圖示來源圖（改圖示設計後執行，再跑 tauri icon）
```

根專案本身沒有執行期依賴，`package.json` 只提供上面這些快捷腳本入口。打包產物為 `target/release/bundle/nsis/Agent2API_<版本>_x64-setup.exe`（目前約 3.0 MB；`src-tauri/.cargo/config.toml` 把 cargo 的 `target-dir` 指到了專案根的 `target/`）。

---

## 使用聲明

### 僅供學習與交流

本專案是一個用於學習 HTTP 反向代理、SSE 串流透傳、多上游協定適配與桌面端打包（Tauri）等技術主題的實踐專案，**僅供個人學習與研究使用**。它不是官方產品，與騰訊公司及 WorkBuddy / CodeBuddy、美團及 CatPaw、商湯及小浣熊、智譜及 AutoClaw / autoglm、阿里巴巴及 Qoder / Accio、華為雲及 CodeArts / OfficeAce、字節跳動及 Trae、科大訊飛及 Loomy、百度文庫及 KukuAI、長亭科技及 MonkeyCode、Command Code、Google 及 Antigravity 均無任何關聯，未獲得其授權、認可或贊助。

### 關於反向代理行為

本專案實作的是本地反向代理：在你自己的機器上重用你自己帳號的登入狀態，把請求轉發到官方上游閘道。它不破解、不繞過任何付費或權限校驗，使用的額度始終來自你自己帳號本就擁有的配額。但需要明確的是，這種「以非官方客戶端形態重用登入狀態」的轉發方式，**可能不符合上游服務的使用者條款或使用約定**；是否使用、由此產生的一切後果（包括但不限於帳號被限流、被風控、被凍結或封鎖），均由使用者自行承擔。

### 禁止用途

禁止將本專案用於任何商業用途、二次分發牟利、批次帳號營運、繞過上游計費或配額限制、或其他違反當地法律法規的活動。如需在生產環境或商業場景中呼叫相關模型服務，請使用官方管道與官方 API。

### 憑證與資料風險

本專案會把帳號憑證（`accessToken` / `refreshToken` 等）以**明文**形式保存在本機設定目錄（預設 `~/.agent2api/`）中，匯出功能產生的檔案同樣包含明文憑證。請自行妥善保管，切勿提交到公開倉庫、上傳到雲端硬碟或分享給他人。因憑證洩露造成的損失由使用者自行承擔。

### 無擔保與權利通知

本專案依「現狀」提供，作者不對其可用性、穩定性、安全性或對特定用途的適用性作任何承諾。上游介面隨時可能變更，本專案可能隨時失效而不再維護。完整條款見 [LICENSE](./LICENSE)。本專案中的介面形態、協定欄位等資訊來自對各官方客戶端公開網路通訊的觀察與整理，相關商標與服務的權利歸其各自所有者；若權利方認為本專案存在不當之處，請聯絡作者，將及時調整或刪除。

---

## 授權

本專案基於 [MIT License](./LICENSE)，可自由使用、修改與分發，須保留版權聲明。

需要留意的是：LICENSE 正文之後附有一份**使用聲明**，其中第 3 條在 MIT 之上**追加了限制**（禁止商業用途、禁止二次分發牟利、禁止批次帳號營運）。因此本專案**不是**純粹的 MIT 專案——**MIT 條款與使用聲明共同構成完整的授權與使用約定**，兩者對同一行為給出不同結論時以更嚴格的一方為準。這也是 `Cargo.toml` 用 `license-file` 指向 LICENSE、而不宣告 SPDX `"MIT"` 的原因。

---

## Star History

<a href="https://star-history.com/#aimod-cc/agent2api&Date">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=aimod-cc/agent2api&type=Date&theme=dark" />
    <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=aimod-cc/agent2api&type=Date" />
    <img alt="Star History Chart" src="https://api.star-history.com/svg?repos=aimod-cc/agent2api&type=Date" />
  </picture>
</a>
