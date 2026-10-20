---
name: provider-adaptation
description: Agent2API（workbuddy）仓库接入新提供商（provider）与验收适配完整度的规范 —— README「支持情况一览」六列的口径、必须登记的代码与前端落点、能力探测脚本的用法、静默失效排查、六份 README 同步。凡是涉及新增一家反代 Agent / 客户端 provider、审核这类 PR、判断某家「做完了没有」、改 providers/ 下的适配器或能力位（supports_*）、给签到清单加一家、或补 README 支持矩阵时，都先按本 Skill 执行 —— 即使对方没提「provider」「适配」这类字眼。
---

# Agent2API 提供商接入规范

覆盖两件事：**接一家新 provider 要改哪些地方**（第 2–5 节），以及**怎么判断它接完了没有**（第 1、6、7 节）。排查见第 8 节。

## 0. 一条红线：README 的支持矩阵就是验收口径

`README.md` 的「支持情况一览」有六列，每一格都是**用户可见的承诺**：

| 列 | 含义 |
|---|---|
| LLM 请求 | 能转发对话（含流式） |
| Token 自动续期 | 凭证过期能自己换新 |
| 模型列表 | 有清单，且支持在线拉取 |
| 余额查询 | 能读余额 / 积分 |
| 签到 | 有每日签到（或等价动作），且接进了自动签到框架 |
| 领取类 | 有可领取的活动（新手礼 / 每日任务 / 限时套餐） |

**打 ✓ 的每一格，必须有一个落地物和一个可执行的验收动作**（见第 6 节）。**打 ✗ 或 — 的每一格，必须能说清「上游确实没有这个能力」是怎么排除的**，不能是「还没做」。

这条红线的由来：本仓库历史上多次出现「只做了对话，模型获取 / 余额 / 签到没做」的半成品，而表格和代码之间没有任何自动校验 —— 表格只能靠人自觉。因此：

- **✗ 不等于没做，等于确认不存在。** 结论要能复述排查过程（打过哪些接口、返回什么）。
- **默认值不是结论。** `ProviderAdapter` 的 `supports_*()` 默认全是 `false`，那是「尚未接入」的语义，不是「这家不支持」。
- **不要为了让表格好看而打 ✓。** 一个必然报错的按钮比没有按钮更糟（见 `auto_checkin.rs` 的 `CHECKIN_PROVIDERS` 注释）。

## 1. 先做的三件事（动手写代码之前）

### 1.1 摸清上游的真实形态

拿到这家客户端的通信证据（抓包 / 安装包逆向 / 官方 CLI 源码 / 参考实现），至少要回答：

- 推理是 OpenAI 兼容，还是自有协议？（决定走 `build_chat_request` 还是 `forward_conversation`）
- 响应是 SSE、NDJSON，还是别的信封？（决定要不要新开 `UpstreamResponse` 变体）
- 鉴权头怎么拼？凭证从哪来、会不会过期、有没有续期接口？
- 有没有模型目录接口、余额接口、签到接口？**每一条都要有实际请求证据。**
- 有没有地区 / 额度池的区分？（决定是不是要拆成两个 provider）

### 1.2 判断要不要拆成两家 provider

**地区 / 额度池是 provider 身份，不是账号上的字段。** 这条在本仓库被反复确认过，走过这条路的：WorkBuddy、AutoClaw、Qoder、Accio、ZCode、MonkeyCode、ModelScope（两地），Cline（两池）。

把地区做成账号字段的后果每次都一样：

1. 模型目录只有一份缓存，两个地区互相覆盖；
2. 模型规则只有一个 `(provider, id)` 命名空间，同名模型无法分别点名；
3. 转发候选不含地区，请求可能落到另一地区的账号上，而「模型不存在」的 400 是 `Fatal`、不换账号，直接失败。

判断依据是**账号体系是否分开**：两套账号、两份目录、令牌不通用 → 拆。只是同一套协议换域名、账号通用 → 不必拆（但仍需确认）。

