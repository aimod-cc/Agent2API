---
name: request-log-analysis
description: Agent2API（workbuddy）网关的请求日志自动排障 —— 从统一库 `{config_dir}/agent2api.db`（安装版默认 `%USERPROFILE%\.agent2api`，WAL 热库先快照只读）读 `requests` 明细、`attempt_details` 重试链、`route` 选路快照与 `logs` 应用日志，把最近的转发（中转）请求按「家 + 状态码 + 归一化错误文案」聚类并分桶定责（客户端断开 / 上游故障 / 限额轮换 / 适配缺陷 / 请求体形态被拒…），定位到具体代码落点并给出原因或修改建议。凡是用户提到「分析最近的请求 / 看看请求日志 / 为什么转发失败 / 一直 429 / 408 / 500 parse response failed / 帮我看看中转哪里有问题 / 某家最近不稳 / 基于请求日志写修复」这类诉求，都先按本 Skill 执行 —— 即使对方没提「日志」「排障」这类字眼。
---

# Agent2API 请求日志排障（中转请求自动分析）

覆盖：定位数据（第 1 节）→ 标准流程（第 2 节）→ 分桶定责与代码落点（第 3 节）→ 伴随脚本（第 4 节）→ 报告（第 5 节）→ 坑（第 6 节）。手头只有 sqlite3 CLI 时用第 7 节的 SQL 速查。

## 0. 三条工作约定

1. **只读**。分析过程绝不写库、不改配置、不动账号；临时快照只落 `%TEMP%`。
2. **先给结论与建议，等用户点头再改代码**；真要改 providers/ 适配器时，接着按 `provider-adaptation` 执行（那是改代码的规范）。
3. **每条结论带证据**：错误原文、重试链、能复现的命令/SQL。「上游故障」也要给依据（多账号同挂、时间窗集中）—— 没证据的定责等于猜。

## 1. 数据在哪、长什么样

### 1.1 库的定位（与服务端 `paths::config_dir()` 同序）

| 优先级 | 来源 |
|---|---|
| 1 | `--db` 显式指定（传目录会自动拼 `agent2api.db`） |
| 2 | 环境变量 `AGENT2API_PROXY_HOME`（旧名 `WORKBUDDY_PROXY_HOME` 兼容） |
| 3 | 默认 `%USERPROFILE%\.agent2api\agent2api.db`（Windows；非 Windows 为 `~/.agent2api`） |

服务器 / Docker 部署是**同一个库文件**，把 `agent2api.db`（连同 `-wal` / `-shm`）拷回本机后用 `--db` 指进来即可。同目录还有 `debug/last-request.json`（最近一次请求落盘）与 `debug/Agent2Providers-src`（插件家源码缓存）。

**网关在跑时库是 WAL 热库 —— 先快照再查**（脚本默认已做：SQLite backup API 拿一致快照，失败则三件套整拷）。

### 1.2 表速查

| 表 | 内容 | 排障用途 |
|---|---|---|
| `requests` | 每次转发一条明细（全量记，与调试开关无关） | **主战场** |
| `logs` | 应用日志（`server` / `upstream` / `model` …） | 载荷改写告警、插件动作、启动事件 |
| `debug_traffic` | **调试模式开着才有**：上游侧原始报文（头部已脱敏） | 看最终发出去的 body 与上游原始响应 |
| `request_raw` | **调试模式开着才有**：下游侧正文（客户端原文） | 对照客户端发来的东西 |
| `request_daily` | 按天聚合 | 趋势对照 |
| `accounts` | 账号（凭证加密落库，明文不在库里） | 对照 account_id / name |
| `kv` | 配置（`modelRules` / `customProviders` / `routingStrategy` / `proxyPool` / `apiKeys` …） | 查映射、自定义家、出口 |

「记录全请求」有两层：`requests` 明细**始终全量**；原始报文（`debug_traffic` / `request_raw`）只在面板设置页开**调试模式**期间落库。要分析某次请求的报文，先开调试模式再复现一次。

### 1.3 `requests` 关键列

