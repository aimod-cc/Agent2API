# Agent2API · 多提供商本地网关

**简体中文** | [English](./README.en.md) | [繁體中文](./README.zh-Hant.md) | [日本語](./README.ja.md) | [한국어](./README.ko.md) | [Português (BR)](./README.pt-BR.md)

把多家 AI 桌面客户端的登录态变成一个本地 **OpenAI 兼容 API 网关**：统一暴露一个 `base_url`，带多账号管理、模型管理、出网代理与请求报表，并附一个开箱即用的 Tauri 桌面端。任何支持自定义 `base_url` 的 OpenAI 客户端都能以 `http://127.0.0.1:3065/v1` 调用这几家的模型额度 —— 不需要上游 API Key，不需要改客户端。

支持情况一览（✓ 支持 · ✗ 不支持 · — 无此概念）：

| 平台 | LLM 请求 | Token 自动续期 | 模型列表 | 余额查询 | 签到 | 领取类 |
| --- | :--: | :--: | :--: | :--: | :--: | :--: |
| WorkBuddy 国内版 | ✓ | ✓ | ✓ 远程 + 静态兜底 | ✓ | ✓ 每日签到 | — |
| WorkBuddy 国际版 | ✓ | ✓ | ✓ 远程 + 静态兜底 | ✓ | ✗ 无签到活动 | — |
| 小浣熊 | ✓ | ✓ | ✓ 远程 + 静态兜底 | ✓ | ✓ 桌面登录积分 | — |
| CatPaw | ✓ | ✗ 无刷新机制 | ✓ 远程 + 静态兜底 | ✓ | ✗ | — |
| AutoClaw（国内版 / 国际版） | ✓ | ✓ | ✓ 远程 + 静态兜底 | ✓ | ✓ 每日签到 | — |
| Qoder | ✓ | ✓ | ✓ 远程（按地区）+ 静态兜底 | ✓ | ✓ 仅中国版 | — |
| Cline（Free / Pass） | ✓ | ✓ | ✓ 远程 + 静态兜底 | ✓ | — | — |
| Accio（国际版 / 国内版） | ✓ | ✓ | ✓ 远程 + 静态兜底 | ✓ 用量百分比 | — | — |
| ZCode（国内版 / 国际版） | ✓ | ✗ | ✗ 静态表 | ✓ 套餐余额 | — | ✓ 限时套餐（手动） |
| CodeArts | ✓ | ✓ 一次性轮换 | ✓ 远程（三源合并） | ✓ 两份账 | — | ✓ 每日福利（手动） |
| Trae | ✓ | ✓ 一次一换 | ✓ 仅远程 | ✓ 两份账 | — | — |
| Loomy（讯飞） | ✓ | ✗ 无续期接口 | ✓ 仅远程 | ✓ 两份积分账 | ✓ 每日赠送积分刷新 | — |
| KukuAI（百度文库） | ✓ | ✗ 无续期接口 | ✓ 远程 + 静态兜底 | ✓ 积分余额 | ✓ 每日签到（免费积分） | — |
| MonkeyCode（长亭科技，国内版 / 国际版） | ✓ | ✗ 无续期接口 | ✓ 仅远程 | — | — | — |
| Command Code | ✓ | ✗ 静态 API Key | ✓ 远程 + 静态兜底 | — | — | — |
| Antigravity（Google，Gemini） | ✓ | ✓ | ✓ 远程 + 静态兜底 | — | — | — |
| OfficeAce（华为云果办 / OfficeClaw） | ✓ | ✓ 控制面自动续期 | ✓ 仅远程 | ✓ 积分余额 | ✓ 每日奖励（签到 + 新人礼） | — |
| 自定义提供商 | ✓ Chat 透传 / Responses / Anthropic | — | ✓ 手动登记 + 服务端拉取 | — | — | — |

`/v1/chat/completions`、`/v1/responses`、`/v1/messages`（含 `count_tokens`）与 `/v1/models` 对所有平台一视同仁；模型映射、全局优先级队列、429 降级、出网代理、出站指纹脱敏与请求报表同样全平台通用。