拆了之后：`ProviderKind` 加两个变体，`PROVIDERS` 加两条（相邻排列），实现是**一套**、按 `Region` 参数化，两个静态实例由 `adapter_for` 分派。地区互查放在 `region.rs`（`kind` / `provider_id` / `from_provider_id`），**别处不要再写 `"xxx-intl"` 这类字面量**。

### 1.3 确认「没有某接口」的排除过程

CatPaw 与 AutoClaw 都曾把「上游没有目录接口」写进注释，后来两次都被证伪：CatPaw 的 `tenant/scene/env` 是 POST body 不是 query；AutoClaw 的目录在同 host 的 `/proxy/` 一级而不是对话用的 `/proxy/autoclaw`。

所以：**「这家没有某接口」的结论要能说清是怎么排除的**，否则它只是一个还没找到的接口。把这个过程写进对应模块的模块头，别只写结论。

## 2. 后端登记点（漏一个就是静默失效）

按顺序改，每一条都写清「漏了会怎样」。

### 2.1 身份与注册表

| 文件 | 改什么 | 漏了会怎样 |
|---|---|---|
| `core/providers/mod.rs` | `ProviderKind` 加变体（**加在末尾**） | 编译不过 |
| 同上 | `PROVIDERS` 加 `ProviderMeta { id, label }` | 编译不过 / 界面无此项 |
| 同上 | `kind_from_id` 加分支 | 编译期不报，运行期「未知 provider」，账号静默失配 |
| 同上 | `kind_id` 加分支 | 编译报错（穷举 match） |
| `core/providers/adapter.rs` | `adapter_for` 加分支 | 编译报错（穷举 match） |
| 同上 | `implemented_kinds()` 加 kind | **不报错**，但后台目录刷新永不调度 —— 症状是「一直是内置兜底、手动刷新也没用」 |

`ProviderMeta.id` 是落盘契约（`accounts.json` 里的 `provider` 字段），**存量 id 不能改名**。新家取新 id；只有国际版这类新增身份才用新串。

### 2.2 账号层

| 文件 | 改什么 | 漏了会怎样 |
|---|---|---|
| `api/accounts.rs` | `add_account` 加 `ProviderKind` 分派分支 | 这家根本加不了号 |
| 同上 | `refresh_provider_account` 分派（如果支持手动刷新凭证） | 界面上「刷新」按钮点了没反应或报未知 provider |
| `core/account_store/<name>_accounts.rs` | 新建：账号记录的读写、公开形态转换、id 生成 | 账号无法落库 / 界面上看不到 |
| `core/account_store/mod.rs` | 加 `is_<name>_family(provider_id)` | 两地 / 两池的家判定失效 |
| `core/account_store/store_view.rs` | `public_account` 加分支（去掉凭证本体） | **凭证会随公开形态泄漏到前端** |

公开形态**绝不能含 accessToken / refreshToken**，只留 `tokenTail` 与「有没有令牌」。这条是安全边界。

### 2.3 目录与转发

| 文件 | 改什么 | 漏了会怎样 |
|---|---|---|
| `core/providers/<name>/models.rs` | `list_models`（静态兜底 + 远程清单） | `/v1/models` 无此项 |
| `core/providers/catalog.rs` | `refresh_meta` 加分支（穷举 match） | 编译报错 |
| `core/providers/catalog_cache.rs` | `SCOPE_*` 常量 + 加进 `ALL_SCOPES` | **不报错**，清单进程重启后丢失 |
| `core/providers/<name>/adapter.rs` | `supports_model_refresh` + `refresh_models` | 界面「刷新模型」按钮无效 |

`ALL_SCOPES` 漏了不会报错 —— 缓存能写进内存，但重启后读不回来。这是最容易漏的一处。

### 2.4 能力位

`ProviderAdapter` 的每个能力位都有明确的消费方，填错会直接影响界面行为：

