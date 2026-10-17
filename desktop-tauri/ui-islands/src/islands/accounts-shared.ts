import { t } from '../i18n'
import type { BalanceQueryPublic } from './balance-query'

/**
 * 账号页各文件共用的**类型与全局桥读取**（非岛：`.ts` 不被 import.meta.glob 当岛加载）。
 *
 * 账号页按「视图层 .tsx + 数据 / 状态层 .ts」拆成了好几个文件，它们都要读同一批
 * window 上的既有全局（workbuddyDesktop / wbApp / wbColSettings …）。若每个文件各写
 * 一份窄类型，接口合并会因同名属性类型不一致直接报 TS2717 —— 并行迁移时必然互相撞车
 * （见 table-col-settings.tsx 的说明）。所以这里**只此一份**：声明 + 读取函数都在这。
 *
 * 这里只声明本页用到的那些桥，且**不往 Window 上加共享属性**：账号页独占的四个对象
 * （wbAccountsView / wbAccountsModel / wbUsageActions / wbAccountPanel）在
 * accounts-data.ts 里 declare 一次（只有一个文件声明，不会撞车）。
 */

/* ─── 后端账号形态（只列本页读到的字段，其余按 unknown 收）───── */

/** 出网代理配置（与后端 workbuddy-proxy.mjs 的形状一致） */
export type ProxyConfig = {
  /** 旧版平铺形状里的 source；新版在 config.source 下，两处都读 */
  source?: string
  label?: string
  /** 解析失败的原因：转发时回退直连，界面上整格标红 */
  error?: string
  /**
   * 轮询档（`source === 'rotate'`，只对 OpenCode Zen 开放）的展示读数：
   * 候选条数与其中可用的条数（后端 `proxy_rotate::summary`）。
   */
  rotate?: { all?: boolean; total?: number; usable?: number } | null
  config?: {
    source?: string
    listenerUid?: string
    /** `source === 'pool'` 时的池条目 id（见 PoolItem） */
    proxyId?: string
    /** `source === 'rotate'`：true = 轮询池里全部启用的出口 */
    all?: boolean
    /** `source === 'rotate' && all !== true`：要轮询的池条目 id */
    proxyIds?: string[]
    protocol?: string
    host?: string
    port?: number
    username?: string
    password?: string
  } | null
}

/** 按模型记的限流记录（`rateLimits[model]`） */
export type RateLimitInfo = {
  status?: number | string
  code?: string | number
  resetAt?: number
  message?: string
}

/**
 * 限制器规则（后端 `core::limiter::LimiterRule` 的公开形态）。每账号一条数组
 * （`limiters`），规则可自定义增删：类型（余额 / Token）+ 阈值 + 触发动作 +
 * 启用开关，Token 规则另带重置周期。
 *
 * 后端公开形态**恒为数组**：记录上没有 `limiters` 键（旧版写的记录）时由旧
 * `lowBalance` / provider 缺省**推导**（见 accounts-domain 的 `limitersOf`），
 * 前端弹窗、徽章与后端选路读到的永远是同一份有效规则。
 */
export type LimiterRule = {
  /** 余额 = 读数低于阈值触发（严格小于）；Token = 重置窗口内累计消耗达到阈值触发（≥） */
  type: 'balance' | 'token'
  /** 跳过 = 自动恢复（余额回升 / 窗口重置）；禁用 = 不自动恢复，需手动启用 */
  action: 'skip' | 'disable'
  /** 阈值：余额与余额列同一数字口径；Token 原始个数（界面以「万」输入） */
  threshold: number
  /**
   * 仅「固定周期」的 Token 规则：重置周期（整数秒，30 分钟 ~ 24 小时，
   * 对齐自然时间的固定窗口）；自然日规则不带这个键。
   */
  period?: number
  /**
   * 仅 Token 规则的重置方式：固定周期（每 N 分钟/小时）或自然日（每天本地
   * 时区 0 点重置 —— 有的账号过了 0 点额度就回来）。余额规则恒 fixed。
   * 缺省（旧记录没有这个键）= fixed。
   */
  reset?: 'fixed' | 'daily'
  /** 停用中的规则不参与判定；后端归一化后恒显式给出 */
  enabled?: boolean
}

