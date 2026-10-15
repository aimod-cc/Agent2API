/**
 * Agent2API · 设置页的**模型层**（桥类型 / 字段表 / 纯函数 / 页面文案）。
 *
 * 从 settings-page.tsx 拆出来：那一份「五个分类 + 十来个面板 + 一个确认框」的视图层装完
 * 已超过项目约定的单文件体量，而这一层的边界很清楚 —— 没有 JSX、没有状态。依赖单向
 * （settings-state.ts 与 settings-page.tsx import 它，它谁都不认识）。
 *
 * 它不是岛：文件名是 .ts，不会被 src/index.tsx 的 `islands/*.tsx` glob 加载；设置页的岛
 * 只有 settings-page.tsx 一个（页面级说明与取舍见它的文件头）。
 *
 * 这里的东西都是「只允许一处说了算」的口径：
 *   · 字段表（保留期 / 重试 / 超时 / 提示词模式）—— 键名必须与后端 config.rs 的常量逐字一致，
 *     散到视图里各写一遍必然漂（旧实现把这份对齐写在 settings-panel.js 的注释里）；
 *   · 页面文案 —— 从 index.html 的静态骨架逐字搬来（提示语 / 说明行 / 开关文字），
 *     静态骨架随本次迁移删除，这些字是页面上唯一的来源；
 *   · 纯函数（校验 / 归一化 / 文案派生）—— 状态层与视图层都要用。
 */

import { t } from '../i18n'


// ── 从 HEAD 恢复的类型（桥成员的返回形状）──────────────

export type DatabaseTableStat = { table?: string; rows?: number }

/** 整库导出结果（壳命令 export_database） */
export type DatabaseExportResult = {
  canceled?: boolean
  bytes?: number
  file?: string
  tables?: DatabaseTableStat[]
}

export type DatabaseImportResult = {
  /** 恢复后的各表行数 */
  tables?: DatabaseTableStat[]
  /** 替换前自动建的安全备份文件名（{config_dir}/backups/ 下） */
  safetyBackup?: string
  /**
   * 密钥指纹不匹配的警告（**只在有问题的恢复里出现**，文案由后端 `db::backup` 的
   * `account_key_warning` 生成并给全）。
   *
   * 它**不是失败**：导入照常完成，但这份备份是用另一把加密密钥做的（或备份时本机
   * 还没生成过密钥），导入后的账号数据解不开、账号列表会是空的。所以界面**必须**
   * 把它常驻显示出来（面板内的警告框）—— 只塞进几秒即逝的 toast 等于没提示，
   * 用户错过后只会对着空账号列表发懵。
   */
  warning?: string
}

export type ModelsDevRefreshResult = {
  status?: string
  message?: string
  count?: number
  refreshedAt?: number
}

/* ─── 桥与共享全局 ─────────────────────────── */

/** 启动与托盘设置（壳命令 get_app_settings / save_app_settings） */
export type AppSettings = {
  closeToTray: boolean
  autostart: boolean
  /** 局域网访问：监听 0.0.0.0（改动随「应用重启」生效，见 TIPS.lan） */
  lanAccess: boolean
  /** 局域网访问开启时是否同时托管网页管理面板（同样随重启生效） */
  lanPanel: boolean
  /** 轻量模式：关窗销毁界面进程（释放 WebView2 内存），托盘按需重建 */
  lightweightMode: boolean
}

/** 导入失败项：customProvider 标记的那条不是账号，是自定义提供商定义 */
export type ImportError = { id?: string; message?: string; customProvider?: boolean }

/** 导入结果（壳命令 import_accounts） */
export type ImportResult = {
  canceled?: boolean
  total?: number
  added?: number
  updated?: number
  skipped?: number
  failed?: number
  errors?: ImportError[]
  /** v2 导出文件附带的自定义提供商定义统计 */
  customProviders?: { added?: number; updated?: number }
}

/** 导出结果（壳命令 export_accounts） */
export type ExportResult = {
  canceled?: boolean
  count?: number
  /** v2 导出文件附带的自定义提供商定义条数 */
  customProviders?: number
  file?: string
}

/**
 * 网关自带提示词的**三段正文**（身份句 / 稳定段 / 动态段）。
 *
 * 三段各自成块是上游对身份的**结构**要求（实测：三段并成一段 → 405/3012，
 * 见后端 `zcode::OFFICIAL_PROMPT_NOTE`），所以界面与配置都按段来，不做
 * 「合成一整段」的编辑方式。动态段里的运行值写成占位符（`{cwd}` 等），
 * 发请求时才换成真实值。
 */
export type GatewayBlocks = { identity: string; stable: string; dynamic: string }

/** 提示词写入载荷：只传变化的那一项（后端允许部分字段），clearDegrade 是同一端点上的动作位 */
export type PromptPatch = {
  promptMode?: string
  promptFile?: string
  /**
   * 界面里编辑的提示词正文（**优先于** `promptFile` 与内置默认）。空串 = 清除
   * 这一份、回落文件 / 内置默认；缺失 = 这一项不改。
   */
  promptText?: string
  /**
   * 按提供商的覆盖：`{ "<providerId>": {promptMode?, promptFile?, promptText?} | null }`
   * （键名与全局那几项、以及响应里每家的字段完全一致，三处只有一套名字）。
   * 值是 `null` = 删掉这一家的覆盖（回落全局设置）；整张表缺失 = 这一维不改。
   */
  promptProviders?: Record<
    string,
    { promptMode?: string; promptFile?: string; promptText?: string } | null
  >
  /**
   * 网关自带提示词的逐家开关：`{ "<providerId>": true | false | null }`。
   * 值 `null` = 删键回到默认（装）；整张表缺失 = 这一维不改（与上面那张表同一约定）。
   */
  promptGateway?: Record<string, boolean | null>
  /**
   * 网关自带提示词的**正文覆盖**：`{ "<providerId>": {identity?, stable?, dynamic?} | null }`。
   * 段是**部分更新**（未出现的段保持原值，空串 = 这一段回到官方原文）；值 `null` =
   * 这一家整个回到官方原文。
   */
  promptGatewayText?: Record<string, Partial<GatewayBlocks> | null>
  clearDegrade?: boolean
}

/** Cline 伪装头接口的响应（GET 与 PUT 同形，键与后端 api/cline_headers.rs 一致） */
export type ClineHeadersData = {
  /** 用户改过的键（原样回显；空值 = 这个头不发送） */
  overrides: Record<string, string>
  /** 默认值（代码里内置的那一套；界面据此判断「改回了默认」） */
  defaults: Record<string, string>
  /** 默认值 ∪ 覆盖合并后的生效值（适配器实际会发的那份） */
  effective: Record<string, string>
}