| 能力位 | 消费方 | 填错的后果 |
|---|---|---|
| `supports_chat` | `account_store::pick_current`（全局队首） | 无转发能力的家排进队首，顶栏显示成「当前登录态」 |
| `supports_refresh` | `credential_maintenance` | 声称支持但刷不动 → 每轮白跑 + 灌失败日志 |
| `supports_model_refresh` | `catalog_refresh` | 声称支持却拿不到新清单，用户以为坏了 |
| `supports_usage` | `usage_query` / 前端积分按钮 | 给了入口但必然失败 |
| `supports_web_login` | `api::session` 登录路由 | 标 true 但 `build_login_url` 返回 None → 能用的功能变 400 |

口径统一为：**「上游到底有没有那个接口」，不是「我们想不想实现」**。判不出来时返回 `false`（宁可漏刷，也不要凭猜测打上游）。

### 2.5 签到与领取

| 文件 | 改什么 | 漏了会怎样 |
|---|---|---|
| `core/auto_checkin.rs` | `CHECKIN_PROVIDERS` 加 id（**数组长度 `[&str; 16]` 也要 +1**） | 签到中心无这家、定时任务不跑 |
| 同上 | `PROVIDERS_VERSION` +1，`PROVIDERS_ADDED_IN` 加条目（**数组长度 `[(&str, i64); 6]` 也要 +1**） | **不报错**，存量用户的签到清单里不会自动出现这家 |
| `core/billing/checkin.rs` | `checkin_for` 的 `match provider_id` 加分支 | 批量签到对这家算 `skipped` |
| `core/providers/<name>/checkin.rs` | 新建：claim 实现 | — |
| 领取类 | 各家自己的入口（`zcode::claim` / `codearts::welfare` / `chatglm::tasks` / `workbuddy::growth`） | 无统一位置，最容易被整块漏掉 |

**`PROVIDERS_ADDED_IN` 是最隐蔽的一处。** 只往 `CHECKIN_PROVIDERS` 加 id 而不升版本号，新家不会进入存量用户的清单；而每次读都补齐又会让用户取消不掉勾选（读完就补回来）。版本号就是那个「只补一次」的记账。

加一家之前**先确认签到链路真的存在**。上游没有独立签到接口但有等价动作（每日首次登录、调一次信息接口就触发）也算，但要在模块头写清依据。完全没活动就不进清单 —— 一个点了必然报错的复选框比没有更糟。

### 2.6 错误分类（最容易做错的一档）

`classify_error` 只能归到四档，**多出来的分类在编排层没有对应分支，会静默落进 `Fatal`**：

| 档 | 动作 | 判错的后果 |
|---|---|---|
| `QuotaLimited` | 标记账号对该模型冷却 + 换账号 | 限额被当 Fatal → 账号永不轮换，每次都撞同一面墙 |
| `TokenExpired` | 刷新凭证后**同一账号**重试一次 | 凭证过期被当 Fatal → 用户看到 401 |
| `ContentBlocked` | **不罚账号**，换中性提示词重试一次 | 审核误报被当限额 → 白扔另一个账号的额度 |
| `Fatal` | 原样透传给客户端 | — |

几条实践要点：

- 有些上游把「额度耗尽」和「速率窗口」都用 429 表达，只能靠**文案**区分，而处置完全不同（前者冷却到次日零点，后者是分钟级窗口）。判据要写清来源。
- 有些上游把业务失败包装成 **HTTP 200**（正文一句固定话术，或流内 `{"type":"error"}`）。这种要靠 `is_error_response` / `stream_refusal_finishes` / `stream_refusal_markers` 拦下，否则会被当成一次成功答复。
- `stream_refusal_finishes` 判的是**内容策略**拒绝（不罚账号、换家）；`stream_refusal_markers` 判的是**本账号额度**问题（必须罚账号）。两条别混。
- 会话式转发（`is_stateful`）的错误默认走 `Fatal`。要参与限额记账必须覆写 `classify_conversation_error`，否则福利池耗尽后每个请求都从头撞一遍全部账号。

