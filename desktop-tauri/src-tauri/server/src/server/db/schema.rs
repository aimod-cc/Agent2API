//! 建表 DDL 与 schema 版本管理（`PRAGMA user_version` 逐版本升级）。
//!
//! ── 为什么自己写十几行而不用 rusqlite_migration ─────────────
//! 迁移库解决的通用问题是「一堆版本文件 + 校验和 + 回滚 + 断点续跑」。本项目
//! 的表**一共 6 张、只增不改**（旧列不删、新列靠可空的 `data` JSON 兜底），
//! 版本推进就是「比大小 → 跑 DDL → 写版本号」三步。引入第三方迁移框架会带来：
//! 一个新依赖、一套它自己的版本表（与 `user_version` 二选一，反而多一份状态）、
//! 以及「框架怎么理解我们的幂等语义」这类需要读源码才能确认的事。十几行确定性
//! 代码可以逐行看懂、出错时能直接对着库文件查，符合本项目一贯取向。
//!
//! ── 为什么全部 DDL 集中在本文件 ─────────────────────────────
//! 表结构是**跨 store 的公共契约**：写侧（各 store 的接线切片）与读侧（API）
//! 分属不同文件、不同任务，两边的字段口径必须有一处唯一的事实来源。字段散在
//! 各 store 的 `CREATE TABLE` 里，加一列就会出现「谁先建谁生效」的库文件差异
//! （同一个版本，A 机器建的库有这列、B 机器没有）。集中之后「库长什么样」只
//! 用读这一个文件。
//!
//! ── 版本推进方式 ────────────────────────────────────────────
//! 读 `user_version`（默认 0 = 全新库），对它到 [`SCHEMA_VERSION`] 之间的
//! 每个版本按序调用 [`apply_version`]，最后把 `user_version` 写成
//! [`SCHEMA_VERSION`]。**每推一个版本都是一个独立事务**（DDL 与版本号一起
//! 提交）：中途失败时已完成的那几个版本保留、最后一个整体回滚，下次启动从
//! 落库的版本号继续 —— 不会出现「表建了一半但版本号已经写新」的错位。
//!
//! ── 为什么所有语句都带 IF NOT EXISTS ────────────────────────
//! 版本号是**唯一**的推进依据，但现实里存在「表已经在了、版本号却是 0」的
//! 情形：用户手工拷过库文件、或者从旧版本二进制回退再前进。DDL 幂等之后这种
//! 库能直接跑过去（缺的补上、有的跳过），而不是在启动时报一句
//! 「table already exists」把用户挡在门外。
//!
//! ── kv 表的键命名规范（后续切片按此约定往里写）──────────────
//! `kv` 是「不属于某张实体表的零散持久状态」的落点，键分两类：
//!   - **网关配置（原 `config.json`）的顶层键直接用键名**，一字不改：
//!     `apiKeys`、`modelRules`、`logRetentionDays`、`debugMode`……
//!     为什么保留原名而不是改成 snake_case：这些名字同时是 **HTTP API 的
//!     契约**（`GET /api/config` 的响应体、前端 `config.X` 的读法），改名会
//!     让前端、账号迁移、模型规则三处同时受影响；数据库的列名与 JSON 键
//!     一一对应，排障时能直接把库里的值贴进配置文件比对。
//!     值的形态也与配置文件一致：**值就是那段 JSON**（标量存 `"30"`、
//!     对象存 `{...}`、数组存 `[...]`），所以 `config.json` → `kv` 的迁移
//!     是「顶层键逐个搬」，不需要为每种类型单独定编码。
//!   - **其它零散状态用固定名**（驼峰，与前面同类）：见 [`RESERVED_KV_KEYS`]。
//!     注意 `desktopSettings` 存的是**整份**设置对象而不是拆成多个键 ——
//!     桌面设置是壳侧 `settings.rs` 的整体读写单元（前端一次 `PUT` 整份），
//!     拆键会让「一次写入」变成多行更新，徒增事务与冲突面。
//!
//! ── 两类键名为什么绝不能相撞（T7 起这是一条硬约束）──────────
//! `config.json` 进了 `kv` 之后，「哪些行属于配置」不再由**表的边界**表达
//! （改造前配置独占一个文件），而是由键名是否落在 [`RESERVED_KV_KEYS`] 里
//! 区分。`config::save_raw` 的「删除已不存在的键」要按这个集合做排除 ——
//! 配置项的名字若与某个固定键撞了，写一次配置就会把那份状态**删掉**
//! （比如配置里出现 `desktopSettings`，用户改一次 API Key 就丢了桌面设置）。
//! 当前配置项（`apiKeys` / `modelRules` / `logRetentionDays` / `debugMode` /
//! `sanitizeBlacklistFingerprints` / `promptMode` / `promptFile` /
//! `scheduledTasks` / `autoCheckin` / `locale` / `lastRequestModel` /
//! `providerRoute` / `updateProxy` / `updateBeta` / 三个 `*Dir` / 三个 `*RetentionDays` /
//! 三个 `retry*` / 六条 `scheduledTasks.*` 子键 / 三个 `*Imported` 标记 /
//! 旧字段 `apiKey`）与固定键**无冲突**，逐项核对过（全仓 `update_raw_field` /
//! `raw.insert` / `KEY_*` 常量的取值集合 vs 下面的 `RESERVED_KV_KEYS`）。
//! 后加固定键名时**必须**回来核对一次：撞名的代价是用户的配置或状态被静默
//! 覆盖，而两处代码离得很远（一个在本文件，一个在使用方）。

use rusqlite::Connection;