/**
 * 一个 Token 规则窗口在**当前窗口**的消耗读数（用量快照 `tokenUsage` 的行，
 * 后端 `core::limiter` 的内存事实表投影）。`kind` 是窗口种类：固定周期 =
 * 周期秒数，自然日 = 0。`windowStart` 是窗口起点（毫秒）：与按种类现算的
 * 窗口起点对不上 = 读数已翻页，判定按 0 算（见 accounts-domain 的
 * `tokenReadingForRule`）。
 */
export type TokenReading = {
  kind: number
  windowStart: number
  used: number
}

/**
 * 账号记录。后端公开形态字段很多、且随 provider 不同（identifier / expiry 的键名
 * 由能力表决定），所以留一条索引签名兜底 —— 能力表指向的字段（uid / userId / account）
 * 只能按动态键读。
 */
export type AccountRecord = {
  id: string
  provider?: string
  name?: string
  nickname?: string
  /** 用户显式设置过备注名（后端在 update_account 真正改到 name 时打的标，见 apply_patch） */
  nameCustom?: boolean
  email?: string
  priority?: number
  enabled?: boolean
  /** 凭证完整性（后端逐家给出；缺省视为可用）：false = 登录态缺失 / 凭证不全。
   *  余额的批量目标集合按它排除（与后端 `resolve_batch_targets` 同一口径），
   *  与 `enabled` 是两回事 —— 禁用只表示不参与转发，余额仍可查。 */
  available?: boolean
  /** 桌面端实时登录态（凭证每次从客户端登录态文件读取） */
  desktop?: boolean
  /**
   * 后端注入的**跨家事实**：这条账号有没有可用凭证（`has_token || desktop ||
   * apiKey || jwt || noAuth`，见后端 `state::has_credentials`）。
   *
   * 界面现在只用它做一件事：给「自定义账号且没有凭证」标一枚「未配置凭证」——
   * 那种账号在选路里会被静默跳过，不标出来用户看不出为什么请求发不出去。
   * 缺失（旧版后端）按「有凭证」处理，宁可少标一枚徽章也不要误报。
   */
  hasCredentials?: boolean
  edition?: string
  editionLabel?: string
  chatSupported?: boolean
  canClaim?: boolean
  /** ZCode：这一行当前走哪条上游通道（`coding-plan` / `start-plan`，见 accounts-domain 的 ZCODE_PLAN_*） */
  zcodePlan?: string
  hasRefreshToken?: boolean
  hasBalanceToken?: boolean
  /**
   * 自定义账号：记录里有没有非空 `apiKey`（设置弹窗据此决定输入框的提示文案与
   * 有无「清除」按钮）。**值本身绝不透出**，后端只给这个布尔。
   */
  hasApiKey?: boolean
  /** 自定义账号：是否声明了「该上游无需鉴权」（与 `hasApiKey` 互斥，见后端 `state::no_auth`） */
  noAuth?: boolean
  /**
   * 自定义账号：账号级 **baseUrl 覆盖项**（记录里写了这个键才透出；缺省表示
   * 回落到提供商的 baseUrl）。出网「轮询」档与后端 `is_opencode_zen_account`
   * 都要看它 —— 两层都可能指着 OpenCode Go 端点，只看提供商那一层会漏判。
   */
  baseUrl?: string
  /**
   * 自定义账号：**余额查询配置**（后端只在这家登记了余额方案时注入；见
   * `providers::custom::balance` 与 `balance-query.tsx`）。
   *
   * 形状恒定的三件事：方案 id（只读事实）、两个非密字段的值（`userId` /
   * `username`）、三个密字段的**存在标记**（`hasAccessToken` / `hasPassword` /
   * `hasToken` —— 值绝不透出）。没有这个键 = 这一家不查余额，「查询设置」段
   * 里也就没有方案字段。
   */
  balanceQuery?: BalanceQueryPublic
  maxConcurrent?: number
  checkinAt?: number
  /**
   * 每账号自动余额查询设置（后端公开形态恒为对象；**缺省 = 开启、1 分钟**，
   * 显式 `{enabled:false}` 才是关；见 accounts-domain 的 `usageQueryOf`）。
   * `interval` 单位是秒（30 ~ 86400）。
   */
  usageQuery?: { enabled?: boolean; interval?: number }
  /**
   * 余额不足时的处理（**旧字段，限制器上线后只是兼容形态**：写入侧由后端在
   * 保存 `limiters` 时同步，读侧的有效规则见 `limiters`）。`threshold` 与余额列
   * 同一数字口径（`available` / workbuddy 家的 `totalLeft`）。
   */
  lowBalance?: { mode?: 'off' | 'skip' | 'disable'; threshold?: number }
  /**
   * 限制器规则列表（后端公开形态恒为数组，未显式配置 = 由旧 lowBalance /
   * provider 缺省推导的有效规则；形状见 `LimiterRule`）。
   */
  limiters?: LimiterRule[]
  addedAt?: number
  updatedAt?: number
  tokenTail?: string
  tokenExpiresAt?: number
  expiresAt?: number
  source?: string
  proxy?: ProxyConfig | null
  rateLimits?: Record<string, RateLimitInfo>
  [key: string]: unknown
}