/** 本页用到的壳 / HTTP 桥（方法名与 bridge.rs 一一对应，一个都不能改） */
export type SettingsBridge = {
  /** 'desktop' | 'web'：面板登录那一块只在网页端有意义 */
  platform?: string
  getAppSettings(): Promise<AppSettings | null | undefined>
  saveAppSettings(patch: AppSettings): Promise<AppSettings | null | undefined>
  exportAccounts(): Promise<ExportResult | null | undefined>
  importAccounts(): Promise<ImportResult | null | undefined>
  getRetention(): Promise<unknown>
  saveRetention(patch: Record<string, number>): Promise<unknown>
  getRetry(): Promise<unknown>
  saveRetry(patch: Record<string, unknown>): Promise<unknown>
  getTimeouts(): Promise<unknown>
  saveTimeouts(patch: Record<string, number>): Promise<unknown>
  getQueue(): Promise<unknown>
  saveQueue(patch: Record<string, number>): Promise<unknown>
  getDebug(): Promise<unknown>
  saveDebug(enabled: boolean): Promise<unknown>
  getSanitize(): Promise<unknown>
  saveSanitize(enabled: boolean): Promise<unknown>
  getClineHeaders(): Promise<unknown>
  saveClineHeaders(overrides: Record<string, string>): Promise<unknown>
  getCors(): Promise<unknown>
  saveCors(enabled: boolean): Promise<unknown>
  getPrompt(): Promise<unknown>
  savePrompt(payload: PromptPatch): Promise<unknown>
  getStorage(): Promise<unknown>
  getCaptchaSetting(): Promise<{ captchaEnabled?: boolean } | null | undefined>
  saveCaptchaSetting(on: boolean): Promise<{ captchaEnabled?: boolean } | null | undefined>
  panelLogout(): Promise<unknown>
  /** 后端就绪状态（局域网地址展示要拼网关端口；其余字段本页不消费） */
  getBackendStatus?(): Promise<{ port?: number } | null | undefined>
  // ── 局域网访问（仅桌面端渲染，web 端 shim 不提供这几个方法）──
  /** 面板管理员是否已注册（开启流程据此决定要不要先走注册步） */
  panelAdminStatus?(): Promise<{ registered?: boolean } | null | undefined>
  /** 注册面板管理员（受信本地路径，不走公开的 HTTP 注册端点） */
  panelRegister?(username: string, password: string): Promise<{ registered?: boolean; existed?: boolean }>
  /** 切换局域网访问（写设置并重启应用）；返回里的 createdKey 非空 = 已自动补了首把 Key */
  changeLanAccess?(enabled: boolean, panel: boolean): Promise<unknown>
  /** 本机在局域网里的地址（查不到为 null） */
  localIp?(): Promise<string | null | undefined>
  /** 打开外链的唯一出口（只放行 http(s)）：桌面走系统浏览器，网页端 shim 是 window.open */
  openReleasePage?(url: string): Promise<unknown>
  // ── 以下成员从 HEAD 恢复（并行改动移走了定义但 settings-state.ts 仍引用）──
    exportDatabase?(includeLogs: boolean): Promise<DatabaseExportResult | null | undefined>
    importDatabase?(): Promise<DatabaseImportResult | null | undefined>
    getCaptchaPool(): Promise<unknown>
    saveCaptchaPool(patch: Record<string, number>): Promise<unknown>
    getRoutingConfig(): Promise<unknown>
    setRoutingConfig(patch: Record<string, unknown>): Promise<unknown>
    getModelsDev(): Promise<ModelsDevState | null | undefined>
    saveModelsDev(on: boolean): Promise<ModelsDevState | null | undefined>
    refreshModelsDev(): Promise<{ result?: ModelsDevRefreshResult; status?: ModelsDevState } | null | undefined>
}

/**
 * window 上由其它脚本 / 其它岛挂载的共享桥。
 *
 * 刻意用「局部窄类型 + 转型」而不是 declare global 往 Window 上加属性：
 * workbuddyDesktop / wbApp / wbUnits 是多个岛共用的桥，各岛各 declare 一份会因同名属性
 * 类型不一致直接报 TS2717（并行迁移时必然撞车）。这里只认本页用到的成员；
 * 设置页独占的 wbSettingsPanel 由 settings-page.tsx 自己 declare。
 */
export type SharedWindow = {
  workbuddyDesktop?: SettingsBridge
  wbApp?: {
    toast?: (message: string, kind?: 'err' | 'ok') => void
    /** 账号被导入改动后让主界面重绘（账号列表、导航计数） */
    refresh?: () => Promise<void> | void
    /** 当前页标识：脚本加载时若已停在设置页，补一次 load() */
    readonly currentPage?: string
    /** 应用显示模式（system / light / dark）：实现与持久化都在 app.js，本页只调用 */
    applyTheme?: (mode: string) => void
    /** 应用界面缩放（传百分数，如 105）：由 app.js 落到 WebView 层并记档 */
    applyZoom?: (percent: number) => void
  }
  /** Token 计量单位的展示口径（units.js）：本页只负责拨开关，格式化在那边 */
  wbUnits?: { isChinese?: () => boolean; setChinese?: (on: boolean) => void }
  /** 软件更新面板的岛（update-panel.tsx）：切到设置页时让它自己刷新一次 */
  wbUpdatePanel?: { load?: () => Promise<void> | void }
  /** 内联图标集（icons.js）：左栏分类图标由它渲染（返回 SVG 串，注入用） */
  wbIcons?: { icon?: (name: string, size?: number) => string }
}

export function shared(): SharedWindow {
  return window as unknown as SharedWindow
}

export function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

/** toast 的统一出口（运行期读 wbApp，不在模块顶层解构） */
export function toast(message: string, kind?: 'err' | 'ok'): void {
  shared().wbApp?.toast?.(message, kind)
}

/**
 * 打开外链：一律交给系统默认浏览器（桌面走壳命令 `open_release_page`，网页端
 * shim 把它映射成 window.open）—— 与 update-shared 的同名函数是**两份实现**，
 * 不跨族 import 是刻意的：这里服务设置页（反馈与需求），那边服务更新面板一族，
 * 各自只依赖自己那份桥类型；但也别再加第三份，要用先从这两处挑。
 */
export async function openExternal(url: string): Promise<void> {
  try {
    await shared().workbuddyDesktop?.openReleasePage?.(url)
  } catch (error) {
    toast(`打开链接失败：${errorMessage(error)}`, 'err')
  }
}