/// `kv` 表里**不属于网关配置**的固定键名（模块头「两类键名绝不能相撞」讲的
/// 就是它与配置顶层键的关系）。
///
/// 为什么要有一份集中清单：`config::save_raw` 必须知道「哪些行不归我管」，
/// 才能在写配置时把「配置里已删掉的键」精确删掉而不误伤别人的状态。没有这份
/// 清单，那个删除只能放宽成「删掉所有不在新配置里的键」—— 桌面设置、账号
/// 优先级作用域、日志下一个 id 会在用户改一次 API Key 时全部消失。
///
/// 它是**事实来源**：各使用方自己的常量（`logs_store::sql::NEXT_ID_KEY`）
/// 仍是它们各自读写时用的名字，本清单只服务
/// 「配置写侧要排除哪些键」这一个判断。两处若不一致，症状是配置写入误删状态
/// —— 所以新增固定键时**两处都要加**（使用方常量 + 本清单），
/// [`is_reserved`] 是唯一的判断入口。
///
/// 清单里可能有**没有使用方常量**的项（如遗留的 `desensitize`）：那些是
/// 「功能已删、数据留着」的键，登记它们的唯一目的就是让配置写入别去动它。
pub const RESERVED_KV_KEYS: &[&str] = &[
    // 账号优先级的全局作用域（core::account_store::sql）
    "priorityScope",
    // 事件日志的下一个 id 水位（logs_store::sql）
    "logsNextId",
    // 整份桌面设置 JSON（壳侧 settings）—— 一个键而不是拆成三个：
    // 桌面设置是整体读写单元（前端一次 PUT 整份），拆键会让一次写入变成
    // 多行更新，徒增事务与冲突面。
    "desktopSettings",
    // **遗留**：旧「敏感词脱敏」的整份状态（词表 / 开关 / 作用角色 / 作用提供商）。
    //
    // 那个功能已随规则集换成硬编码指纹脱敏（`core::sanitize`）而整体删除，
    // 这个键**不再有任何读写方**。仍然登记在这里是有意的：它意味着配置写入
    // 不会顺手把这行删掉 —— 老用户库里那份自定义词表原样留着（想查还能
    // `sqlite3` 看），而不是因为改一次 API Key 就静默消失。真要清理时手工
    // 删这一行即可。
    "desensitize",
    // 面板管理员（server::access）：{username, hash} 整份 JSON 一个键 ——
    // 首次部署在面板上注册产生（环境变量预置时优先于它），属于「其它零散
    // 状态」一类：不是配置项，配置写入绝不能动它。
    "panelAdmin",
    // 面板刷新令牌（server::access）：整份记录数组一个键（条目是个位数：
    // 每设备一条活链），轮换/撤销都是整键重写。刷新令牌在库里只存 sha256。
    "panelTokens",
    // 面板 access 会话表（server::access）：与 `panelTokens` 同一形状、同样只存
    // sha256。落库的理由是「重启不该把所有已登录的浏览器打成无效」—— 重启前它们
    // 只在内存，重启后浏览器带着还在的 cookie 撞 401，而 401 又不清 cookie，
    // 现场表现为「同一把对不上任何登录的指纹反复出现几十次」。
    "panelAccessSessions",
    // ALTCHA 机器人校验的 HMAC 密钥（server::altcha）：首次签发 challenge
    // 时生成并落库，只经 HMAC 使用、从不外发。属于「其它零散状态」，
    // 配置写入绝不能动它。
    "panelAltchaSecret",
    // config.json 迁移的完成标记（db::migrate::config）。为什么配置这一项
    // **需要**标记键而其余项不需要：配置项是开放集合，没法问「配置迁过了
    // 没有」（理由与完整论证见 db::migrate::config 的模块头）。
    "configMigrated",
    // 请求明细 / 聚合的迁移完成标记（request_stats::legacy）。
    //
    // 这两项用标记键，而 `logs` / `accounts` / `debug_traffic` 三项不用，
    // 原因是**那三张表有可用的业务主键**（历史行号 / 记录 uuid / 请求关联 id），
    // 「这条已经导过了」能直接由主键判断（`INSERT OR IGNORE`）；而
    // `requests` 的主键是自增 `row_id`（与内容无关）、`id` 列在旧行里可能是
    // 空串，`request_daily` 的 `date` 又会被运行期 UPSERT 改写 —— 都当不了
    // 「这条已经导过了」的判据。完整论证见
    // `request_stats::legacy::MARKER_REQUESTS` 的文档注释。
    //
    // 登记在这里是**必须**的：`config::save_raw` 写配置时要靠本清单排除
    // 「不归我管」的键，漏登记会让用户改一次 API Key 就把这两个标记删掉
    // —— 下次点「升级」会重复导入一遍历史明细。
    "requestsMigrated",
    "dailyMigrated",
    // 模型用量口径订正的完成标记（request_stats::aggregate 的
    // `MODEL_UPSTREAM_MARKER`）—— #139 的根因就是这个键漏登记：
    // 它在 `remap_model_days` 里**直接写库**（与重算同一事务，有意的：
    // 中断后标记必然没写、下次整批重跑），而 `config::init` 把全库读进内存
    // 快照发生在那之前 —— 不登记的话，用户改任意一项配置时 `save_raw` 会把
    // 它当「配置里已删掉的键」清掉，下次启动同一批历史被重算第二遍
    // （日期键未变时是幂等覆盖、静默；变过日期键就留下重复的聚合行）。
    "modelUsageUpstreamRemapped",
    // 远程模型清单的持久化缓存（core::providers::catalog_cache）：整份
    // 「各家上次成功拉到的清单」一个键（十份清单挤一个键的理由见那个模块头）。
    // 属于「其它零散状态」——它不是配置项，配置写入绝不能动它。
    "modelCatalogCache",
    // 后台任务的持久化排期与执行状态（core::task_state）：整份「每条任务的
    // 上次尝试 / 上次成功 / 下次执行 / 失败冷却 / 缓存值」一个键。它替代了
    // 改造前「排期只在内存里、重启即从头」的形态 —— 于是重启不再让
    // GitHub 检查、余额查询、模型刷新、凭证维护各多打一轮上游请求。
    // 属于「其它零散状态」：配置写入绝不能动它（否则排期归零、重启又立刻重跑）。
    "backgroundTaskState",
    // 旧「定时查询积分」的结果快照（全局任务已退役）：键保留在保留清单里是
    // 刻意的 —— `core::usage_records` 启动时把它的行导入 account_usage_records
    // 表后删除本键；若那次迁移失败，这里不让配置写入把唯一的迁移来源误清掉。
    // 迁移成功后这个键不再出现，条目留在清单里只是防御。
    "usageQuerySnapshot",
    // 出网代理池（core::proxy_pool）：整份 `{items: [...]}` 一个键 ——
    // 「网络代理」页维护的命名代理，账号可按 id 引用。属于「其它零散状态」：
    // 它不是配置项（配置写入绝不能动它，否则用户配好的代理会被改一次 API Key
    // 清空）。条目不落独立表、只整份读写，理由见那个模块头。
    "proxyPool",
    // 代理设置（core::proxy_settings）：`{listenerPortBase, listenerPortEnd,
    // testUrl}` 一个键 —— 「网络代理」页设置弹窗维护的节点监听端口范围与
    // 出口测试网址。与 proxyPool 同类零散状态：不是配置项，配置写入绝不能
    // 动它（否则用户改一次 API Key 端口范围就退回默认、大订阅又分配不出端口）。
    "proxySettings",
    // 订阅（core::subscriptions）：整份 `{items: [...]}` 一个键 —— 「网络代理」
    // 页「订阅管理」维护的订阅链接，内核（sub_kernel）拉起时从 api 层拿启用
    // 清单。与 proxyPool 同类零散状态：不是配置项，配置写入绝不能动它。
    "subscriptions",
    // OfficeAce 模型可用性探测的结论（core::providers::officeace::probe）：
    // 整份 `{unavailable:[…], probedAt, account}` 一个键。探测「目录里列着、这个
    // 号实际打不通」的模型（上游回 81004/81009），结论由 `advertise_models` 用来
    // 默认隐藏它们。属于「其它零散状态」——不是配置项，配置写入绝不能动它
    // （否则每次改配置都让隐藏集合清零、客户端又照着一个一个试）。
    "officeaceModelProbe",
    // models.dev 模型元数据的刷新结果（core::model_meta::snapshot）：整份
    // 「上次成功拉到的精简索引」一个键（文件形态与内嵌快照逐字相同，生成口径见
    // `scripts/sync-modelsdev.mjs`）。**不是**配置项 —— 配置写入绝不能动它，
    // 否则用户改一次 API Key 就退回发版时那份更旧的内嵌快照。
    // 与上面的 `modelCatalogCache` 是两码事：那个存各家上游的**模型清单**，
    // 这个存模型的**元数据**（能力位 / 思考档位），刷新链路与失效语义各不同。
    "modelsDevCatalog",
];

/// 这个键是否属于「其它零散状态」（即不归网关配置管）。
///
/// 唯一入口：`config::save_raw` 用它决定哪些行不参与「删除已不存在的键」。
/// 做成函数而不是让调用方自己 `.contains()`：将来若固定键改成带前缀的命名，
/// 改一处即可（与 `db::migrate` 各迁移项包一层 `sql::has_key` 同一手法）。
pub fn is_reserved(key: &str) -> bool {
    RESERVED_KV_KEYS.contains(&key)
}