| 列 | 含义 |
|---|---|
| `ts` | 毫秒时间戳 |
| `model` / `client_model` / `upstream_model` | 请求侧解析名 / 客户端点名的原值 / 实际发给上游的名字（空串 = 一次都没发出去） |
| `provider` / `account_id` / `account_name` | 实际承载者；空串 = 没选上路就失败（如模型不存在） |
| `status` | **回给客户端的** HTTP 状态码；`0` = 没发出去就失败 |
| `error` | 失败摘要，成功为 NULL。注意 `status=200` 且 `error` 非空 = 客户端断开类，不是上游失败 |
| `attempts` | 尝试次数（含首次），>1 说明走过重试/轮换 |
| `attempt_details` | 重试链全史：JSON 数组 `[{provider, account, status, error, retries:[{reason, status, delayMs}], notice}]` |
| `route` | 选路快照 `{strategy, sticky, stickyHit, picked, ordered[≤12], blocked[≤12]}`，每项 `{id, name, provider, state, reason}` —— 「当时为什么这么排」 |
| `duration_ms` / `first_response_ms` | 总耗时 / 上游首帧耗时（NULL = 一帧都没收到，转发前就失败） |
| `*_tokens` / `cache_read_tokens` | 用量（失败恒 0） |
| `client` | 下游客户端归类（`Codex` / `Claude Code` / `deepseek-harness` / `other` …） |
| `is_test` | 模型管理页「测试」发起的流量，**分析一律默认排除** |
| `phase` / `phase_started_at` | 在途阶段（非空 = 这行还没跑完） |
| `sensitive_hits` | 命中的脱敏规则标签 |

**成功口径（与后端 `RequestEntry::is_success` 逐字一致）**：`status BETWEEN 200 AND 299 AND error IS NULL`，其余一律算失败。`error` 里「上游返回 X」的 X 是**上游侧**状态码，与 `status` 列（回给客户端的）可以不同 —— 两个都看。

## 2. 标准分析流程（五步）

```bash
# 用 DSH 会话时先 load_workspace_dependencies 拿内置 Python；任何 Python 3.8+ 均可
python .agents/skills/request-log-analysis/analyze.py overview  --hours 24
python .agents/skills/request-log-analysis/analyze.py failures --hours 24 --limit 15
python .agents/skills/request-log-analysis/analyze.py detail --id <请求id>
python .agents/skills/request-log-analysis/analyze.py logs --level warn --category upstream
```

1. **总览**（`overview`）：总数、成功率、状态码分布、按家失败率与耗时、重试率、按客户端。先看形状：是个别家失败还是整体掉线；失败集中在哪个客户端。
2. **失败聚类**（`failures`）：按「家 + 状态码 + 归一化文案」分组并自动分桶。错误文案里常带 request_id / 时间戳 / 数字等动态值，**聚类前必须归一化**（脚本已做），否则同一种错误被拆成几十组。
3. **分桶定责**：对照第 3 节映射表，给每个聚类判定「上游的锅 / 客户端的锅 / 网关适配缺陷 / 配置问题」。
4. **单条深挖**（`detail`）：每个可疑聚类挑 1–2 条代表（最新一条 + 重试链最长的一条），看 `attempt_details`（换了谁、为什么换）与 `route`（为什么排它先上）、邻近 `logs`。要看原始报文再开调试模式复现。
5. **出报告**（第 5 节骨架）：现象 → 证据 → 定责 → 建议；无需改码的明确说无需改码。

## 3. 分桶定责映射（哪类错误 → 查哪里）

