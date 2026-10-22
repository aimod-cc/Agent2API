# QClaw 网页登录 / 云端推理 · 最终逆向报告

> 依据：QClaw `v0.3.0.631` 的 `resources/app.asar`（3 个 renderer bundle + 主进程 `out/main/index.cjsc` 字节码）
> + **`resources/openclaw/config/extensions/qclaw-llm-provider/` 的 TypeScript 源码原文**
> + GitHub 同类项目（`photon-hq/qclaw-wechat-client`、`xuqian-git/GetQClawAPIKey`、`wicm84266964/Buddy2api`）
> + 本机真实登录态实测 + 官方公开报道。
>
> **本版给出最终结论。前两版分别错判为「不做」和「卡在请求形态」。**

---

## 零、最终结论（一句话）

> **QClaw 已于 2026-09-24 宣布停运（2026-12-24 正式停服），且其登录/推理链路上有腾讯图灵盾
> 设备风控，二者叠加使「网页登录 + 独立转发」不可行。** 同类项目 Buddy2api 已把 qclaw
> 通道**主动下架**（README 原话：「QClaw 已停止运营，通道已下架」）。

---

## 一、决定性事实：QClaw 已宣布停运

来源：腾讯官方公告（2026-09-24），多家媒体报道

| 项 | 内容 |
|---|---|
| 公告日 | **2026-09-24** |
| 正式停服 | **2026-12-24 00:00** |
| 已生效的限制 | **停止新用户注册**、停止订阅购买与续费；官网出现「停服专区」「申请退款」「迁移到 WorkBuddy」 |
| 数据处置 | 可迁移至 WorkBuddy（个人版），迁移用户补贴 1000 积分；备份下载窗口至 2027-03-24 |
| 官方口径 | 「因公司业务发展调整，为进一步整合资源为用户提供更好的 Agent 服务」 |

**旁证**：JS 客户端网关项目 `wicm84266964/Buddy2api`（与本项目同类，channels 里本有 qclaw）
的 README 现写着：

> 「QClaw 已停止运营，通道已下架，积分请改用 WorkBuddy。」
> 「`qclaw` 已下架，环境变量里的 `qclaw` 会被忽略。」

**这是「同类项目主动不做」，不是「技术做不出来」** —— 直接回答了「为什么 WorkBuddy 行、QClaw 不行」。

---

## 二、技术原因：图灵盾设备风控挡在推理链路上

### 2.1 已实证的登录链（**技术上完全可行**）

出处：GitHub `xuqian-git/GetQClawAPIKey` 的 `server.js`（实测跑通过）+ 本项目逆向

| 步骤 | 端点 | 说明 |
|---|---|---|
| 1 | `POST jprx.m.qq.com/data/4050/forward` `{guid}` | 取 CSRF state |
| 2 | `GET open.weixin.qq.com/connect/qrconnect?appid=wx9d11056dd75b7240&scope=snsapi_login&redirect_uri=<security.guanjia.qq.com/login>&state=<state>&login_type=jssdk&self_redirect=true&style=white` | 二维码页面 |
| 3 | `GET open.weixin.qq.com/connect/qrcode/{uuid}` | 二维码图 |
| 4 | `GET long.open.weixin.qq.com/connect/l/qrconnect?uuid={uuid}&last={last}` | 长轮询（404=已扫待确认 / 403=取消 / 402=过期 / **405=成功拿 code**） |
| 5 | `POST data/4026/forward` `{guid, state, code}` | **换 jwt + channel_token + user_info** |
| 6 | `POST data/4027/forward`（条件） | 补 user_info |
| 7 | `POST data/4058/forward`（条件） | 补 channel_token |
| 8 | `POST data/4055/forward` `{}` | **换 `sk-` API Key** ← 我实测成功 |
| 9 | `POST data/4155/forward` `{scene:"login", userId, deviceToken, extra:{client_end, macOS_id}, eventTime}` | **风控上报（激活）** ← **实测 `risk assessment failed`** |

**实测拿到的东西**：

```json
// data/4055 → 成功
{"key":"sk-<已脱敏：32 位 hex 实测值>",
 "masked_key":"sk-2dbba405...51c6","created_at":"2026-03-24 10:39:38"}

// data/4155 → 失败（这就是卡点）
{"ret":0,"data":{"resp":{"common":{"code":-1,"message":"risk assessment failed"}}}}
```

### 2.2 卡点的根因：**腾讯图灵盾（Turing Shield）**

出处：`out/main/index.cjsc` 字节码

```
TuringShieldManager
TURING_SHIELD_BRIDGE_EXE
TURING_SHIELD_CHANNEL_ID
TURING_SHIELD_IPC
TURING_SHIELD_PRODUCT_NAME
[TuringShield:bridge]
[IPC] turingShield:fetchTicket failed:
VALID_RISK_LEVELS
missing required field: risklevel
```

安装目录里也确有其物：`resources/turing-shield/`（原生二进制）。
另有 `resources/qimei/`（**QIMEI** —— 腾讯设备指纹 SDK，与图灵盾同一套设备标识体系）。

**这条链的含义**：

```
微信扫码登录 ──► 拿 jwt / channel_token / sk- API Key        ← 全部能拿到
                        │
                        ▼
                  data/4155 风控上报
                        │  需要「图灵盾 ticket」（设备指纹 + 环境完整性证明）
                        │  ticket 由 QClaw 客户端内的原生模块（TURING_SHIELD_BRIDGE_EXE）
                        │  在本机环境下现算，交由腾讯服务端校验
                        ▼
                  ✗ 外部无法伪造 ── 故 returns "risk assessment failed"
                        │
                        ▼
                  云端 chat/completions 拒绝（invalid_request，非认证错误）
```