/** `wbApp.getState()?.accounts` 的形状（providers 摘要 + 账号清单） */
export type AccountsSnapshot = {
  accounts?: AccountRecord[]
  providers?: Array<{ id?: string; label?: string; count?: number }>
}

/** 余额缓存的四种形态：undefined 未查 / null 查询中 / string 失败 / 对象结果 */
export type UsageEntry = null | string | Record<string, unknown> | undefined

/** 列的对齐档（与 table-col-settings 的 Align 同一套取值） */
export type Align = 'left' | 'center' | 'right'

/**
 * 行内明细面板的 kind。**只剩「限流明细」一种**：签到曾也有一个明细面板，
 * 已随表格化删除；账号页的签到按钮又整体迁去了「签到中心」，行内面板
 * 与签到从此互不相干。
 */
export type PanelKind = 'limits'

/* ─── Loomy 新手任务 ─────────────────────────
 *
 * 类型在这里、消费在签到中心（checkin-page.tsx / checkin-state.ts）：任务状态
 * 是上游查询，签到中心的快照刻意不带，按账号惰性查询后缓存。账号页曾有一个
 * 「签到后自动弹窗领取」的链路（accounts-dialog-onboarding），随账号页签到
 * 按钮一起移除 —— 签到后的自动处理由 checkin-state.ts 承接。 */

/** 后端任务行的原始形状（`GET /api/accounts/{id}/onboarding` 的 tasks 数组元素） */
export type OnboardingTaskRaw = {
  key?: unknown
  title?: unknown
  group?: unknown
  points?: unknown
  done?: unknown
  /** 前置没满足 ⇒ 这一条现在领不动（CodeArts 新人礼未到门槛 / 活动未开始） */
  blocked?: unknown
}

/** 渲染用的归一形状（claiming / error 是前端运行态，后端没有） */
export type OnboardingTask = {
  key: string
  title: string
  group: string
  points: number
  done: boolean
  /** 见 {@link OnboardingTaskRaw.blocked}：既不计入待领数，也不发领取 */
  blocked: boolean
  claiming?: boolean
  error?: string
}

/* ─── 全局桥 ─────────────────────────────────── */

type ConfirmOptions = {
  title?: string
  /** 正文，允许 <strong> 等少量标记；内容由调用方负责转义 */
  html?: string
  okText?: string
  /** danger = 不可恢复的危险操作（确认键走红） */
  okClass?: string
  bodyClass?: string
}