| 桶 | 实测典型文案 | 定责 | 建议落点 |
|---|---|---|---|
| 客户端断开 | `客户端在响应完成前断开连接`；`响应流未完整下发（客户端中断或服务退出）` | 客户端取消/超时，**不是网关的锅** | 看 `client` 列哪家高频 → 建议调该客户端超时；网关无需改码 |
| 无账号可转发 | `所有账号均已禁用…`；`所有启用中的账号都被限制器拦下…` | 配置/限制器 | 面板账号页启用、检查限制器规则；无需改码 |
| 模型/路由不存在 | `模型不存在: luna。完整列表见 GET /v1/models`；`上游返回 400: 模型不存在…`；`model [x] service info not found`；`model [x] is not allowed for user type [y]` | 客户端点名的模型没有映射到可用家；或上游账号档位不承载 | 模型规则/映射（面板「模型规则」，`kv.modelRules`；代码 `core/model_rules/`）；该家目录 `core/providers/<name>/models.rs`。`user type` 是上游账号档位问题 → 换绑承载家，不是改映射 |
| 解析/翻译层 | `上游返回 500: parse response failed`；`Parse message failed: unsupported content type …: encrypted_content` | **适配缺陷，最值得改代码**：报文形态超出该家适配层处理范围 | 内置家：`core/providers/<name>/` 与 `core/upstream/translate`；插件家去插件仓库（见下） |
| 请求体形态被上游拒 | `Invalid 'tools': array too long … 253 instead`（上限 128）；`prompt is too long: … > … maximum` | 网关发出的 body 超出上游硬限制，未做裁剪 | `core/upstream/payload` 组装处按家裁剪（工具条数/上下文长度）；先确认该家适配层是否声明了对应能力 |
| 限额/轮换 | `上游返回 429: usage exceeds frequency limit … reset at …`；`too many requests（所有候选账号…均已限额或禁用）` | 限额分档与冷却是否生效 | `classify_error` 四档与冷却标记（`provider-adaptation` §2.6）；用 `attempt_details` 验证**是否真换了账号** —— 同一账号反复撞墙 = 分档或冷却失效，这才是要修的 |
| 凭证/权限 | 401 / 402 / 403 及上游对应文案 | TokenExpired 分档、续期失效 | 该家 `adapter.rs` 的 `supports_refresh` 与 `credential_maintenance` |
| 超时 | `连接中超时(30秒，出口 直连)`；408 / 504 | 网络/上游慢 | 出口与代理（面板 `proxyPool` 设置）；一般无需改码 |
| 上游故障 | `上游返回 500/502/503: …`；`Upstream service temporarily unavailable` | 上游侧 | 无需改码；多账号同挂 = 上游整体故障；某家长期高占比时建议下线或降级 |
| 其它 | — | 逐条看 | — |

**内置家 vs 插件家**：本仓库 `core/providers/` 下是 `accio / autoclaw / cline / loomy / modelscope / monkeycode / qoder / workbuddy / zcode`（外加 `catalog` / `custom`）；`raccoon`、`trae`、`antigravity` 等是**插件家**，实现在插件仓库（`Agent2Providers`），本机最近一次插件源码缓存在 `%USERPROFILE%\.agent2api\debug\Agent2Providers-src` —— 修适配别在本仓库里瞎找。`custom-<hash>` 这类 id 是自定义 provider，名字与 base_url 去 `kv.customProviders` 对。

## 4. 伴随脚本 analyze.py

四个子命令：`overview`（总览）/ `failures`（聚类分桶）/ `detail`（单条深挖）/ `logs`（应用日志）。零第三方依赖（Python 3.8+ 标准库）。

- **选项一律放在子命令后面**（脚本如此设计：公共选项只挂子解析器）。
- 默认先快照再查（`%TEMP%\a2a-logs-*`）；`--no-copy` 直连只读；`--db` 指其它库（服务器拷回 / 复用上次的快照目录继续追问）。
- `--json` 额外输出机器可读结构；`--include-test` 才包含模型测试流量；`--since` / `--until`（`YYYY-MM-DD HH:MM` 本地时间）替代 `--hours`。
- `detail --body` 会打印报文正文 —— `request_raw` 是用户原文，**报告里不要整段外贴**。

常用组合：

```bash
python analyze.py failures --hours 72 --limit 20          # 近三天的失败全景
python analyze.py detail --id <id> --log-window 120       # 深挖 + 前后 2 分钟日志
python analyze.py detail --id <id> --body                 # 连原始报文一起看（调试模式需已开）
python analyze.py logs --level warn --category upstream   # 载荷改写类告警（剔除工具声明等）
```