我用 `sk-` key 打云端时，**认证是通过的**（401 → 400）：

| 请求形态 | 结果 | 含义 |
|---|---|---|
| 用 jwt | 401 `invalid_api_key` | 用错凭据 |
| 用 `sk-` key + `model:"qclaw/xxx"` | 400 `model_not_valid` | 认证过了，模型名该剥前缀 |
| 用 `sk-` key + `model:"modelroute"`/`"pool-xxx"` | 400 `invalid_request` | **认证全过，被风控/权限层拦下** |
| 4155 激活 | `risk assessment failed` | **风控没过** |

### 2.3 我穷尽的尝试（都失败）

| 维度 | 试过的取值 | 结果 |
|---|---|---|
| 模型名 | `modelroute` / `pool-hy3-preview` / `qclaw/pool-*` / `deepseek-v3.2` / `deepseek-v3.1-terminus` / `openclaw` / `openclaw/default` / `openclaw/main` / `openclaw:main` / `agent:main` | 全部 400 |
| 认证头 | `Bearer` / `x-api-key` / 六件套 / `X-OpenClaw-Token` / `X-Qclaw-DeviceToken` / `JPrx-Ctx`（按源码算法签名） | 全部 400 |
| 请求体 | 加/不加 `stream` / `max_tokens` / `temperature` / `user` / `guid` / 数组式 content / system 消息 | 全部 400 |
| 路径 | `/chat/completions` / `/completions` / `/responses` / `/openai/chat/completions` / `/models` | 仅 `/chat/completions` 存在 |
| agent 协议 | `model=openclaw` + `x-openclaw-model` 头 | `model_not_valid` |

**这说明问题不在「请求拼得对不对」，而在服务端有一道校验我们过不去。**

---

## 三、与 WorkBuddy 的架构对照（回答最初的疑问）

| | WorkBuddy | QClaw |
|---|---|---|
| 登录 | `POST {endpoint}/v2{prefix}/auth/state?platform=workbuddy`（**纯 HTTP，匿名可取**） | 微信 JS SDK 扫码 + 回调拦截 |
| 推理 | 登完直接调同一套 HTTP API | **baseUrl 由本机 AuthGateway 动态注入**（`getRunningAuthGatewayLlmBaseUrl`） |
| 风控 | 无设备指纹强校验 | **图灵盾 + QIMEI 双设备指纹** |
| 产品状态 | 腾讯战略级（马化腾亲自参与） | **已公告停运** |
| 第三方支持 | Buddy2api 等主力支持 | **同类项目已下架** |

关键源码原话（`qclaw-main.cjsc`）：`apiKey is proxy-managed`
—— 明说这 key 是**代理托管**的，不该脱离本机 AuthGateway 使用。

---

## 四、可行路径（现实建议）

| 方案 | 建议 | 理由 |
|---|---|---|
| **A. 不做网页登录，UI 说明原因** ⭐ | **推荐** | 产品已公告停运（2026-12-24 停服），现在投入做登录链，不到 2 个月就作废 |
| B. 保留现有「导入本机网关」 | 维持现状 | 停服前仍可用，零额外投入；停服后自然失效 |
| C. 转做 WorkBuddy | 已有支持 | 官方指定的迁移目标，且第三方生态成熟 |
| D. 硬做云端直连 | **不建议** | 需绕过图灵盾（法律与合规风险），且上游 2 个月内下线 |

**给用户的建议**：把 QClaw 通道定位为「停服前的过渡能力」，不要投入做网页登录。
如果要跟进腾讯这条线，把精力放在 **WorkBuddy**（已是本项目支持的 provider）。

---

## 五、参考资料

| 项目 | 价值 |
|---|---|
| `photon-hq/qclaw-wechat-client` (794★, archived) | WeChat Access API 完整 TypeScript 实现 + AGP WebSocket 协议；**已归档** |
| `xuqian-git/GetQClawAPIKey` (46★) | 完整扫码登录 + `4055` 取 key + `4155` 风控 + curl 示例；README 顶部注明「客户端已更新授权方式，此项目方式暂时不可用」 |
| `wicm84266964/Buddy2api` (122★) | 同类网关，**已下架 qclaw 通道** |
| `jooooock/QClaw` | 客户端逆向 + 自定义 wss 服务器方案 |
| `HenryXiaoYang/wechat-openclaw-channel` (637★) | 「同时支持 QClaw 和 WorkBuddy 方式登录」的 OpenClaw 插件 |

### 本机实证脚本（留在 `.workbuddy/tools/`）

| 脚本 | 用途 |
|---|---|
| `qclaw-api-probe.py` | jprx 通用调用（含 leveldb 凭据解密） |
| `qclaw-points-probe.py` | 积分 / 订阅 / 流水端点 |
| `qclaw-ioa-probe.py` | IOA 登录链（内网绑定模式） |
| `qclaw-cloud-infer-probe.py` | 云端推理：头组合穷举 |
| `qclaw-cloud-sig-probe.py` | 云端推理：JPrx-Ctx 签名实现 + 头组合 |
| `qclaw-activation-probe.py` | `4155` 激活 + 推理形态验证 |