/* ─── 分类与偏好键 ─────────────────────────── */

/**
 * 左侧分类。顺序 = 界面顺序；showCategory 用它校验传进来的值（旧实现是查 DOM，
 * 这里改成查这份表 —— 分类不再由 HTML 声明，而是本页渲染出来的）。
 *
 * 「重试」「超时」从「网关」里拆出来独立成菜单（原先挤在网关一栏里，排在排队等待
 * 两侧）：这两组是**每次转发都会读**的网络行为参数，出问题时最常被翻，单独一栏
 * 少一次翻找。数据加载本就不分分类（settings-state 的 load 一次并行取全部），
 * 拆分只是视图层的两段搬迁。
 *
 * 「显示」紧跟「通用」：显示模式 / 界面缩放 / 语言都是**纯前端偏好**（存在
 * localStorage 里，与主题同族），不碰后端配置 —— 放在最前面那几栏里最顺手。
 *
 * 「反馈与需求」是纯跳转面板（三个 GitHub issue 表单入口，见 settings-page 的
 * FeedbackPane），排在「更新」上面 —— 都是「对外」的两栏，挨着放。
 *
 * `icon` 是 icons.js 里那组设置页分类图标的键（描边风格，见那边的说明）；标签
 * 不再受两字限制，图标负责在窄栏里一眼认出，文字负责说清。
 *
 * 「更新」（原「关于」）**id 保持 `about` 不变**：它同时是 update-panel.tsx 的挂载点
 * 选择器（`.settings-pane[data-cat="about"]`）与用户 localStorage 里存着的分类值，
 * 改名会让两者当场失配 —— 用户看到的只是标签，id 是内部契约。
 */
export const CATEGORIES = [
  { id: 'general', label: '通用', icon: 'sliders' },
  { id: 'display', label: '显示', icon: 'display' },
  { id: 'gateway', label: '网关', icon: 'traffic' },
  { id: 'retry', label: '重试', icon: 'refresh' },
  { id: 'timeout', label: '超时', icon: 'timer' },
  { id: 'security', label: '安全', icon: 'shield' },
  { id: 'data', label: '数据', icon: 'database' },
  { id: 'feedback', label: '反馈与需求', icon: 'feedback' },
  { id: 'about', label: '更新', icon: 'download' },
] as const

/**
 * 「设置页当前分类」的 localStorage 键：纯前端偏好（与主题、计量单位同类），
 * 主进程不参与。键名沿用旧实现的取值，别改 —— 否则用户上次停留的分类会丢。
 */
export const SETTINGS_CAT_KEY = 'workbuddy-desktop-settings-cat'

/* ─── 显示偏好（「显示」分类） ───────────────── */

/**
 * 「显示」分类的三项偏好：显示模式 / 界面缩放 / 语言。
 *
 * ⚠ 前两项的**应用入口都不在本页**，而在 ui/app.js（applyTheme / applyZoom）：
 *   · 主题还要同步操作系统标题栏的深浅色，并处理「跟随系统」时窗口主题会污染
 *     WebView 颜色偏好的问题（那段长注释在 app.js）；
 *   · 缩放要走 WebView 层（壳命令 set_zoom），不是页面自己能做完的事。
 * 本页只做两件事：**读** localStorage 把控件摆到当前值上；改动时调
 * `wbApp.applyTheme / applyZoom`，再靠 'wb:theme' / 'wb:zoom' 事件跟随 ——
 * 侧边栏的主题三键与这里的档位是同一个设置的两个入口，必须互相同步。
 *
 * 键名与 app.js 里的字面量是同一份契约（两侧各写一份，改一处必然漂）；
 * 事件名同理（app.js 派发，本页监听）。
 */
export const THEME_KEY = 'workbuddy-desktop-theme'
export const ZOOM_KEY = 'workbuddy-desktop-zoom'
export const THEME_EVENT = 'wb:theme'
export const ZOOM_EVENT = 'wb:zoom'

/** 缩放档位边界与步长：与 app.js 的 ZOOM_MIN / ZOOM_MAX / ZOOM_STEP 同源 */
export const ZOOM_MIN = 80
export const ZOOM_MAX = 130
export const ZOOM_STEP = 5

/** 缩放候选项（80% ~ 130%，5% 一档共 11 档）：下拉的 value 就是百分数本身 */
export const ZOOM_PERCENTS: number[] = Array.from(
  { length: (ZOOM_MAX - ZOOM_MIN) / ZOOM_STEP + 1 },
  (_, index) => ZOOM_MIN + index * ZOOM_STEP,
)

/**
 * 显示模式三档：value 与 app.js / `data-theme` 的取值逐字一致（system / light / dark）。
 * 文案取侧边栏那三个按钮的 title（跟随设备 → 跟随系统，是同一件事的两种叫法，
 * 这里用了更书面的一种）。
 */
export const THEME_MODES = [
  { value: 'system', label: '跟随系统' },
  { value: 'light', label: '浅色' },
  { value: 'dark', label: '深色' },
] as const

/** 显示模式取值（SegmentedControl 的泛型参数要用它，免得在视图里转字面量联合） */
export type ThemeMode = (typeof THEME_MODES)[number]['value']

/**
 * 语言选项已迁到运行时：界面语言的可选列表由 ui/i18n.js 暴露的 `wbI18n.LOCALES`
 * 提供（label 是各语言自己的写法），持久化键 workbuddy-desktop-locale。
 * 这里不再维护 LANGUAGES —— 两份列表放两处必然漂，语言这件事只有 ui/i18n.js 一个出处。
 */

/** 读当前显示模式（app.js 是写入方）：读到非法值按跟随系统 */
export function readThemeMode(): ThemeMode {
  try {
    const mode = localStorage.getItem(THEME_KEY)
    return mode === 'light' || mode === 'dark' ? mode : 'system'
  } catch {
    return 'system'
  }
}

/** 读当前缩放（百分数）：非法 / 越界值回落 100，与 app.js 的 storedZoom 同一口径 */
export function readZoomPercent(): number {
  try {
    const text = localStorage.getItem(ZOOM_KEY)
    // 空串 / 缺失要单独挡：Number('') 与 Number(null) 都是 0（不是 NaN），
    // 不挡就会被当成「0%」一路夹到 80% —— 「没设过」必须等于默认的 100%
    if (text === null || text.trim() === '') return 100
    const raw = Number(text)
    if (!Number.isFinite(raw)) return 100
    const snapped = Math.round(raw / ZOOM_STEP) * ZOOM_STEP
    return Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, snapped))
  } catch {
    return 100
  }
}

