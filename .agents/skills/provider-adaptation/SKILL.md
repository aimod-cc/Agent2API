---
name: provider-adaptation
description: Agent2API（workbuddy）提供商接入与完整度审核规范，以独立 WASM 插件为主流程：单提供商包、无 WASI 沙箱、宿主能力与权限声明、大正文资源通道与内存保护、动态登录及三端界面、账号和模型隔离、六项能力的实现与真实验收状态、旧原生插件停用后的迁移与兼容。凡涉及新增反代 Agent / 网页客户端 provider、审核适配 PR、改 WASM 提供商或 supports_* 能力、签到与领取、支持矩阵和旧包迁移时，都先执行本 Skill，即使请求未出现「provider」「适配」等字眼。
---

# Agent2API 提供商接入规范（WASM）

覆盖新提供商接入、现有提供商维护、存量迁移，以及适配完整度审核。**所有新老提供商均以独立 WASM 插件为主流程；旧原生可执行插件（非 `.wasm`）已停止支持并由宿主拒绝加载，不再把上游适配写进主程序，也不再新增分平台原生插件。** 不得因此自行删除用户已有账号、旧凭据迁移或兼容保留逻辑。

本规范以当前工作区的实际实现为准。遇到宿主契约与 README、旧模块注释不一致，先核对代码，不把历史说明当成新接口承诺。

## 0. 先确定架构与事实来源

### 0.1 当前目录与职责

以下路径均相对 **Agent2API 仓库根**，不是相对本 Skill 目录。`plugin/` 下是独立 Git 仓库的本地工作副本，操作前分别检查状态，不能假定它们与主仓库一起提交。

| 位置 | 职责 |
|---|---|
| `plugin/Agent2Providers/wasm/<id>/` | Agent / 客户端提供商的独立 WASM 实现、包清单、权限清单、图标 |
| `plugin/Agent2WebProviders/wasm/<id>/` | 网页提供商的独立 WASM 实现 |
| `plugin/Agent2Providers/wasm/sdk/` | `agent2api-wasm-sdk`，提供当前 WASM ABI 与受控宿主调用 |
| `plugin/Agent2Providers/crates/provider-protocol/src/` | 共用的提供商、账号、模型与错误数据类型；部分说明仍源自原生插件阶段 |
| `desktop-tauri/src-tauri/server/src/server/` | 宿主后端；下文的 `core/...`、`api/...` 均相对这里 |
| `desktop-tauri/ui-kit/`、`desktop-tauri/ui-shared/` | 私有组件库与桌面、网页、手机共用的插件登录、权限、设置与任务界面 |
| `desktop-tauri/ui-islands/`、`desktop-tauri/mobile/` | 桌面 / 网页面板与手机端的声明式界面消费方 |

**WASM SDK 与旧的 `crates/provider-sdk/` 不是一回事。** 后者的原生进程服务循环、标准输入输出、`reqwest` / `tokio` 直接联网方式，不是新插件模板。旧实现只作协议参考与迁移来源，不能原样当作 WASM 运行方式。

### 0.2 动手前必须读的契约

按所需功能读对应文件，不必一次读完所有提供商：

- 包与权限：`core/plugin_host/wasm/manifest.rs`、`core/plugin_host/support.rs`（非 WASM 包一律拦截为 `LegacyUnsupported`）。
- 模块导入、ABI、每次调用的资源与取消：`core/plugin_host/wasm/runtime.rs`、`core/plugin_host/wasm/resources.rs`、`plugin/Agent2Providers/wasm/sdk/src/{abi,http}.rs`。
- 握手与动态登记：`core/plugin_host/wasm/host.rs`、`core/plugin_host/bootstrap.rs`、`core/plugin_host/registry.rs`。
- 可调用的宿主服务：`core/plugin_host/wasm/services.rs`、`core/upstream_versions.rs` 与对应的 SDK 模块。
- 业务数据形状：`plugin/Agent2Providers/crates/provider-protocol/src/{provider,account,model,chat}.rs`；**再对照宿主实际消费代码和当前 WASM 示例**，旧注释可能落后于实现。
- 在线宿主的能力摘要由受保护的 `GET /api/plugins` 返回：顶层 `wasm` 给出 ABI、能力、内存与体积限制，顶层 `protocol` 给出包通信协议。不要把本机平台的 `target` 当成 WASM 构建目标。

新增宿主能力时，先确定输入、输出、权限、账号范围、取消和错误契约，再同时对齐宿主与 SDK。**不能先在 `requires` 中编一个名字，再假定主程序能执行。** 改共用契约还要核对宿主 `Cargo.toml` 当前使用的 Git tag；修改本地副本不会自动改变宿主依赖，勿把私人路径写进正式依赖。

### 0.3 六项能力仍是验收口径，但不再固定放在根 README

根 `README.md` 与 `docs/README.*.md` 当前是网关介绍，不再承载旧的完整提供商支持表。具体支持情况主要在对应插件仓库的 `README.md` / `README.zh-CN.md`，以及面板读取的运行时声明中。