> **本项目仅供学习与交流使用。** 它以非官方客户端形态复用你自己账号的登录态，可能不符合上游服务的使用条款，风险（含账号被风控、封禁）自负；禁止商用或绕过计费。详见[使用声明](#使用声明)与 [LICENSE](./LICENSE)。
>
> 本项目与各上游厂商及其官方产品均无关（名单见[使用声明](#使用声明)）；接口形态来自对各家客户端通信的观察，上游随时可能调整。

---

## 目录

- [快速开始](#快速开始)
- [Docker 部署](#docker-部署)
- [界面预览](#界面预览)
- [项目结构](#项目结构)
- [开发与构建](#开发与构建)
- [使用声明](#使用声明)
- [许可](#许可)

---

## 快速开始

从 Releases 下载安装包（NSIS，简体中文，默认装到 `C:\Program Files\Agent2API`，安装时需要管理员授权），安装后启动即可，**无需安装 Node 或任何其它运行时**。

1. 首次启动即在应用进程内启动本机网关（端口 3065）并打开主窗口；若检测到旧版本的数据目录或数据文件，会弹窗提示迁移，按指引操作即可。
2. 点「账号」页的「添加账号」，选提供商后按弹窗提示完成登录或填写凭证即可（方式以弹窗为准：网页登录 / 手机验证码 / 粘贴凭证 / 导入本机登录态）。
3. 把 OpenAI 客户端的 `base_url` 填成 `http://127.0.0.1:3065/v1`，`api_key` 随便填（例如 `sk-local`，未启用鉴权时服务端不校验）。

关闭窗口默认最小化到托盘，网关继续在后台转发；彻底退出请在托盘图标右键选「退出」。

### 验证

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

### 局域网访问

默认只监听 `127.0.0.1`。开启「设置 → 通用 → 局域网访问」后改听所有网卡，局域网设备把 API 地址指向本机 IP 即可共用（界面会给出完整地址）。出于安全，开启前必须注册面板管理员：管理接口要求管理员会话或网关 Key，无可用 Key 时转发接口同样拒绝服务（开启流程会自动补一把「默认」Key）；也可选择同时开放网页面板（默认关闭）。改动重启生效。

---

## Docker 部署

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

从源码构建：克隆本仓库后 `docker compose up -d --build`（镜像里只有网关与面板，不含 Rust 工具链）。

**网页端功能差异**（源于「没有本机桌面客户端」）：网页登录（WorkBuddy / Qoder / Cline）、手机验证码、粘贴凭证完全可用；需要本机回调的网页登录（AutoClaw / CatPaw / Accio / CodeArts / Trae）与「导入本机登录态」不可用，请改用粘贴凭证。

---

## 界面预览

### 账号

全部账号排在同一条队列里（左起第二列是优先级），可逐条启停；限额冷却、有效期与余额都在行内显示，余额不足默认按阈值跳过。

![账号管理页：全局队列、按模型限额冷却、有效期与余额](./assets/screenshots/accounts.png)

添加账号先选提供商再登录；同一家可保存多版账号（如 WorkBuddy 国内 / 国际版）：

![添加账号：选择提供商与版本，然后走网页登录](./assets/screenshots/add-account.png)

### 报表

概览：请求数 / 成功率 / Token 总量 / Top 模型与账号、提供商排名，附用量环图与 365 天活跃热力图：

![报表概览：统计卡片、Top 账号 / 提供商、模型与提供商用量环形图](./assets/screenshots/report-overview.png)

趋势：24 小时**缓存命中率**（折线）与 **Token 消耗**（面积）双轴同图，底部按天 Token 柱状图：

![报表趋势：缓存命中率与 Token 消耗双轴趋势、按天 Token 柱状图](./assets/screenshots/report-trends.png)

### 定时任务

后台任务统一在这一页（开关 / 间隔 / 上次结果 / 立即执行）；清单存在 `~/.agent2api/config.json` 的 `scheduledTasks`，改动即时生效。

![定时任务页：自动签到、凭证维护、模型目录刷新等后台任务的开关与间隔](./assets/screenshots/scheduled-tasks.png)

---

## 项目结构

后端是 `src-tauri/` 下的 Rust 进程内 HTTP 服务器，前端是 `ui/` 下的原生 HTML/CSS/JS。

<details>
<summary>完整目录树（点开）</summary>

```
agent2api/
├─ desktop-tauri/
│  ├─ src-tauri/
│  │  ├─ server/                 网关本体 crate（agent2api-server，独立编译：
│  │  │                          桌面端与 headless 二进制共用；src/server/ 下
│  │  │                          的实现与 bin/agent2api-server.rs 无 GUI 依赖）
│  │  │  ├─ mod.rs               服务组装：ServerState、启动、停机、启动迁移
│  │  │  ├─ http.rs              路由表、CORS、API Key 中间件、body 限制、headless 静态托管
│  │  │  ├─ config.rs / logging.rs / logs_store.rs / errors.rs
│  │  │  ├─ config_migration.rs  1.x 配置目录迁移（~/.workbuddy-proxy → ~/.agent2api，启动第一步）
│  │  │  ├─ request_stats.rs + request_stats/   统计的时钟窗口、写入、聚合与裁剪
│  │  │  ├─ core/
│  │  │  │  ├─ providers/        ★ 多提供商层（本改造的核心）
│  │  │  │  │  ├─ mod.rs        ProviderKind（内置各家；Cline 按额度池拆两池、Accio 与
│  │  │  │  │  │                ZCode 按地区拆两地）+ PROVIDERS 注册表 + id 互查
│  │  │  │  │  ├─ adapter.rs    ProviderAdapter trait + adapter_for + implemented_kinds
│  │  │  │  │  ├─ router.rs     模型名 → 候选 provider 集合（聚合目录）
│  │  │  │  │  ├─ catalog.rs    聚合模型目录（清单合并 / 同名去重 / 可用性判定）
│  │  │  │  │  ├─ catalog_cache.rs  各家远程清单的持久化缓存（重启后读回，不再回落到内置清单）
│  │  │  │  │  ├─ refresh_flight.rs  凭证刷新的单飞去重
│  │  │  │  │  ├─ workbuddy.rs  WorkBuddy 适配器（头集合 / system 注入 / 6004 / 11-128）
│  │  │  │  │  ├─ raccoon/      小浣熊：mod / models / credentials / jwt / oauth / balance
│  │  │  │  │  ├─ catpaw/       CatPaw：adapter（is_stateful）/ conversation（轮次状态机）/
│  │  │  │  │  │                turn_executor / prepare / decision（轮次判定）/ fingerprint /
│  │  │  │  │  │                registry/（会话注册表：表与句柄 / 账号身份 / 作废）/
│  │  │  │  │  │                messages / blocks / tools / openai（翻译层）/
│  │  │  │  │  │                upstream_http / image_compress / models / credentials / balance
│  │  │  │  │  ├─ autoclaw/     智谱 autoglm（国内版 + 国际版两家）：region（两地域名与身份）/
│  │  │  │  │  │                adapter / credentials / refresh / crypto / models /
│  │  │  │  │  │                balance / login（手机验证码，仅国内版）/
│  │  │  │  │  │                oauth（Zai / Google 网页登录，仅国际版）/ checkin（每日签到任务）
│  │  │  │  │  ├─ qoder/        Qoder：adapter / endpoints（两站地址）/ oauth（设备授权）/
│  │  │  │  │  │                auth / cosy（COSY 签名与体编码）/ protocol（信封解码）/
│  │  │  │  │  │                chat（会话式转发）/ stream / machine（PKCE 与机器标识）/
│  │  │  │  │  │                credentials / refresh / models / balance / risk（UMID 风控组件）
│  │  │  │  │  ├─ cline/        Cline：adapter（Bearer + 产品面头）/ credentials（workos: 前缀
│  │  │  │  │  │                + 桌面端登录态 + 姓名解析）/ login（WorkOS 设备授权）/
│  │  │  │  │  │                refresh（单飞续期）/ models（两额度池 + 默认映射种子）/
│  │  │  │  │  │                balance（credit 余额，微 credit ÷1e6）
│  │  │  │  │  ├─ accio/        Accio（国际版 + 国内版两家）：endpoints（两地区与端点）/
│  │  │  │  │  │                credentials / auth / refresh（单飞续期）/
│  │  │  │  │  │                oauth（PKCE 网页登录 + loopback 回调）/
│  │  │  │  │  │                models（静态兜底 + /api/llm/config/v2）/
│  │  │  │  │  │                protocol（OpenAI ↔ ADK 的 Gemini 风格信封）/
│  │  │  │  │  │                chat（会话式转发）/ stream（ADK SSE 解包）/ balance
│  │  │  │  │  ├─ zcode/        ZCode（智谱 Z.AI，国内版 + 国际版两家）：region（两地域名与身份）/
│  │  │  │  │  │                adapter（无状态、按地区参数化）/ credentials（令牌 + 套餐 JWT）/
│  │  │  │  │  │                oauth（CLI 轮询登录）/ coding_key（换推理用 API Key）/ models（静态表）/
│  │  │  │  │  │                balance（套餐余额）/ plan + claim（活动套餐通道与限时领取）/
│  │  │  │  │  │                captcha（人机验证令牌池）/ reasoning（GLM-5.3 思考预算）/
│  │  │  │  │  │                zcode_system.json（系统提示词）
│  │  │  │  │  ├─ codearts/     CodeArts（华为云码道）：signer（华为云 SDK-HMAC-SHA256，
│  │  │  │  │  │                与参考实现逐字节对账）/ credentials / dpop（ES256 DPoP proof）/
│  │  │  │  │  │                oauth（PKCE 网页登录 + loopback 回调）/ refresh（单飞续期）/
│  │  │  │  │  │                session（chat-session 心跳与每账号并发准入）/ chat（会话式转发）/
│  │  │  │  │  │                stream_fault（HTTP 200 的流内错误信封）/ redact（错误体脱敏）/
│  │  │  │  │  │                models（agent / builtin / 福利网关三源合并）/
│  │  │  │  │  │                balance（订阅统计 + 福利网关两份账）/
│  │  │  │  │  │                welfare（每日福利领取：幂等键先落盘、回读二次确认）
│  │  │  │  │  ├─ trae/         Trae（字节 AI IDE SOLO 通道）：credentials / device（设备密钥对）/
│  │  │  │  │  │                login + oauth（PKCE 网页登录 + 换证候选）/ callback_server
│  │  │  │  │  │                （本机随机端口回调与噪音过滤）/ refresh（单飞续期）/
│  │  │  │  │  │                payload（SOLO 信封白名单重建）/ headers（SOLO 头集合）/
│  │  │  │  │  │                stream（SSE→chunk 翻译）/ forward（有状态转发）/
│  │  │  │  │  │                errors（错误分类与死配置名单）/ models（get_detail_param 目录）/
│  │  │  │  │  │                usage（权益包 + 套餐 quota 两份账）/ profile（身份解析）
│  │  │  │  │  ├─ loomy/        Loomy（讯飞）：login（手机验证码）/ credentials（session 14 天、无续期接口）/
│  │  │  │  │  │                sign（复刻客户端 HMAC-SHA1 签名头）/ endpoints / client（集成网关）/
│  │  │  │  │  │                models（/api/v1/models 远程目录，上游无内置兜底清单）/
│  │  │  │  │  │                balance（永久积分 + 每日赠送两份账）/ checkin（每日首次登录刷新赠送积分）
│  │  │  │  │  ├─ kuku/         KukuAI（百度文库「库库 AI / GenFlowPro」，kuku.baidu.com）：
│  │  │  │  │  │                adapter（is_stateful）/ session（bdstoken/uinfo/uk 三件套缓存）/
│  │  │  │  │  │                engine（STOKEN 换发）/ http / login（主站网页登录 + 壳侧收 Cookie）/
│  │  │  │  │  │                credentials（BDUSS Cookie）/ models（静态兜底 + 远程刷新）/
│  │  │  │  │  │                chat（建会话 → 分配算力 → SSE）/ balance（积分余量）/
│  │  │  │  │  │                checkin（每日免费积分）
│  │  │  │  │  ├─ monkeycode/   MonkeyCode（长亭科技，国内版 + 国际版两家）：region（两站域名与身份）/
│  │  │  │  │  │                adapter / endpoints（路径 / cookie 名 / interface_type→CLI 映射）/
│  │  │  │  │  │                client / credentials（session + imageId）/ login（粘贴 session 与自动发现）/
│  │  │  │  │  │                models（两站各一格清单，仅远程）/ task（建任务）/ stream（WS 任务流）/
│  │  │  │  │  │                translate（ACP 事件 → chat 帧，含工具自动批准与提问自动应答）
│  │  │  │  │  ├─ commandcode/  Command Code：adapter / endpoints / credentials / login /
│  │  │  │  │  │                fingerprint（确定性设备指纹与 8h 上报）/ models /
│  │  │  │  │  │                plan（8 键信封与 params 重写、session id 派生）
│  │  │  │  │  ├─ antigravity/  Antigravity（Google，Gemini）：oauth（refresh token 续期 + 单飞）/
│  │  │  │  │                   project（loadCodeAssist → onboardUser 发现）/ credentials /
│  │  │  │  │                   endpoints（三环境基址与头集合）/ login /
│  │  │  │  │                   models（fetchAvailableModels + 内置兜底 + 上游真名映射）/ adapter
│  │  │  │  │  └─ officeace/    OfficeAce（华为云果办 / OfficeClaw）：oauth（PKCE + P-256 DPoP 自助登录）/
│  │  │  │  │                   signer（SDK-HMAC 与 V11-HMAC 两套签名 + 向量夹具）/ credentials（网关 Basic 与控制面临时 AK/SK 两层）/
│  │  │  │  │                   chat（无状态 OpenAI 兼容转发）/ models（远程目录 + 每模型倍率）/
│  │  │  │  │                   probe（模型可用性探测，打不通的默认隐藏）/ subscription（订阅 / 额度 / 奖励）/
│  │  │  │  │                   onboarding / checkin / balance
│  │  │  │  ├─ protocol/        协议转换层（各家 wire ↔ 标准 chat SSE 的请求/响应翻译：
│  │  │  │  │                    Anthropic Messages / Responses / NDJSON（Command Code）/
│  │  │  │  │                    Gemini 信封（Antigravity），以及工具 plan 与历史修复）
│  │  │  │  ├─ upstream/        转发编排：全局账号队列循环（provider_loop）+ 发送体处理
│  │  │  │  │                    （payload）+ SSE 透传/聚合 + usage 旁路提取 + 非 chat 协议的
│  │  │  │  │                    翻译流接入（translate：Anthropic / NDJSON / Gemini 三条并列）
│  │  │  │  ├─ account_store/   账号存储（全局优先级、限额冷却、各家添加与导入）
│  │  │  │  ├─ models/          模型目录底层（workbuddy 内置清单 + /v3/config 刷新）
│  │  │  │  ├─ model_rules.rs   模型管理规则（禁用 / 隐藏 / 映射 alias）
│  │  │  │  ├─ api_keys.rs      网关 Key 列表（多把 Key，任一启用即通过）
│  │  │  │  ├─ auth.rs / auth_http.rs / login.rs   会话、出网传输、无头登录
│  │  │  │  │                    （login/ 下是各家特有的登录流程：catpaw 的
│  │  │  │  │                    loopback 回调、qoder 的设备授权）
│  │  │  │  ├─ routing.rs / billing/   账号选路（全局优先级 + 限额冷却）/ 积分签到运营
│  │  │  │  ├─ proxies.rs / clash.rs / egress.rs   出网代理与按出口缓存 Client
│  │  │  │  ├─ sanitize.rs      出站指纹脱敏（硬编码规则集：表头剥离 + 模板句最小改写）
│  │  │  │  ├─ prompt.rs        网关自有系统提示词（透传 / 替换 / 追加三模式）
│  │  │  │  ├─ degrade.rs       内容拦截降级状态机（撞审核误报后到次日 00:00 用中性提示词）
│  │  │  │  ├─ credential_maintenance.rs  已过期 / 临期凭证的批量刷新
│  │  │  │  ├─ usage_query.rs     余额 / 积分查询（跨账号并发 + 定时那一轮的快照）
│  │  │  │  ├─ scheduled_tasks.rs  间隔型定时任务注册表与调度循环（开关 / 间隔 /
│  │  │  │  │                      上次结果；配置在 config.json 的 scheduledTasks）
│  │  │  │  └─ account_transfer.rs + account_transfer/ / auto_checkin.rs / update/
│  │  │  │                       导入导出（含身份归一）/ 定时签到 / 软件更新
│  │  │  └─ api/                 各路由 handler（health/session/accounts/accounts_usage/
│  │  │                          chat/models/keys/model_manage/stats/logs/billing/
│  │  │                          sanitize/prompt/auto-checkin/scheduled-tasks/update/…）
│  │  ├─ lib.rs                 应用入口（配置目录迁移 → 设置 → 托盘 → 主窗口 → 启动后端）
│  │  ├─ backend.rs              进程内服务器生命周期
│  │  ├─ legacy_install.rs       旧「当前用户」安装的清理（目录 / 快捷方式 / 卸载项 / 自启；仅 release）
│  │  ├─ gateway.rs              壳侧访问管理 API 的 HTTP 客户端
│  │  ├─ login.rs / commands.rs  登录窗口与轮询、暴露给前端的 invoke 命令
│  │  ├─ login_profile.rs        每次网页登录独享一个临时 WebView2 数据目录（登完即删）
│  │  ├─ bridge.rs               注入 window.workbuddyDesktop 的桥接脚本
│  │  └─ update.rs / settings.rs / state.rs / tray.rs
│  ├─ ui/                        前端（原生 HTML/CSS/JS，无框架）
│  ├─ i18n/                      界面词典源与构建脚本（六语）
│  └─ src-tauri/tauri.conf.json  打包配置（NSIS）
├─ build/make-icon.mjs           生成应用图标源图
├─ assets/screenshots/           README 配图（界面截图）
├─ Dockerfile / .dockerignore    headless 镜像（多阶段构建，只含网关与面板）
├─ docker-compose.yml / .env.example   部署（单容器：面板 + 网关同端口）
└─ package.json                  构建脚本入口（tauri:dev / tauri:build / build:icon）
```

</details>

---

## 开发与构建

### 环境要求

- Rust >= 1.77 与 Tauri 2 工具链（编译桌面端本体；Windows 上还需 WebView2 运行时）
- Node.js >= 18.17（只用于 `npm run tauri:*` 与前端构建脚本，桌面端运行时不依赖 Node）

### 常用脚本

```bash
npm run tauri:install      # 安装桌面端依赖（等价 npm --prefix desktop-tauri install）
npm run tauri:dev          # 开发模式调起桌面端（自动热重载）
npm run tauri:build        # 构建桌面端安装包

npm run build:icon         # 生成图标源图（改图标设计后执行，再跑 tauri icon）
```

打包产物为 `target/release/bundle/nsis/Agent2API_<版本>_x64-setup.exe`（当前约 3.0 MB；`src-tauri/.cargo/config.toml` 把 cargo 的 `target-dir` 指到了项目根的 `target/`）。

---

## 使用声明

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

---

## 许可

本项目基于 [MIT License](./LICENSE)，可自由使用、修改与分发，须保留版权声明。

需要留意的是：LICENSE 正文之后附有一份**使用声明**，其中第 3 条在 MIT 之上**追加了限制**（禁止商业用途、禁止二次分发牟利、禁止批量账号运营）。因此本项目**不是**纯粹的 MIT 项目——**MIT 条款与使用声明共同构成完整的授权与使用约定**，两者对同一行为给出不同结论时以更严格的一方为准。这也是 `Cargo.toml` 用 `license-file` 指向 LICENSE、而不声明 SPDX `"MIT"` 的原因。

---

## Star History

<a href="https://star-history.com/#aimod-cc/agent2api&Date">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=aimod-cc/agent2api&type=Date&theme=dark" />
    <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=aimod-cc/agent2api&type=Date" />
    <img alt="Star History Chart" src="https://api.star-history.com/svg?repos=aimod-cc/agent2api&type=Date" />
  </picture>
</a>