/// 当前 schema 版本。加表 / 加列时 +1，并在 [`apply_version`] 里补一个分支。
///
/// 为什么从 1 而不是 0 起步：0 是全新库的初始值，用它当「已是最新版」会让
/// 全新库跳过建表。1 起步后「库里什么都没有」与「schema 版本 1」是两件事。
///
/// ── 版本 2：requests 表加两个 JSON 文本列 ──────────────────────
/// 见 [`V2_SCHEMA`]。新库会按 1 → 2 的顺序跑完（v1 建表时**不含**这两列，
/// 由 v2 的 ALTER 加上），所以「全新库」与「从 v1 升上来的库」最终形态一致 ——
/// 这也是为什么 v1 的 DDL **不**回填这两列：那里改了会让两条路径分叉
/// （新库有列、老库经 ALTER 也有列，但列的定义来源变成两处，下次改动容易漏一处）。
///
/// ── 版本 3：request_raw 表（原始报文，预览对话功能的地基）───────
/// 见 [`V3_SCHEMA`]。与 v2 的差别：这次是**新表**而不是加列，所以走
/// `CREATE TABLE IF NOT EXISTS`（不需要 ALTER，也就不需要「v1 不建、v2 补」
/// 那种两段式）。老库（user_version=2）与全新库都会跑 v3 —— 老库靠这一步
/// 建出表，全新库在 v1 里**也不建**它（定义只有 v3 一处，与 v2 那条
/// 「不要把列塞回 v1」的纪律同理）。
///
/// ── 版本 4：把 v3 的建表语句**再幂等地跑一遍**（存量库修复）──────
/// 见 [`V4_SCHEMA`]。这不是一次结构变更，而是对「版本号写着 3、表却不在」
/// 这类存量库的修复：实测有库 `user_version=3` 却没有 `request_raw`（早期
/// 二进制曾在这个版本号下放过别的 DDL，撞号了）。后果是记账收尾事务里那句
/// `DELETE FROM request_raw` 报「no such table」→ **整个事务回滚** → 明细的
/// 进行中行永远收不了尾（界面上表现为「只有进行中、没有结束」）。
/// v3 的 DDL 本身是 `CREATE TABLE IF NOT EXISTS`，重跑一次零成本：
/// 表在就跳过，表不在就补上，两种库的最终形态一致。
///
/// ── 版本 5：requests 表加两个思考等级列 ──────────────────────
/// 见 [`V5_SCHEMA`]。与 v2 完全同一形态（既有表加列，ALTER 两连）：
/// v1 的 DDL 不回填，两条建库路径（全新库跑 1→5 / 老库从任意版本升上来）
/// 得到同一份表结构。
///
/// ── 版本 6：requests 表加阶段与阶段计时两列 ───────────────────
/// 见 [`V6_SCHEMA`]。同样是既有表加列（ALTER 两连）。这两列只在**在途**
/// 期间有值，收尾时一律清空（见 `request_stats::sql` 的各条收尾语句）——
/// 「有没有阶段」因此就是「这一行还在跑」的第二个读数，与 status=0 同进同退。
///
/// ── 版本 7：requests 表加「测试来源」一列 ─────────────────────
/// 见 [`V7_SCHEMA`]。同样是既有表加列（单条 ALTER）。模型测试走的是真实转发
/// 链路，它产生的请求会照常进请求日志 —— 这一列把它与真实流量分开，
/// 于是报表聚合能把它排除（测试是人工反复发起的样本，混进去会把真实流量读歪），
/// 而请求日志那边照旧能看到它、并标成「测试」。
/// ── 版本 8：account_usage_records 表（每账号的余额查询记录）─────
/// 见 [`V8_SCHEMA`]。与 v3 同一形态（新表，`CREATE TABLE IF NOT EXISTS`，
/// 老库与全新库都会跑这一版建出它）。它取代旧 kv 快照 `usageQuerySnapshot`
/// 成为「每账号最近一次余额查询结果」的权威存放处：全局「定时查询积分」
/// 退役后查询按账号各自到期触发，一行一账号才能逐条更新、逐条被选路读取
/// （kv 整份读改写撑不住这个粒度）。旧快照的数据由 `core::usage_records`
/// 启动时导入，升级后余额列不会变空白。
///
/// ── 版本 11：checkin_records 表（签到记录的每账号明细台账）───────
/// 见 [`V11_SCHEMA`]。与 v3 / v8 同一形态（新表，`CREATE TABLE IF NOT EXISTS`）。
/// 它是签到中心「签到记录」页的数据源：每执行一次签到，每个账号一行
/// （领成功 / 已领过 / 失败及其错误原文 / 收益文案），补上改造前「只有
/// config.json 里一条聚合同 lastResult」看不见的明细。写入方是
/// `core::checkin_records`（记账点唯一：`core::billing::checkin::run_checkin`）。
///
/// ── 版本 12：requests 补「来源客户端」一列 ─────────────────────
/// 见 [`V12_SCHEMA`]。与 v5 / v10 同一形态（既有表加列，单条 ALTER）。
/// 请求日志的「客户端」列读它：入口 handler 从下游 `User-Agent` 按
/// `core::client` 的规则表判出客户端类型（`Codex` / `ZCode` / `Claude Code`
/// …），随记账落库。存归类结果而不是原始 UA：UA 带版本号，逐条存既撑大
/// 明细也读不出「是谁」。旧行 DEFAULT '' = 没有记录（那时还没采集），
/// 展示层显示占位符 —— 不给旧数据猜值。
///
/// ── 版本 13：清空 `request_raw` 的历史正文（一次性空间回收）─────
/// 预览对话的下游正文从本版起改为**仅调试模式开启时采集**
/// （对齐 OmniProxy 的口径：不开调试模式就不保存任何原始正文），而升级前
/// 无条件采集下来的那批正文在此清空 —— 它们对用户不再有意义（新语义下也
/// 不会有新数据填补开关关闭的时段），留着只白占空间。
///
/// 注意这一版是**数据动作**而不是结构变更（表结构一字未动）。放在 schema
/// 版本里而不是启动期的 kv 标记：两者都「只跑一次」，但版本号是既有的幂等
/// 机制（`user_version` 与语句同事务提交），复用它就是零新增状态 —— 那条
/// kv 标记会变成第二个「这条数据动作跑过没有」的事实来源。
///
/// 语句不写成 `*_SCHEMA` 常量而在 `apply_version` 的 v13 分支里单独执行：
/// 要拿**被删的行数**，`migrate` 据此决定是否紧跟着 VACUUM（把空闲页真正
/// 还给文件系统 —— 光 DELETE 不会让库文件缩小，而「省空间」正是这一版的
/// 目的），完整说明见 `migrate` 尾部那段。
///
/// ── 版本 14：账号 `data` 列改为密文（`core::crypto` 的 `enc1:` 信封）──
/// 这一版**没有任何 DDL**，也不清任何历史行（`cleaned_raw_rows` 写 0）：
/// 它只是一个**内容形态的标记**。`accounts.data` 从明文 JSON 变成 AES-256-GCM
/// 密文，而这件事没有列要加、没有表要建 —— 加密是逐行的内容变换，由
/// `core::account_store::store_admin::encrypt_legacy_accounts`（挂在
/// `account_bootstrap::run` 的启动迁移里）把存量明文行就地加密；密钥在库外
/// （`{config_dir}/account.key`，兜底 `AGENT2API_ACCOUNT_KEY`，见
/// `core::crypto` 模块头）。所以 `apply_version` 的 v14 分支是空操作。
///
/// **为什么仍要占一个版本号**（而不是只靠那段启动迁移）：因为「库里的数据形态」
/// 真的变了，而 `user_version` 是它唯一的事实来源。升到 14 的库，其 `data` 列
/// 应被当成密文；这条事实**在旧程序那一侧才有用** —— `migrate` 新加的「库版本
/// 高于程序版本就拒绝打开」判据（见 [`migrate`]）正建立在版本号上：旧程序
/// （`SCHEMA_VERSION <= 13`）读到 v14 库会明确失败，而不是把密文当坏数据跳过、
/// 再在写回时把「读不出来的行」当成「内存里没有」DELETE 掉（即「回退一次 =
/// 账号被删」）。对本程序，v14 只是一次空操作。
///
/// ── 版本 15：requests 补两条索引（`status` 与报表窄列覆盖）─────
/// 见 [`V15_SCHEMA`]。两条索引都来自同一次实测：明细三万余行、库已经四百多
/// MB 的库上，两个最热的读路径分别在整表扫描与逐行回表。
///
///   · `idx_requests_status`：`SELECT COUNT(*) FROM requests WHERE status = 0`
///     在**没有任何索引可用**时对整表做 `SCAN`（实测 3 万行 50 ms 量级，
///     而命中的通常只有个位数行）。这条语句不属于报表 —— 它是前端「进行中」
///     徽标的全局轮询（每 5 秒一次，与当前停在哪个页面无关，见 `ui/app.js`
///     的 `syncRunningRequests`）与请求日志列表每次查询的一部分。加索引后
///     按 status 直接定位，不再扫全表。
///   · `idx_requests_summary`：`/api/stats/summary` 的近 24 小时明细窗口
///     （`request_stats::sql::select_summary_rows`）只需要 6 列，而非覆盖索引
///     必须逐行回表 —— 明细行里躺着 KB 级的 `route` / `attempt_details`，
///     回表读回来的几乎全是被随后丢掉的字节。这条索引让该查询变成**纯索引
///     扫描**（列序与那条 SELECT 的列序一致，改动任一侧都要回来对一眼）。
///
/// 两条都是 `CREATE INDEX IF NOT EXISTS`（与其余版本的幂等纪律一致）：在存量库
/// 上建索引要扫一遍表，代价与表大小成正比、只付一次；`requests` 只有几万行时
/// 在百毫秒量级，不会让启动明显变慢。**不要把这两条往 [`V1_SCHEMA`] 里补**：
/// 新库按 1→15 依次跑，v15 那一步同样会建出来，而 v1 里多写一份等于同一条索引
/// 有了两处定义 —— 将来改一处忘一处，正是 `V4_SCHEMA` 那类错位的来源。
pub const SCHEMA_VERSION: i64 = 15;