| 能力 | 必须说清的内容 |
|---|---|
| LLM 请求 | 对话与流式；工具、思考、图片、生图 / 编辑分别声明，不用一个 ✓ 包办全部 |
| Token 自动续期 | 上游可续期、插件已接通；哪些账号来源具备续期材料或明确授权 |
| 模型列表 | 静态兜底、在线刷新、账号可用模型目录分别说明 |
| 余额查询 | 金额、积分、百分比用量、套餐状态分别说明，不把使用率叫作余额 |
| 签到 | 真正的每日动作或等价动作，以及可参与的账号与时间窗口 |
| 领取类 | 新手礼、已完成任务奖励、活动套餐等；查状态不能偷偷领取 |

结论至少分开：

- **已验证支持**：当前 WASM 实现有对应真实验收证据，才可无条件写 ✓。
- **已实现，未验收**：代码与契约已接通，但没有真实账号结果，必须明确标注。
- **未确认 / 未接入**：不能由能力位为 `false` 推导出「上游没有」。
- **不支持 / 不适用**：有上游证据或账号形态依据，不能代替「还没做」。

- 面板的能力说明已有 `supported`、`implemented_unverified`、`unsupported`、`not_applicable`、`undeclared` 五种状态。当前 WASM 的 `verified: false` 会把已开启的能力显示为「已实现，未验收」；**签名、编译成功、能安装都不是验收证明**。`verified` 是包级标记，不要为了一个能力通过就让所有能力看起来都已验证。

插件仓库 README 中若仍留有原生能力表、五平台构建说明等过渡内容，审核与修改时必须以 `wasm/` 下独立 WASM 实现、`wasm.json`、`verified` 与真实实测记录为准，不能继承旧表的 ✓，也不恢复「主程序内置 N 家」或写死提供商数量。

## 1. 写代码前先摸清上游

### 1.1 先查已有方案，再核对实际通信

反代提供商问题先网络搜索同类项目的 Issue、PR、修复记录与可靠参考实现，再判断是否适用于本项目；没有可用方案时再开展针对性排查。不确定的上游事实要查证，不能猜。

通信证据可来自官方源码、公开接口、用户授权的抓包 / 安装包分析、已有适配。至少回答：

- 请求是 OpenAI Chat、Responses、Anthropic，还是多步自有会话？响应是 SSE、NDJSON、快照还是轮询任务？
- 工具是上游原生支持，还是主程序模拟？工具结果怎样回灌？哪些模型支持图片、思考或生图？
- 鉴权材料从哪来、生命周期多长、是否需要设备身份、是否有真实续期链路？
- 模型、余额、签到、领取有哪些接口？查询是否有副作用？
- 接口域名、账户域、地区与额度池是否独立？登录只适用于桌面本机，还是远程面板也能完成？

实际请求或官方协议证据要可复述，并先脱敏。记录真正的约束与排除依据，不写「这次为了某任务」之类临时注释，也不额外创建使用文档。

### 1.2 地区 / 额度池是提供商身份

两套账号、不同目录或额度池、令牌不通用，就拆成不同 provider id 和**不同 WASM 包**。同一账号体系只是换域名，不必机械拆分，但要核实。

- 每个包只声明一个身份，例如 `<id>` 与 `<id>-intl` 分开。
- 共用协议可放共享模块 / crate，但每个包的身份、网络权限、目录、账号与状态必须独立。
- `pack.json`、`init`、`provider.info` 返回的 id 必须一致。**存量 provider id 不能改名**，包 id 与 provider id 是两个概念。
- 不通过账号的 `edition` 字段把两个身份硬塞成一家，也不新增 `ProviderKind` 变体来登记普通插件。

否则模型缓存互相覆盖、同名模型无法分别选路，或者一地区的请求落到另一地区的账号上。

### 1.3 「没有接口」不是默认结论

CatPaw 目录参数实际在 POST body，AutoClaw 目录实际在另一层路径前缀；这两家都曾被误判为没有目录接口。

因此「不支持」要能说清检索、请求、返回及适用版本。旧版没有实现、SDK 默认关闭、仅找到静态清单，都不是排除证据。一次性网络失败也不是「接口不存在」。

## 2. WASM 后端接入

### 2.1 一个包的必需落点

在对应 `wasm/` 工作区新增 `<id>/`，参照当前独立包，至少包含：

| 文件 / 声明 | 要求 |
|---|---|
| `<id>/Cargo.toml` | `agent2api-wasm-<id>`、`cdylib`、依赖 WASM SDK；各独立包维护自身版本号（如 `0.1.0-beta.x`），不夹带发版 |
| `wasm/Cargo.toml` | 登记 workspace member；依赖变化时同步该 WASM 工作区的锁文件 |
| `<id>/pack.json` | 唯一包 id、唯一 provider、唯一 `entry.wasm32-unknown-unknown`，入口是包内 `.wasm`，版本号与 `<id>/Cargo.toml` 一致 |
| `<id>/wasm.json` | 真实 ABI、`requires`、HTTPS 来源白名单、内存/并发限制（当前官方包统一声明 `"memory_mib": 128`）、验收状态与所需受控声明 |
| `<id>/src/lib.rs` | 通过 `sdk::export_provider!` 导出 ABI，并实现握手与业务分派 |
| `<id>/assets/` | 声明图标目录后必须有可显示的图标；必要的第三方许可保留在 `licenses/` |