## 3. 前端登记点

| 文件 | 改什么 | 漏了会怎样 |
|---|---|---|
| `ui-islands/src/islands/add-account-configs.ts` | `ProviderConfig`：表单字段、登录方式（`webLogin` / `smsLogin` / `oauthLogin` / 访客） | 添加账号弹窗里没有这家的表单 |
| `ui-islands/src/islands/add-provider-pick.tsx` | 卡片列表登记 | 选不到这家 |
| `ui-islands/src/islands/add-provider-blocks.tsx` | 特殊块（如 WorkBuddy 的版本 + 打开方式） | 仅特殊形态需要 |
| `ui-islands/src/islands/accounts-domain.ts` | 账号行的能力标记（`usage` / `creditDetail` / `edition` / `identifier` / `expiry` / `longLived`） | 账号卡片上的徽章、余额格、有效期显示不对 |
| `ui-islands/src/islands/checkin-page.tsx` | `PROVIDER_DESC` 加链路说明 | 签到中心该家分组没有说明文字 |
| `ui/assets/providers/<name>.png` | 图标 | 卡片没有图标 |

`ui/providers.js` **不用改** —— 它从 `/api/session` 的 `accounts.providers`（后端注册表摘要）自动派生。

改完前端要重新构建岛：`npm run ui:islands`（产物 `ui/islands/ui.js` 要提交）。

## 4. i18n 与文档

### 4.1 i18n

中文是源语言，`i18n/i18n-src/` 下只有五份译文（`en` / `ja` / `ko` / `pt-BR` / `zh-Hant`）。新增的中文文案会被 `scan.mjs` 扫出来，缺译文就是空值。

```bash
cd desktop-tauri/i18n
npm run scan     # 扫出中文键，补齐到 i18n-src/*
npm run check    # 检查是否有空值（CI 会拦）
npm run build    # 生成 ui/i18n/*.js
```

历史教训：上游 v3.0.7 加了六条中文文案但没跑扫描，`scan.mjs --check` 时被扫出空值。**改完前端一定要跑 `check`。**

### 4.2 文档

`README.md` 的「支持情况一览」**六份一一对应**，加一家要六份一起改：

- `README.md`（简体中文，源）
- `docs/README.en.md`
- `docs/README.zh-Hant.md`
- `docs/README.ja.md`
- `docs/README.ko.md`
- `docs/README.pt-BR.md`

另外注意「内置 N 家提供商」这个数字（「核心特性」一节，**六份 README 都有**，各自语言的措辞不同）。它数的是**表格里的内置行数**（不含「自定义提供商」那一行），不是 `PROVIDERS` 的条目数：

- 表格把两地 / 两池合并成一行（如 `AutoClaw（国内版 / 国际版）`、`Cline（Free / Pass）`），而 `PROVIDERS` 里是两个条目；
- 所以加一家两地 provider 时，`PROVIDERS` +2、表格只 +1 行、这个数字也只 +1。

当前状态：表格 22 行内置 + 1 行自定义 = 23 行，`PROVIDERS` 29 条。改之前先数一遍，别按 `PROVIDERS` 的条数算。

安装 / 部署相关的内容只在 `docs/install*.md` 里维护，不要往 README 塞。

## 5. 施工顺序

按功能纵向闭环，不要先把后端全写完再做前端：

1. **身份**：`ProviderKind` + `PROVIDERS` + `kind_from_id` / `kind_id` + `adapter_for` + `implemented_kinds`
2. **账号**：`<name>_accounts.rs` + `add_account` 分派 + 公开形态 + 前端表单 → 能加号、能在列表里看到
3. **目录**：`list_models` + `catalog_cache` scope + `refresh_models` → `/v1/models` 有内容、刷新按钮有效
4. **转发**：`build_chat_request` 或 `forward_conversation` + `classify_error` → 能对话、限额能轮换
5. **余额**：`supports_usage` + `query_usage` → 账号行能看到余额
6. **签到 / 领取**：`CHECKIN_PROVIDERS` + 版本号 + `checkin_for` 分派
7. **前端收尾**：`accounts-domain` 标记、签到中心说明、图标
8. **i18n + 六份 README**