/* ─── 数字字段表 ───────────────────────────── */

export type NumberField = {
  /** 后端 config 的 JSON 键（大小写必须逐字一致，否则 PUT 被静默忽略） */
  key: string
  /** 控件 id：保留旧 id 便于排查（没有任何外部脚本再按 id 读这些控件） */
  id: string
  label: string
  /** 输入框右侧的单位 */
  unit: string
  min: number
  max: number
  /** 行下方的说明（逐字来自 index.html 静态骨架） */
  hint: string
}

/** 与后端 RETENTION_MIN_DAYS / RETENTION_MAX_DAYS 同源（非法值后端会 400） */
export const RETENTION_MIN = 1
export const RETENTION_MAX = 3650

export const RETENTION_FIELDS: NumberField[] = [
  {
    key: 'logRetentionDays',
    id: 'settings-retention-log',
    label: '事件日志保留天数',
    unit: '天',
    min: RETENTION_MIN,
    max: RETENTION_MAX,
    hint: '登录 / 账号切换 / 429 切换等系统事件，可填 1–3650 天。',
  },
  {
    key: 'requestRetentionDays',
    id: 'settings-retention-request',
    label: '请求日志保留天数',
    unit: '天',
    min: RETENTION_MIN,
    max: RETENTION_MAX,
    hint: '「请求日志」页的逐条请求记录，可填 1–3650 天。',
  },
  {
    key: 'dailyRetentionDays',
    id: 'settings-retention-daily',
    label: '按天聚合保留天数',
    unit: '天',
    min: RETENTION_MIN,
    max: RETENTION_MAX,
    hint: '报表页的热力图与按天趋势，可填 1–3650 天。',
  },
]

/**
 * 三项重试字段。第二项的键名 `retryCrossProviderCount` 是**旧措辞**（配置兼容，
 * 改名会让老配置读不到、静默回落默认值），后端常量已经改叫 KEY_RETRY_ACCOUNT_SWITCH_COUNT
 * —— 这里必须沿用旧字符串，只有展示名跟着真语义走。
 */
export const RETRY_FIELDS: NumberField[] = [
  {
    key: 'retryCount',
    id: 'settings-retry-count',
    label: '同一账号重试次数',
    unit: '次',
    min: 0,
    max: 10,
    hint: '在同一个账号上原地重发几次（不含首发；整条请求共用一份，用完后换来的账号只发一次就继续换）。可填 0–10，0 表示失败立即换号。',
  },
  {
    key: 'retryCrossProviderCount',
    id: 'settings-retry-cross-provider-count',
    label: '切换账号重试次数',
    unit: '次',
    min: 0,
    max: 10,
    hint: '失败后最多再换几个账号试（填 N = 最多换 N 个，首发那个不算）。换到同一家名下另一个账号、或换到另一家，都各算一次；换满仍失败就返回错误。可填 0–10，0 表示失败立即报错。',
  },
  {
    key: 'retryIntervalSeconds',
    id: 'settings-retry-interval',
    label: '重试间隔',
    unit: '秒',
    min: 0,
    max: 300,
    hint: '两次重试之间的等待时间，两项共用，可填 0–300 秒。',
  },
]

/** 超时字段：键名与 config.rs 的 KEY_TIMEOUT_* 指向的 JSON 键逐字一致 */
export const TIMEOUT_FIELDS: NumberField[] = [
  {
    key: 'connectTimeoutSeconds',
    id: 'settings-timeout-connect',
    label: '连接中超时',
    unit: '秒',
    min: 1,
    max: 3600,
    hint: '建立上游 TCP/TLS 连接或代理隧道的最大等待时间（秒），默认 30 秒，范围 1~3600。',
  },
  {
    key: 'headersTimeoutSeconds',
    id: 'settings-timeout-headers',
    label: '等待响应超时',
    unit: '秒',
    min: 1,
    max: 3600,
    hint: '请求发出后等待上游响应头的最大时间（秒），默认 300 秒，范围 1~3600。',
  },
  {
    key: 'streamIdleTimeoutSeconds',
    id: 'settings-timeout-stream-idle',
    label: '流式响应空闲超时',
    unit: '秒',
    min: 1,
    max: 3600,
    hint: '流式响应相邻数据之间允许的最大空闲时间（秒），收到新数据后重新计时，默认 300 秒，范围 1~3600。',
  },
  {
    key: 'bodyTimeoutSeconds',
    id: 'settings-timeout-body',
    label: '非流式响应超时',
    unit: '秒',
    min: 1,
    max: 3600,
    hint: '读取完整非流式响应体允许的最大时间（秒），默认 300 秒，范围 1~3600。',
  },
]

/** 排队等待字段：键名与 config.rs 的 KEY_QUEUE_* 指向的 JSON 键逐字一致 */
export const QUEUE_FIELDS: NumberField[] = [
  {
    key: 'queueMaxWaits',
    id: 'settings-queue-max-waits',
    label: '排队等待次数',
    unit: '次',
    min: 0,
    max: 10,
    hint: '上游模型繁忙时会先排队（回一句「建议 N 秒后再来」，Qoder 免费模型就是这样）：网关等一会儿再发同一请求，最多等几次。默认 2 次，可填 0–10；填 0 表示不等待，排队直接返回 503 并说明「不是登录态或额度问题」。',
  },
  {
    key: 'queueWaitSeconds',
    id: 'settings-queue-wait-seconds',
    label: '排队等待秒数',
    unit: '秒',
    min: 0,
    max: 120,
    hint: '每次等待的时长，默认 0 = 跟随上游建议（实测 9~30 秒，上游没给建议时用 15 秒）。可填 0–120 秒强制一个固定时长。',
  },
]

/** 「指定错误码直接换号」：键名、状态码范围与名单上限与后端 retry_api.rs 逐字同源 */
export const NO_RETRY_CODES_KEY = 'noRetryStatusCodes'
export const RETRY_CODE_MIN = 100
export const RETRY_CODE_MAX = 599
export const RETRY_MAX_CODES = 50

/** 提示词模式：下拉的展示文案（含「（默认）」）与 toast 里用的短名分开 —— 旧实现是两份表 */
export type PromptModeOption = { value: string; optionLabel: string; toastLabel: string }