export type AccountsBridge = {
  getAccountConnections(): Promise<{ counts?: Record<string, unknown> } | null | undefined>
  updateAccount(id: string, patch: Record<string, unknown>): Promise<{ changes?: string[] } | null | undefined>
  /** 整队重排（分组模式）：把新队列整条交上去（从队首到队尾的 id 数组） */
  reorderAccounts(queue: string[]): Promise<unknown>
  moveAccount(id: string, direction: 'up' | 'down'): Promise<unknown>
  clearRateLimits(id: string, model?: string | null): Promise<unknown>
  /**
   * 账号路由（策略 / 粘性 / 退避 / 垫底 / 熔断）与候选队列可见化。
   * `setRoutingConfig` 收**部分**补丁、返回生效后的全量值（与 /api/timeouts
   * 同一模式）；`getRoutingQueue` 的 model 缺省 = 不限模型的全局队列。
   */
  /** 全量会话态（/api/session，app.js 的首屏数据源）；路由面板读 models / lastRequestModel */
  getState?(): Promise<{
    models?: Array<{ id?: string; providerLabel?: string }>
    lastRequestModel?: string | null
  } | null | undefined>
  getRoutingConfig(): Promise<Record<string, unknown> | null | undefined>
  setRoutingConfig(patch: Record<string, unknown>): Promise<Record<string, unknown> | null | undefined>
  getRoutingQueue(model?: string): Promise<Record<string, unknown> | null | undefined>
  /**
   * 批量操作（后端 `POST /api/accounts/batch`）。`proxy` 与 `maxConcurrent` 是
   * 两个带参动作的载荷：前者对应 action=proxy（null = 改为直连），后者对应
   * action=concurrency（0 = 不限）。**两条桥都按字段白名单转发**（bridge.rs /
   * web_shim.rs），加参数时两处都要补 —— 漏了会静默丢参数，服务端只报「缺字段」。
   */
  batchAccounts(payload: { action: string; ids: string[]; proxy?: unknown; maxConcurrent?: number }): Promise<{
    ok?: Array<{ id?: string; changes?: string[] }>
    removed?: unknown[]
    failed?: Array<{ id?: string; error?: string }>
  } | null | undefined>
  getAllBalances(id?: string): Promise<{ results?: Array<Record<string, unknown>> } | null | undefined>
  /**
   * 用量快照（后端 `usage_query::snapshot`）：余额结论之外还带 Token 限制器的
   * 周期读数 —— `tokenAt` 是这批读数的计算时刻（与余额的 `at` 各走各的，前端
   * 按它判断要不要应用），`tokenUsage` 按账号给启用中的每个 Token 周期一条
   * `{period, windowStart, used}`（没配规则的账号不出现）。
   */
  getBalancesSnapshot(): Promise<{
    at?: number
    results?: Array<Record<string, unknown>>
    tokenAt?: number
    tokenUsage?: Record<string, unknown>
  } | null | undefined>
  getProxies(): Promise<{ clash?: ClashSnapshot } | null | undefined>
  /** 代理池列表（「网络代理」页维护的命名代理）：账号代理表单的
   *  「已保存的代理」下拉读它；写侧（增删改）只有那一页用，不在这份桥里 */
  getProxyPool(): Promise<{ items?: PoolItem[] } | null | undefined>
  /** 测一条池条目（账号表单里选中池引用时的「测试出口」）：
   *  结果会记进那条代理的「上次测试」，与代理页那颗按钮同一个动作 */
  testProxyPoolItem(id: string): Promise<{
    success?: boolean
    ip?: string
    durationMs?: unknown
    error?: string
  } | null | undefined>
  testProxy(payload: { proxy: unknown }): Promise<{
    success?: boolean
    status?: unknown
    ip?: string
    durationMs?: unknown
    error?: string
  } | null | undefined>
  switchAccount(id: string): Promise<{ changed?: boolean } | null | undefined>
  refreshAccountToken(id: string): Promise<unknown>
  removeAccount(id: string): Promise<unknown>
  /**
   * ZCode 活动套餐通道的**验证码令牌池**概况（见后端 `providers/zcode/captcha.rs`）。
   *
   * 活动套餐的推理端点每条请求都要一个当次铸的阿里云验证码令牌，令牌由界面后台
   * 静默铸造（`ui/zcode-captcha-pool.js`）。这里读的是「当前库存 / 目标 / 是否
   * 正在缺货」—— 账号设置里选活动套餐时用它给用户一个可查的状态，
   * 否则「转发失败但不知道为什么」只能靠日志。
   */
  zcodeCaptchaStats?(): Promise<{
    ready?: number
    target?: number
    startPlanAccounts?: number
    rejected?: number
    minted?: number
    consumed?: number
    ttlMs?: number
  } | null | undefined>
}