每一步做完都能独立验证，不要攒到最后一起测。

## 6. 完成度验收清单

每一格都要能跑出结果，不是「我实现了」。

### 6.1 用探测脚本验（推荐）

```bash
node scripts/probe-provider.mjs \
  --base http://127.0.0.1:3065 \
  --key sk-dev-<provider> \
  --provider <id> \
  --model <模型名>
```

前置：在面板「网关 Key」页建一把**只允许目标 provider** 的 Key。不建的话，同名模型在多家里都有映射时会被路由到别家，结论无法归因（脚本启动时会先校验并报错退出）。

脚本由两个文件组成：`scripts/probe-provider.mjs`（CLI 外壳：参数、HTTP、前置校验、输出）与 `scripts/probe-cases.mjs`（九个用例的请求构造与判定）。加新用例改后者，`CASE_ORDER` 决定执行顺序。

用例与能力位的对应：

| 用例 | 验的是什么 | 对应 README 列 / 能力位 |
|---|---|---|
| `stream` | 流式 SSE 能正常逐帧下发 | LLM 请求 |
| `tool_call` | `tools` 声明被上游接受，返回 `tool_calls` | LLM 请求（工具调用） |
| `parallel_tools` | 一次返回多个 `tool_calls` | LLM 请求（并行工具） |
| `tool_roundtrip` | 工具结果回灌后能继续生成 | LLM 请求（多轮循环） |
| `image` | 图片输入被接受 | LLM 请求（多模态） |
| `reasoning` | 思考字段能透出 | LLM 请求（思考） |
| `json_schema` | 结构化输出可用 | LLM 请求（结构化输出） |
| `usage` | 终帧带非零 usage | 报表 / 用量统计 |
| `context_window` | 目录声明的上下文窗口（读 `/v1/models`，不发上游） | 模型列表 |

判定口径：

- **必需项失败 = 这家的「LLM 请求」不能算接完。** `tool_call` 即使被目录声明为不支持，也记 **FAIL** 而不是 SKIP —— 记 SKIP 会让汇总显示「必需项全部通过」，而实际上 agent 客户端根本用不了这家。
- 可选项目录声明不支持时（`image` / `reasoning`）记 **SKIP** 而不是 FAIL，否则每家都会刷一堆假失败。`--force` 可强测（用于「怀疑目录写错了」）。
- `tool_call` 允许重试一次（模型偶尔就是不调工具），失败后换更硬的提示再发一次。
- `parallel_tools` 只返回 1 个 call 时记 **INFO** —— 模型可能打算分两轮返回，那是行为差异不是适配缺陷。
- `context_window` 记 **INFO/PASS** —— 它读的是目录声明，不是上游行为，和其余用例性质不同。

退出码：`0` = 没有必需项失败；`1` = 有必需项失败；`2` = 用法或前置条件错误（连不上网关、模型不在清单里、模型不由该家承载）。

### 6.2 逐列验收

| README 列 | 验收动作 |
|---|---|
| LLM 请求 ✓ | 探测 `stream` + `tool_call` + `tool_roundtrip` 全 PASS；用真实客户端（Claude Code / Codex 等）跑一轮多轮对话不报错 |
| Token 自动续期 ✓ | 账号页能看到有效期；等一个临期窗口（或手动改凭证时间）后跑 `credential_maintenance`，日志里有刷新成功记录；401 时能自动恢复而不是报错给客户端 |
| 模型列表 ✓ | `/v1/models` 有该家的条目；模型管理页点「获取模型」后清单变化；**重启进程后清单还在**（验 `ALL_SCOPES` 有没有漏） |
| 余额查询 ✓ | 账号行显示余额；点「积分明细」能展开；读不到的数值留空显示，不是 0 |
| 签到 ✓ | 签到中心出现该家分组且有说明文字；手动点一次成功；次日定时任务跑完台账有记录；**存量配置升级后这家在默认清单里**（验 `PROVIDERS_VERSION`） |
| 领取类 ✓ | 对应按钮能点且返回可读结果；没有活动时返回中性说明而不是红色失败 |
| 某列 ✗ | 能在代码注释里找到「怎么排除的」记录（打过哪些接口、返回什么） |