export const PROMPT_MODES: PromptModeOption[] = [
  { value: 'passthrough', optionLabel: '透传客户端 system（默认）', toastLabel: '透传客户端 system' },
  { value: 'custom', optionLabel: '替换为网关提示词', toastLabel: '替换为网关提示词' },
  { value: 'append', optionLabel: '追加网关提示词', toastLabel: '追加网关提示词' },
]

/* ─── 校验与归一化 ─────────────────────────── */

export type ParseResult = { ok: true; value: number } | { ok: false; message: string }

/**
 * 前端校验：min–max 的整数（后端也会挡，这里先挡省一次往返，且提示更短）。
 * 用 /^\d+$/ 而不是 Number() + Number.isInteger()：后者会把 "1e2" 认成 100、
 * "0x10" 认成 16，这些都不是「用户填了几天 / 几次」的直觉答案，不如直接判非法。
 * `noun` 是提示语的主语：保留期用「天数」，其余面板用自己的标签。
 */
export function parseInteger(raw: unknown, min: number, max: number, noun: string): ParseResult {
  const text = String(raw ?? '').trim()
  // 空串要单独挡：空值过不了下面的正则，但提示语不同（「不能为空」比「必须是整数」更准）
  if (!text) return { ok: false, message: `${noun}不能为空（可填 ${min}–${max}）` }
  if (!/^\d+$/.test(text)) return { ok: false, message: `${noun}必须是整数` }
  const value = Number(text)
  if (value < min || value > max) {
    return { ok: false, message: `${noun}必须在 ${min}–${max} 之间（当前填的是 ${text}）` }
  }
  return { ok: true, value }
}

/**
 * 保存返回值归一化：后端按契约应回传完整设置，但只认它是布尔才采纳，
 * 缺字段时沿用本次提交的值，避免把开关误渲染成「关闭」。
 */
export function normalizeApp(saved: unknown, fallback: AppSettings): AppSettings {
  const result: AppSettings = { ...fallback }
  if (saved && typeof saved === 'object') {
    const record = saved as Record<string, unknown>
    if (typeof record.closeToTray === 'boolean') result.closeToTray = record.closeToTray
    if (typeof record.autostart === 'boolean') result.autostart = record.autostart
    if (typeof record.lanAccess === 'boolean') result.lanAccess = record.lanAccess
    if (typeof record.lanPanel === 'boolean') result.lanPanel = record.lanPanel
    if (typeof record.lightweightMode === 'boolean') result.lightweightMode = record.lightweightMode
  }
  return result
}

/**
 * 数字面板的响应归一化（保留期 / 重试 / 超时共用同一口径）：
 * 只采纳「范围内的整数」，缺字段 / null / 字符串一概沿用上一轮的有效值 ——
 * 不把线上没给的项清空，也不写 "undefined" 进输入框。
 * 返回 null 表示整块不可用（响应形状不对，或三项都拿不到有效值）。
 */
export function normalizeNumbers(
  fields: NumberField[],
  data: unknown,
  previous: Record<string, number> | null,
): Record<string, number> | null {
  if (!data || typeof data !== 'object') return null
  const record = data as Record<string, unknown>
  const next: Record<string, number> = { ...(previous || {}) }
  for (const field of fields) {
    const value = Number(record[field.key])
    if (Number.isInteger(value) && value >= field.min && value <= field.max) {
      next[field.key] = value
    }
  }
  // 一项有效值都拿不到（换壳后接口形状变了之类）：按不可用处理，
  // 不让一排空输入框留在页面上
  if (!fields.some(field => Number.isInteger(next[field.key]))) return null
  return next
}

/* ─── 读数格式化（数据存储面板） ─────────────── */

/** 字节数 → 可读大小（库主文件通常几百 KB 到几十 MB，四档够用） */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B'
  if (bytes < 1024) return `${bytes} B`
  const kb = bytes / 1024
  if (kb < 1024) return `${kb.toFixed(1)} KB`
  const mb = kb / 1024
  if (mb < 1024) return `${mb.toFixed(1)} MB`
  return `${(mb / 1024).toFixed(2)} GB`
}

/** 条数 → 带千分位的文本（用户对着看更省事）；非法值给「—」 */
export function formatCount(value: unknown): string {
  const count = Number(value)
  if (!Number.isFinite(count) || count < 0) return '—'
  return count.toLocaleString('zh-CN')
}

/* ─── 页面文案（逐字来自 index.html 静态骨架） ─── */