/** `/api/proxies` 里 clash 那一段（出口列表 + 可用性） */
export type ClashSnapshot = {
  available?: boolean
  error?: string
  dir?: string
  options?: Array<{
    uid?: string
    name?: string
    port?: number
    profileActive?: boolean
    enabled?: boolean
  }>
}

/**
 * 「网络代理」页（代理池）的一条条目 —— 只列账号表单用到的字段，
 * 完整形态见 ui-islands/src/islands/proxies-page.tsx 的 PoolItem。
 * 账号的 proxy 可以按 id 引用它（`{source:'pool', proxyId}`）。
 */
export type PoolItem = {
  id: string
  name?: string
  /** manual | clash（表单里按它标「手动 / Clash Verge」） */
  source?: string
  enabled?: boolean
  /** 订阅节点的监听端口；null = 未分配（lazy listeners，见网络代理页） */
  port?: number | null
  /** 解析后的出口（Clash 条目的端口在这里是实时值）；不受控字段按 unknown 收 */
  resolved?: { protocol?: string; host?: string; port?: number | null; label?: string } | null
  /** 解析失败的原因（引用了已删除的 Clash 监听器等） */
  resolveError?: string | null
  /** 引用它的账号（后端在池列表里带的） */
  usedBy?: Array<{ id?: string; name?: string; enabled?: boolean }>
}

/**
 * 池条目的地址串：`HTTP 127.0.0.1:7910`（协议大写 + 主机 + 端口）。
 *
 * 与 OmniProxy 的名称列第二行同一口径（它的 ProxiesPage 就在名字下面写
 * `${protocol.toUpperCase()} ${host}:${port}`）。用解析后的 host/port 而不是
 * `resolved.label`：label 对 Clash 条目是「监控器名（:端口）」、对混合端口是
 * 「Clash 混合端口 7892」—— 都不含主机，且形态随来源变。这里统一成同一串，
 * 用户在下拉里能直接核对「这条连的是哪儿」。
 *
 * 取不到（解析失败 / 字段缺失）时回落 `resolved.label`，再没有就是空串。
 */
export function poolItemAddress(item: PoolItem): string {
  // lazy listeners：订阅节点可能还没有分配监听端口（未引用 / 引用刚保存
  // 内核还没重建完）—— 在这里标注出来，下拉与地址列都能看到原因
  if (item.source === 'subscription' && (item.port === null || item.port === undefined)) {
    return t('未分配端口')
  }
  const resolved = item.resolved
  if (resolved?.protocol && resolved.host && resolved.port) {
    return `${String(resolved.protocol).toUpperCase()} ${resolved.host}:${resolved.port}`
  }
  return resolved?.label || ''
}

/**
 * 池条目在下拉里的展示名：`香港HK-A（HTTP 127.0.0.1:7909）` /
 * `香港HK-A（HTTP 127.0.0.1:7909 · 已禁用）` / `香港HK-A（解析失败）`。
 *
 * 三个地方共用（账号表代理列 / 账号弹窗的代理表单 / 更新设置的出网线路）——
 * 各写一份迟早会漂移成本次这种事（有一处忘了写地址，用户下拉里只能看到名字）。
 * 条目的来源（手动 / Clash）不在这里标：下拉里已经有地址可比对，来源在
 * 「网络代理」页的类型列上。
 */