Web 提供商在 `pack.json` 中声明 `provider_category: "web"`，否则默认归为 Agent。分类是宿主读取的包元数据，不要自行把它等同于某项业务权限。

当前宿主检查：

1. `providers` 与 `entry` 都只能有一项，目标固定为 `wasm32-unknown-unknown`，跨平台使用同一包。
2. `wasm.json` 的 ABI 与宿主一致，所需能力全部存在，依赖声明齐全；`memory_mib` 合法范围以 `manifest.rs` 为准（默认 `128`，宿主运行期会为普通调用保底 128 MiB、为对话调用兜底抬高至 256 MiB，避免大正文在实例内触发内存溢出）。
3. `init` 返回**单元素提供商数组**，id 与包清单一致。
4. 模块只允许导入 `agent2api.call` / `agent2api.read`，导出 `memory`、`a2a_abi_version`、`a2a_alloc`、`a2a_call`。
5. 不能携带可执行网页；`pack.json.ui` 必须为空 / 缺省，**包内连 `ui/` 目录也不允许**。

ABI、资源上限与能力清单以 `manifest.rs`、`runtime.rs`、`resources.rs` 和宿主摘要为准，不把当前数值复制成永久常量。不要使用 WASI、wasm-bindgen 浏览器导入、任意文件 / 进程 / 环境变量接口，也不直接把原生依赖树搬过来。

### 2.2 两种声明要同时对齐

**`ProviderInfo.capabilities` 表示插件实现了什么；`wasm.json.requires` 表示需要宿主提供什么。它们不是同一张表。** 声称有业务能力，却没有对应实现或宿主权限，会加载失败或出现必然报错的入口。

| 业务能力 / 元数据 | 对应插件操作 / 约束 |
|---|---|
| `chat` | 实现 `conversation`；至少声明 `chat.stream.v1` 与 `accounts.cas.v1` |
| `stateful` | 按上游是否多步会话如实填写；WASM 即使为 `false`，转发仍由宿主走 `conversation` |
| `refresh` | `token.ensure`、`token.refresh`、`token.expiring` 与安全凭证写回 |
| `model_refresh` | `models.list`、`models.refresh` 与 `catalog.v1`；账号目录另需 `catalog.accounts.v1` |
| `usage` | `usage.query`，并与 `account_features.usage`、实际读数和单位一致 |
| `checkin` / `claims` | `checkin.claim` / `claim.status` / `claim`，以及实际存在的入口与签到元数据 |
| `web_login` | 声明 `login.forms.v1`，实现相应登录流程与动态登录方式 |
| `account_features` | 标识、有效期、积分明细、套餐、并发等字段与公开视图一致 |
| `settings_schema` / `settings_group` | 需要设置面板时在 `init` 元数据声明；若客户端版本需跟随上游官网发布，使用 `type: "upstream_version"` + `source_id` 消费宿主从官网 CDN 同步的版本号（`core/upstream_versions.rs`），不要写死过时硬编码版本 |

新包显式填写相关能力，不依赖省略字段的默认值。当前共用协议的 `chat` 默认是 `true`，**不能再套用「所有能力默认 false」的旧说法**。默认值无论真假都不构成上游或实现证据。

`anonymous_default_session` 与 `env_credentials` 在 WASM 中不能开启。不能声称隐式读取本机登录态；本机导入要走明确授权的受控入口。

### 2.3 网络、系统能力与授权边界

- 所有网络经 SDK / 宿主发出。`origins` 只接受明确的 HTTPS origin，不含路径、通配符、账号密码、query 或 fragment；普通请求不得靠重定向跳出授权来源。
- HTTP、WebSocket、资源上传与下载分别核对对应权限。发送较大对话请求时应配合 `resources.v1` 与 SDK 大正文资源通道（超阈值自动走分片资源 / `open_json_resource`，以宿主的资源池与内存上限约束，不要整包复制为 base64 字符串或受限于单条 ABI 大小）。不要凭证跟随公共图片 URL，也不要用公共下载入口访问本机或内网地址。
- 账号代理、可信请求上下文由宿主决定；插件不能用传入 JSON 自选另一个账号、代理、跟踪标识或可信调用来源。
- 设置声明只放可公开的配置；令牌、密码、Cookie 放账号 `secret`，不能塞进全局设置或表单默认值。
- 动态账号域名需要 `http.account-origin.v1` 与受控 `account_origin` 声明、明确授权，不能扩大为任意 URL；该入口已支持绑定账号代理与公网 IP 校验，代理不可用时应明确失败，不静默回退直连。
- 本机凭据、托管登录、验证码、固定续期、MonkeyCode 任务等走现成受控能力；**声明不是用户授权**，敏感授权绑定具体包与来源，更新 / 切换后按宿主规则重新核对。
- 某些能力只允许特定 provider 使用（如 CodeArts 会话/登录/换证、Cline Free / Kiro / AutoClaw / Raccoon 本机续期、CatPaw 轮次、豆包对话与任务领奖、MonkeyCode 任务与计算验证、ChatGLM 本机导入、DeepSeek 网页入站图片资源等，见 `manifest.rs`）。不得把固定入口或遗留凭据导入白名单复制给新 id，也不得用普通 HTTP 绕开固定入口。
- 能力确实不够时，明确缺口并先设计宿主契约，不引入任意原生执行来补洞。不要为了兼容新插件而放宽沙箱。