### 6.3 静默失效自查

这些**不会报错**，只能靠主动验：

- [ ] 重启进程后模型清单还在（`ALL_SCOPES`）
- [ ] 后台目录刷新真的会调度到这家（`implemented_kinds`）
- [ ] 存量用户升级后签到清单里有这家（`PROVIDERS_VERSION` + `PROVIDERS_ADDED_IN`）
- [ ] 公开形态里没有 token 本体（`store_view::public_account`）
- [ ] 限额错误真的触发了账号轮换，而不是当 Fatal 透传（看请求日志的 attempts 与冷却标记）
- [ ] 前端中文文案全部有译文（`npm run check`）

## 7. 调试设施（都是现成的，别重造）

| 要看的 | 怎么看 |
|---|---|
| **发往上游的最终 body**（已按家改写、已脱敏） | 面板设置页开「调试模式」→ 跑一次请求 → 请求日志详情页「原始报文」标签。数据在 `debug_traffic` 表，保留最近 500 条 |
| 上游原始响应（含 SSE 全文） | 同上，同一个标签 |
| 客户端发来的请求体 | 调试模式开启后同样可见（`request_raw` 表） |
| 单次转发的选路、重试链、usage | 模型管理页的「测试」按钮（`POST /api/models/test`），走真实转发链路、钉住 provider + account、结果失败也返回 2xx |
| 帧级翻译问题（Anthropic / Gemini / NDJSON / Chatglm 快照） | 调试报文里的上游原始 SSE，对照 `core/upstream/translate` 的输出 |

**出站报文不需要新做工具** —— `TrafficCapture::reset_request` 抓的就是「即将发送前」那一份，四家路径全覆盖。

## 8. 已知坑

- **「上游没有这个接口」要先怀疑找法。** CatPaw 的目录参数在 POST body 里（看着像 query），AutoClaw 的目录在另一个路径前缀下（顺着 `chat/completions` 找永远找不到）。两次结论都错。
- **429 不一定都是限额。** 有的家把额度耗尽、速率窗口、未验证邮箱都用 429/403 表达，处置完全不同（次日零点 vs 分钟级 vs 永久）。只看状态码会把三种混成一档。
- **HTTP 200 可能是失败。** 正文一句固定话术、或流内 `{"type":"error"}`、或 `finish_reason: content_filter`，都要单独识别。
- **`supports_*` 默认 false 不是「不支持」。** 它是「尚未接入」。判据必须是上游有没有那个接口。
- **两个地区拆家时，`PROVIDERS` 里要相邻排列。** 中间隔着别家会让「找国际版」变成一次扫描。
- **`implemented_kinds()` 与 `PROVIDERS` 是两个口径。** 前者是「适配器接线了没有」，后者是「注册了没有」。占位实现不该进前者。
- **签到清单的默认全选是个陷阱。** 加一家进 `CHECKIN_PROVIDERS` 会让存量用户第二天零点就多发一轮请求。想默认开启就升版本号；不想默认开启就别进 `PROVIDERS_ADDED_IN`。
- **改完前端要重新构建岛并提交产物。** `ui/islands/ui.js` 是检入仓库的，只改 tsx 不构建等于没改。

## 9. 参考实现

`git show 1576330` 是 ModelScope 接入的完整提交，48 个文件，正好覆盖第 2–4 节的每一类落点。拿不准该改哪些文件时，对着这个提交的 `--stat` 过一遍。