export function poolItemLabel(item: PoolItem): string {
  const name = item.name || item.id
  const address = item.resolveError ? t('解析失败') : poolItemAddress(item)
  const parts = [address, item.enabled === false ? t('已禁用') : ''].filter(Boolean)
  return parts.length ? t('{name}（{parts}）', { name, parts: parts.join(' · ') }) : name
}

/**
 * 代理下拉里「池条目」这一类的值前缀：`pool:<proxyId>`。
 *
 * 为什么需要前缀而不是直接用 proxyId：同一个下拉里还列着 Clash 的出口 uid
 * （来自 verge.yaml，形态不受我们控制，`px_...` 撞上它不是不可能），
 * 以及几个 `__proxy_*__` 占位值。前缀把三类值域彻底分开，判定只看开头即可。
 * 两个消费方（账号页代理列 / 更新设置的出网线路）读的是同一个常量 ——
 * 一边改了前缀、另一边没改，症状是「选了代理池条目却报不支持」，很难查。
 */
export const POOL_VALUE_PREFIX = 'pool:'

/**
 * window 上由别的脚本 / 别的岛挂载的共享桥。**只声明本页用到的成员**，
 * 且用「局部窄类型 + 转型」读，不 declare global（见文件头）。
 */
export type SharedWindow = {
  workbuddyDesktop?: AccountsBridge
  wbApp?: {
    esc?: (value: unknown) => string
    toast?: (message: string, kind?: 'err' | 'ok') => void
    formatTime?: (value: unknown) => string
    showPage?: (page: string) => void
    refresh?: () => Promise<unknown> | unknown
    runAccountAction?: (action: string, id: string) => void
    getState?: () => { accounts?: AccountsSnapshot } | null | undefined
    /** 顶栏状态区重画：余额结论会翻转「已限流」计数，应用完读数后调它跟上 */
    renderTopbarStatus?: () => void
    readonly currentPage?: string
  }
  wbConfirm?: { ask?: (options: ConfirmOptions) => Promise<boolean> }
  wbProviders?: {
    labelOf?: (id: string) => string
    all?: () => Array<{ id?: string; label?: string }>
    customList?: () => Array<{
      id?: string; name?: string; protocol?: string; baseUrl?: string
      /** 客户端形态伪装（`'opencode'` = 按 OpenCode 官方 CLI 形状补齐请求）。
       *  账号设置里判断「这家是不是 OpenCode Zen」的第二半判据，见 accounts-domain
       *  的 `isOpencodeZenProvider`。 */
      clientEmulation?: string
    }>
    customRequest?: (method: string, path: string, body?: unknown) => Promise<unknown>
    PROTOCOL_OPTIONS?: Array<{ value: string; label: string }>
  }
  wbFilterMemory?: {
    load<T extends Record<string, string>>(key: string, defaults: T): T
    save(key: string, patch: Record<string, string>): void
  }
  wbIcons?: { icon?: (name: string, size?: number) => string }
  wbColSettings?: {
    register(spec: ColSettingsSpec): ColSettingsHandle
    apply<C extends { key: string }>(id: string, columns: C[]): Array<C & { align: Align }>
    configOf(id: string): Array<{ key: string; visible: boolean; align: Align }> | null
    syncStaticHead(id: string, table: Element | null | undefined, viewHidden?: ReadonlySet<string> | null): void
    close(): void
  }
  /** 自定义提供商目录（查一家 / 改一家 / 删一家），账号设置弹窗里的「提供商」一段用它 */
  wbCustomProvidersUi?: {
    find?: (id: string) => Promise<{
      id: string; name?: string; protocol?: string; baseUrl?: string
      /** 客户端形态伪装（'' / 'opencode'）：账号设置弹窗的「提供商」段读它做初值 */
      clientEmulation?: string
    } | null | undefined>
    update?: (patch: {
      id: string; name: string; protocol: string; baseUrl: string; clientEmulation?: string
    }) => Promise<unknown>
    remove?: (id: string) => Promise<boolean>
  }
  /**
   * ZCode 领取的**网络侧**（ui/zcode-claim.js）：`preview` 探测可领取的一份份套餐
   * （不要验证码，失败时 throw），`claim` 领指定的一份（要验证码，返回业务结果）。
   * 界面在套餐明细弹窗（zcode-plans-modal.tsx）里，本文件只声明桥的形状。
   */
  wbZcodeClaim?: {
    preview?: (accountId: string) => Promise<unknown>
    claim?: (accountId: string, planId: string) => Promise<unknown>
  }
  /** ZCode 套餐明细弹窗（zcode-plans-modal.tsx）：可领取 + 名下已有的套餐，逐份领取 */
  wbZcodePlans?: { open?: (options: { id: string; name?: string }) => void }
  /** CodeArts「领福利」流程（ui/codearts-welfare.js：只读探测 → 确认 → 领取 → 回读） */
  wbCodeArtsWelfare?: { start?: (account: AccountRecord | undefined) => Promise<unknown> }
  /** 「添加账号」弹窗（归另一个代理，本页只调它的 open） */
  wbAddAccountModal?: { open?: () => void; close?: () => void }
  /** 添加表单的步骤复位（打开弹窗后按 providers 摘要重画卡片） */
  wbAccountAddForms?: { syncAddProvider?: () => void }
}