### 2.4 账号、凭证与状态

新提供商使用通用插件账号路径，不新建宿主的 `<name>_accounts.rs`，不新增 `api/accounts.rs` 或 `store_view.rs` 的厂商分支。

- `account.normalize` 返回 `{secret, public, identity, desktop}`；`account.view` 从凭证产生同样安全的公开形态。
- 新包原则上声明 `accounts.identity.v1`，返回稳定、非空身份，并实现 `account.identity`。凭证轮换不应生成新账号；不得跨地区、跨额度池合并。
- 迁移的旧账号可能没有稳定 user id。保留已存在的兼容策略并明确风险，不能为了新契约拒掉原来能用的账号，更不能靠易变 token 假装用户身份恒定。
- 公开形态绝不包含 token / Cookie / 密码本体或嵌套副本。`tokenTail` 最多 6 字符，到期时间用毫秒；具体安全字段以 `core/plugin_host/wasm/public_view.rs` 为准。
- 当前账号只能通过受控 `account.get` 读取。续期写回使用 `account.update` 的 `expect`、`secret`、`public`；锁内重读，比较失败后重新读取，不覆盖用户刚换上的凭证。
- 只有访问令牌的账号不能自动获得续期权；需要登记 / 明确授权的固定续期链路，也不能给旧导入账号自动补权。

**每次 WASM 调用都会新建实例。** 不能把登录状态、刷新结果、会话、目录、台账存在插件全局变量里期待下一次仍在：

| 状态 | 当前宿主方式与生命周期 |
|---|---|
| 登录 | `state.v1` 的 `login` 范围，绑定 flow；取消 / 重载后按宿主规则清理 |
| 会话 | `state.v1` 的 `conversation` 范围，隔离 provider、账号、身份与 session；可跨普通重载，不能承诺跨进程重启 |
| 账号持久状态 | `state.v1` 的 `account` 范围，加密持久化、格式版本与比较更新 |
| 短期缓存 | `sessions.v1`，临时 TTL 缓存；不能当长期数据库，也不要混用不同账号 / flow 的键 |
| 模型 / 台账 | 现有 catalog 与 ledger 服务，不另建插件数据库或本地文件 |

状态读取要处理 `compatible` / schema，比较更新失败要回读。重启后临时状态消失是边界，不应伪造成续接成功。

### 2.5 模型目录与请求转发

- 模型目录由插件给出 `default_models` / `models.list`，在线变化由 `models.refresh` 返回。可复用 SDK 的目录结果构造，不手写两套不同字段。
- 成功刷新含 `refreshed`、`count`、`models`、`meta.remote`、`meta.refreshed_at`；确实成功取到的空目录与失败不同。失败保留最后成功清单，不假报刷新成功。
- `force` 表示手动刷新，跳过插件缓存 / TTL；宿主的在途占位、取消与失败冷却仍生效。
- 不同账号可用模型不同，声明 `catalog.accounts.v1`，由宿主保存各账号快照并聚合展示；不能把一家账号的目录当成另一账号的访问许可。
- 新插件目录使用动态缓存 / 账号快照。**不新增旧的 `SCOPE_*` / `ALL_SCOPES`，不往 `implemented_kinds()` 登记。** 自动刷新遍历运行时已安装的插件。
- `supportsToolCall`、`supportsImages`、`supportsReasoning`、上下文长度等按模型填写，有证据才声明，不把目录里的名字出现过等同于功能可用。

WASM 对话实现 `conversation`，通过 SDK 调用网络，然后输出标准 Chat SSE（`stream.head`、`stream.chunk`）。`stateful: false` 不代表应返回旧原生 `chat` 请求计划。