/** 标题右侧问号（`[data-tip]`）的说明全文：tooltip.js 仍在页面上跑，照旧服务这些元素 */
export const TIPS = {
  displayTheme: '控制界面的深浅色，与左侧边栏底部的三个主题按钮是同一个设置（改哪一处，另一处立刻跟上）。「跟随系统」会随操作系统当前的浅色 / 深色自动切换，并在系统主题变化时即时跟上；选「浅色」或「深色」则把界面固定在该模式，不再随系统变化。窗口标题栏的深浅色会一并同步，不会出现深色界面配一条浅色标题栏的情况。',
  displayZoom: '等比放大或缩小整个界面（文字、控件、间距一起变），效果与浏览器按 Ctrl +/- 相同：80%–130%、5% 一档。窗口本身不缩放，变的是页面内容的显示比例，设置立即生效并记住，下次启动直接按这个比例打开。放得越大可视范围越小，窗口较窄或表格较宽时不建议调得太大。',
  displayLanguage: '界面语言。可选简体中文、繁體中文、English、日本語、한국어、Português (Brasil)，或「跟随系统」（按浏览器 / 系统的语言列表自动选）。切换后整页立即刷新；该偏好只存在本机（localStorage），不影响账号、密钥与转发。',
  tray: '默认关闭窗口不会退出程序，而是把窗口缩到系统托盘，转发继续在后台运行（OpenAI 客户端不受影响）；要彻底退出程序，请在托盘图标上右键选「退出」。关掉这个开关后，点关闭按钮即退出程序、转发随之中断。「开机自动启动」开启后，登录系统时会自动启动本程序（通常直接驻留托盘），不需要手动打开。',
  lightweight: '开启后，关闭窗口不只是隐藏，而是把界面窗口整个销毁：底层的 WebView2 六个常驻进程（约 200MB 内存）随之退出，网关转发、定时签到、余额查询都在后台进程里继续，不受影响；需要界面时点托盘图标重新打开（WebView 冷启动约 1 秒）。注意：界面销毁期间「ZCode 活动套餐」通道的验证码令牌由界面铸造，无法补充 —— 走该通道的请求会以 503 落进请求日志（写明原因），重新打开界面即自动恢复。此开关只在「关闭窗口时最小化到托盘」开启时有意义，最小化窗口不受影响。',
  lan: '默认网关只监听 127.0.0.1，只有本机能访问。开启后网关改听所有网卡，同一局域网内的设备可以把 API 地址指向本机 IP 一起使用。出于安全考虑，开启前必须先注册一个面板管理员：管理接口从此要求管理员会话或网关 Key，模型额度不会对局域网裸奔；一把启用的网关 Key 都没有时，转发接口也会拒绝服务，直到创建第一把。改动需要重启应用生效。「同时开放网页管理面板」把管理界面也出给局域网（其他设备的浏览器打开本机 IP 即可管理），默认关闭 —— 桌面端的面板仍只由本程序自己出。',
  units: '控制报表与请求日志里 Token 读数的写法：开启后按中文量级显示（1.2亿 / 8400万），关闭则用 k / M 缩写（与上游文档、接口字段的写法一致）。这只影响显示口径，不改变任何统计与存储的数值。',
  queue: '只对「排队制」的上游生效（目前是 Qoder 的免费模型）：模型繁忙时上游不报错，只回一句「建议 N 秒后再来」（业务码 10605），网关按建议时长等一会儿再发同一请求，等满次数仍排不上才把「排队中」作为错误返回（HTTP 503，文案会说明这不是登录态或额度问题）。等待发生在首个字节之前，吃的是「等待响应超时」那份预算 —— 两项设置一起决定一次请求最多卡多久；排队不会标记账号限额、也不会换账号（换谁都一样在排队）。保存后对下一个请求立即生效，不用重启。',
  timeouts: '上游请求四个阶段各自的等待上限（秒，1~3600）：①「连接中超时」= 建立 TCP/TLS 连接或代理隧道的最大等待，默认 30 秒；②「等待响应超时」= 请求发出后等上游响应头的最大时间，默认 300 秒 —— 网关请求上游恒带 stream，响应头在 SSE 建立时就到达，与模型思考多久无关；③「流式响应空闲超时」= 流式响应相邻两块数据之间允许的最大空闲，收到新数据即重新计时，默认 300 秒 —— 上游长时间不吐数据即判定连接僵死并断开；④「非流式响应超时」= 读完整份非流式响应体的总预算（一次性计时、不重置），默认 300 秒。四项都是「等待上限」，只要数据还在来，正常的流式回答就一直往下传。保存后对下一个请求立即生效，不用重启。',
  retry: '请求转发到上游失败时（服务器瞬时错误、连接失败，以及 WorkBuddy 的 11-128 敏感词拦截），网关会等一段间隔后再发一次。次数分两档，管的是两件事：①「同一账号重试次数」在同一账号上原地重发几次 —— 一般是提示词被上游拦截、或链路瞬时抖动，等一会儿重发往往就好了；②「切换账号重试次数」这份请求最多再换几个账号试 —— 首发的那个账号不算，每换一个扣一次，不分是不是同一家（换到同一家的下一个账号也算一次），换满还失败就把错误返回给客户端。两项都设为 0 表示失败不重试、直接报错。注意账号级限额（429）不占这里的换号次数 —— 那类失败走「冷却该账号并换下一个账号」的降级，与这里的重试是两条路。另有「指定错误码直接换号」名单（默认 402）：命中名单的上游状态码不在同一账号重发，直接按队列换下一个账号继续试（换号也占「切换账号重试次数」），换满仍失败才把错误返回给客户端。',
  sanitize: '上游用「逐字精确匹配」的方式审核请求体（不是语义审核）：客户端注入的固定模板句、计费头字段名、以及某些裸错误码出现在报文里就会整单拦截，返回 400。开启本项后，网关在每次转发前改写这些指纹 —— 表头键值整段删除，承载语义的模板句只换一个词（如 official CLI for Claude → official CLI tool for Claude），对话内容与语义都不受影响。规则集是内置的，不需要也无法维护词表。关掉本项后客户端模板会原样发往上游，可能重新出现模板句被误拦的报错。',
  clineHeaders: 'Cline 上游按请求头判定「调用方是不是 Cline 自家产品」：X-CLIENT-TYPE: cline-sdk 是硬门槛（缺了它免费池模型一律 403），版本号与平台标识让请求更贴近官方客户端的形态。这里列出的是网关默认会发的头：改某一行的值 = 覆盖默认值（也可以新增默认清单之外的自定义头）；把值清空保存 = 这个头不发送；在界面删掉被改过的行 = 回落默认值。改完点「保存」，下一个 Cline 请求立即生效。不懂某一行的含义就不要动它 —— 伪装头的价值在「与官方客户端逐字一致」。',
  cors: '网关面（/v1/*）默认不应答浏览器跨源请求：页面直连会被预检拦下，只报「无法连接 API」。注意来源是 * —— 未配「网关 Key」时任何网页都能借本机网关打上游，故默认关闭；只给自己用的话，让页面与网关同源（本地反代 /v1）更稳妥。面板接口（/api/*）不受影响。',
  prompt: '客户端（Claude Code / Codex 等 CLI）会在 system 提示词里注入几十句固定模板，上游按逐字匹配审核，命中就整单拦截（HTTP 400）。指纹脱敏能改写实测命中过的那几句，但换一个客户端版本就可能冒出新的。这里可以把 system 整段换成网关自己那份：①「透传」= 不动客户端 system（默认，行为与之前完全一致）；②「替换」= 删掉客户端所有 system / developer 消息，换成网关的提示词（客户端项目规范随之消失）；③「追加」= 在开头连续 system 块之后插入一条网关提示词，既有消息逐字不动（客户端规范与网关提示词并用）。那份提示词有两个来源：提示词文件（留空用内置默认），或直接在界面上编辑正文 —— 编辑过就以那份正文为准（优先于文件），清空则回到文件 / 内置默认。另外，透传 / 追加模式下撞了内容拦截（多半是指纹误报）时，网关会自动换一段最小中性提示词重试一次，并把「降级期」开到次日 00:00 —— 期间所有请求直接带中性提示词出门，不再先撞一次 400；这里能看见并提前解除它。上面几项是**默认值**：想给某个提供商单独配，用下面的「按提供商」加一行（带「网关自带」的家默认就列在那里）—— 不同的上游对这些内容的接受程度不一样。',
  debug: '开启后，网关会把每次转发**发给上游的请求**（请求头 + 请求体）与**上游返回的响应**（状态码 + 响应头 + 响应体）完整保存到本地，供请求日志页的「详情」查看。请求头里的 Authorization、Cookie、API Key 等凭据字段一律替换成 [redacted]，不会明文落盘；请求体与响应体按原样保存（可能包含你的对话内容）。报文只保留最近 500 条，超出后丢弃最旧的。这是排障用的临时开关，不需要时建议关闭。',
  io: '导出会把全部账号与自定义提供商定义写入一个 JSON 文件，可以拷到另一台机器上导入后继续使用。导入采用合并策略：同提供商下按业务身份（UID / userId / apiKey 等）去重 —— 已存在的账号只更新凭证，保留本机原有的优先级顺序；新账号追加到转发顺序末尾，不会抢占当前正在使用的账号；自定义提供商定义按 id 合并，本机缺失时自动补建。',
  retention: '三类数据各自独立计时，超出保留天数的部分会被删除：事件日志是登录、账号切换、429 切换这类系统事件；请求日志是网关每次转发到上游的逐条记录；按天聚合供报表页的热力图与按天趋势使用。把某一档改小（例如 30 天改成 7 天）保存后会立即删除超出的历史数据，此操作不可恢复；改大或保持不变不会删除任何数据。三项的可填范围均为 1–3650 天。',
  storage: '全部数据（账号、事件日志、请求记录、调试报文、设置）统一保存在配置目录下的 agent2api.db 这一个 SQLite 数据库里。备份时只需拷贝这个文件；更换保存位置请设置环境变量 AGENT2API_PROXY_HOME 后重启程序。',
} as const