/** 列设置的登记项（照 table-col-settings.tsx 的 TableSpec 收窄成本页用到的那几个字段） */
export type ColSettingsSpec = {
  id: string
  label?: string
  columns: Array<{ key: string; label: string; align: Align; legacyAlign?: Align }>
  mount?: () => Element | null
  onChange?: () => void
}

export type ColSettingsHandle = {
  apply<C extends { key: string }>(columns: C[]): Array<C & { align: Align }>
  config(): Array<{ key: string; visible: boolean; align: Align }>
}

export function shared(): SharedWindow {
  return window as unknown as SharedWindow
}

/* ─── 小工具（跨文件共用的三件：播报 / 转义 / 时间）───── */

export function toast(message: string, kind?: 'err' | 'ok'): void {
  shared().wbApp?.toast?.(message, kind)
}

/**
 * HTML 转义：跨文件一律走 wbApp（app.js 里的全局单份实现），只有它还没就绪时才自己
 * 转一遍 —— wbConfirm.ask 收的是 HTML 片段，插值漏出去就是注入。
 */
export function esc(value: unknown): string {
  const fn = shared().wbApp?.esc
  if (fn) return fn(value)
  return String(value ?? '').replace(/[&<>"']/g, char => (
    { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[char] || char
  ))
}

/** 时间戳 → 本地时间串（0 与非法值返回空串，调用处据此省略那半句提示） */
export function formatTime(value: unknown): string {
  const fn = shared().wbApp?.formatTime
  if (fn) return fn(value)
  const time = Number(value)
  if (!Number.isFinite(time) || time <= 0) return ''
  return new Date(time).toLocaleString('zh-CN')
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

/**
 * 数值字段 → 展示串：**最多两位小数**，取不到给「—」。
 *
 * ── 为什么要有小数上限 ──────────────────────────────────────
 * 上游有几家给的是 `*Precise` 这类**高精度浮点**（WorkBuddy 国内版的
 * `CycleCapacityRemainPrecise` 实测回 `3547.430000335`），后端按「缺键不写、
 * 给什么写什么」如实透传，于是那串尾巴会铺满悬停提示与积分明细。积分读到两位
 * 小数已经比上游自己面板上给的还细，再多就是噪声。收口只在这里一处
 * （余额读数 / 余量 / 总量 / 明细行 / 限制器实况都走它），别在十来处各写一遍。
 *
 * 整数不受影响（`1500.00` → `1500`，尾随零由 `Number` 归掉）；解析不出数字的值
 * **原样透出** —— 上游偶尔给「不限」这类文案串，不能把一个有内容的值改写成「—」。
 */
export function numberText(value: unknown): string {
  if (value === null || value === undefined || value === '') return '—'
  const number = Number(value)
  if (!Number.isFinite(number)) return String(value)
  return String(Number(number.toFixed(2)))
}