- 现有 Chat / Anthropic / Responses 请求与响应转换、Gemini 流转换可使用 `protocol.convert.v1`，转换器句柄只属于当前调用；自有信封在插件内解析，不默认给宿主新增响应枚举。
- 保留完整工具声明、工具调用 id、工具结果、多轮历史、思考与 usage；不要靠删工具、截断结果或吞错误实现「能回复」。
- **不要对对话 `messages` 数组增设固定条数上限（例如 512 / 4096 条）**；入站仅校验非空数组，超长会话保护交由请求体字节上限（流式计数如 `json_exceeds`）、JSON 节点与嵌套深度预算及 WASM 运行内存控制承担，避免打断多轮 Agent 长历史。
- 网页模型若走主程序工具模拟，按现有 `toolEmulation` 契约声明，并保持原生 `supportsToolCall` 的真实含义。文案说明模拟可关闭，不能宣传为网页原生工具能力。
- 图片输入与图片生成 / 编辑是不同能力。生成 / 编辑另核对 `images.v1`、`images.capabilities` 与对应标准入口；大图不能靠无限放宽普通 JSON 限制处理。
- 原始客户端流式意图需要时用 `request.context.v1`；不能把插件内部为了向上游取流而修改的 `params.stream` 当成客户端原始选择。
- 上游失败应在下发正常内容前识别。已有实际输出后不能偷偷从头重发或换账号拼接两次回答；取消、超时、流中断必须如实结束并释放资源。

### 2.6 错误分类

`classify` 返回现有四档（JSON 使用 snake_case），不发明第五档，也不要拿未解析的文案字符串代替结构化分类：

| `kind` | 网关处置含义 |
|---|---|
| `quota_limited` | 冷却账号对应模型并换账号；恢复时间与业务码要保留 |
| `token_expired` | 有实际续期能力时刷新，再按网关策略重试同一账号 |
| `content_blocked` | 内容审核不是账号配额，不罚额度，按网关内容重试策略处理 |
| `fatal` | 不可恢复错误，保留可读原因并返回客户端 |

HTTP 200 仍可能是业务失败、流内 error 或固定拒绝话术；需要插件解析和 `is_error_response` / 拒绝标记共同处理。`refusal_finishes` 属于内容拒绝，`refusal_markers` 属于账号配额，不能混。

429 可能是额度耗尽、速率窗口、设备 / 账号限制，不只看状态码。区分分钟级与实际额度重置窗口，不能全写次日零点。`error.policy`、`retry.decision`、`retry_advice`、`cooldown_models` 等按宿主实际调用名称实现；未知操作返回明确 unsupported，不能返回空成功掩盖缺口。

SDK 错误中的 `status`、`code`、`upstream_code`、`reset_at`、`credential_scope` 按需保留，防止本来可轮换 / 可续期的错误被折成 502。对上游返回的业务报错详情（如非 200 JSON / 原文提示），做好敏感凭证脱敏后尽量透传真实原因与状态码，不要二次覆盖成笼统未知提示。WASM 的会话出口同样要接通错误分类，不能沿用「会话错误一律 Fatal」的旧假设。

### 2.7 签到、活动与插件任务

- 每日动作声明 `capabilities.checkin` 与 `checkin` 元数据：说明、`daily`、`modes`、访客排除等，与实际实现一致。新包显式写 `daily`，避免默认值混淆。
- 只有活动领取、没有每日动作时用 `daily: false`，并按实际声明 `plans` / `onboarding` / `growth`，不能把活动账号拉进每日签到。
- 标准结果是 `claimed`、`done`、`ineligible`、`inactive`、`captcha`、`failed`；没有活动不等于红色失败，验证码不等于签到成功。
- 新插件每日签到通过运行时能力加入清单，宿主使用 `pluginProvidersSeen` 一次性补齐定时范围。**不修改旧 `CHECKIN_PROVIDERS`、`PROVIDERS_ADDED_IN` 或 `PROVIDERS_VERSION`。** 这些常量仅服务旧名单兼容。
- 核对首次补齐、用户取消勾选不再补回、停用 / 卸载后不执行且保留配置、重新安装可恢复。不能每次读配置都全选。
- 特殊保活、成长、奖励、维护任务用 `tasks.v1`、`tasks.list`、`tasks.run`，可附 `tasks.categories`。任务分类只决定界面位置，**不授予权限**；同一每日动作不要又进签到调度又进插件任务导致双跑。
- 活动状态用 `claim.status` / 现有只读管理入口，写动作独立。付费兑换、消耗兑换码或改变套餐等必须明确用户意图；页面打开 / 状态刷新不能自动提交。
- 超时、取消或未知领取结果不盲目重领；回读上游状态并沿用宿主台账和已存在的受控入口。

## 3. 前端：先声明，不再逐家写死

WASM 不能自带 HTML / JS 页面。普通新提供商优先使用主程序已有界面，不照抄旧插件的 `ui/` 页面、iframe 或手工厂商卡片。

| 插件声明 | 宿主消费与注意事项 |
|---|---|
| `pack.json` / `ProviderInfo` | 动态提供商列表、分类、标签和包内图标；未安装 / 未启用不出现可操作入口 |
| `admin.action: auth.methods` | 动态添加账号表单；具体消费见 `api/session.rs` 与 `desktop-tauri/ui-islands/src/islands/add-dynamic-provider.tsx` |
| `account_features` + `account.view` | `/api/session.accountFeatures` 与账号卡显示；桌面及手机以声明为准，不新增旧 fallback 常量表 |
| `checkin`、`tasks.*` | 签到分组、模式、任务和说明；不能只做按钮而没有执行入口 |
| `settings_schema` | `desktop-tauri/ui-shared/plugin-provider-settings.tsx` 等共用设置界面 |
| 登录交互、敏感权限 | `desktop-tauri/ui-shared/plugin-login-*`、`plugins-permissions.tsx` 等现有组件 |