/// 版本 1 的全部表与索引：改造前所有 JSON / JSONL 文件的对应形态。
///
/// 为什么把六张表写成一个 batch 而不是每张一个常量再拼：它们共同构成「版本 1
/// 的 schema」这一个整体，分开的常量只会让「v1 到底包含什么」需要跨常量拼接
/// 才能看清。单张表的定界用注释块表达（下方按表分块），语句本身用
/// `execute_batch` 一次提交。
const V1_SCHEMA: &str = "
-- ── kv：通用键值表 ────────────────────────────────────────────
-- 存什么：网关配置的每个顶层键、桌面设置整份、各存储的元数据、零散状态。
-- 为什么要有它：这些数据的共同点是「没有固定结构、读写都是整份进出」——
-- 为它们各建一张表，表数量会随配置项增长；塞进一个 TEXT 列则失去按 key 精确
-- 读写的能力（那是它们唯一需要的查询能力）。一列值 + 主键键名正好。
-- 为什么值是 TEXT 而不是 BLOB / JSONB：SQLite 没有原生 JSON 类型，JSON 就是
-- 文本；存 TEXT 时 `json_extract` 等 JSON 函数可以直接用来做条件查询，
-- 而 BLOB 会强制每次比较都做编码转换。值里存什么形态由写入方决定（见模块头
-- 的键命名规范），本表不做校验。
CREATE TABLE IF NOT EXISTS kv (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

-- ── accounts：账号表（原 accounts.json）────────────────────────
-- 存什么：一条账号记录一行。`data` 列存**整条账号记录的 JSON 原文**。
-- 为什么保留 JSON 原文而不是把每个字段都拆成列：「用户升级绝不能丢账号里
-- 任何字段」是本项目的硬约束（含未知字段、用户手工加的备注、旧版本遗留的
-- currentAccountId），而账号里有 `rateLimits` 这类嵌套结构、各家 provider
-- 的字段集还在演进。拆列意味着每加一个字段都要一次 schema 迁移 + 一次数据
-- 重写，且**漏掉一个字段就等于升级时把那列的数据静默丢掉** —— 这是
-- `core::account_store` 模块头第 1 条不变量，JSON 原文是唯一能自然满足它的形态。
-- 那一列的例外：需要**排序 / 筛选 / 校验唯一性**的字段额外提取成列，
-- 它们的值从 JSON 里复制一份（权威副本仍是 `data`，提取列是查询用的投影）。
--   id       主键，账号唯一标识（与 JSON 里的 id 同值）
--   provider 提供商 id（workbuddy / raccoon / cline-free…），按家分组查
--   priority 优先级数值，越小越先用（全局唯一，见账号存储不变量 2）
--   enabled  是否启用，0/1。SQLite 没有布尔类型，按惯例用 INTEGER
--   added_at 加入时间（毫秒 Unix 时间戳），同优先级时的稳定排序依据
--   data     整条记录的 JSON 原文（**权威副本**）
-- 「已启用且有凭证的第一个账号」这个派生（pick_current）走下面的复合索引：
-- 先按 provider 分片、再按 priority 升序，added_at 保证同优先级顺序稳定。
CREATE TABLE IF NOT EXISTS accounts (
  id       TEXT PRIMARY KEY,
  provider TEXT NOT NULL,
  priority INTEGER NOT NULL,
  enabled  INTEGER NOT NULL DEFAULT 1,
  added_at INTEGER NOT NULL DEFAULT 0,
  data     TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_accounts_order ON accounts(provider, priority, added_at);

-- ── logs：事件日志（原 logs.jsonl）─────────────────────────────
-- 存什么：日志页展示的运行日志，一行一条。
-- 为什么 id 是 INTEGER PRIMARY KEY 而不另建自增列：SQLite 里
-- 「INTEGER PRIMARY KEY」这一列**就是** rowid 的别名，天然自增、天然唯一，
-- 且是物理存储顺序（按 id 查是 O(log n) 的 B 树查找）。这正是原来内存里
-- 那条「自增 id」的等价物，前端按 id 增量拉取（`since_id`）因此直接可用。
-- 为什么 data 可空：结构化事件的附加字段（429 切换的 from/to 等）只有部分
-- 日志有；空串与「没有 data」语义不同，所以用 NULL 而不是默认 '{}'。
-- 为什么 ts 存 INTEGER 毫秒：原文件里就是 `Date.now()` 的毫秒数，且范围
-- 与负数场景都在 i64 内，不需要 TEXT 的日期格式（排序即数值排序才是重点）。
-- 三个索引（ts / level / category）对应日志页固定的三种过滤维度：
-- 按时间倒序翻页、按「级别及以上」筛选、按分类筛选。刻意**不做**复合索引：
-- 三个维度是独立下拉、组合方式多，复合索引只能覆盖其中一种组合，反而不如
-- 让 SQLite 自己挑（数据量级是几百到几万行，单列索引足够）。
CREATE TABLE IF NOT EXISTS logs (
  id       INTEGER PRIMARY KEY,
  ts       INTEGER NOT NULL,
  level    TEXT NOT NULL,
  category TEXT NOT NULL,
  message  TEXT NOT NULL,
  data     TEXT
);
CREATE INDEX IF NOT EXISTS idx_logs_ts ON logs(ts);
CREATE INDEX IF NOT EXISTS idx_logs_level ON logs(level);
CREATE INDEX IF NOT EXISTS idx_logs_category ON logs(category);

-- ── requests：请求明细（原 requests.jsonl）─────────────────────
-- 存什么：每次对话转发的记账明细（模型、账号、状态、耗时、token 用量）。
-- 为什么是 AUTOINCREMENT 的 row_id 而不是让 id 当主键：
--   row_id 是**自增物理行号**（内部用，保证稳定顺序、可作游标）；
--   id 是请求的**关联键**（与调试报文 debug_traffic.id 同值，靠它把明细与
--   原始报文对上），旧数据里可能是空串，**且不保证唯一**（重试、去重排队
--   的若干路径会产生同 id 的记账）。两个字段职责不同，混用会让「空串 id
--   被当成主键」直接撞唯一约束而丢掉整条记录。
-- 为什么显式写 AUTOINCREMENT：不加它时 rowid 会被复用（删掉最大行后新行
-- 会拿到同一个号），而这里 row_id 会被当作翻页游标，复用会让「翻页漏行 /
-- 重行」。代价是多一张 sqlite_sequence 表，可忽略。
-- 为什么 token 四列各自 DEFAULT 0 而不是可空：原 JSON 里缺失就是 0
-- （前端直接做加法与求和），可空会让每个读点都要处理 NULL；这四个数的
-- 「没有」与「0」在语义上也没有区别。
-- 其余 DEFAULT 同理：都是「旧数据没有这个字段」时的缺省值，含义与文件版
-- 的读取兜底（`Number(x) || 0`）逐字对应 —— 导入旧数据时不必预先补齐每个
-- 字段，缺的列走 DEFAULT 即可。
CREATE TABLE IF NOT EXISTS requests (
  row_id            INTEGER PRIMARY KEY AUTOINCREMENT,
  id                TEXT NOT NULL DEFAULT '',
  ts                INTEGER NOT NULL,
  model             TEXT NOT NULL,
  account_id        TEXT NOT NULL DEFAULT '',
  account_name      TEXT NOT NULL DEFAULT '',
  status            INTEGER NOT NULL DEFAULT 0,
  duration_ms       INTEGER NOT NULL DEFAULT 0,
  first_response_ms INTEGER,
  attempts          INTEGER NOT NULL DEFAULT 1,
  error             TEXT,
  prompt_tokens     INTEGER NOT NULL DEFAULT 0,
  completion_tokens INTEGER NOT NULL DEFAULT 0,
  total_tokens      INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens INTEGER NOT NULL DEFAULT 0,
  provider          TEXT NOT NULL DEFAULT '',
  client_model      TEXT NOT NULL DEFAULT '',
  upstream_model    TEXT NOT NULL DEFAULT ''
);
-- ts：报表按区间聚合（近 24 小时 / 7 天 / 30 天）与明细按时间倒序翻页。
-- id：由请求 id 反查明细（与调试报文对照、按 id 看一次请求的完整记录）。
CREATE INDEX IF NOT EXISTS idx_requests_ts ON requests(ts);
CREATE INDEX IF NOT EXISTS idx_requests_id ON requests(id);

-- ── request_daily：按天聚合（原 request-daily.jsonl）──────────
-- 存什么：一天一行（date 是**本地时区**的 `YYYY-MM-DD`，与文件名版逐字相同；
-- 时区处理在 `request_stats::clock`，本表只存字符串、不做时区推断）。
-- 为什么 date 直接当主键：一天一行是这张表的全部语义，主键即唯一约束，
-- 天然让「同一天重复记账」变成 UPSERT 而不是插入两行。
-- 为什么三个 *Stats 列是 JSON 数组文本：它们是「当天按模型 / 提供商 / 账号
-- 的累计」，条目数随用户配置变化（模型目录是远程刷新的）、且**整体读写**
-- （报表要整份拿出来算），拆表会让「取一天的三个维度」变成三次 join。
-- 与文件版的行内嵌数组一一对应，导入时原样搬过来即可。
-- 默认 `'[]'` 而不是 NULL：空数组与「这一列没有数据」在这张表里是同一件事，
-- 统一成 `'[]'` 让读取方少一个分支。
-- requests/successful/tokens 这几列是「当天总量」，保留成独立列而不是也从
-- JSON 里算：热力图与汇总行只读它们，拆出来才走得上主键索引。
CREATE TABLE IF NOT EXISTS request_daily (
  date               TEXT PRIMARY KEY,
  requests           INTEGER NOT NULL DEFAULT 0,
  successful         INTEGER NOT NULL DEFAULT 0,
  tokens             INTEGER NOT NULL DEFAULT 0,
  cache_hit_tokens   INTEGER NOT NULL DEFAULT 0,
  cache_input_tokens INTEGER NOT NULL DEFAULT 0,
  model_tokens       TEXT NOT NULL DEFAULT '[]',
  provider_stats     TEXT NOT NULL DEFAULT '[]',
  account_stats      TEXT NOT NULL DEFAULT '[]'
);

-- ── debug_traffic：调试报文（原 debug-traffic.jsonl）──────────
-- 存什么：调试模式下的**上游侧**原始请求/响应（头部已脱敏，脱敏不可关闭）。
-- 为什么 id 是主键：它与 requests.id 同值、是跨表关联键；一条请求只有一份
-- 报文，所以天然唯一（与 requests 不同 —— 那边同 id 可以有多行记账）。
-- 为什么 request/response 的 body 列可空、而 header 列 NOT NULL 带默认值：
-- 请求侧一定存在（我们至少发出去过），响应侧可能因为上游断连而没有；
-- 而头部哪怕没有也应该是 `'{}'`（前端按对象读）。
-- 为什么体用 TEXT 而不用 BLOB：里面是 SSE 文本 / JSON 文本，存 TEXT 才能在
-- `sqlite3` 命令行里直接看（排障场景这是主要用法）。
-- truncated 标记「体被单条上限截断过」（见 debug_traffic::MAX_ENTRY_BYTES），
-- 缺了它前端会把半截响应当完整响应展示。
-- size 是该条**落库后实际占用的字节数**（截断后的三份 JSON 文本 + 两份可空
-- 文本 + 身份字段的长度合计，见 `debug_traffic::sql::payload_bytes`），供两道闸
-- 的字节统计用（`SUM(size)` 与 MAX_TOTAL_BYTES 比较）。
-- 它**不是**「未截断前的原始大小」：闸门守的是「这张表实际占多大」，用截断后的
-- 体积才对得上。这一行原先按「将来可能存原始大小」的设想写成「未截断前」，与
-- 实现不符 —— 改造前的 entry_size 也是截断后体积（原版 record 先 truncate 再算
-- size），所以现在的口径是承接既有实现、不是本次改造新定的。将来真需要原始
-- 大小就**另加一列**（在截断前记一笔），不要复用 size。
-- 索引只有 ts：调试报文按时间倒序翻页是唯一的批量读法，其余都按主键 id 取。
CREATE TABLE IF NOT EXISTS debug_traffic (
  id               TEXT PRIMARY KEY,
  ts               INTEGER NOT NULL,
  url              TEXT NOT NULL DEFAULT '',
  provider         TEXT NOT NULL DEFAULT '',
  request_headers  TEXT NOT NULL DEFAULT '{}',
  request_body     TEXT NOT NULL DEFAULT '',
  status           INTEGER,
  response_headers TEXT,
  response_body    TEXT,
  truncated        INTEGER NOT NULL DEFAULT 0,
  size             INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_debug_traffic_ts ON debug_traffic(ts);
";

/// 版本 2：给 `requests` 补两列 JSON 文本（重试链 + 敏感词命中）。
///
/// ── 这两列解决什么 ──────────────────────────────────────────
/// 请求日志的「重试」列原先只能显示一个「重试」标记（只知道次数），因为
/// 「每一轮换了谁、为什么换」与「这次命中了哪些敏感词」在明细里都没有落点 ——
/// 前者只在转发链路的局部变量里存在过，后者被脱敏模块的聚合统计吃掉。
/// 两列补上之后，那颗标签才有内容可弹（见 `ui/requests-panel.js` 的重试列）。
///
/// ── 为什么是「可空的 JSON 文本」而不是拆表 ─────────────────────
/// 它们是**一条明细的附属历史**，读写都是整份进出、没有按单次尝试查询的
/// 需求（要排障就在 `sqlite3` 里对 JSON 列用 `json_each`）。与 `request_daily`
/// 的三个 `*Stats` 列同一取舍 —— 理由在 `request_stats::record::RequestEntry
/// ::attempt_details` 的注释里写得更细。
///
/// ── 默认值为什么是 `'[]'` 而不是 NULL ─────────────────────────
/// 与 `request_daily` 那三个列一致：空数组与「这一列没有数据」是同一件事，
/// 统一成 `'[]'` 让读取方少一个分支。写入侧（`sql::encode_json_list`）也把
/// 序列化失败兜成 `'[]'`，两边形态一致。
///
/// ── 为什么用 ALTER 而不重建表 ────────────────────────────────
/// 两张表的其它列与索引一个都不动，重建要「建新表 → 搬数据 → 删旧表 → 重建索引」，
/// 而 `requests` 是全量保留的明细表（长期使用可达几万甚至几十万行）。
/// ALTER ADD COLUMN 是 SQLite 的元数据操作（常数时间、不重写数据），代价可以忽略。
///
/// ── 为什么这里不能写 IF NOT EXISTS ───────────────────────────
/// SQLite 的 `ALTER TABLE ... ADD COLUMN` **不支持** `IF NOT EXISTS`（这是它与
/// 本文件所有 CREATE 语句的关键差别）。幂等性因此由**版本号**保证 ——
/// 一个版本只跑一次，跑完立刻把 `user_version` 写在同一个事务里（见
/// `migrate` 的说明）。写这段时不要「顺手加一个 IF NOT EXISTS」：
/// 语句会直接报语法错，而不是变成空操作。
///
/// 另外：全新库会先跑 v1（建出**不含**这两列的 requests 表）再跑本版本，
/// 于是两条路径（新库 / 老库升级）得到同一份表结构。不要为了「少跑一条 ALTER」
/// 把这两列塞回 v1 的 DDL —— 那样老库升级后与 v1 的 DDL 定义就对不上了。
const V2_SCHEMA: &str = "
-- ── requests 补两列（schema v2）───────────────────────────────
-- attempt_details：每次上游尝试的明细 `[{provider,status,error}]`，
--   按发生顺序。条数 <= attempts（采集侧一一对应），另受
--   `core::upstream::usage::MAX_ATTEMPT_DETAILS`（24）的体积闸约束。
-- sensitive_hits：本次请求命中的脱敏规则 `[{word,count}]`，按次数降序。
--   来源是脱敏模块算出的命中明细（改造前是「命中的敏感词」，现在是
--   「命中的硬编码规则标签」，见 `core::sanitize`；列名与结构未动，
--   前端请求日志的「敏」标签因此照常工作）。
-- 两列都 NOT NULL DEFAULT '[]'：见上面「默认值为什么是 '[]'」。
-- 「DEFAULT 后面的值必须是常量」——SQLite 对带非常量默认值的 ADD COLUMN 会
-- 直接报错，`'[]'` 是字面量，满足这条。
ALTER TABLE requests ADD COLUMN attempt_details TEXT NOT NULL DEFAULT '[]';
ALTER TABLE requests ADD COLUMN sensitive_hits TEXT NOT NULL DEFAULT '[]';
";

/// 版本 3：`request_raw` 表 —— 请求/响应的**下游侧**原始正文（预览对话功能的地基）。
///
/// ── 它和 debug_traffic 表是什么关系 ──────────────────────────
/// `debug_traffic`（调试报文）是调试模式的**上游侧**报文（头部脱敏、
/// 有全局字节闸）；本表是**下游侧**正文（客户端发来的请求体 + 网关下发的
/// 响应体），供请求日志的「预览对话」弹窗用。两表按同一个 `id`（请求关联 id）
/// 各存各的，谁也不依赖谁。
///
/// ── 采集条件：与调试模式同一个开关（v13 起）───────────────────
/// 正文**只在调试模式开启时采集**（对齐 OmniProxy 的口径：不开调试模式就不
/// 保存任何原始正文）—— 关着时连截断转换都不做，采集路径零开销。v13 之前的
/// 版本是无条件采集的，那批历史正文已由 v13 一次清空（见 [`SCHEMA_VERSION`]）。
///
/// ── 为什么独立成表而不是给 requests 加两列 ────────────────────
/// 正文是大字段（两侧各 128 KiB 上限），明细列表的每次分页都要取
/// `REQUEST_COLUMNS` —— 混进主表会让「只看列表」的请求反复搬运几百 KB 的
/// TEXT。独立表后主查询零开销，按 id 取详情时才 join 进来；请求日志列表页
/// 也不需要改任何 SQL（OmniProxy 的 request_log_raw 副表同此取舍）。
///
/// ── 为什么 id 直接当主键 ────────────────────────────────────
/// 一条请求只有一份「下游视角」的正文，天然一对一（与 requests 表的 id
/// 不同 —— 那边同 id 可以多行，所以不敢当主键）。主键即唯一约束，
/// 「同 id 再写」天然是 UPSERT 覆盖，不需要额外判重。
///
/// ── 为什么体用 TEXT 而不用 BLOB ─────────────────────────────
/// 与 debug_traffic 同一取向：里面是 JSON / SSE 文本，存 TEXT 才能在
/// `sqlite3` 命令行里直接看（排障的主要用法）。
///
/// ── 为什么没有索引（连 ts 都不建）────────────────────────────
/// 本表只有两种读法：按主键 id 取单行（主键自带）、清理时按
/// 「`id IN (SELECT id FROM requests …)`」定位（主键查找）。唯一的全表扫描是
/// 用户手动清空（`DELETE FROM request_raw`），而正文只在调试模式开启期间写入
/// —— 表的增长由那个开关天然约束，不会长到需要为检索建索引。
const V3_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS request_raw (
  id            TEXT PRIMARY KEY,
  ts            INTEGER NOT NULL,
  request_body  TEXT NOT NULL DEFAULT '',
  response_body TEXT NOT NULL DEFAULT '',
  size          INTEGER NOT NULL DEFAULT 0
);
";

/// 版本 4：v3 建表语句的幂等重放（存量库修复，完整背景见 [`SCHEMA_VERSION`]）。
///
/// 直接复用 [`V3_SCHEMA`] 而不是复制一份：两处若各写一份，将来 request_raw
/// 加列时必然只改一处，而「表定义只有一处事实来源」正是 v2 那条纪律。
/// `CREATE TABLE IF NOT EXISTS` 保证对正常库（表已在）是无操作。
const V4_SCHEMA: &str = V3_SCHEMA;

/// 版本 5：`requests` 补两列（schema v5）—— 思考等级的双端记录。
///
/// ── 存什么 ──────────────────────────────────────────────────
///   - `client_reasoning`：**下游请求体里**客户端显式指定的思考等级
///     （识别键与归一规则见 `core::model_rules::reasoning::read_client_level`）。
///     空串 = 客户端没指定（或该行来自还没有此列的旧版本）。
///   - `upstream_reasoning`：**实际随上游请求发出**的思考等级（映射绑定注入的
///     或客户端显式指定且承载家接等级的最终值；采集点在
///     `core::upstream::payload::send_body`，与 `upstream_model` 同点同时）。
///     空串 = 没有等级随行（客户端没指定且映射没绑、承载家不接等级、
///     「关闭思考」档、或一次都没发出去）。
///
/// 两列都 NOT NULL DEFAULT ''：与 `client_model` / `upstream_model` 同一套
/// 「键恒在、空串就是没有」的存储契约，前端不必处理第三种「键缺失」形态。
/// 加列走 ALTER 的全部理由（为什么不动 v1 的 DDL、为什么不能写
/// IF NOT EXISTS、幂等靠版本号）与 [`V2_SCHEMA`] 完全相同，不赘述。
const V5_SCHEMA: &str = "
-- ── requests 补两列（schema v5）──────────────────────────────
ALTER TABLE requests ADD COLUMN client_reasoning TEXT NOT NULL DEFAULT '';
ALTER TABLE requests ADD COLUMN upstream_reasoning TEXT NOT NULL DEFAULT '';
";

/// 版本 6：`requests` 补两列（schema v6）—— 在途请求的**阶段**与**阶段计时**。
///
/// ── 存什么 ──────────────────────────────────────────────────
///   - `phase`：该请求当前所处的转发阶段，取值是 `core::upstream::usage::
///     LogPhase` 的四个字面量（`connecting` 连接中 / `waiting` 等待响应 /
///     `streaming` 响应中 / `retrying` 重试中 / `queued` 排队中）；空串 = 不在途（终态行、
///     旧行、以及转发前就失败从未插入过在途行的行）。
///   - `phase_started_at`：**进入当前阶段**的时刻（毫秒时间戳，与 `ts`
///     同一口径）。阶段计时（请求日志状态列第二行）由它算出来。
///
/// ── 为什么两列都只服务「在途」───────────────────────────────
/// 它们是进行中行的展示字段（列表状态列显示「响应中 1分53秒」这类读数），
/// 终态行一个字都不读 —— 所以收尾（`update_running_request` /
/// `finish_stale_running` / `finish_running_request`）一律把两列清回
/// 空串 / NULL：不清的话，一条已经失败的行会留着「上次看到的阶段」，
/// 而那个值既不是事实、也没有任何读点。
///
/// ── 为什么不像 OmniProxy 那样只存阶段、计时刻现算 ────────────
/// 那边阶段计时存的是 `phase_started_at`（SQL 里 `julianday('now')` 现减），
/// 这里同一形态：`phase_started_at` 入库、`phaseElapsedMs` 在 API 序列化时
/// 现算（见 `request_stats::report::entry_json`）。前端因此拿到的永远是一个
/// 与服务端时钟同源的读数，不必依赖浏览器本地时钟去减。
///
/// 两列都 NOT NULL / 可空的分工：`phase` 跟 `client_model` 那一套「键恒在、
/// 空串就是没有」的契约（前端少一种「键缺失」形态）；`phase_started_at` 与
/// `first_response_ms` 一样是**真的可能没有**（不在途的行），所以可空，
/// 用 NULL 而不是 0 表达「没有这个时刻」。
/// 加列走 ALTER 的全部理由（为什么不动 v1 的 DDL、为什么不能写
/// IF NOT EXISTS、幂等靠版本号）与 [`V2_SCHEMA`] 完全相同，不赘述。
const V6_SCHEMA: &str = "
-- ── requests 补两列（schema v6）──────────────────────────────
ALTER TABLE requests ADD COLUMN phase TEXT NOT NULL DEFAULT '';
ALTER TABLE requests ADD COLUMN phase_started_at INTEGER;
";

/// 版本 7：`requests` 补一列（schema v7）—— 这条请求是不是**模型测试**发起的。
///
/// ── 存什么 ──────────────────────────────────────────────────
/// `is_test = 1`：这条明细来自模型管理页的「测试」（`api::model_test`）；
/// `0`（默认，含全部旧行）：真实流量。
///
/// ── 为什么要有它（而不是靠「模型名 / 来源」猜）────────────────
/// 测试请求与真实请求**走的是同一条转发链路**（这正是测试的价值：结论与生产
/// 同源），所以它们在明细里长得一模一样 —— 模型、账号、状态码、用量全都真实。
/// 唯一能分开它们的就是「这次是谁发起的」，而那只有入口知道。报表按天聚合
/// （`request_stats` 的 `fold_into_daily` 与 `rebuild_day`）据此跳过测试行，
/// 请求日志则照常显示并标出来。
///
/// 加列走 ALTER 的全部理由（为什么不动 v1 的 DDL、为什么不能写
/// IF NOT EXISTS、幂等靠版本号）与 [`V2_SCHEMA`] 完全相同，不赘述。
const V7_SCHEMA: &str = "
-- ── requests 补一列（schema v7）──────────────────────────────
ALTER TABLE requests ADD COLUMN is_test INTEGER NOT NULL DEFAULT 0;
";

/// 版本 8：account_usage_records 表 —— 每账号一条余额查询记录（一账号一行，
/// 主键即账号 id，重复写入天然是 UPSERT）。
///
/// 存什么：`usage` 是适配器返回的**归一化余额 JSON 原文**（与 `/api/accounts/usage`
/// 行里的 `usage` 同一份形状，前端按它渲染余额列），`error` / `code` 是失败行的
/// 原因与机器标记（与快照行同语义）。`remaining` 是从 usage 里提出的**数字**投影
/// （归一化家 = `available`，workbuddy 既有形状 = `totalLeft`）：转发选路的
/// 「余额不足跳过」每条请求都要比一次，从 JSON 里现场解析太奢侈，主键查一列即可；
/// `unlimited` 单独一列，workbuddy 的 ∞ 账号不参与阈值判定。
///
/// ── 为什么 `remaining` 只在成功时覆写 ────────────────────────
/// 失败行保留上次成功的数值：欠费判定不因「这次查询失败」而放行（避免
/// 「查询失败 → 放行 → 402」的窗口期），恢复需要查询成功且数值回到阈值之上。
/// 失败本身的展示信息在 `error` / `code` 里，二者互不覆盖。
///
/// 为什么时间是 INTEGER 毫秒而不是 DATETIME：调度判定（到期 = 上次尝试 +
/// 间隔）要的是可直接比较的毫秒数，与 `logging::now_ms` 同一口径，读出来即用。
const V8_SCHEMA: &str = "
-- ── account_usage_records：每账号的余额查询记录（schema v8）──
CREATE TABLE IF NOT EXISTS account_usage_records (
  account_id      TEXT PRIMARY KEY,
  usage           TEXT,
  error           TEXT,
  code            TEXT,
  remaining       REAL,
  unlimited       INTEGER NOT NULL DEFAULT 0 CHECK (unlimited IN (0, 1)),
  last_success_at INTEGER NOT NULL DEFAULT 0,
  last_attempt_at INTEGER NOT NULL DEFAULT 0,
  updated_at      INTEGER NOT NULL DEFAULT 0
);
";

/// 版本 9：给 `requests` 补「按账号 + 时间」的聚合索引。
///
/// ── 它服务什么 ──────────────────────────────────────────────
/// 账号限制器的 **Token 消耗规则**：每条规则问的是「这个账号在当前重置窗口
/// （对齐自然时间的固定窗口，30 分钟 ~ 24 小时）内一共消耗了多少 Token」——
/// 一次 `SELECT account_id, SUM(total_tokens) FROM requests
/// WHERE account_id = ? AND ts >= ?` 的聚合。没有这个索引它只能走
/// `idx_requests_ts` 全表扫到过滤条件上，限制器的心跳刷新（每 10 秒一轮）
/// 会把请求明细表整个翻一遍。
///
/// 为什么是复合索引 `(account_id, ts)` 而不是只补 `account_id`：窗口过滤
/// （`ts >= 窗口起点`）要在**同一账号内**做范围扫描，复合索引让聚合变成
/// 一次有序的范围读；单独的 account_id 索引拿到行号后还得回表逐行比 ts。
///
/// 聚合查询的全部调用方：`core::limiter` 的窗口刷新（按 distinct 周期分组
/// 查）与单账号即时复查（自动禁用判定）。
const V9_SCHEMA: &str = "
-- ── requests 补按账号聚合的索引（schema v9）──────────────────
CREATE INDEX IF NOT EXISTS idx_requests_account_ts ON requests(account_id, ts);
";

/// 版本 10：requests 补「选路决策快照」一列。
///
/// 存整个决策 JSON 的原文（`{strategy, sticky, stickyHit, picked, ordered[],
/// blocked[]}`，采集侧已截断到各有界条数）。DEFAULT NULL：旧行与「没走到
/// 选路」的请求都是「没有决策」—— 展示层对两者不渲染同一段，与
/// `attempt_details` 的 `'[]'` 不同（那表达「空表」是有意的信息，这里 NULL
/// 就是「没这回事」，不必造一种空形态）。
const V10_SCHEMA: &str = "
-- ── requests 补选路决策快照列（schema v10）───────────────────
ALTER TABLE requests ADD COLUMN route TEXT;
";

/// 版本 11：checkin_records 表 —— 签到记录的**每账号明细台账**。
///
/// 存什么：每执行一次签到（定时条目轮次 / 启动补签 / 手动批量 / 单账号 /
/// 国际版活动领取），每个账号落一行。列的口径：
///   - `ts`：记账时刻（毫秒，`logging::now_ms` 同口径；排序与翻页游标）；
///   - `date`：北京时间 `YYYY-MM-DD`（上游自然日，`core::beijing` 的日期键 ——
///     「哪天的签到」按它算，跟机器时区无关，与调度判定同一份口径）；
///   - `source`：触发来源 `schedule / startup / manual / single / activity`
///     （见 `core::checkin_records` 的模块头），界面显示为「来源」列；
///   - `reason`：给日志看的那句（「到点触发（08:00）」「手动签到」…），原样存；
///   - `account_id` / `account_name` / `provider`：账号三件套（结果行里就有，
///     原样复制；账号删除后历史行仍然可读）；
///   - `outcome`：`success / already / pending / failed`（判定复用签到执行体的
///     同一对判据，判据只此一份，见 `core::checkin_records::classify`）；
///   - `ok`：`success|already → 1` 的布尔投影（「今日已完成」口径的 SQL 过滤列，
///     免得每个读点都写 `(outcome='success' OR outcome='already')`）；
///   - `message`：claim.msg 原文（「今日积分 +100」等**收益就在这里**）；
///   - `error`：执行错误原文（失败行的原因）；
///   - `kept_active`：国际版保活成功标记（保活对话 `pokeSucceeded` 或网页
///     会话 `webConversation.succeeded` 任一，见 `checkin_records::kept_active_of`）。
///
/// 为什么 row_id 用 AUTOINCREMENT：与 `requests` 同一理由 —— 它当翻页游标，
/// 复用 rowid 会让「翻页漏行 / 重行」。
///
/// 索引三个（ts / outcome / provider），对应记录页固定的三种过滤维度，
/// 刻意不做复合索引（与 logs 表同一取舍：组合多、量级小，让 SQLite 自己挑）。
const V11_SCHEMA: &str = "
-- ── checkin_records：签到记录的每账号明细台账（schema v11）────
CREATE TABLE IF NOT EXISTS checkin_records (
  row_id       INTEGER PRIMARY KEY AUTOINCREMENT,
  ts           INTEGER NOT NULL,
  date         TEXT NOT NULL DEFAULT '',
  source       TEXT NOT NULL DEFAULT '',
  reason       TEXT NOT NULL DEFAULT '',
  account_id   TEXT NOT NULL DEFAULT '',
  account_name TEXT NOT NULL DEFAULT '',
  provider     TEXT NOT NULL DEFAULT '',
  outcome      TEXT NOT NULL DEFAULT '',
  ok           INTEGER NOT NULL DEFAULT 0,
  message      TEXT,
  error        TEXT,
  kept_active  INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_checkin_records_ts ON checkin_records(ts);
CREATE INDEX IF NOT EXISTS idx_checkin_records_outcome ON checkin_records(outcome);
CREATE INDEX IF NOT EXISTS idx_checkin_records_provider ON checkin_records(provider);
";

/// 版本 12：requests 补「来源客户端」一列（schema v12）。
///
/// 与 v5 / v7 / v10 同一形态（既有表加列，单条 ALTER，幂等靠版本号）：
/// `client` 存入口从下游 `User-Agent` 判出的客户端类型（`core::client`），
/// 请求日志的「客户端」列读它。DEFAULT ''：旧行与「请求没带 UA」的路径都是
/// 空串 —— 两者在展示层的处理相同（占位符），不必分两种形态。
const V12_SCHEMA: &str = "
-- ── requests 补来源客户端列（schema v12）─────────────────────
ALTER TABLE requests ADD COLUMN client TEXT NOT NULL DEFAULT '';
";
// v13 没有对应的 `*_SCHEMA` 常量：它只有一条 DELETE，且在 `apply_version`
// 的分支里用 `execute` 单独执行（要拿被删行数），语句就在那里 —— 见
// `SCHEMA_VERSION` 的 v13 段。

/// 版本 15：requests 补两条索引（schema v15）。
///
/// 与 v9 同一形态（既有表加索引，`CREATE INDEX IF NOT EXISTS`，幂等靠版本号）。
/// 两条索引各自的来历与代价见 [`SCHEMA_VERSION`] 的 v15 段。
///
/// `idx_requests_summary` 的列序 = `sql::SUMMARY_COLUMNS` 的列序**加前缀
/// `ts, is_test`**：`ts` 在最前才能支撑 `WHERE ts >= ? AND ts <= ?` 的范围扫描，
/// `is_test` 紧随其后是为了在索引内就过滤掉模型测试行（不必回表才知道要丢）。
/// 其余 5 列（`model` / `upstream_model` / `total_tokens` / `cache_read_tokens`
/// / `prompt_tokens`）就是报表要读的全部内容 —— 少一列就要回表，多一列白占
/// 空间与写入开销。
const V15_SCHEMA: &str = "
-- ── requests 补 status 索引（schema v15）─────────────────────
CREATE INDEX IF NOT EXISTS idx_requests_status ON requests(status);
-- ── requests 补报表窄列覆盖索引（schema v15）─────────────────
CREATE INDEX IF NOT EXISTS idx_requests_summary ON requests(
  ts, is_test, model, upstream_model, total_tokens, cache_read_tokens, prompt_tokens
);
";

/// 把库升到 [`SCHEMA_VERSION`]（幂等：已是最新版时什么都不做）。
///
/// **库版本高于本程序时返回 `Err` 拒绝打开**（v14 起有意反转的一条既有决定，
/// 完整论证见函数体里那个分支的注释）：加密后的 `accounts.data` 被旧程序当坏
/// 数据跳过、再被它的差异写当成「内存里没有」删掉，静默放行等于「回退一次 =
/// 账号被删」。库版本**低于**本程序则照常逐版本升级；**等于**则空操作。
///
/// 返回 `rusqlite::Result` 而不是本模块自造的字符串错误：调用方 `Db::open`
/// 是唯一消费点，它把错误包成一句人能读的话（`format!`）就够了；而这里需要
/// 区分「读版本号失败 / 跑 DDL 失败 / 写版本号失败」三种，保留 rusqlite 的
/// 错误类型（含 SQLite 的错误码与消息）才说得清是哪一步坏的。那句「库比程序新」
/// 的拒绝理由用 `ToSqlConversionFailure` 承载（它的 `Display` 就是原话）。
pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if current == SCHEMA_VERSION {
        // 已是最新版：什么都不做（全新库跑完建表之后、已升级的库每次启动都走这里）。
        return Ok(());
    }
    if current > SCHEMA_VERSION {
        // ── 库比程序新：**拒绝打开**（本切片有意反转的一条既有决定）─────────
        //
        // 改造前这里的口径是「不报错、也不动库」：理由写的是「用户从新版回退到
        // 旧版，报错会让旧版直接起不来，而他只是想回退一次」。那条理由在
        // `accounts.data` 是明文的年代成立 —— 旧程序读不懂新库的新列/新表，
        // 但账号那几张表它写得回去，最坏是丢一点新功能的数据。
        //
        // **本版起它不再成立**：`accounts.data` 是 `enc1:` 密文（v14 起），而旧程序
        // （`SCHEMA_VERSION <= 13`）的 `decode_data` 只认明文 JSON —— 于是：
        //   1. 旧程序读出每一行都会失败，把它们**当成坏数据跳过** → 界面上账号
        //      全部消失；
        //   2. 更致命的是写路径：旧程序的 `save_state` 按 id 做差异写，「库里有、
        //      内存状态里没有」的行会被 `DELETE FROM accounts WHERE id = ?` ——
        //      它读不出来的那些密文行**恰好**落进这个分支。于是「回退一次并触发
        //      任意一次账号写入」= 把加密后的账号**静默删光**。
        // 静默放行等于「回退一次 = 账号被删」，而账号数据丢了是不可逆的用户损失
        // —— 所以这里改成明确失败：让启动在日志里说清原因，用户退回去用新版。
        //
        // 这不是「降级 schema」（那才需要删列删表、不可逆，本函数从没做过，也不
        // 会做）：这里只是**在动手之前先拒绝**，库文件一个字节都不碰。
        //
        // 用什么承载这句错误：`ToSqlConversionFailure` 的 `Display` 就是里面那句
        // 话的原文（本仓既有手法，见 `db::migrate::logs::assign_log_id` 的说明），
        // 于是 `Db::open` 的 `format!("建表/升级 schema 失败: {error}")` 会把这段
        // 话原样带到启动日志里。
        return Err(rusqlite::Error::ToSqlConversionFailure(
            format!(
                "库 schema 版本（{current}）高于当前程序（{SCHEMA_VERSION}）：这通常是回退到了旧版本。\
                 请使用更新的程序，或从备份恢复——用旧程序打开会把加密后的账号静默删除。"
            )
            .into(),
        ));
    }
    // v13 清掉的历史正文行数（0 = 这一轮什么都没删）：循环后据此决定是否
    // VACUUM（见本函数尾部那段）。放循环外是因为 VACUUM 不能在事务里跑。
    let mut cleaned_raw_rows = 0usize;
    for version in (current + 1)..=SCHEMA_VERSION {
        // 每个版本一个独立事务：DDL 与版本号必须一起生效或一起不生效，
        // 否则中断后会出现「表已建、版本号还是旧的」（下次启动重跑 DDL，
        // 靠 IF NOT EXISTS 能过）与「版本号已新、表没建全」（**跑不过去**）
        // 两种错位，后者更糟，所以宁可牺牲「一次提交全部版本」的原子性。
        //
        // 用 `unchecked_transaction()` 而不是 `transaction()`：后者要
        // `&mut Connection`，而本函数的签名是 `&Connection`（它在 `Db::open`
        // 里被调用，那时 `Db` 还没构造出来，拿不到 `with_mut`）。「同一连接上
        // 不能有嵌套事务」这条约束由调用点保证 —— `Db::open` 持有唯一那把锁，
        // 迁移期间不会有第二个访问者。
        //
        // 注意 `tx` 不需要 `mut`：`pragma_update` / `commit` 都只取 `&self`，
        // 写事务的独占性由 SQLite 自己保证（同一连接上不允许第二条语句并发），
        // 不是靠 Rust 的独占借用。
        let tx = conn.unchecked_transaction()?;
        apply_version(&tx, version, &mut cleaned_raw_rows)?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    if cleaned_raw_rows > 0 {
        // ── 删完之后顺手 VACUUM 一次（v13 的「省空间」在这里真正落地）──
        // `DELETE` 只是把行标成空闲页，库文件**不会缩小** —— 对「升级后马上想
        // 看到磁盘降下来」的用户，只有 VACUUM（重建整库文件）才把空闲页还给
        // 文件系统。触发条件是 `cleaned_raw_rows > 0`：全新库（表本来就空）与
        // 已升过级的库都删 0 行、不付这个代价；只有「确实有历史正文被清掉」的
        // 那次升级会做一遍整库重建（几十 MB 的库通常秒级，一次性成本）。
        //
        // VACUUM 不能在事务里，所以放在循环外（每个版本的事务都已提交）；
        // 失败**不报错**：数据已经删掉了，回收空间是尽力而为的优化 —— 用户
        // 之后仍可在请求日志页的「清理」弹窗里手动压缩（那条路有后台线程与
        // 进度状态，见 `RequestStats::compact`）。
        let _ = conn.execute_batch("VACUUM;");
    }
    Ok(())
}

/// 执行某个版本的 DDL / 数据动作，并把「清掉的历史正文行数」带回给 `migrate`
/// （只有 v13 会写非零值，其余版本写 0）。**新增版本时在这里加一个分支**，
/// 并在上面写好该版本的建表/改表语句常量。
///
/// `cleaned_raw_rows` 做成出参而不是返回值：绝大多数分支只是跑一段 DDL、
/// 没有数据要上报，若改成 `Result<usize>` 则每个分支都得包一层 `Ok(0)`
/// （形式噪音），出参让「只有关心它的那一版」写它。
///
/// `_` 分支返回 Ok 而不是 unreachable!：版本号是外部输入（库文件可能来自
/// 更新的二进制，被旧版读到），越过 [`SCHEMA_VERSION`] 的版本不该 panic。
/// `migrate` 的主循环也不可能走到这里（循环上界就是 SCHEMA_VERSION），
/// 所以这个分支实际是「调用方传了越界的 version」的防御。
fn apply_version(
    conn: &Connection,
    version: i64,
    cleaned_raw_rows: &mut usize,
) -> rusqlite::Result<()> {
    match version {
        1 => conn.execute_batch(V1_SCHEMA),
        // v2：requests 补两个 JSON 文本列（重试链 / 敏感词命中）。
        // 与 v1 的差别只有一条：这里是 ALTER 而不是 CREATE，所以**没有**
        // IF NOT EXISTS 可用（见 V2_SCHEMA 的说明），幂等由版本号保证。
        2 => conn.execute_batch(V2_SCHEMA),
        // v3：request_raw 原始报文表（新表用 CREATE IF NOT EXISTS，见 V3_SCHEMA）
        3 => conn.execute_batch(V3_SCHEMA),
        // v4：重放 v3 建表（修复「版本号 3、表却不在」的存量库，见 SCHEMA_VERSION）
        4 => conn.execute_batch(V4_SCHEMA),
        // v5：requests 补两个思考等级列（下游指定 / 上游实际发出，见 V5_SCHEMA）
        5 => conn.execute_batch(V5_SCHEMA),
        // v6：requests 补阶段与阶段计时两列（在途请求的状态列读数，见 V6_SCHEMA）
        6 => conn.execute_batch(V6_SCHEMA),
        // v7：requests 补「测试来源」一列（模型测试的流量标记，见 V7_SCHEMA）
        7 => conn.execute_batch(V7_SCHEMA),
        // v8：account_usage_records 每账号余额记录表（取代旧 kv 快照，见 V8_SCHEMA）
        8 => conn.execute_batch(V8_SCHEMA),
        // v9：requests 补按账号聚合的索引（限制器的 Token 周期消耗聚合，见 V9_SCHEMA）
        9 => conn.execute_batch(V9_SCHEMA),
        // v10：requests 补「选路决策快照」一列（请求详情的路由回放，见 V10_SCHEMA）
        10 => conn.execute_batch(V10_SCHEMA),
        // v11：checkin_records 签到记录明细台账（签到记录页的数据源，见 V11_SCHEMA）
        11 => conn.execute_batch(V11_SCHEMA),
        // v12：requests 补「来源客户端」一列（请求日志的客户端列，见 V12_SCHEMA）
        12 => conn.execute_batch(V12_SCHEMA),
        // v13：清空预览对话的历史正文（一次性空间回收，见 SCHEMA_VERSION 的
        // v13 段）。先重放 v3 建表：存量库里有「版本号不低、request_raw 表却
        // 缺失」的个例（v4 就是为修它而加的）—— 对那种库直接 DELETE 会报
        // 「no such table」并把整个版本事务回滚，迁移就卡死了。
        // 用 `execute` 而不是 `execute_batch`：要拿被删行数带出去，`migrate`
        // 据此决定是否紧跟着 VACUUM（光 DELETE 不会让库文件缩小）。
        13 => {
            conn.execute_batch(V3_SCHEMA)?;
            *cleaned_raw_rows = conn.execute("DELETE FROM request_raw", [])?;
            Ok(())
        }
        // v14：账号 `data` 列改为密文（`enc1:` 信封）。
        // **这一版没有 DDL**，所以是空操作：加密是逐行的内容变换（由启动迁移
        // `account_store::store_admin::encrypt_legacy_accounts` 把存量明文行就地
        // 加密），没有列要加、没有表要建、也没有历史行要删（`cleaned_raw_rows`
        // 保持 0，不触发 VACUUM）。版本号的作用在**旧程序那一侧**：它让旧程序
        // 读到 v14 库时明确失败（见 `migrate` 的拒绝分支），而不是把密文当坏
        // 数据跳过后又删掉。完整说明见 [`SCHEMA_VERSION`] 的 v14 段。
        14 => Ok(()),
        // v15：requests 补两条索引（status 的定位 + 报表窄列的覆盖，见
        // V15_SCHEMA 与 SCHEMA_VERSION 的 v15 段）。
        15 => conn.execute_batch(V15_SCHEMA),
        _ => Ok(()),
    }
}