/** 面板底注（`.hint.retention-note`）与各面板内的说明行 */
export const NOTES = {
  timeouts: '保存后立即生效，不用重启；超时按上游失败处理（计入请求日志的「错误」列，并按「请求重试」的设置决定是否重试）。',
  queue: '等待期间请求日志的状态列会显示「排队中」阶段，详情弹窗的「内部重试」里能逐次看到等待时长；等待用尽后返回 HTTP 503（不是 401/429：既不会刷新凭证，也不会把账号标成限额）。',
  retry: '保存后立即生效，不用重启；次数用尽仍失败时，错误原样返回给客户端。',
  retryCodes: '命中这些上游状态码的失败不在同一账号重发，直接换下一个账号继续试（换号占用「切换账号重试次数」，换满仍失败才把错误原样返回给客户端）。输入 100–599 的状态码后回车添加，点 × 或退格删除；默认 402（积分不足）。清空全部表示任何错误都照常重试。',
  sanitize: '默认开启。命中明细会记进请求日志的「敏」标签（悬停可看命中了哪几条规则）。本项只改发给上游的副本，客户端看到的响应内容不变。',
  clineHeaders: '值与默认值相同的行不占覆盖（存的是「改过的键」，不是全量配置）；值留空保存表示这个头不发送，删除改过的行表示回落默认值。改完立即生效，不用重启。',
  cors: '默认关闭。改动立即生效，不用重启；关闭期间的跨源请求会被预检拦下。',
  debug: '开启后新发生的请求才会被记录（已经过去的请求补不回来）。报文统一存放在本地数据库里，只保留最近 500 条（超出后丢弃最旧的），条数见左侧「数据 → 数据存储」的「调试报文」。',
  promptMode: '替换 = 客户端 system / developer 消息全部删掉，换成网关的提示词；追加 = 保留客户端内容，只在开头 system 块之后多插一条。两者都只改发给上游的副本，客户端看到的响应不变。',
  promptFile: 'UTF-8 文本文件路径，留空用内置默认提示词（通用编码助手提示词，不含任何会被上游拦截的模板句）。只在「替换 / 追加」模式下生效，透传模式不读它。',
  promptText: '在这里直接编辑提示词正文：保存后以这份文本为准（优先于上面的提示词文件），清空则回到文件 / 内置默认。改一个字不必再去编辑器里开文件。',
  promptGatewayText: '上游按「结构」校验身份：三段必须各自成块，所以这里分开编辑三段（不能合成一段）。动态段里的运行值写成占位符，发请求时才替换：{cwd} 工作目录、{platform} 平台、{shell} shell、{os_version} 系统版本、{git} 是否 git 仓库、{provider} 提供方标识、{model} 模型名。清空某一段 = 那一段恢复官方原文。第三段发出时前面会自带一个空行（官方形状）。',
  prompt: '改完立即生效，不用重启（下一个请求就用新模式）。降级是自动的临时状态，解除后本模式自己的提示词立刻恢复。',
  promptProviders: '「用哪份提示词」本来是按上游分别决定的事，所以每家可以有自己的模式、提示词文件与正文：不在这里列出的提供商一律沿用上面那份默认设置，「跟随默认」把这一家取消、回到默认。带「网关自带」那一行的家（目前是 ZCode 两家）默认就列在这里 —— 那一段是网关自己装上去的文本，不来自客户端也不来自提示词文件，默认开启、可以关掉，也可以改它的正文（改过的那一段以你的文本为准）；关掉后还能不能通过上游校验，取决于上游当下的口径，行内说明了实测依据。「模式 / 提示词文件 / 正文」管的是客户端自己的 system 怎么处理，与那个开关是两件事。',
  retention: '保存后立即生效：改小保留天数会立刻删除超出的历史数据，且不可恢复。',
  storageFile: '配置目录下的 agent2api.db（账号、日志、请求记录、调试报文与设置都在里面）。',
  storageSize: '库主文件的大小（不含运行期间的 WAL 临时文件，退出时已自动并回）。',
  storage: '数据统一保存在配置目录下的 agent2api.db；如需更换位置，请设置环境变量 AGENT2API_PROXY_HOME 后重启程序。',
  captcha: '开启后，登录页在浏览器后台自动完成验证（对真人无感），而脚本每次尝试都要先算一道题 —— 暴力破解与抢注的成本显著上升。仅影响面板的登录 / 注册，与 API 客户端的 API Key 无关。',
  panelLogin: '当前浏览器以管理员身份登录着本面板。「退出登录」会撤销这台设备的登录会话（30 天内的自动续期一并失效），需要重新输入账号密码；其他已登录的设备不受影响。',
  exportDanger: '导出文件内含 accessToken / refreshToken / apiKey 等凭证与自定义提供商定义，可直接用于登录。请妥善保管，不要外传或上传到公共位置。',
} as const