新包实现 `auth.methods`，不要只靠旧 `login_methods` 兜底。方法按现有 `oauth` / `api` / `sms` 契约返回，`methodIndex` 稳定且不重复；条件字段、选项、敏感输入、说明与真实提交形状对齐。

- 登录方式要区分桌面独立窗口、系统浏览器、本机导入、远程手动回调或凭证粘贴。上游不接受回调时明确限制，不能渲染一个必然失败的入口。
- 短信、验证码、托管登录按现有独立契约；flow、一次性输入、取消和回调归属由宿主核对。
- 选择框显式传当前选项的展示文案，不依赖 value 自动显示。
- 必须核对桌面、网页面板、手机端都能消费声明，不把桌面本机能力宣传为服务端 / 手机直接可用。
- 通用界面确实缺能力时，先定前后端契约，再按功能纵向闭环修改 `ui-shared` 与两端消费方；不要只给一家堆私有页面。

## 4. 多语言、文案与分发落点

### 4.1 多语言

主程序中文仍是源语言，译文在 `desktop-tauri/i18n/i18n-src/{en,ja,ko,pt-BR,zh-Hant}/`。主程序新增文案时扫描、补译并生成产物；插件动态返回的说明和表单不会因为主程序扫描自动获得翻译，应按 `auth.methods` 收到的 locale 等现有契约自行提供六语文案。

主程序前端变化涉及的构建入口（共享组件在 `desktop-tauri/ui-kit/`）：

```bash
npm run ui:islands
npm run mobile:build
npm --prefix desktop-tauri/i18n run scan
npm --prefix desktop-tauri/i18n run build
```

译文补齐后再生成词典；按实际改动重建相关产物，不是每次插件变化都重建全部主程序。岛产物在 `desktop-tauri/ui/islands/`，手机网页产物与缓存脚本在 `desktop-tauri/ui/m/` 与 `desktop-tauri/ui/sw.js`，词典在 `desktop-tauri/ui/i18n/`；仓库的 `.githooks/pre-commit` 与 CI 工作流 `.github/workflows/frontend-artifacts.yml` 会按字节核对生成产物一致性。CI 的译文与产物检查是现有要求，但**本 Skill 不授权忽略用户「不运行测试 / 检查」的指示**。

### 4.2 支持说明

- 只改某个插件的能力时，在对应插件仓库当前维护的 README 语言版本中同步；当前两家主要维护英文与简体中文两份，不默认新建六份插件 README。
- 普通 Agent 插件看 `plugin/Agent2Providers/README.md` / `README.zh-CN.md`；网页插件看 `plugin/Agent2WebProviders/README.md` / `README.zh-CN.md`。分别标注 WASM 的实现、验收与限制。
- 主程序定位、通用功能或用户承诺变化，才同步根英文 `README.md` 和 `docs/README.{zh-CN,zh-Hant,ja,ko,pt-BR}.md`。不要把旧完整矩阵和「内置家数」塞回根 README。
- 安装 / 部署 / 升级 / 排错说明仍只维护 `docs/install.md` 与另外五份译文，产物与参数变化六份同步；不额外创建使用文档。
- 旧 `ui/provider-docs.json` 属于原生包的能力说明通路。**当前 WASM 禁止 `ui/`，打包器也不包含它**，不能为了复用说明数据就拷进 WASM 包；逐项说明的宿主契约不足时明确缺口，不静默假装已支持。

### 4.3 构建与发布边界

开发构建使用现有脚本，不沿用旧的分平台 `scripts/pack.mjs`：

```bash
# 从 Agent2API 仓库根执行；只在用户请求实现 / 构建的范围内运行
node plugin/Agent2Providers/wasm/build.mjs <id>
node plugin/Agent2WebProviders/wasm/build.mjs <id>
```

构建器按对应 WASM 工作区 `--locked` 构建 `wasm32-unknown-unknown`，打出 `<pack-id>-<version>-wasm32-unknown-unknown.zip` 与 `.sha256`。Web 构建复用相邻 Agent2Providers 仓库的 SDK 和工具，缺少副本时明确报缺，不切回原生构建。

本地开发包可以未签名，但必须如实显示未签名与未验收，不冒用官方密钥。要改版本、签名配置、CI、官网 / CDN 发布与目录清单、tag、Release 或推送，另按 `release-flow` 和用户确认执行；本 Skill 不授予这些操作权限。仅更新本规范时不构建、不安装、不发布目录。

## 5. 按功能纵向闭环施工