## 5. 报告骨架

```markdown
## 中转请求分析报告（YYYY-MM-DD HH:MM，窗口 …）
### 总览
总数 / 成功率 / 失败率 / 重试率，1–2 行；失败最集中的家与客户端。
### 失败聚类与定责
| # | 条数 | 现象（家+状态码+样例文案） | 桶 | 定责 | 证据 |
### 结论与建议
1. 【无需改码】…（依据：上游侧故障/客户端断开/配置项）
2. 【建议改码】<文件与位置> —— 现象、根因假设、建议做法
### 复现
脚本命令与/或 SQL
```

## 6. 已知坑

1. **WAL 热库**：只拷 `.db` 不拷 `-wal` 会丢最近几分钟的数据；脚本默认 backup API 快照，手工操作请三件套一起拷。
2. **脚本选项必须放子命令后**：`analyze.py overview --hours 6`，不是 `analyze.py --hours 6 overview`。
3. **同 id 多行**：重试、去重排队会给同一个 `requests.id` 记多行；按 `ts DESC, row_id DESC` 取最新，统计别拿 id 当唯一键。`debug_traffic` / `request_raw` 的 id 才是主键一对一。
4. **error 带动态值**：聚类前必须归一化（request_id / 时间戳 / 数字），否则聚不拢；看样例原文别只看归一键（模型名里的数字也会被抹掉，如 `gpt-6-luna` → `gpt-#-luna`）。
5. **status=200 + error 非空** = 客户端断开类，别当上游失败算进失败率定责。
6. **调试报文只有调试模式开着才落库**；`request_raw` 是用户原文，外贴注意脱敏。
7. `is_test=1` 默认排除；`ts` 是毫秒，展示按本地时区换算。
8. **Windows 控制台默认 GBK**：脚本已强制 UTF-8 输出；自己写 Python 内联 SQL 时记得 `PYTHONIOENCODING=utf-8` 或 `sys.stdout.reconfigure(encoding="utf-8")`，否则中文错误文案全是乱码、聚类也没法做。
9. **快照 = 一份整库**：库可能到 GB 级（调试模式开着时尤其），快照目录在 `%TEMP%\a2a-logs-*`。
   脚本**退出时自动删除**本次自己建的快照目录（含异常 / Ctrl-C）；只有 `--db` 指进去复用的旧快照不归本次管，需要手工清理。
   系统盘紧张时用 `--no-copy` 直连只读（WAL 热库也能读，只是可能少最后几秒的数据）。

## 7. 手头只有 sqlite3 CLI 时（SQL 速查）

在拷贝出来的快照上（`sqlite3 agent2api.db` 会自动恢复 WAL）：

```sql
-- 按家汇总（成功口径与后端一致）
SELECT provider, COUNT(*) total,
       SUM(CASE WHEN status BETWEEN 200 AND 299 AND error IS NULL THEN 1 ELSE 0 END) ok
FROM requests WHERE ts >= (strftime('%s','now') - 86400) * 1000
GROUP BY provider ORDER BY total DESC;

-- 失败 Top（粗聚类：substr 代替归一化，动态文案会拆散，尽量用脚本）
SELECT provider, status, COUNT(*) c, MAX(error) sample
FROM requests
WHERE ts >= (strftime('%s','now') - 86400) * 1000
  AND (status < 200 OR status > 299 OR error IS NOT NULL)
GROUP BY provider, status, substr(error, 1, 60)
ORDER BY c DESC;

-- 重试链展开
SELECT r.ts, r.provider, j.value
FROM requests r, json_each(r.attempt_details) j
WHERE r.ts >= (strftime('%s','now') - 86400) * 1000 AND r.status <> 200;
```

---

产出「原因 + 建议」是本 Skill 的终点；要动手改适配器 / 模型规则时按 `provider-adaptation` 执行，涉及提交与发版按 `release-flow` 执行。