/** 状态行（`.settings-state`）的派生文案：与旧实现的赋值逐字一致 */
export const STATES = {
  appLoading: '—',
  appUnavailable: '主进程未返回启动设置，请更新桌面端后重试',
  appTrayOn: '关闭窗口时程序不退出，转发继续在后台运行；退出请用托盘菜单',
  appTrayOff: '关闭窗口即退出程序，后台转发随之中断',
  appLightOn: '轻量模式已开启：关窗即销毁界面进程（释放约 200MB 内存），网关在后台继续；界面销毁期间 ZCode 活动套餐通道不可用，相关请求会在请求日志里注明原因',
  appLightOff: '未开启轻量模式：关窗仅隐藏窗口，界面进程常驻内存，随时打开不等待',
  unitsOn: '当前显示为「1.2亿 / 8400万」这类中文量级。',
  unitsOff: '当前显示为「1.20M / 8.4k」这类英文缩写。',
  debugUnavailable: '未能读取调试模式设置，请稍后重试',
  debugOn: '正在保存上游原始报文：',
  debugOff: '未开启，转发时不保存任何原始报文。',
  sanitizeUnavailable: '未能读取指纹脱敏设置，请稍后重试',
  sanitizeOn: '正在剥离出站请求里的审核指纹：表头键值整段删除，模板句最小改写。',
  sanitizeOff: '未开启，客户端 system 模板会原样发往上游，可能被内容审核误拦（400）。',
  corsUnavailable: '未能读取网关跨域访问设置，请稍后重试',
  corsOn: '正在应答跨源请求：浏览器跨域请求可直连 /v1/*。',
  corsOff: '未开启，浏览器跨域无法直连网关。',
  promptUnavailable: '未能读取系统提示词设置，请稍后重试',
  degradeUntilFallback: '次日 00:00',
  // ── 显示分类（本次新增：不来自静态骨架，是新写的文案）──
  themeSystem: '当前跟随操作系统的深浅色设置，系统切换时界面会自动跟上。',
  themeLight: '当前固定为浅色模式，不随系统变化。',
  themeDark: '当前固定为深色模式，不随系统变化。',
  zoomWeb: '网页端的界面缩放由浏览器自己控制（Ctrl + / Ctrl -，或浏览器菜单里的缩放），此项不可调。',
  zoomDefault: '当前按 100% 显示（默认比例）。',
  languageHint: '界面语言只保存在本机（localStorage）。切换后整页立即刷新，不影响账号、密钥与转发。',
} as const

// ── 以下成员从 HEAD 恢复 ──────────────────────────────
// settings-state.ts（上一版拆分前的整体状态层）仍 import 它们；并行改动
// 移走了定义但没改 import，构建因此断在这里。恢复定义而不是动那份 import，
// 等那边的迁移完成后可整体搬走。

export const ROUTE_STRATEGIES = [
  {
    value: 'order',
    label: t('顺序优先'),
    tip: t('排第 1 的接走所有请求，直到它不能答（限流 / 故障 / 额度尽）才轮到下一位 —— 就是「从上往下」的现状行为。'),
  },
  {
    value: 'rotate',
    label: t('负载均衡'),
    tip: t('每条请求轮换到下一个可用账号；新会话交给当前最闲的那位。适合想把手里的号摊开用、不让某一个一直扛。'),
  },
  {
    value: 'provider',
    label: t('家内轮询'),
    tip: t('家之间仍按优先级，只有同一家内部轮流用各账号 —— 想优先用某几家、又不愿把某一家的某个账号焊死时用它。'),
  },
] as const

/** 会话粘性档位：同一会话的后续请求是否尽量跟住同一个账号（会话 = 模型 + 首条用户消息） */

export const ROUTE_STICKY_OPTIONS = [
  {
    value: 'auto',
    label: t('自动'),
    tip: t('同一会话在 5 分钟内尽量跟住同一个账号；停了 5 分钟再发就算新会话。多数场景用它 —— 多轮对话少换号，也不至于把账号焊死。'),
  },
  {
    value: 'session',
    label: t('会话内'),
    tip: t('同一会话在 6 小时内跟住同一个账号；适合一坐几个小时的长会话，上游的上下文缓存能吃到更多。'),
  },
  {
    value: 'turn',
    label: t('每轮'),
    tip: t('每条请求都重新按策略选路 —— 行为与「关闭」一致，仅保留会话识别的记录。'),
  },
  {
    value: 'off',
    label: t('关闭'),
    tip: t('不做会话识别，每条请求都重新按策略选路。'),
  },
] as const

/** 路由档位的取值（SegmentedControl 的泛型参数用它，视图里不必再写字面量联合） */

export const CAPTCHA_POOL_FIELDS: NumberField[] = [
  {
    key: 'zcodeCaptchaPoolTarget',
    id: 'settings-captcha-pool-target',
    label: t('库存目标'),
    unit: t('个'),
    min: 1,
    max: 10,
    hint: t('ZCode 活动套餐通道每条请求都要消耗一个当次铸造的人机验证令牌（一次性，2 分钟过期），库存低于该值时界面自动补铸到它。默认 3 个。调大能吸收突发并发，但用不掉的令牌 2 分钟就过期作废，烧的是上游风控配额。可填 1–10。'),
  },
  {
    key: 'zcodeCaptchaPoolPollMs',
    id: 'settings-captcha-pool-poll-ms',
    label: t('轮询间隔'),
    unit: t('毫秒'),
    min: 1000,
    max: 30000,
    hint: t('铸造守卫每隔多久看一次库存（毫秒）。只影响发现缺货的反应速度 —— 库存够用时轮询不会铸造任何令牌。默认 2500 毫秒，可填 1000–30000。'),
  },
  {
    key: 'zcodeCaptchaPoolMaxPerRound',
    id: 'settings-captcha-pool-max-per-round',
    label: t('单轮铸造上限'),
    unit: t('个'),
    min: 1,
    max: 5,
    hint: t('发现缺货时一轮最多铸几个（连铸之间固定隔 500 毫秒）。默认 1 个：SDK 同一实例连铸容易互相干扰。调大补货更快，但单轮连铸越多越容易触发上游风控。可填 1–5。'),
  },
]

/** 「指定错误码直接换号」：键名、状态码范围与名单上限与后端 retry_api.rs 逐字同源 */

export type ModelsDevState = {
  /** 联网刷新开关（关闭后已装载的数据继续用，见后端 `KEY_MODELS_DEV_ENABLED`） */
  modelsDevEnabled?: boolean
  /** 这份索引的来历：`builtin`（内嵌快照）/ `cache`（上次刷新结果）/ `network`（刚拉的） */
  source?: string
  /** 这份数据的生成时刻（毫秒；0 = 未知） */
  updatedAt?: number
  /** 收录的模型条数 */
  count?: number
}

/** 立即刷新的一行结果（字段与各家目录刷新的结果行同形，见后端 `api::model_meta`）。 */