1. **证据与契约**：上游形态、身份拆分、六项能力状态；确定宿主是否已有全部必要能力。
2. **包与握手**：独立工作区成员、两份清单、ABI、唯一身份、动态登记。
3. **账号与界面**：`auth.methods`、规范化、稳定身份、公开视图、声明式表单；先做到可添加与安全展示。
4. **目录与选路**：静态 / 在线 / 账号目录、模型能力、缓存持久化与账号权限隔离。
5. **对话与错误**：请求、流式、工具循环、思考 / 图片、usage、错误分类、取消与真实结束。
6. **维护能力**：按事实接续期、余额、签到 / 领取、插件任务，每项对齐声明和界面。
7. **文案与交付**：相关多语言、实际受影响的前端产物、插件支持状态与迁移限制。

不要先写完整后端再补全部前端。涉及两端时先确认契约；未经用户允许不创建子智能体。单个新文件尽量不超过 800 行，优先按上游职责拆分。

## 6. 完成度验收清单

验收口径保留，但不等于授权自动测试。**遵守用户要求：不新增测试文件，不擅自运行测试、真实账号探测或触发签到 / 领取。** 未运行的项目必须写「未验收」，不能据源码、编译或签名宣称通过。仅更新 Skill 时做静态核对即可。

### 6.1 基础与安全核对

- [ ] 包内唯一 provider、唯一 WASM 入口，包身份与握手一致，分类和图标正确。
- [ ] `requires` 存在且依赖完整，无 WASI / 浏览器绑定 / 未授权系统导入，也无 `ui/`。
- [ ] 安装 / 启用后动态登记；停用、切换、加载失败不显示假可用入口、不偷偷调用旧原生实现。
- [ ] 普通新 provider 没有新增主程序枚举、账号厂商分支、固定缓存 scope 或签到常量。
- [ ] 公开形态、表单默认值、全局设置、日志与交付材料没有凭证本体。
- [ ] 账号身份稳定，地区 / 池不混用；并发刷新采用锁内重读与比较写回。
- [ ] 本机能力、托管续期与任务有对应明确授权，不能通过 JSON 重建可信来源。
- [ ] 临时状态、账号状态、模型快照符合各自生命周期，升级不覆盖用户设置。

### 6.2 六项能力与三端闭环

| 能力 | 用户要求验收时的动作 |
|---|---|
| LLM 请求 | 钉住当前 WASM 包、provider、账号和模型，检查流式、工具往返、usage；额外能力逐项验；取消 / 中断不自动重发 |
| 自动续期 | 用具备实际续期材料 / 授权的账号核对临期刷新、401 恢复、并发写回与公开有效期；只持 access token 的账号不假报支持 |
| 模型列表 | 分别核对静态与远程来源、手动强刷、成功空目录、失败保留、重启恢复；账号 A 的目录不授权账号 B |
| 余额查询 | 核对真实读数、单位、缺少查询凭证时中性提示；未知不是 0，比例不是金额 |
| 签到 | 动态分组、访客 / 地区排除、每日窗口、手动结果与定时台账；首次加入、取消不补回、停用不执行 |
| 领取类 | 查询只读、明确领取、无活动中性返回；验证码 / 未知结果如实显示，不自动兑换或重领 |

桌面、网页面板、手机端分别检查声明式登录、账号能力、设置与任务显示。本机导入或回调受平台限制时，远程端必须给出能理解的限制或可用替代登录方式。

### 6.3 现有探测脚本（只有用户要求时才运行）

`scripts/probe-provider.mjs` 与 `scripts/probe-cases.mjs` 仍可用于网关侧行为探测，不是旧二进制插件验收器：

```bash
node scripts/probe-provider.mjs \
  --base http://127.0.0.1:3065 \
  --key <仅允许目标提供商的网关Key> \
  --provider <id> \
  --model <模型名>
```

**必须自行确认 Key 只允许目标 provider，并确认当前已加载目标 WASM 插件包。** `--provider` 不是路由强制开关；当前脚本只查健康、模型存在及 `owned_by`，缺 Key 只警告，**不会检查 Key 的完整 provider 白名单**（旧原生动态库已停止加载，运行中可排除回退旧原生）。不能再沿用旧规范「脚本会自动保证仅测试这家」的说法。

| 用例 | 当前口径 |
|---|---|
| `stream` | 必需项，流式 SSE |
| `tool_call` | 必需项，目录不支持工具也记 FAIL，不用 SKIP 掩盖 Agent 能力不足 |
| `tool_roundtrip` | 必需项，工具结果回灌；前置未通过时跳过，不能把跳过当通过 |
| `parallel_tools` | 只返回一个调用可记 INFO，不机械当适配错误 |
| `image` / `reasoning` | 可选项，目录未声明时可 SKIP；`--force` 仅在用户要求时强探 |
| `json_schema` / `usage` | 结构化输出与非零 usage，不能从通过推导余额 / 自动续期可用 |
| `context_window` | 只读目录声明，不是实际长上下文行为证明 |

退出码 `0` 只表示本次没有记录到必需项 FAIL，`1` 为必需项失败，`2` 为用法 / 前置错误。使用 `--cases` 缩小范围或发生 SKIP 时，**不能凭退出码 0 声称所有必需项通过**。

仅文本支持与完整 Agent 客户端支持分开说明；没有工具闭环不能宣传完整 Agent 适配。主程序工具模拟通过也要标注模拟边界，不能改称上游原生支持。

## 7. 调试与静默失效排查

先查现成设施，不重造抓包器，也不假定旧路径覆盖了全部 WASM 专用入口：

| 要看什么 | 当前落点 |
|---|---|
| 包是否安装、启用、加载完成，ABI 与能力 | 插件管理、启动状态与 `GET /api/plugins` |
| 为什么提供商不出现 / 被其他包接管 | `core/plugin_host/bootstrap.rs`、`registry.rs`、`ownership.rs` 与启动失败原因 |
| 登录按钮 / 表单不对 | `api/session.rs` 的动态登录元数据校验、`auth.methods`、现有共用登录组件 |
| 模型刷新与重启恢复 | `core/plugin_host/wasm/catalog.rs`、`rpc_cache.rs`、`core/providers/catalog_refresh.rs` |
| 签到分组 / 定时范围 | `core/auto_checkin.rs`、`core/checkin_schedules.rs`、`core/billing/checkin.rs` 与声明 |
| 最终上游报文、原始响应、选路、重试与 usage | 现有调试模式及请求日志「原始报文」；未覆盖的专用路径如实注明，不承诺一律全覆盖 |
| 钉住 provider 与账号的单次模型请求 | 现有模型测试入口 `POST /api/models/test`，仅在用户要求时调用 |

「能安装但没出现」先看握手、暂停名单、包 / 单项开关与归属，不给主程序新增硬编码 id 解决。目录总是兜底先查 `model_refresh`、账号上下文和返回结构，不去旧 `ALL_SCOPES` 补常量。

## 8. 旧原生插件迁移的边界与常见误区

- **旧非 WASM 原生插件已停止支持并拒绝加载。** `core/plugin_host/support.rs` 会将非 `.wasm` 旧包拦截为不受支持的旧格式（CI `.github/workflows/rust-tests.yml` 也已移除原生 `--plugins` 集成测试）；`plugin/Agent2Providers/crates/providers/`、`plugin/Agent2WebProviders/crates/providers/` 仅作协议与迁移参考，不是新增模板。
- **保留 provider id、旧账号、设置与授权边界。** 换包不能重命名身份或默认重建所有账号；不自动迁移 / 删除用户数据，不给存量导入账号补敏感权限。退场映射以代码为准（例如 `cline-pass` 已转入内置直连模板，而 `cline-free` 保留独立 WASM 包与桌面凭据刷新）。
- **加载失败不能回退旧原生，也不得绕过归属控制。** 当前归属优先级、支持门禁与旧包特殊迁移以 `core/plugin_host/ownership.rs`、`support.rs` 和 `providers/availability.rs` 为准。
- **主程序旧 `ProviderKind` / `PROVIDERS` 是历史身份与少量真实内置执行体，不是插件登记入口。** 新插件走运行时注册表；只为宿主真正的新内置功能修改它时，先说明这是另外一种任务。
- **无需给每个插件新建厂商前端表、账号存储分支、缓存 scope、签到版本。** 有遗留 fallback 不代表需要继续扩展。
- **`verified`、签名与权限是三件事。** 已签名不等于真实验收，权限声明不等于用户批准，未声明能力不等于上游没有。
- **原生表的 ✓ 不会自动迁到 WASM。** 特别核对模型在线刷新、余额单位、工具续接、登录 / 本机导入、续期材料、签到台账和领取入口。
- **`ui/provider-docs.json` 不能直接塞入 WASM。** 如需新说明通路，先确定受控宿主契约，不放开可执行网页限制。
- **先确认输出是否已经开始，再决定重试。** 不把取消、流中断或未知领奖结果改写成普通成功，也不重复上游副作用。

## 9. 当前参考实现

按功能参考当前源码，不能整包照抄所有权限或未验收能力：

- `plugin/Agent2Providers/wasm/atomcode/`：独立包、账号规范化、动态登录、比较写回、账号目录与流内失败识别。
- `plugin/Agent2Providers/wasm/workbuddy/`：对话、图片生成 / 编辑、成长与奖励任务；国内 / 国际身份隔离。
- `plugin/Agent2Providers/wasm/zcode/`：声明式设置、每日窗口、活动与验证码受控流程，以及旧无 user id 凭证的兼容约束。
- `plugin/Agent2WebProviders/wasm/deepseek-web/`：网页会话、图片输入、工具模拟声明与未确认能力的明确关闭。
- `plugin/Agent2WebProviders/wasm/tabbit/`、`tabbit-intl/`、`tabbit-common/`：双地区独立包与共享实现、短信登录、在线目录、百分比额度。

旧 ModelScope 接入提交以及原生插件编写指南只用于理解历史，不再作为「新 provider 必须修改哪些主程序文件」的施工清单。
