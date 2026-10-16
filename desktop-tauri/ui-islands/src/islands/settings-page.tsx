import * as React from 'react'
import { flushSync } from 'react-dom'
import { createRoot } from 'react-dom/client'
import {
  Badge,
  Button,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Input,
  Label,
  SegmentedControl,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Switch,
} from '@ui'
import {
  CATEGORIES,
  CAPTCHA_POOL_FIELDS,
  modelsDevStateText,
  NOTES,
  PROMPT_MODES,
  QUEUE_FIELDS,
  RETENTION_FIELDS,
  RETRY_FIELDS,
  ROUTE_PROTECTION_SWITCHES,
  ROUTE_STICKY_OPTIONS,
  ROUTE_STRATEGIES,
  STATES,
  THEME_EVENT,
  THEME_MODES,
  TIPS,
  TIMEOUT_FIELDS,
  ZOOM_EVENT,
  ZOOM_PERCENTS,
  formatBytes,
  formatCount,
  openExternal,
  readThemeMode,
  readZoomPercent,
  shared,
  type GatewayBlocks,
  type NumberField,
  type RouteStickyValue,
  type RouteStrategyValue,
  type ThemeMode,
} from './settings-model'
import {
  addProviderPrompt,
  addRetryCode,
  applyUnits,
  cancelLanRegister,
  clearDegrade,
  confirmDbRestore,
  dropLastRetryCode,
  exportAccounts,
  exportDatabase,
  importAccounts,
  load,
  panelLogout,
  refreshDebug,
  refreshDeveloperRoleCompat,
  refreshClineHeaders,
  refreshCors,
  refreshCaptchaPool,
  refreshModelsDev,
  refreshPrompt,
  refreshQueue,
  refreshRetention,
  refreshRetry,
  refreshRouting,
  refreshSanitize,
  refreshStorage,
  refreshTimeouts,
  removeProviderPrompt,
  removeRetryCode,
  renderDebug,
  renderCors,
  renderPrompt,
  renderRetention,
  renderRetry,
  renderSanitize,
  renderSettings,
  renderStorage,
  resolveDbBackupConfirm,
  resolveLanConfirm,
  resolveRetentionConfirm,
  restoreCategory,
  saveCaptcha,
  saveClineHeaders,
  saveCors,
  saveCaptchaPoolField,
  saveDebug,
  saveModelsDev,
  savePromptFile,
  savePromptMode,
  saveProviderGatewayPrompt,
  saveProviderPromptFile,
  saveProviderPromptMode,
  saveQueueField,
  saveRetentionField,
  saveRetryField,
  saveRoutingField,
  saveSanitize,
  saveDeveloperRoleCompat,
  saveTimeoutField,
  saveToggle,
  selectCategory,
  showCategory,
  submitLanRegister,
  toggleDbIncludeLogs,
  toggleLan,
  toggleLanPanel,
  useSettings,
  type ClineHeadersState,
  type DebugState,
  type LanConfirm,
  type LanRegister,
  type LoadStatus,
  type NumericState,
  type PromptState,
  type ProviderPromptOption,
  type ProviderPromptState,
  type RoutingState,
  type SettingsSnapshot,
  type StorageState,
} from './settings-state'
import {
  GatewayTextButton,
  ProviderPromptTextButton,
  PromptTextButton,
  gatewayEditedText,
} from './settings-prompt-editor'
// AI 助手分类的面板（对话模型 / 系统提示词 / 工具勾选）。独立成文件、自带一份
// 数据流（/api/assistant/config），见该文件的模块头
import { AssistantSettingsPane } from './assistant-settings-pane'
import { t } from '../i18n'

/**
 * Agent2API · 设置页（React 岛）。
 *
 * 替换 ui/settings-panel.js（那份自持状态、按 id 读写 DOM、用 innerHTML 拼导入失败明细的
 * 老实现）。对外接口与原实现**逐字一致**（见文件末尾）：调用点一行都不用改 ——
 * app.js:135 load()、upgrade-panel.js:74 load()、islands/update-panel.tsx:972 showCategory('about')。
 *
 * ── 三块文件的分工 ──────────────────────────
 * settings-page.tsx（本文件）= 视图 + 挂载 + 对外契约；settings-state.ts = 快照 store 与全部
 * 读写流程；settings-model.ts = 桥类型 / 字段表 / 页面文案。后两个是 .ts，不会被
 * `islands/*.tsx` 的 glob 当岛加载 —— 设置页的岛只有本文件一个。
 *
 * ── ⚠ 「更新」分类（原「关于」）的面板不归本文件 ───────────
 * `.settings-pane[data-cat="about"]` 是**另一个岛**（update-panel.tsx）的挂载点：它在模块
 * 加载期就 `document.querySelector('.settings-pane[data-cat="about"]')`，找到才把 React root
 * 建上去。所以这里必须做到两件事：
 *   ① 原样渲染出一个**空的** `<div class="settings-pane" data-cat="about" />` —— 里面一个子
 *      节点都不能放（会被那个岛清掉），React 也不能在后续渲染里动它的 DOM 子树。React 对
 *      「没有 children 的宿主元素」不会去碰它的子节点，所以只要**始终**渲染这个 div、
 *      不给它 children、不改变它在兄弟中的位置，它的内容就一直是那个岛的；
 *   ② 让它在**模块加载期就同步存在于 DOM 里** —— 见 mount() 的注释（flushSync）。
 * 分类切换（`.active`）走 React 的 className：切到「更新」时这个 div 会拿到 active，
 * 那个岛的面板跟着显示，与旧实现命令式切 class 等价。
 *
 * ── 页面骨架照抄 index.html，只换控件 ────────────
 * 布局类名原样保留（`.settings-layout` / `.settings-nav` / `.settings-nav-item` /
 * `.settings-panes` / `.settings-pane` / `.panel` / `.panel-head` / `.panel-body` /
 * `.head-actions` / `.settings-switches` / `.settings-state` / `.retention-list` /
 * `.retention-row` / `.retention-input` / `.unit` / `.hint` / `.retention-note` /
 * `.prompt-input` / `.tag-input` / `.tag-chip` / `.tag-x` / `.storage-line` /
 * `.storage-path` / `.storage-counts` / `.storage-count` / `.danger-zone` / `.io-result`），
 * 它们是这一页的排版而不是「组件」（样式在 ui/css/page-settings.css）。
 * 控件换组件库：原生 input → Input、原生 checkbox → Switch、原生 select → Select 一族、
 * button → Button、`.badge` → Badge、`#retention-modal` → Dialog。
 * 左栏分类项带**图标**（icons.js 里那组描边风格的设置页图标，17px 图标盒与主侧栏
 * 同款）—— 标签因此不再受「两字」限制：图标负责一眼认出、文字负责说清。
 *
 * 三处细节：
 *   · 问号仍用 `[data-tip]`（10 条说明动辄几百字，tooltip.js 的自动增强仍在页面上跑，
 *     含 MutationObserver 接住 React 动态插入的元素）—— 不换成 Tooltip 是刻意的：换了要
 *     把十条长文各包一层组件，观感与行为却完全一样；
 *   · 数字框的宽度 / 居中 / 等宽数字写成工具类：组件库的 Input 自带 `w-full`（工具类带
 *     !important），page-settings.css 里 `.retention-input input[type="number"]` 的 92px
 *     只保得住 max-width，宽度得自己带（与任务面板的间隔框同一处理）；
 *   · 显隐一律**条件渲染**，不写 hidden / style.display —— 组件库的 Tailwind 工具类分层且
 *     带 !important，会压掉 tokens.css 里未分层的 `[hidden]{display:none!important}`。
 */

/* ─── 小组件 ───────────────────────────────── */

/**
 * 图标（icons.js 的内联 SVG 串）：整站共用一份图标集，这里只做注入。
 * 图标在 set（左栏分类）里都已存在于 icons.js；取不到时返回空串（图标位留空，
 * 不影响文字与点击 —— 按名字取不到是开发期错误，不该把页面弄崩）。
 */
function iconHtml(name: string, size: number): string {
  return shared().wbIcons?.icon?.(name, size) || ''
}

/** 三态徽章：`.badge`（检测中…）/ `.badge.ok` / `.badge.bad` → Badge 的 variant */
function StatusBadge({ tone, children }: { tone: 'idle' | 'ok' | 'bad'; children: React.ReactNode }) {
  const variant = tone === 'ok' ? 'success' : tone === 'bad' ? 'destructive' : 'outline'
  return <Badge variant={variant}>{children}</Badge>
}

type PanelHeadProps = {
  title: string
  /** 标题右侧问号的说明全文（沿用 `[data-tip]`） */
  tip?: string
  badge?: React.ReactNode
  actions?: React.ReactNode
}

/** 面板标题栏：标题 + 问号 + 徽章 + 右侧操作（DOM 顺序与静态骨架一致） */
function PanelHead({ title, tip, badge, actions }: PanelHeadProps) {
  return (
    <div className='panel-head'>
      <h2>{title}</h2>
      {tip ? <span className='tip-q' data-tip={tip}></span> : null}
      {badge}
      {actions ? <div className='head-actions'>{actions}</div> : null}
    </div>
  )
}

/**
 * 可折叠面板：标题栏就是 `<details>` 的 summary，正文平时收起 —— 给「行数多、
 * 但多数时候不用动」的面板用（目前是 Cline 伪装头）。开合状态不持久化，跟
 * 用户上次的选择，不需要额外持久化。
 */
function CollapsiblePanel({ title, tip, badge, actions, children }: PanelHeadProps & { children: React.ReactNode }) {
  // `summary` 里的一切点击都会切换开合（原生行为），而标题栏里有两类**独立
  // 动作**的控件：问号提示（点开 tooltip）与右侧操作按钮（如「刷新」）。
  // 点它们不该顺手把面板折起来 —— 尤其是「展开 → 点问号看说明」这个动作，
  // 正文会在眼前消失。
  //
  // 用 `preventDefault` 而不是 `stopPropagation`：开合是 click 的**默认动作**
  // （由 `<details>` 在事件传播结束后执行），`stopPropagation` 拦不住它。
  function keepOpen(event: React.MouseEvent<HTMLElement>): void {
    const target = event.target as HTMLElement
    if (target.closest('.tip-q, .head-actions')) event.preventDefault()
  }
  return (
    <details className='panel panel-fold'>
      <summary className='panel-head' onClick={keepOpen}>
        <span className='fold-caret' aria-hidden dangerouslySetInnerHTML={{ __html: iconHtml('caretRight', 14) }} />
        <h2>{title}</h2>
        {tip ? <span className='tip-q' data-tip={tip}></span> : null}
        {badge}
        {actions ? <div className='head-actions'>{actions}</div> : null}
      </summary>
      {children}
    </details>
  )
}

type SwitchRowProps = {
  id: string
  label: string
  checked: boolean
  disabled?: boolean
  /** 行内问号提示（与 PanelHead 同款 `[data-tip]`，tooltip.js 统一服务） */
  tip?: string
  onCheckedChange: (next: boolean) => void
}

/**
 * 一行开关。开关与文字同在一个 `<label class="switch">` 里（与旧模板同构）：Base UI 的
 * Switch 会渲染一个隐藏 checkbox，label 的原生激活行为照样把点击转给它 —— 点文字也能拨动。
 */
function SwitchRow({ id, label, checked, disabled, tip, onCheckedChange }: SwitchRowProps) {
  return (
    <label className='switch'>
      <Switch id={id} checked={checked} disabled={disabled} onCheckedChange={next => onCheckedChange(next)} />
      <span>{label}</span>
      {tip ? <span className='tip-q' data-tip={tip}></span> : null}
    </label>
  )
}

type NumberRowProps = {
  field: NumberField
  /** 生效值；null = 没读到（输入框保持禁用，旧实现同） */
  value: number | null
  /** 面板忙碌（提交在途）：本次操作期间禁用，与旧实现置 DOM disabled 等价 */
  disabled: boolean
  onCommit: (raw: string) => Promise<void>
}

/**
 * 一行数字设置（保留期 / 重试 / 超时共用）。
 *
 * 未提交的编辑是**本组件的草稿**：聚焦时把生效值抄进草稿，失焦 / 回车提交，提交结束
 * （无论成败）把草稿收掉、显示回到生效值 —— 这就是旧实现那套「正在编辑的那一项不回填 +
 * 失败回滚 + 保存后按后端值规范化」的等价物，只是状态只存在于这一处。
 */
function NumberRow({ field, value, disabled, onCommit }: NumberRowProps) {
  const [draft, setDraft] = React.useState<string | null>(null)
  const text = draft !== null ? draft : value === null ? '' : String(value)

  async function commit(): Promise<void> {
    const raw = draft
    if (raw === null) return
    try { await onCommit(raw) } finally { setDraft(null) }
  }

  return (
    <div className='retention-row'>
      <label htmlFor={field.id}>{field.label}</label>
      <span className='retention-input'>
        <Input
          id={field.id}
          type='number'
          min={field.min}
          max={field.max}
          step={1}
          inputMode='numeric'
          className='w-[92px] max-w-[92px] text-center tabular-nums'
          value={text}
          disabled={disabled || value === null}
          onChange={event => setDraft(event.target.value)}
          onFocus={() => setDraft(String(value ?? ''))}
          onBlur={() => void commit()}
          // 回车等价于「失焦提交」：不同内核里 Enter 是否派发 change 并不一致，
          // 这里主动 blur 一次把它统一成「值已提交」这一条路径
          onKeyDown={event => { if (event.key === 'Enter') event.currentTarget.blur() }}
        />
        <span className='unit'>{field.unit}</span>
      </span>
      <div className='hint'>{field.hint}</div>
    </div>
  )
}

/** 数字面板的徽章：首屏「检测中…」、读到「已生效」、读不到「不可用」 */
function numericBadge(state: NumericState): React.ReactNode {
  if (state.status === 'ready') return <StatusBadge tone='ok'>{t('已生效')}</StatusBadge>
  if (state.status === 'unavailable') return <StatusBadge tone='bad'>{t('不可用')}</StatusBadge>
  return <StatusBadge tone='idle'>{t('检测中…')}</StatusBadge>
}

/** 刷新按钮（无修饰的 button → Button 的 outline 档，与静态骨架的观感一致） */
function RefreshButton({ id, onClick }: { id: string; onClick: () => void }) {
  return <Button id={id} variant='outline' onClick={onClick}>{t('刷新')}</Button>
}

/* ─── 通用分类 ─────────────────────────────── */

/** 「局域网访问」面板的状态行：开着时给出局域网设备该填的 API 地址 */
function lanStateText(snap: SettingsSnapshot): string {
  const { lanAccess, lanPanel, lanIp, port } = snap.app
  if (!lanAccess) return t('未开启：网关只监听 127.0.0.1，仅本机可以访问。')
  const base = `http://${lanIp || t('<本机IP>')}${port ? `:${port}` : ''}`
  const panel = lanPanel ? t('；网页管理面板已一并开放（浏览器打开同一地址）') : ''
  return t('已开启：其他设备把 API 地址指向 {url}{panel}。', { url: `${base}/v1`, panel })
}

function GeneralPane({ snap }: { snap: SettingsSnapshot }) {
  const app = snap.app
  // 轻量模式只挂在「关闭到托盘」上（关闭即退出时没有窗口可轻量），
  // 状态行按 未开启托盘 → 开启托盘 → 轻量模式 三级派生
  const appState = app.status === 'unavailable'
    ? STATES.appUnavailable
    : app.status === 'loading'
      ? STATES.appLoading
      : !app.closeToTray
        ? STATES.appTrayOff
        : app.lightweightMode ? STATES.appLightOn : STATES.appTrayOn

  return (
    <>
      <section className='panel'>
        <PanelHead
          title={t('启动与托盘')}
          tip={TIPS.tray}
          badge={app.status === 'ready'
            ? <StatusBadge tone='ok'>{t('已应用')}</StatusBadge>
            : app.status === 'unavailable'
              ? <StatusBadge tone='bad'>{t('不可用')}</StatusBadge>
              : <StatusBadge tone='idle'>{t('检测中…')}</StatusBadge>}
        />
        <div className='panel-body'>
          <div className='settings-switches'>
            {/* 主进程没返回启动设置时开关仍可拨（照旧实现）：拨了直接发全量 patch，
                保存成功即回到「已应用」，不必让用户重开程序 */}
            <SwitchRow
              id='settings-close-to-tray'
              label={t('关闭窗口时最小化到托盘')}
              checked={app.closeToTray}
              disabled={snap.busy === 'app'}
              onCheckedChange={next => void saveToggle('tray', next)}
            />
            {/* 轻量模式依赖「关闭到托盘」：关掉托盘时该开关无意义，置灰并提示 */}
            <SwitchRow
              id='settings-lightweight'
              label={t('轻量模式（关窗释放界面内存）')}
              tip={TIPS.lightweight}
              checked={app.lightweightMode}
              disabled={snap.busy === 'app' || !app.closeToTray}
              onCheckedChange={next => void saveToggle('lightweight', next)}
            />
            <SwitchRow
              id='settings-autostart'
              label={t('开机自动启动')}
              checked={app.autostart}
              disabled={snap.busy === 'app'}
              onCheckedChange={next => void saveToggle('autostart', next)}
            />
          </div>
          <div className='settings-state'>{appState}</div>
        </div>
      </section>

      {!snap.panelLogin && (
        <section className='panel'>
          <PanelHead title={t('局域网访问')} tip={TIPS.lan} />
          <div className='panel-body'>
            <div className='settings-switches'>
              {/* 开 / 关都不直接落盘：走确认框（→ 需要时注册管理员 → 写设置并重启），
                  流程与文案在 settings-state 的 toggleLan / resolveLanConfirm */}
              <SwitchRow
                id='settings-lan-access'
                label={t('允许局域网内的设备访问网关')}
                checked={snap.app.lanAccess}
                disabled={snap.busy === 'lan'}
                onCheckedChange={toggleLan}
              />
              <SwitchRow
                id='settings-lan-panel'
                label={t('同时开放网页管理面板')}
                checked={snap.app.lanPanel}
                disabled={snap.busy === 'lan' || !snap.app.lanAccess}
                onCheckedChange={toggleLanPanel}
              />
            </div>
            <div className='settings-state'>{lanStateText(snap)}</div>
          </div>
        </section>
      )}

      <section className='panel'>
        <PanelHead title={t('计量单位')} tip={TIPS.units} />
        <div className='panel-body'>
          <div className='settings-switches'>
            <SwitchRow
              id='settings-chinese-units'
              label={t('使用中文单位（亿 / 万）')}
              checked={snap.unitsChinese}
              onCheckedChange={applyUnits}
            />
          </div>
          <div className='settings-state'>
            {snap.unitsChinese ? STATES.unitsOn : STATES.unitsOff}
          </div>
        </div>
      </section>
    </>
  )
}

/* ─── 显示分类 ─────────────────────────────── */

/** 显示模式三档的状态行文案（三条文案在 settings-model 的 STATES 里） */
function themeStateText(mode: ThemeMode): string {
  if (mode === 'light') return STATES.themeLight
  if (mode === 'dark') return STATES.themeDark
  return STATES.themeSystem
}

/**
 * 显示分类：显示模式 / 界面缩放 / 语言。
 *
 * 三项都是**纯前端偏好** —— 与后端配置无关，所以不参与 settings-state 的 load
 * （那边一次并行取全部后端设置），这里自己持两个受控值就够了。
 *
 * 主题与缩放的**唯一应用入口都在 app.js**（见 settings-model 的显示偏好一节）：
 * 这里改完只调 wbApp.applyTheme / applyZoom，再靠 'wb:theme' / 'wb:zoom' 事件
 * 跟随 —— 侧边栏底部的主题三键是同一个设置的另一个入口，两边必须互相同步；
 * 谁也不去读对方的状态，只认事件里带的新值。
 *
 * 网页端：主题照常可用（data-theme 是纯 CSS 生效，窗口主题在 shim 里是空实现）；
 * 界面缩放禁用 —— 浏览器里的缩放归浏览器自己的 Ctrl +/- 管。
 */
function DisplayPane() {
  const [theme, setTheme] = React.useState<ThemeMode>(readThemeMode)
  const [zoom, setZoom] = React.useState<number>(readZoomPercent)
  // 端别取自桥（platform）而不是 navigator：与设置页其它端别判断同一口径
  const isWeb = shared().workbuddyDesktop?.platform === 'web'

  // 跟随别人的改动（侧边栏三键、index.html 头部内联脚本的抢先应用、或本页自身）：
  // 事件里带着新值，直接采纳，不必回头读 localStorage
  React.useEffect(() => {
    const onTheme = (event: Event) => setTheme(String((event as CustomEvent).detail || 'system') as ThemeMode)
    const onZoom = (event: Event) => setZoom(Number((event as CustomEvent).detail) || 100)
    window.addEventListener(THEME_EVENT, onTheme)
    window.addEventListener(ZOOM_EVENT, onZoom)
    return () => {
      window.removeEventListener(THEME_EVENT, onTheme)
      window.removeEventListener(ZOOM_EVENT, onZoom)
    }
  }, [])

  const zoomLabel = zoom === 100 ? t('100%（默认）') : `${zoom}%`

  return (
    <>
      <section className='panel'>
        <PanelHead title={t('显示模式')} tip={TIPS.displayTheme} />
        <div className='panel-body'>
          <div>
            <SegmentedControl<ThemeMode>
              aria-label={t('显示模式')}
              options={THEME_MODES}
              value={theme}
              onValueChange={next => shared().wbApp?.applyTheme?.(next)}
            />
          </div>
          <div className='settings-state'>{themeStateText(theme)}</div>
        </div>
      </section>

      <section className='panel'>
        <PanelHead title={t('界面缩放')} tip={TIPS.displayZoom} />
        <div className='panel-body'>
          <div className='retention-list'>
            <div className='retention-row'>
              <label htmlFor='settings-zoom'>{t('缩放比例')}</label>
              <span className='prompt-input'>
                {/* 档位是 80%–130% 的 11 个定值，用下拉而不是滑块：每一档都要能精确
                    复述（用户问「我现在多少」时答案是个整数），且组件库目前没有 Slider。
                    展示文案显式给 SelectValue，不依赖 value 自动显示。 */}
                <Select
                  value={String(zoom)}
                  onValueChange={next => shared().wbApp?.applyZoom?.(Number(next))}
                >
                  <SelectTrigger
                    id='settings-zoom'
                    className='w-[140px]'
                    disabled={isWeb}
                    aria-label={t('界面缩放比例')}
                  >
                    <SelectValue>{zoomLabel}</SelectValue>
                  </SelectTrigger>
                  <SelectContent>
                    {ZOOM_PERCENTS.map(percent => (
                      <SelectItem key={percent} value={String(percent)}>
                        {percent === 100 ? t('100%（默认）') : `${percent}%`}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </span>
              <div className='hint'>
                {t('放大或缩小整个界面（文字与控件一起变），效果与浏览器 Ctrl +/- 相同：共 11 档，立即生效并记住。')}
              </div>
            </div>
          </div>
          <div className='settings-state'>
            {isWeb ? STATES.zoomWeb : zoom === 100 ? STATES.zoomDefault : t('当前按 {n}% 显示。', { n: zoom })}
          </div>
        </div>
      </section>

      <section className='panel'>
        <PanelHead title={t('语言设置')} tip={TIPS.displayLanguage} />
        <div className='panel-body'>
          <LanguageRow />
          <div className='settings-state'>{STATES.languageHint}</div>
        </div>
      </section>
    </>
  )
}

/**
 * 「界面语言」一行：下拉选 auto 或某个具体语言，选中即整页刷新。
 *
 * ── 为什么不进 settings-state 的后端模型 ──────────────────────
 * 语言只改本机界面文案：不碰转发、不碰账号，跟「主题 / 缩放 / 计量单位」同属纯前端
 * 偏好。所以它只读写 localStorage（键 workbuddy-desktop-locale，见 ui/i18n.js），
 * 与后端 desktop-settings.json 无关，也正因如此这里不需要受控 state —— 切换会
 * location.reload()，重载后本行自然按新值重新渲染。
 *
 * ── 选项文案为什么不都走 t() ─────────────────────────────────
 * 'auto' 的标签是中文键（跟随系统），要翻译；其余语言的 label 是**各语言自己的写法**
 * （English / 日本語 / 한국어 …），选之前就该看得懂，所以照搬 wbI18n.LOCALES，不翻译。
 */
function LanguageRow() {
  const api = window.wbI18n
  const value = api?.rawLocale?.() ?? 'auto'
  const locales = api?.LOCALES ?? []
  const labelOf = (code: string): string => (
    code === 'auto' ? t('跟随系统') : (locales.find(item => item.code === code)?.label ?? code)
  )

  return (
    <div className='retention-list'>
      <div className='retention-row'>
        <label htmlFor='settings-language'>{t('界面语言')}</label>
        <span className='prompt-input'>
          {/* 与「界面缩放」同一个 Select 形态：展示文案显式给 SelectValue，不依赖 value 自动显示。
              组件的 onValueChange 可能给 null（被清空），这里只在拿到非空值时切换 */}
          <Select value={value} onValueChange={next => { if (next) api?.setLocale(next) }}>
            <SelectTrigger id='settings-language' className='w-[200px]' aria-label={t('界面语言')}>
              <SelectValue>{labelOf(value)}</SelectValue>
            </SelectTrigger>
            <SelectContent>
              <SelectItem value='auto'>{t('跟随系统')}</SelectItem>
              {locales.map(item => (
                <SelectItem key={item.code} value={item.code}>{item.label}</SelectItem>
              ))}
            </SelectContent>
          </Select>
        </span>
      </div>
    </div>
  )
}

/* ─── 网关分类 ─────────────────────────────── */

/** 「指定错误码直接换号」的标签输入（GitHub Topics 同款：徽章 + 行内输入框） */
function RetryCodesField({ codes, status, busy }: {
  codes: number[] | null
  status: LoadStatus
  busy: boolean
}) {
  const [text, setText] = React.useState('')
  const inputRef = React.useRef<HTMLInputElement | null>(null)
  // 读不到重试设置时整框禁用（旧实现：field.disabled + .disabled 类 + 占位符换「—」）
  const locked = status === 'unavailable'

  return (
    <div className='retention-row'>
      <label htmlFor='settings-retry-no-codes-input'>{t('指定错误码直接换号')}</label>
      <div className='retention-input'>
        {/* 点框体空白处 = 聚焦输入框（整框是一个输入控件的观感，旧实现同） */}
        <div
          className={locked ? 'tag-input disabled' : 'tag-input'}
          onClick={() => inputRef.current?.focus()}
        >
          {(codes || []).map(code => (
            <span className='tag-chip' key={code}>
              <span className='v'>{code}</span>
              {/* 徽章上的 ✕：小号圆角热区，悬停加深 —— 用组件库的小件档 + 语义色，
                  「能点删」的视觉暗示与旧实现一致 */}
              <Button
                type='button'
                variant='ghost'
                size='icon-2xs'
                className='tag-x text-muted-foreground hover:bg-destructive-soft hover:text-destructive'
                title={t('删除 {code}', { code })}
                aria-label={t('删除状态码 {code}', { code })}
                onClick={event => { event.stopPropagation(); void removeRetryCode(code) }}
              >
                ✕
              </Button>
            </span>
          ))}
          <Input
            ref={inputRef}
            id='settings-retry-no-codes-input'
            type='text'
            // 框的描边由外层 .tag-input 出，这里把组件库 Input 的边框 / 底色 / 内边距
            // 用工具类压平（.tag-input-field 的老规则是非分层的，压不过工具类）
            className='tag-input-field h-auto border-0 bg-transparent px-0 shadow-none focus:shadow-none'
            placeholder={locked ? '—' : t('输入状态码，回车添加')}
            inputMode='numeric'
            autoComplete='off'
            value={text}
            disabled={locked || busy}
            onChange={event => setText(event.target.value)}
            onKeyDown={event => {
              if (event.key === 'Enter') {
                event.preventDefault()
                void addRetryCode(text)
                setText('') // 提交后立即清空输入框（旧实现同）
                return
              }
              // 与 GitHub Topics 一致：输入框为空时退格删掉最后一枚
              if (event.key === 'Backspace' && !text) void dropLastRetryCode()
            }}
          />
        </div>
      </div>
      <div className='hint'>{NOTES.retryCodes}</div>
    </div>
  )
}

/** 系统提示词：文件路径输入框（草稿机制与数字框同款，失焦 / 回车才提交） */
function PromptFileRow({ prompt, locked, busy }: {
  prompt: PromptState
  locked: boolean
  busy: boolean
}) {
  const [draft, setDraft] = React.useState<string | null>(null)

  async function commit(): Promise<void> {
    const raw = draft
    if (raw === null) return
    try { await savePromptFile(raw) } finally { setDraft(null) }
  }

  return (
    <div className='retention-row'>
      <label htmlFor='settings-prompt-file'>{t('提示词文件')}</label>
      <span className='prompt-input'>
        <Input
          id='settings-prompt-file'
          type='text'
          placeholder={t('留空 = 用内置默认提示词')}
          value={draft !== null ? draft : prompt.file}
          disabled={locked || busy}
          onChange={event => setDraft(event.target.value)}
          onFocus={() => setDraft(prompt.file)}
          onBlur={() => void commit()}
          onKeyDown={event => { if (event.key === 'Enter') event.currentTarget.blur() }}
        />
      </span>
      <div className='hint'>{NOTES.promptFile}</div>
    </div>
  )
}

/** 提示词状态行：模式说明 + 来源与行数 + 文件读取告警 */
function promptStateText(prompt: PromptState): string {
  if (prompt.status === 'loading') return STATES.appLoading
  if (prompt.status === 'unavailable') return STATES.promptUnavailable
  const source = promptSourceText(prompt.source)
  const parts: string[] = []
  if (prompt.mode === 'passthrough') {
    parts.push(t('客户端 system 原样出站（只靠指纹脱敏改写模板句）。'))
  } else {
    parts.push(
      t('{mode}生效：上游收到的 system 来自{source}{lines}。', {
        mode: prompt.mode === 'custom' ? t('替换') : t('追加'),
        source,
        lines: prompt.lines ? t('（{n} 行）', { n: prompt.lines }) : '',
      }),
    )
  }
  if (prompt.fileError) parts.push(`⚠️ ${prompt.fileError}`)
  return parts.join('')
}

/** 正文来源的中文说法（三处状态行共用；'' = 没有正文可讲） */
function promptSourceText(source: string): string {
  if (source === 'inline') return t('界面里编辑的正文')
  if (source === 'file') return t('提示词文件')
  if (source === 'builtin') return t('内置默认提示词')
  return ''
}

/** 降级行的说明：只在真的处于降级期时出现（平时它是一行与用户无关的状态噪音） */
function degradeHint(prompt: PromptState): string {
  return t('已自动切换到最小中性提示词（撞了上游内容拦截，多半是 system 指纹误报），到 {until} 自动解除。', {
    until: prompt.degradeUntilText || STATES.degradeUntilFallback,
  })
    + t('期间本模式自己的提示词不会发出；把提示词改好后可以立即解除。')
}

/**
 * 提示词文件输入框（草稿机制与上面那个全局的同款，按 id 提交到对应那一家）
 */
function ProviderFileRow({ item, label, locked, busy }: {
  item: ProviderPromptState
  label: string
  locked: boolean
  busy: boolean
}) {
  const [draft, setDraft] = React.useState<string | null>(null)

  async function commit(): Promise<void> {
    const raw = draft
    if (raw === null) return
    try {
      // 只在与「显示值」真的不同时才提交：这一行可能是「跟随全局」的（显示的是
      // 全局那两份），点进去再点出来不该凭空生成一条覆盖
      if (raw !== item.file) await saveProviderPromptFile(item, raw)
    } finally { setDraft(null) }
  }

  return (
    <span className='prompt-input'>
      <Input
        id={`settings-prompt-file-${item.id}`}
        type='text'
        placeholder={t('留空 = 用内置默认提示词')}
        aria-label={t('{label} 提示词文件', { label })}
        value={draft !== null ? draft : item.file}
        disabled={locked || busy}
        onChange={event => setDraft(event.target.value)}
        onFocus={() => setDraft(item.file)}
        onBlur={() => void commit()}
        onKeyDown={event => { if (event.key === 'Enter') event.currentTarget.blur() }}
      />
    </span>
  )
}

/** 一家的状态行：模式说明 + 来源与行数 + 文件读取告警（与全局那行同一套措辞） */
function providerStateText(item: ProviderPromptState, configured: boolean): string {
  // 没单独配过的家（只在「有网关自带提示词」时才会被列出来）走全局那份 ——
  // 不说这一句的话，用户会以为这一行显示的就是「这家的设置」
  if (!configured) return t('未单独配置：模式、提示词文件与正文都跟随上面的全局配置（当前「{mode}」）。', {
    mode: PROMPT_MODES.find(option => option.value === item.mode)?.toastLabel ?? item.mode,
  })
  if (item.mode === 'passthrough') return t('客户端 system 原样出站。')
  const head = t('{mode}生效：上游收到的 system 来自{source}{lines}。', {
    mode: item.mode === 'custom' ? t('替换') : t('追加'),
    source: promptSourceText(item.source),
    lines: item.lines ? t('（{n} 行）', { n: item.lines }) : '',
  })
  return item.fileError ? `${head} ⚠️ ${item.fileError}` : head
}

/**
 * 一家的**网关自带提示词**那一行（只有 registry 里标了 `gatewayNote` 的家才有）。
 *
 * 这段文本不来自客户端、也不来自提示词文件，是网关自己装上去的（ZCode 活动套餐
 * 通道的官方三段身份块）。它默认开、可以关，**也可以改正文** —— 关掉后还能不能跑
 * 取决于上游当前的校验口径，所以关掉时把这一行**换成警示语气**（`tone` 为非 ok
 * 的徽章 + 粗体），而不是留一个看起来无害的灰开关。
 */
function ProviderGatewayRow({ item, option, over, locked, busy }: {
  item: ProviderPromptState
  option: ProviderPromptOption
  /** 这一家已存的正文覆盖（只含改过的段；缺省 = 全是官方原文） */
  over: GatewayBlocks | undefined
  locked: boolean
  busy: boolean
}) {
  const on = item.gateway
  const chars = option.gatewayChars
  const size = chars ? t('约 {n} 字符', { n: chars.toLocaleString('zh-CN') }) : t('一段内置装配')
  const edited = gatewayEditedText(over)
  return (
    <div className='retention-row'>
      <label className='prompt-gateway-label'>{t('网关自带')}</label>
      <span className='prompt-input'>
        <SwitchRow
          id={`settings-prompt-gateway-${item.id}`}
          label={t('装上{size}的官方身份提示词', { size })}
          checked={on}
          disabled={locked || busy}
          onCheckedChange={next => void saveProviderGatewayPrompt(item.id, next)}
        />
        {/* 正文编辑与开关并排：两件事（装不装 / 长什么样）在同一个可视范围里，
            但各走各的接口字段，改一个不会动另一个（见 settings-state 两个动作） */}
        <GatewayTextButton item={item} option={option} over={over} locked={locked} busy={busy} />
      </span>
      <div className={on ? 'hint' : 'hint prompt-gateway-off'}>
        {on ? option.gatewayNote : t('已关闭。{note}', { note: option.gatewayNote })}
        {edited ? t(' 正文已改：{edited}（其余段用官方原文）。', { edited }) : ''}
      </div>
    </div>
  )
}

/**
 * 「按提供商配置」区块：列**两类**家 ——
 *
 *   1. 注册表里带 `gatewayNote` 的家（有网关自带提示词可拨开关）——**默认就列**，
 *      不需要先去「添加提供商…」：一个开关藏在添加动作后面等于没有；
 *   2. 用户单独配过模式 / 文件的家（`promptProviders` 里有的）。
 *
 * 第 1 类即使没有 `promptProviders` 项也要出现在列表里，所以这里合成一行
 * （模式 / 文件取全局默认值：`prompt` 自己那两份）。用户一动这个模式 / 文件，
 * 后端就会为它落一条显式覆盖 —— 这正是「从默认值开始配」的自然路径。
 */
function ProviderPromptRows({ prompt, locked, busy }: {
  prompt: PromptState
  locked: boolean
  busy: boolean
}) {
  const options = prompt.options
  const configured = new Map(prompt.providers.map(item => [item.id, item]))
  // 顺序：先按注册表顺序列出带开关的家，再补齐用户配过、但注册表里没标开关的家
  // （自定义提供商不会出现在 options 里，它们只能靠 promptProviders 出现）
  const rows: ProviderPromptState[] = []
  for (const option of options) {
    if (!option.gatewayNote) continue
    rows.push(configured.get(option.id) ?? {
      id: option.id,
      // 没单独配过的行显示**生效值**（= 全局那份）：界面上一眼看不出「这行是不是
      // 自己配过」，但显示的值必须是真的，否则「跟随默认」这句话就是空话
      mode: prompt.mode,
      // 显示**生效值**（= 全局那份）：这一行的模式 / 文件 / 正文都跟随全局，
      // 输入框与编辑器里给出真实的那份；改动任何一个控件时会把当前显示的这几个值
      // 一起落成这一家自己的覆盖（见 saveProviderPromptMode 的说明），
      // 所以「只改模式」不会把全局的提示词文件弄丢
      file: prompt.file,
      text: prompt.text,
      source: prompt.source,
      lines: prompt.lines,
      fileError: prompt.fileError,
      gateway: prompt.gateway[option.id] ?? true,
      configured: false,
    })
  }
  for (const item of prompt.providers) {
    if (rows.some(row => row.id === item.id)) continue
    rows.push({ ...item, gateway: prompt.gateway[item.id] ?? true, configured: true })
  }
  const listed = new Set(rows.map(row => row.id))
  const candidates = options.filter(item => !item.gatewayNote && !listed.has(item.id))
  const [pending, setPending] = React.useState('')
  const labelOf = (id: string) => options.find(item => item.id === id)?.label || id
  const optionOf = (id: string) => options.find(item => item.id === id)
  const noteOf = (id: string) => optionOf(id)?.gatewayNote || ''

  return (
    <>
      {rows.map(item => {
        const label = labelOf(item.id)
        const modeLabel = PROMPT_MODES.find(option => option.value === item.mode)?.optionLabel ?? item.mode
        const note = noteOf(item.id)
        const option = optionOf(item.id)
        return (
          <React.Fragment key={item.id}>
            <div className='retention-row'>
              <label htmlFor={`settings-prompt-mode-${item.id}`}>{label}</label>
              <span className='prompt-input'>
                <Select
                  value={item.mode}
                  onValueChange={next => {
                    // 同上：null = 没选（清空 / 取消），不是一个叫 "null" 的模式。
                    // 模式与文件一起提交：这一行可能还在「跟随全局」，只写模式会让
                    // 它的文件从屏幕上的路径变成内置默认（见 saveProviderPromptMode）
                    if (next != null && String(next)) {
                      void saveProviderPromptMode(item, String(next))
                    }
                  }}
                >
                  <SelectTrigger
                    id={`settings-prompt-mode-${item.id}`}
                    className='w-[240px]'
                    disabled={locked || busy}
                    aria-label={t('{label} 的提示词模式', { label })}
                  >
                    <SelectValue>{modeLabel}</SelectValue>
                  </SelectTrigger>
                  <SelectContent>
                    {PROMPT_MODES.map(option => (
                      <SelectItem key={option.value} value={option.value}>{option.optionLabel}</SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <ProviderFileRow item={item} label={label} locked={locked} busy={busy} />
                <ProviderPromptTextButton item={item} label={label} locked={locked} busy={busy} />
                {/* 「跟随默认」只在**单独配过**的行上出现：没配过的行点了它也删不掉
                    任何东西（后端此时没有这一项），留一颗无效按钮只会让人以为点坏了 */}
                {item.configured ? (
                  <Button
                    variant='outline'
                    disabled={locked || busy}
                    onClick={() => void removeProviderPrompt(item.id)}
                  >
                    {t('跟随默认')}
                  </Button>
                ) : null}
              </span>
              <div className='hint'>{providerStateText(item, item.configured)}</div>
            </div>
            {/* 网关自带那段（只有 registry 标了 note 的家有）——单独一行开关，
                与上面那行不是一回事：那个管客户端 system、这个管网关自己装什么 */}
            {option && note ? (
              <ProviderGatewayRow
                item={item}
                option={option}
                over={prompt.gatewayText[item.id]}
                locked={locked}
                busy={busy}
              />
            ) : null}
          </React.Fragment>
        )
      })}

      <div className='retention-row'>
        <label htmlFor='settings-prompt-provider-add'>{t('添加提供商')}</label>
        <span className='prompt-input'>
          {/* 组件的 Select 不支持占位（必须有一个 value），所以第一个选项就是
              「选择要配置的提供商」这个动作本身；选完立刻重置回它。
              `next` 可能是 null（Base UI 在「清空 / 取消选择」时回调的就是它，
              见 @base-ui 的 `onValueChange` 类型）：照 `String(next)` 走会把它
              变成字符串 "null"，而后端会如实回一句「未知的提供商 id：null」——
              一次「没选」不该变成一次失败的保存。 */}
          <Select
            value={pending}
            onValueChange={next => {
              setPending('')
              const id = next == null ? '' : String(next)
              if (id) void addProviderPrompt(id)
            }}
          >
            <SelectTrigger
              id='settings-prompt-provider-add'
              className='w-[240px]'
              disabled={locked || busy || candidates.length === 0}
              aria-label={t('添加要单独配置的提供商')}
            >
              <SelectValue>
                {pending ? labelOf(pending) : (candidates.length ? t('添加提供商…') : t('全部已配置'))}
              </SelectValue>
            </SelectTrigger>
            <SelectContent>
              {candidates.map(item => (
                <SelectItem key={item.id} value={item.id}>{item.label}</SelectItem>
              ))}
            </SelectContent>
          </Select>
        </span>
        <div className='hint'>{NOTES.promptProviders}</div>
      </div>
    </>
  )
}

function PromptPanel({ snap }: { snap: SettingsSnapshot }) {
  const prompt = snap.prompt
  const locked = prompt.status !== 'ready'
  const busy = snap.busy === 'prompt'
  const modeLabel = PROMPT_MODES.find(item => item.value === prompt.mode)?.optionLabel ?? prompt.mode

  return (
    <section className='panel'>
      <PanelHead
        title={t('系统提示词')}
        tip={TIPS.prompt}
        badge={prompt.status === 'ready'
          ? <StatusBadge tone='ok'>{prompt.mode === 'passthrough' ? t('透传') : t('已接管')}</StatusBadge>
          : prompt.status === 'unavailable'
            ? <StatusBadge tone='bad'>{t('不可用')}</StatusBadge>
            : <StatusBadge tone='idle'>{t('检测中…')}</StatusBadge>}
        actions={<RefreshButton id='btn-prompt-refresh' onClick={() => void refreshPrompt()} />}
      />
      <div className='panel-body'>
        <div className='retention-list'>
          {/* 两个分组条把「默认」与「某家的例外」分开：上面那几行是所有未单独配置的
              家共用的默认值，下面那张表是逐家的例外 —— 不分开时它们是一串同构的行，
              用户读不出哪几行管全部、哪几行只管一家（见 page-settings.css 的 .prompt-group）。 */}
          <div className='prompt-group'>
            {t('全局配置')}<span className='note'>{t('所有未单独配置的提供商都用这一份')}</span>
          </div>

          <div className='retention-row'>
            <label htmlFor='settings-prompt-mode'>{t('模式')}</label>
            <span className='prompt-input'>
              {/* 旧实现是原生 <select>（select.js 增强 + wbSelect.sync）；这里换成组件库的
                  Select：触发器是按钮，page-settings.css 的 `.prompt-input select{width:240px}`
                  不再命中，宽度得用工具类补回（否则触发器按内容宽度缩成一团）。
                  展示文案显式给 SelectValue，不依赖 value 自动显示。 */}
              <Select
                value={prompt.mode}
                // 同「添加提供商」那条：null = 没选，不当作一个模式名（见那里的说明）
                onValueChange={next => { if (next != null && String(next)) void savePromptMode(String(next)) }}
              >
                <SelectTrigger
                  id='settings-prompt-mode'
                  className='w-[240px]'
                  disabled={locked || busy}
                  aria-label={t('系统提示词模式')}
                >
                  <SelectValue>{modeLabel}</SelectValue>
                </SelectTrigger>
                <SelectContent>
                  {PROMPT_MODES.map(item => (
                    <SelectItem key={item.value} value={item.value}>{item.optionLabel}</SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </span>
            <div className='hint'>{NOTES.promptMode}</div>
          </div>

          <PromptFileRow prompt={prompt} locked={locked} busy={busy} />

          {/* 提示词正文：与文件是同一件事的两个来源（正文优先），所以紧挨着文件那一行。
              行里只放按钮与状态 —— 几百行文本塞进行内输入框既看不清也没法编辑 */}
          <div className='retention-row'>
            <label htmlFor='btn-prompt-edit-body'>{t('提示词正文')}</label>
            <span className='prompt-input'>
              <PromptTextButton prompt={prompt} locked={locked} busy={busy} />
            </span>
            <div className='hint'>
              {NOTES.promptText}
              {prompt.source === 'inline'
                ? t('（当前生效的就是这一份，{n} 行）', { n: prompt.lines })
                : prompt.text.trim()
                  ? t('（已存一份正文，{n} 行；切成「替换 / 追加」后生效）', { n: prompt.lines })
                  : ''}
            </div>
          </div>

          <div className='prompt-group'>
            {t('按提供商配置')}<span className='note'>{t('只列单独配置过的家，其余沿用上面的全局配置')}</span>
          </div>

          <ProviderPromptRows prompt={prompt} locked={locked} busy={busy} />

          {/* 降级行只在真的处于降级期时渲染（旧实现是切 hidden） */}
          {prompt.status === 'ready' && prompt.degradeActive ? (
            <div className='retention-row'>
              <label>{t('内容拦截降级')}</label>
              <span className='prompt-input'>
                <Button
                  id='btn-prompt-clear-degrade'
                  variant='outline'
                  disabled={busy}
                  onClick={() => void clearDegrade()}
                >
                  {t('立即解除')}
                </Button>
              </span>
              <div className='hint'>{degradeHint(prompt)}</div>
            </div>
          ) : null}
        </div>
        <div className='settings-state'>{promptStateText(prompt)}</div>
        <div className='hint retention-note'>{NOTES.prompt}</div>
      </div>
    </section>
  )
}

/** 调试模式的状态行：条数只在后端给了两个整数时才提 */
function debugStateText(debug: DebugState): string {
  if (debug.status === 'loading') return STATES.appLoading
  if (debug.status === 'unavailable') return STATES.debugUnavailable
  const stored = debug.count !== null && debug.limit !== null
    ? t('已保存 {count} / {limit} 条报文（超出后丢弃最旧的）。', { count: debug.count, limit: debug.limit })
    : ''
  return debug.on ? `${STATES.debugOn}${stored}${t('凭据类请求头已脱敏。')}` : STATES.debugOff
}

/* ─── 重试 / 超时分类（从「网关」拆出的两个独立菜单）── */

/** 请求超时（拆出理由见 settings-model 的 CATEGORIES 说明） */
function TimeoutPane({ snap }: { snap: SettingsSnapshot }) {
  return (
    <section className='panel'>
      <PanelHead
        title={t('请求超时')}
        tip={TIPS.timeouts}
        badge={numericBadge(snap.timeouts)}
        actions={<RefreshButton id='btn-timeouts-refresh' onClick={() => void refreshTimeouts()} />}
      />
      <div className='panel-body'>
        <div className='retention-list'>
          {TIMEOUT_FIELDS.map(field => (
            <NumberRow
              key={field.key}
              field={field}
              value={snap.timeouts.values?.[field.key] ?? null}
              disabled={snap.busy === 'timeouts'}
              onCommit={raw => saveTimeoutField(field, raw)}
            />
          ))}
        </div>
        <div className='hint retention-note'>{NOTES.timeouts}</div>
      </div>
    </section>
  )
}

/** 请求重试：数字参数 + 「指定错误码」名单（两段共用同一份保存流程） */
function RetryPane({ snap }: { snap: SettingsSnapshot }) {
  return (
    <section className='panel'>
      <PanelHead
        title={t('请求重试')}
        tip={TIPS.retry}
        badge={numericBadge(snap.retry)}
        actions={<RefreshButton id='btn-retry-refresh' onClick={() => void refreshRetry()} />}
      />
      <div className='panel-body'>
        <div className='retention-list'>
          {RETRY_FIELDS.map(field => (
            <NumberRow
              key={field.key}
              field={field}
              value={snap.retry.values?.[field.key] ?? null}
              disabled={snap.busy === 'retry'}
              onCommit={raw => saveRetryField(field, raw)}
            />
          ))}
          <RetryCodesField
            codes={snap.retryCodes}
            status={snap.retry.status}
            busy={snap.busy === 'codes' || snap.busy === 'retry'}
          />
        </div>
        <div className='hint retention-note'>{NOTES.retry}</div>
      </div>
    </section>
  )
}

/* ─── 路由分类（转发选路的策略 + 会话粘性）── */

/**
 * 自愈开关的后端键 → 前端状态字段。
 *
 * 两边命名不同（后端是 `routingSink` 这类带前缀的长键，前端状态是 `sink` 这类短字段），
 * 这里显式列一张表而不是拼字符串 —— 拼出来的名字一旦对不上，表现是「开关点了没反应」，
 * 那种静默失败最难查。
 */
const PROTECTION_STATE_FIELD = {
  routingFailoverBackoff: 'failoverBackoff',
  routingSink: 'sink',
  routingBreakerEnabled: 'breakerEnabled',
} as const

function RoutingPane({ snap }: { snap: SettingsSnapshot }) {
  const routing: RoutingState = snap.routing
  // 读不到（桥不在 / 后端不可用）或还在检测时锁住两档 —— 锁住比发出一次注定失败的保存强
  const disabled = snap.busy === 'routing' || routing.status !== 'ready'
  // 快照值只在档位表里取（老后端 / 异常值兜底回默认档），顺带把类型收窄到字面量联合
  const strategy: RouteStrategyValue =
    ROUTE_STRATEGIES.find(item => item.value === routing.strategy)?.value ?? 'order'
  const sticky: RouteStickyValue =
    ROUTE_STICKY_OPTIONS.find(item => item.value === routing.sticky)?.value ?? 'auto'
  return (
    <section className='panel'>
      <PanelHead
        title={t('账号路由')}
        tip={TIPS.routing}
        badge={routing.status === 'ready' ? undefined
          : routing.status === 'unavailable'
            ? <StatusBadge tone='bad'>{t('不可用')}</StatusBadge>
            : <StatusBadge tone='idle'>{t('检测中…')}</StatusBadge>}
        actions={<RefreshButton id='btn-routing-refresh' onClick={() => void refreshRouting()} />}
      />
      <div className='panel-body'>
        <div className='retention-list'>
          {/* 两行的说明都收进行内问号（[data-tip]，tooltip.js 统一服务），并且**跟随当前档位**
              换内容 —— 每档干的事不一样，一段固定文案说不清 */}
          <div className='prompt-group'>{t('选路规则')}</div>
          <div className='retention-row'>
            <label>{t('路由策略')}</label>
            <div className='routing-row'>
              <span className='tip-q' data-tip={ROUTE_STRATEGIES.find(item => item.value === strategy)?.tip}></span>
              <SegmentedControl
                options={ROUTE_STRATEGIES.map(item => ({ value: item.value, label: item.label }))}
                value={strategy}
                disabled={disabled}
                aria-label={t('路由策略')}
                onValueChange={value => void saveRoutingField({ routingStrategy: String(value) })} />
            </div>
          </div>
          <div className='retention-row'>
            <label>{t('会话粘性')}</label>
            <div className='routing-row'>
              <span className='tip-q' data-tip={ROUTE_STICKY_OPTIONS.find(item => item.value === sticky)?.tip}></span>
              <SegmentedControl
                options={ROUTE_STICKY_OPTIONS.map(item => ({ value: item.value, label: item.label }))}
                value={sticky}
                disabled={disabled}
                aria-label={t('会话粘性')}
                onValueChange={value => void saveRoutingField({ routingSticky: String(value) })} />
            </div>
          </div>
          {/* 三个自愈开关：分组条把「怎么排队」与「出了事怎么自愈」分开 —— 不分开时它们是
              一串同构的行，读不出哪几行管选路、哪几行管失败处置 */}
          <div className='prompt-group'>
            {t('自愈保护')}<span className='note'>{t('默认值通常够用，出问题时才需要调')}</span>
          </div>
          {ROUTE_PROTECTION_SWITCHES.map(item => {
            // label 与 Switch 靠 htmlFor/id 配对（同页既有写法，见 SwitchRow 与
            // DB 面板）：点文字也能拨动，读屏才念得出这一项叫什么
            const switchId = `route-protection-${item.key}`
            return (
              <div key={item.key} className='retention-row'>
                <label htmlFor={switchId}>{item.label}</label>
                <div className='routing-row'>
                  <span className='tip-q' data-tip={item.tip}></span>
                  <Switch
                    id={switchId}
                    checked={routing[PROTECTION_STATE_FIELD[item.key]]}
                    disabled={disabled}
                    onCheckedChange={next => void saveRoutingField({ [item.key]: next })} />
                </div>
              </div>
            )
          })}
        </div>
        <div className='hint retention-note'>{NOTES.routing}</div>
      </div>
    </section>
  )
}

/* ─── 网关分类 ─────────────────────────────── */

function GatewayPane({ snap }: { snap: SettingsSnapshot }) {
  return (
    <>
      <section className='panel'>
        <PanelHead
          title={t('排队等待')}
          tip={TIPS.queue}
          badge={numericBadge(snap.queue)}
          actions={<RefreshButton id='btn-queue-refresh' onClick={() => void refreshQueue()} />}
        />
        <div className='panel-body'>
          <div className='retention-list'>
            {QUEUE_FIELDS.map(field => (
              <NumberRow
                key={field.key}
                field={field}
                value={snap.queue.values?.[field.key] ?? null}
                disabled={snap.busy === 'queue'}
                onCommit={raw => saveQueueField(field, raw)}
              />
            ))}
          </div>
          <div className='hint retention-note'>{NOTES.queue}</div>
        </div>
      </section>

      <section className='panel'>
        <PanelHead
          title={t('验证码令牌池')}
          tip={TIPS.captchaPool}
          badge={numericBadge(snap.captchaPool)}
          actions={
            <RefreshButton id='btn-captcha-pool-refresh' onClick={() => void refreshCaptchaPool()} />
          }
        />
        <div className='panel-body'>
          <div className='retention-list'>
            {CAPTCHA_POOL_FIELDS.map(field => (
              <NumberRow
                key={field.key}
                field={field}
                value={snap.captchaPool.values?.[field.key] ?? null}
                disabled={snap.busy === 'captchaPool'}
                onCommit={raw => saveCaptchaPoolField(field, raw)}
              />
            ))}
          </div>
          <div className='hint retention-note'>{NOTES.captchaPool}</div>
        </div>
      </section>

      <section className='panel'>
        <PanelHead
          title={t('models.dev 模型元数据')}
          tip={TIPS.modelsDev}
          badge={snap.modelsDev.status === 'ready'
            ? <StatusBadge tone='ok'>{t('{count} 条', { count: Number(snap.modelsDev.count).toLocaleString('zh-CN') })}</StatusBadge>
            : snap.modelsDev.status === 'unavailable'
              ? <StatusBadge tone='bad'>{t('不可用')}</StatusBadge>
              : <StatusBadge tone='idle'>{t('检测中…')}</StatusBadge>}
          actions={
            <RefreshButton id='btn-modelsdev-refresh' onClick={() => void refreshModelsDev()} />
          }
        />
        <div className='panel-body'>
          <SwitchRow
            id='settings-modelsdev'
            label={t('用 models.dev 补齐上游没声明的模型能力与思考档位（联网刷新，内嵌快照始终生效）')}
            checked={snap.modelsDev.enabled}
            disabled={snap.modelsDev.status !== 'ready' || snap.busy === 'modelsDev'}
            onCheckedChange={next => void saveModelsDev(next)}
          />
          <div className='settings-state'>{modelsDevStateText(snap.modelsDev)}</div>
          <div className='hint retention-note'>{NOTES.modelsDev}</div>
        </div>
      </section>

      <section className='panel'>
        <PanelHead
          title={t('指纹脱敏')}
          tip={TIPS.sanitize}
          badge={snap.sanitize.status === 'ready'
            ? <StatusBadge tone='ok'>{t('已生效')}</StatusBadge>
            : snap.sanitize.status === 'unavailable'
              ? <StatusBadge tone='bad'>{t('不可用')}</StatusBadge>
              : <StatusBadge tone='idle'>{t('检测中…')}</StatusBadge>}
          actions={<RefreshButton id='btn-sanitize-refresh' onClick={() => void refreshSanitize()} />}
        />
        <div className='panel-body'>
          <div className='settings-switches'>
            <SwitchRow
              id='settings-sanitize'
              label={t('剥离上游审核黑名单指纹（改写请求体里的模板句与表头）')}
              checked={snap.sanitize.on}
              // 读到后端值之前不许切（否则会出现「切了但不知道后端原本是什么」，回滚也没依据）
              disabled={snap.sanitize.status !== 'ready' || snap.busy === 'sanitize'}
              onCheckedChange={next => void saveSanitize(next)}
            />
          </div>
          <div className='settings-state'>
            {snap.sanitize.status === 'loading'
              ? STATES.appLoading
              : snap.sanitize.status === 'unavailable'
                ? STATES.sanitizeUnavailable
                : snap.sanitize.on ? STATES.sanitizeOn : STATES.sanitizeOff}
          </div>
          <div className='hint retention-note'>{NOTES.sanitize}</div>
        </div>
      </section>

      <section className='panel'>
        <PanelHead
          title={t('开发者角色兼容')}
          tip={TIPS.developerRoleCompat}
          badge={snap.developerRoleCompat.status === 'ready'
            ? <StatusBadge tone='ok'>{t('已生效')}</StatusBadge>
            : snap.developerRoleCompat.status === 'unavailable'
              ? <StatusBadge tone='bad'>{t('不可用')}</StatusBadge>
              : <StatusBadge tone='idle'>{t('检测中…')}</StatusBadge>}
          actions={
            <RefreshButton id='btn-role-compat-refresh' onClick={() => void refreshDeveloperRoleCompat()} />
          }
        />
        <div className='panel-body'>
          <div className='settings-switches'>
            <SwitchRow
              id='settings-developer-role-compat'
              label={t('转发前将 developer 角色消息转换为 system，兼容不支持 developer 角色的 OpenAI 兼容上游')}
              checked={snap.developerRoleCompat.on}
              // 读到后端值之前不许切（否则会出现「切了但不知道后端原本是什么」，回滚也没依据）
              disabled={snap.developerRoleCompat.status !== 'ready' || snap.busy === 'developerRoleCompat'}
              onCheckedChange={next => void saveDeveloperRoleCompat(next)}
            />
          </div>
          <div className='settings-state'>
            {snap.developerRoleCompat.status === 'loading'
              ? STATES.appLoading
              : snap.developerRoleCompat.status === 'unavailable'
                ? STATES.roleCompatUnavailable
                : snap.developerRoleCompat.on ? STATES.roleCompatOn : STATES.roleCompatOff}
          </div>
          <div className='hint retention-note'>{NOTES.developerRoleCompat}</div>
        </div>
      </section>

      <ClineHeadersPanel snap={snap} />

      <PromptPanel snap={snap} />

      <section className='panel'>
        <PanelHead
          title={t('调试模式')}
          tip={TIPS.debug}
          badge={snap.debug.status === 'ready'
            ? <StatusBadge tone='ok'>{t('已生效')}</StatusBadge>
            : snap.debug.status === 'unavailable'
              ? <StatusBadge tone='bad'>{t('不可用')}</StatusBadge>
              : <StatusBadge tone='idle'>{t('检测中…')}</StatusBadge>}
          actions={<RefreshButton id='btn-debug-refresh' onClick={() => void refreshDebug()} />}
        />
        <div className='panel-body'>
          <div className='settings-switches'>
            <SwitchRow
              id='settings-debug-mode'
              label={t('保存上游原始报文（请求头、请求体、响应头、响应体）')}
              checked={snap.debug.on}
              disabled={snap.debug.status !== 'ready' || snap.busy === 'debug'}
              onCheckedChange={next => void saveDebug(next)}
            />
          </div>
          <div className='settings-state'>{debugStateText(snap.debug)}</div>
          <div className='hint retention-note'>{NOTES.debug}</div>
        </div>
      </section>
    </>
  )
}

/* ─── Cline 伪装头（网关分类）───────────────── */

/** 伪装头面板的可编辑行：默认行的键锁定，自定义行的键可写 */
type ClineHeaderRow = { key: string; value: string; isDefault: boolean }

/**
 * 从快照拼出可编辑的行：默认清单全量在前（保持后端给的顺序），覆盖表里
 * 多出来的自定义头跟在后面。默认行的值取「覆盖值优先」（空串 = 用户显式
 * 删了这个头），自定义行原样来自覆盖表。
 */
function clineRowsFrom(state: ClineHeadersState): ClineHeaderRow[] {
  const rows: ClineHeaderRow[] = Object.entries(state.defaults).map(([key, value]) => ({
    key,
    value: key in state.overrides ? state.overrides[key] : value,
    isDefault: true,
  }))
  for (const [key, value] of Object.entries(state.overrides)) {
    if (!(key in state.defaults)) rows.push({ key, value, isDefault: false })
  }
  return rows
}

/** 两张覆盖表是否等价（键集合 + 每个键的值，与键序无关） */
function sameOverrides(a: Record<string, string>, b: Record<string, string>): boolean {
  const keys = Object.keys(a)
  if (keys.length !== Object.keys(b).length) return false
  return keys.every(key => a[key] === b[key])
}

/**
 * Cline 伪装头面板：一张「头名 → 头值」的行编辑器。
 *
 * 编辑都在**本组件的草稿**里进行（与 NumberRow 同一取向，只是整张表一份草稿）；
 * 后端值一变（加载完成 / 保存成功 / 刷新 / 失败回滚）草稿整体重置。「保存」把
 * 草稿折算成覆盖表交给状态层 —— 默认行改回了默认不进表（后端存的是「改过的
 * 键」，不是全量配置）、默认行值留空 = 这个头不发、自定义行必须有名有值。
 */
function ClineHeadersPanel({ snap }: { snap: SettingsSnapshot }) {
  const state = snap.clineHeaders
  const [rows, setRows] = React.useState<ClineHeaderRow[]>(() => clineRowsFrom(state))
  React.useEffect(() => { setRows(clineRowsFrom(state)) }, [state])

  const busy = snap.busy === 'clineHeaders'
  const ready = state.status === 'ready'

  function updateRow(index: number, patch: Partial<ClineHeaderRow>): void {
    setRows(current => current.map((row, i) => (i === index ? { ...row, ...patch } : row)))
  }

  function buildOverrides(): Record<string, string> {
    const out: Record<string, string> = {}
    for (const row of rows) {
      const key = row.key.trim()
      if (!key) continue
      if (row.isDefault) {
        if (row.value !== (state.defaults[key] ?? '')) out[key] = row.value
      } else if (row.value !== '') {
        out[key] = row.value
      }
    }
    return out
  }

  // 与后端的覆盖表比「键值是否相同」，**不比对键序**：草稿按行顺序产出（默认行
  // 在前、自定义行追加在后），而后端的 BTreeMap 是字典序 —— 串起来比字符串会让
  // 「覆盖了某个默认头 + 加了一个排序在它前面的自定义头」这种组合在保存成功后
  // 仍被判定为「有未保存的修改」，保存按钮一直亮着。
  const dirty = ready && !sameOverrides(buildOverrides(), state.overrides)

  return (
    <CollapsiblePanel
      title={t('Cline 伪装头')}
      tip={TIPS.clineHeaders}
      badge={state.status === 'ready'
        ? <StatusBadge tone='ok'>{t('已生效')}</StatusBadge>
        : state.status === 'unavailable'
          ? <StatusBadge tone='bad'>{t('不可用')}</StatusBadge>
          : <StatusBadge tone='idle'>{t('检测中…')}</StatusBadge>}
      actions={<RefreshButton id='btn-cline-headers-refresh' onClick={() => void refreshClineHeaders()} />}
    >
      <div className='panel-body'>
        <div className='retention-list'>
          {ready
            ? rows.map((row, index) => (
              <div key={String(index)} className='flex items-center gap-2'>
                <Input
                  className='w-[190px] shrink-0 font-mono text-[13px]'
                  value={row.key}
                  readOnly={row.isDefault}
                  disabled={busy}
                  placeholder='X-Custom-Header'
                  onChange={event => updateRow(index, { key: event.target.value })}
                />
                <Input
                  className='flex-1 font-mono text-[13px]'
                  value={row.value}
                  disabled={busy}
                  placeholder={row.isDefault ? t('值（留空 = 不发送）') : t('值')}
                  onChange={event => updateRow(index, { value: event.target.value })}
                />
                <Button
                  variant='ghost'
                  disabled={busy}
                  onClick={() => {
                    if (row.isDefault) {
                      updateRow(index, { value: state.defaults[row.key] ?? '' })
                    } else {
                      setRows(current => current.filter((_, i) => i !== index))
                    }
                  }}
                >
                  {row.isDefault ? t('还原') : t('删除')}
                </Button>
              </div>
            ))
            : null}
          <div className='flex items-center gap-2'>
            <Button
              variant='outline'
              disabled={busy || !ready}
              onClick={() => setRows(current => [...current, { key: '', value: '', isDefault: false }])}
            >
              {t('添加自定义头')}
            </Button>
            <div className='flex-1' />
            <Button
              variant='ghost'
              disabled={busy || !ready || Object.keys(state.overrides).length === 0}
              onClick={() => void saveClineHeaders({})}
            >
              {t('恢复默认')}
            </Button>
            <Button disabled={busy || !ready || !dirty} onClick={() => void saveClineHeaders(buildOverrides())}>
              {t('保存')}
            </Button>
          </div>
        </div>
        <div className='settings-state'>
          {state.status === 'loading'
            ? STATES.appLoading
            : state.status === 'unavailable'
              ? t('未能读取 Cline 伪装头设置，请稍后重试')
              : dirty
                ? t('有未保存的修改')
                : t('{n} 个头将随每个 Cline 请求发送', { n: Object.keys(state.effective).length })
                  + (Object.keys(state.overrides).length > 0
                    ? t('（{n} 项被覆盖）', { n: Object.keys(state.overrides).length })
                    : t('（全部为默认值）'))}
        </div>
        <div className='hint retention-note'>{NOTES.clineHeaders}</div>
      </div>
    </CollapsiblePanel>
  )
}

/* ─── 安全分类 ─────────────────────────────── */

function SecurityPane({ snap }: { snap: SettingsSnapshot }) {
  // 「退出登录」成功后整页跳回登录页，那时组件已不在；只有失败才需要把按钮解禁
  const [loggingOut, setLoggingOut] = React.useState(false)

  async function onLogout(): Promise<void> {
    setLoggingOut(true)
    const ok = await panelLogout()
    if (!ok) setLoggingOut(false)
  }

  return (
    <>
      <section className='panel'>
        <PanelHead
          title={t('网关跨域访问（CORS）')}
          tip={TIPS.cors}
          badge={snap.cors.status === 'ready'
            // 极性与指纹脱敏相反：这个开关「关闭」才是不扩大暴露面的常态，
            // 所以开着时给提醒色（bad），关着才是 ok
            ? (snap.cors.on ? <StatusBadge tone='bad'>{t('已开启')}</StatusBadge> : <StatusBadge tone='ok'>{t('已关闭')}</StatusBadge>)
            : snap.cors.status === 'unavailable'
              ? <StatusBadge tone='bad'>{t('不可用')}</StatusBadge>
              : <StatusBadge tone='idle'>{t('检测中…')}</StatusBadge>}
          actions={<RefreshButton id='btn-cors-refresh' onClick={() => void refreshCors()} />}
        />
        <div className='panel-body'>
          <div className='settings-switches'>
            <SwitchRow
              id='settings-cors'
              label={t('允许浏览器里的页面跨来源调用网关（/v1/*）')}
              checked={snap.cors.on}
              // 读到后端值之前不许切（同指纹脱敏：切了也不知道后端原本是什么）
              disabled={snap.cors.status !== 'ready' || snap.busy === 'cors'}
              onCheckedChange={next => void saveCors(next)}
            />
          </div>
          <div className='settings-state'>
            {snap.cors.status === 'loading'
              ? STATES.appLoading
              : snap.cors.status === 'unavailable'
                ? STATES.corsUnavailable
                : snap.cors.on ? STATES.corsOn : STATES.corsOff}
          </div>
          <div className='hint retention-note'>{NOTES.cors}</div>
        </div>
      </section>

      <section className='panel'>
        <PanelHead title={t('机器人校验')} />
        <div className='panel-body'>
          <SwitchRow
            id='settings-captcha'
            label={t('登录 / 注册需要通过 ALTCHA 人机验证（工作量证明）')}
            checked={snap.captcha.enabled}
            disabled={!snap.captcha.available || snap.busy === 'captcha'}
            onCheckedChange={next => void saveCaptcha(next)}
          />
          <div className='hint'>{NOTES.captcha}</div>
        </div>
      </section>

      {/* 面板登录：仅网页端（桌面壳的面板跟着应用走，没有「登录面板」的概念） */}
      {snap.panelLogin ? (
        <section className='panel' id='panel-login-section'>
          <PanelHead title={t('面板登录')} />
          <div className='panel-body'>
            <div className='hint'>{NOTES.panelLogin}</div>
            <div className='mt-2.5'>
              <Button
                id='btn-panel-logout'
                variant='outline'
                disabled={loggingOut}
                onClick={() => void onLogout()}
              >
                {t('退出登录')}
              </Button>
            </div>
          </div>
        </section>
      ) : null}
    </>
  )
}

/* ─── 数据分类 ─────────────────────────────── */

/** 数据存储面板的一格计数（键在上、值在下） */
function StorageCount({ label, value }: { label: string; value: string }) {
  return (
    <div className='storage-count'>
      <span className='k'>{label}</span>
      <span className='v'>{value}</span>
    </div>
  )
}

function storageBadge(state: StorageState): React.ReactNode {
  if (state.status === 'loading') return <StatusBadge tone='idle'>{t('检测中…')}</StatusBadge>
  if (state.status === 'unavailable') return <StatusBadge tone='bad'>{t('不可用')}</StatusBadge>
  // 「数据库不可用」是读到了概况但库打不开，与「读不到」不是一回事（旧实现同）
  return <StatusBadge tone={state.available ? 'ok' : 'bad'}>
    {state.available ? t('已生效') : t('数据库不可用')}
  </StatusBadge>
}

function DataPane({ snap }: { snap: SettingsSnapshot }) {
  const io = snap.busy
  const storage = snap.storage
  // 端别取自桥（platform）：整库备份 / 恢复的文件对话框只有桌面壳有
  const isWebBridge = shared().workbuddyDesktop?.platform === 'web'
  // 库打不开时各计数都是后端回落出来的 0，与「真的没有数据」在数字上无法区分 ——
  // 整排显示「—」，不误导用户以为数据丢了；路径仍然照显（文件位置是已知的）
  const ready = storage.status === 'ready'
  const counts = ready && storage.available
  const path = ready && storage.file ? storage.file : '—'
  const count = (value: number | null): string => (counts && value !== null ? formatCount(value) : '—')

  return (
    <>
      {/* 整库备份 / 恢复：仅桌面端渲染（web shim 没有这两个桥方法）。
          排在「账号导入 / 导出」之前 —— 恢复是整库替换，两种语义别让用户混着看 */}
      {!isWebBridge && (
        <section className='panel'>
          <PanelHead
            title={t('整库备份 / 恢复')}
            tip={TIPS.backup}
            actions={
              <>
                <Button
                  id='btn-settings-db-export'
                  variant='outline'
                  disabled={io === 'dbExport' || io === 'dbImport'}
                  onClick={() => void exportDatabase()}
                >
                  {io === 'dbExport' ? t('备份中…') : t('备份整库')}
                </Button>
                <Button
                  id='btn-settings-db-restore'
                  variant='destructive'
                  disabled={io === 'dbExport' || io === 'dbImport'}
                  onClick={() => confirmDbRestore()}
                >
                  {io === 'dbImport' ? t('恢复中…') : t('恢复整库')}
                </Button>
              </>
            }
          />
          <div className='panel-body'>
            {/* 恢复后的密钥警告（后端 warning 原文，密钥对不上时导入后账号解不开）：
                排在面板最前 —— 用户刚点完标题栏的「恢复整库」，视线就在这片区域，
                这条警告必须第一眼看到；而且它**常驻**，不像 toast 那样几秒后消失。
                呈现沿用本页既有的 .danger-zone 警示框（与下面那条备份说明同款），
                不自造组件；文案是服务端生成的数据，原样显示、不翻译 */}
            {snap.dbWarning ? (
              <div className='danger-zone' id='settings-db-warning'>
                <strong>{t('⚠️ 密钥警告')}</strong>
                <br />
                {snap.dbWarning}
              </div>
            ) : null}
            <div className='retention-row'>
              <label htmlFor='settings-db-include-logs'>{t('包含日志与调试报文')}</label>
              <Switch
                id='settings-db-include-logs'
                checked={snap.dbIncludeLogs}
                disabled={io !== null}
                onCheckedChange={toggleDbIncludeLogs}
              />
              <div className='hint'>
                {t('取消勾选后只备份业务数据（账号、配置与余额记录），文件显著变小；事件日志、请求统计与调试报文不会进备份文件。')}
              </div>
            </div>
            <div className='danger-zone'>
              <strong>{NOTES.backupDanger}</strong>
            </div>
          </div>
        </section>
      )}

      <section className='panel'>
        <PanelHead
          title={t('账号导入 / 导出')}
          tip={TIPS.io}
          actions={
            <>
              {/* 忙碌守卫与旧实现的 guard() 同形：点下去的那个按钮禁用并换文案，
                  另一个保持可点但会被守卫挡下 */}
              <Button
                id='btn-settings-export'
                variant='outline'
                disabled={io === 'export'}
                onClick={() => void exportAccounts()}
              >
                {io === 'export' ? t('导出中…') : t('导出账号')}
              </Button>
              <Button
                id='btn-settings-import'
                variant='default'
                disabled={io === 'import'}
                onClick={() => void importAccounts()}
              >
                {io === 'import' ? t('导入中…') : t('导入账号')}
              </Button>
            </>
          }
        />
        <div className='panel-body'>
          <div className='danger-zone'>
            <strong>{t('导出文件内含 accessToken / refreshToken / apiKey 等凭证与自定义提供商定义')}</strong>
            {t('，可直接用于登录。请妥善保管，不要外传或上传到公共位置。')}
          </div>
          {/* 失败明细（旧实现写 innerHTML 并 display:none 收起空结果，这里条件渲染） */}
          {snap.ioFailure ? (
            <div className='io-result'>
              <span className='text-destructive'>
                {t('失败 {n} 个：{detail}', { n: snap.ioFailure.failed, detail: snap.ioFailure.detail })}
                {snap.ioFailure.more ? t(' 等') : ''}
              </span>
            </div>
          ) : null}
        </div>
      </section>

      <section className='panel'>
        <PanelHead
          title={t('数据保留')}
          tip={TIPS.retention}
          badge={numericBadge(snap.retention)}
          actions={<RefreshButton id='btn-retention-refresh' onClick={() => void refreshRetention()} />}
        />
        <div className='panel-body'>
          <div className='retention-list'>
            {RETENTION_FIELDS.map(field => (
              <NumberRow
                key={field.key}
                field={field}
                value={snap.retention.values?.[field.key] ?? null}
                disabled={snap.busy === 'retention'}
                onCommit={raw => saveRetentionField(field, raw)}
              />
            ))}
          </div>
          <div className='hint retention-note'>{NOTES.retention}</div>
        </div>
      </section>

      <section className='panel'>
        <PanelHead
          title={t('数据存储')}
          tip={TIPS.storage}
          badge={storageBadge(storage)}
          actions={<RefreshButton id='btn-storage-refresh' onClick={() => void refreshStorage()} />}
        />
        <div className='panel-body'>
          <div className='retention-list'>
            <div className='retention-row'>
              <label htmlFor='storage-db-path'>{t('数据库文件')}</label>
              <span className='storage-line'>
                {/* 悬停看完整路径（元素上是折行显示的，长路径会被截成好几行） */}
                <span className='storage-path' id='storage-db-path' title={path}>{path}</span>
              </span>
              <div className='hint'>{NOTES.storageFile}</div>
            </div>
            <div className='retention-row'>
              <label htmlFor='storage-db-size'>{t('占用大小')}</label>
              <span className='storage-line'>
                <span className='storage-path' id='storage-db-size'>
                  {counts ? formatBytes(storage.bytes) : '—'}
                </span>
              </span>
              <div className='hint'>{NOTES.storageSize}</div>
            </div>
          </div>
          <div className='retention-list storage-counts'>
            <StorageCount label={t('账号')} value={count(storage.accounts)} />
            <StorageCount label={t('事件日志')} value={count(storage.logs)} />
            <StorageCount label={t('请求记录')} value={count(storage.requests)} />
            <StorageCount label={t('报表天数')} value={count(storage.dailyDays)} />
            <StorageCount label={t('调试报文')} value={count(storage.debug)} />
          </div>
          <div className='hint retention-note'>{NOTES.storage}</div>
        </div>
      </section>
    </>
  )
}

/* ─── 反馈与需求分类 ───────────────────────── */

/**
 * 三个入口，分别指向仓库里预设好模板的 GitHub issue 表单。
 * 用系统默认浏览器打开（`openExternal` → 壳命令 `open_release_page`，只放行
 * http(s)）：应用内 webview 打开会白屏，而且提交 issue 需要用户自己的 GitHub 登录态。
 */
const FEEDBACK_LINKS = [
  {
    title: t('问题反馈'),
    desc: t('遇到 Bug、报错或异常行为'),
    cta: t('去反馈'),
    url: 'https://github.com/aimod-cc/agent2api/issues/new?template=bug_report.yml',
  },
  {
    title: t('功能建议'),
    desc: t('想要的新功能或改进想法'),
    cta: t('提建议'),
    url: 'https://github.com/aimod-cc/agent2api/issues/new?template=feature_request.yml',
  },
  {
    title: t('请求提供商 / 模型支持'),
    desc: t('希望接入新的提供商或模型'),
    cta: t('去申请'),
    url: 'https://github.com/aimod-cc/agent2api/issues/new?template=provider_request.yml',
  },
] as const

function FeedbackPane() {
  return (
    <section className='panel'>
      <PanelHead title={t('反馈与需求')} />
      <div className='panel-body'>
        <div className='hint'>
          {t('点下面的入口会用系统默认浏览器打开 GitHub 的对应表单（需要 GitHub 账号，模板已预设好，填完直接提交即可）。')}
        </div>
        <div className='mt-3 flex flex-col gap-2'>
          {FEEDBACK_LINKS.map(item => (
            <div
              key={item.url}
              className='flex items-center justify-between gap-3 rounded-md border border-hairline bg-surface-2 px-3 py-2.5'
            >
              <div className='min-w-0'>
                <div className='text-[12.5px] text-foreground'>{item.title}</div>
                <div className='mt-0.5 text-[12px] text-subtle'>{item.desc}</div>
              </div>
              <Button variant='outline' size='sm' onClick={() => void openExternal(item.url)}>
                {item.cta}
              </Button>
            </div>
          ))}
        </div>
      </div>
    </section>
  )
}

/* ─── 保留期改小的确认框 ───────────────────── */

/**
 * 缩短保留天数的二次确认（旧实现是 index.html 的 `#retention-modal` + 一堆 class 开关）。
 *
 * 用 Dialog 而不是 AlertDialog：旧实现的五条出口（确认 / 取消 / 右上角 ✕ / 点遮罩 / Esc）里
 * 有四条都算「取消」，即它是**可以糊弄过去**的普通弹窗，不是 AlertDialog 那种必须表态的框。
 * 焦点落在「取消」而不是危险键：这是不可恢复的删除操作，敲回车不该等于同意删除。
 */
function RetentionConfirmDialog({ confirm }: { confirm: { head: string } | null }) {
  const cancelRef = React.useRef<HTMLButtonElement | null>(null)

  return (
    <Dialog open={confirm !== null} onOpenChange={next => { if (!next) resolveRetentionConfirm(false) }}>
      {/* 旧 .modal-confirm 把宽度收到 440px（这类框只有一段话加两个按钮，620px 太宽） */}
      <DialogContent className='w-[min(440px,calc(100vw-48px))]' initialFocus={cancelRef}>
        <DialogHeader>
          <DialogTitle>{t('确认缩短保留天数')}</DialogTitle>
        </DialogHeader>
        <DialogBody>
          <div className='danger-zone'>
            {confirm?.head}
            <br />
            <strong>{t('超出的历史数据会被立即删除，且不可恢复。')}</strong>{t('确定继续？')}</div>
        </DialogBody>
        <DialogFooter>
          <div className='mr-auto' />
          <Button ref={cancelRef} variant='outline' onClick={() => resolveRetentionConfirm(false)}>
            {t('取消')}
          </Button>
          <Button variant='destructive' onClick={() => resolveRetentionConfirm(true)}>
            {t('继续并清理')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

/* ─── 局域网访问的确认 / 注册弹窗 ─────────────── */

/** 三种确认（开启 / 关闭 / 面板子开关）各自的标题、正文与确认键文案 */
const LAN_CONFIRM_COPY: Record<
  NonNullable<LanConfirm>['mode'],
  { title: string; body: (panel: boolean) => React.ReactNode; label: string }
> = {
  enable: {
    title: t('开启局域网访问'),
    body: () => (
      <>
        {t('为了安全，开启前需要先注册一个')}<b>{t('面板管理员账号')}</b>{t('（已注册过会跳过这一步，直接生效）。')}<br />
        {t('开启后网关将监听所有网卡，同一局域网内的设备即可把 API 地址指向本机 IP 一起使用；管理接口从此要求管理员会话或网关 Key，转发接口在没有一把启用的 Key 时也会拒绝服务（届时会自动创建一把名为「默认」的 Key）。')}<br />
        <strong>{t('保存后应用将重启以生效。')}</strong>{t('确定继续？')}
      </>
    ),
    label: t('继续'),
  },
  disable: {
    title: t('关闭局域网访问'),
    body: () => (
      <>
        {t('关闭后网关回到只监听 127.0.0.1，局域网内的设备将无法继续访问；已配置的网关 Key 与账号都不受影响。')}<br />
        <strong>{t('保存后应用将重启以生效。')}</strong>{t('确定继续？')}
      </>
    ),
    label: t('关闭并重启'),
  },
  panel: {
    title: t('变更网页管理面板'),
    body: panel =>
      panel ? (
        <>
          {t('开放后，局域网内其他设备的浏览器打开本机 IP 即可进入管理面板（需管理员账号登录）。')}<br />
          <strong>{t('保存后应用将重启以生效。')}</strong>{t('确定继续？')}
        </>
      ) : (
        <>
          {t('关闭后，管理界面不再从局域网提供，只有本机的桌面程序可以管理；已开启的 API 转发不受影响。')}<br />
          <strong>{t('保存后应用将重启以生效。')}</strong>{t('确定继续？')}
        </>
      ),
    label: t('保存并重启'),
  },
}

/**
 * 「恢复整库备份」的确认弹窗（骨架照 RetentionConfirmDialog）。
 *
 * 恢复是**替换**语义：本机的账号、配置、日志、面板登录态会整体换成备份文件
 * 的状态。焦点落在「取消」—— 危险操作敲回车不该等于同意（与保留期确认同一
 * 取向）；确认后才弹系统文件框（壳侧 import_database）。
 */
function DbBackupConfirmDialog({ confirm }: { confirm: { includeLogs: boolean } | null }) {
  const cancelRef = React.useRef<HTMLButtonElement | null>(null)

  return (
    <Dialog open={confirm !== null} onOpenChange={next => { if (!next) resolveDbBackupConfirm(false) }}>
      <DialogContent className='w-[min(440px,calc(100vw-48px))]' initialFocus={cancelRef}>
        <DialogHeader>
          <DialogTitle>{t('恢复整库备份')}</DialogTitle>
        </DialogHeader>
        <DialogBody>
          <div className='danger-zone'>
            {t('本机的全部数据（账号、网关配置、日志统计与桌面设置）将被所选备份文件')}
            <strong>{t('整体替换')}</strong>{t('，包括恢复后需要重新登录管理面板。')}
            <br />
            <strong>{t('替换前会自动把当前数据备份到配置目录的 backups/ 文件夹。')}</strong>{t('确定继续？')}
          </div>
        </DialogBody>
        <DialogFooter>
          <div className='mr-auto' />
          <Button ref={cancelRef} variant='outline' onClick={() => resolveDbBackupConfirm(false)}>
            {t('取消')}
          </Button>
          <Button variant='destructive' onClick={() => resolveDbBackupConfirm(true)}>
            {t('选择备份文件并恢复')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

/**
 * 局域网访问的两段式弹窗（骨架照 RetentionConfirmDialog）：
 *   · 确认段（`confirm` 非空）：开 / 关 / 面板子开关各自的后果与「需重启」；
 *   · 注册段（`register` 非空）：确认开启但还没有管理员时接着出现的表单 ——
 *     就是确认文案里说的「注册管理员账号」那一步，注册成功由流程层直接继续
 *     开启（`submitLanRegister`），用户不需要再点一次确认。
 * 取消 / 右上角 ✕ / 点遮罩 / Esc 都算「不继续」：开关保持原状（受控组件自动弹回）。
 * 注册段焦点落在取消键：表单里有未提交的输入，敲回车该走提交而不是关闭。
 */
function LanDialog({ confirm, register }: { confirm: LanConfirm; register: LanRegister }) {
  const cancelRef = React.useRef<HTMLButtonElement | null>(null)
  const nameRef = React.useRef<HTMLInputElement | null>(null)
  const passwordRef = React.useRef<HTMLInputElement | null>(null)

  const open = confirm !== null || register !== null
  const close = () => {
    if (register !== null) cancelLanRegister()
    else resolveLanConfirm(false)
  }
  const submitRegister = () => {
    submitLanRegister(nameRef.current?.value ?? '', passwordRef.current?.value ?? '')
  }

  return (
    <Dialog open={open} onOpenChange={next => { if (!next) close() }}>
      <DialogContent className='w-[min(480px,calc(100vw-48px))]' initialFocus={cancelRef}>
        {register !== null ? (
          <>
            <DialogHeader>
              <DialogTitle>{t('注册管理员账号')}</DialogTitle>
            </DialogHeader>
            <DialogBody>
              <div>
                {t('局域网开放后，管理接口要求登录。请设置管理员账号与密码，注册完成会直接继续开启流程。')}
              </div>
              {/* 允许浏览器自带的账号密码记忆：autoComplete 与登录页同款 */}
              <form
                onSubmit={event => { event.preventDefault(); if (!register.busy) submitRegister() }}
              >
                <Label htmlFor='lan-admin-name' className='mt-3.5 mb-[5px] block text-[12.5px]'>{t('账号')}</Label>
                {/* 提交时壳侧按 ref 读值（submitLanRegister），ref 必须真的挂上 */}
                <Input ref={nameRef} id='lan-admin-name' autoComplete='username' placeholder={t('管理员账号')} disabled={register.busy} />
                <Label htmlFor='lan-admin-password' className='mt-3.5 mb-[5px] block text-[12.5px]'>{t('密码')}</Label>
                <Input
                  ref={passwordRef}
                  id='lan-admin-password'
                  type='password'
                  autoComplete='new-password'
                  placeholder={t('至少 8 位')}
                  disabled={register.busy}
                />
              </form>
              <div className='mt-3 min-h-5 text-[13px] whitespace-pre-wrap text-destructive'>
                {register.error}
              </div>
            </DialogBody>
            <DialogFooter>
              <div className='mr-auto' />
              <Button ref={cancelRef} variant='outline' onClick={cancelLanRegister} disabled={register.busy}>
                {t('取消')}
              </Button>
              <Button onClick={submitRegister} disabled={register.busy}>
                {register.busy ? t('正在注册…') : t('注册并开启')}
              </Button>
            </DialogFooter>
          </>
        ) : (
          <>
            <DialogHeader>
              <DialogTitle>{confirm ? LAN_CONFIRM_COPY[confirm.mode].title : ''}</DialogTitle>
            </DialogHeader>
            <DialogBody>
              <div>{confirm ? LAN_CONFIRM_COPY[confirm.mode].body(confirm.panel) : null}</div>
            </DialogBody>
            <DialogFooter>
              <div className='mr-auto' />
              <Button ref={cancelRef} variant='outline' onClick={() => resolveLanConfirm(false)}>
                {t('取消')}
              </Button>
              <Button onClick={() => resolveLanConfirm(true)}>
                {confirm ? LAN_CONFIRM_COPY[confirm.mode].label : t('继续')}
              </Button>
            </DialogFooter>
          </>
        )}
      </DialogContent>
    </Dialog>
  )
}

/* ─── 页面 ─────────────────────────────────── */

function SettingsPage() {
  const snap = useSettings()
  const panesRef = React.useRef<HTMLDivElement | null>(null)

  // 切换分类后把内容栏滚回顶部（旧实现是命令式写 scrollTop）：否则上一类的滚动位置会
  // 带到新分类上，打开「更新」却停在半截。scrollReset 每次 showCategory 都递增，
  // 于是「切回同一分类」（页面重入时 load → restoreCategory）同样会滚回顶部。
  React.useEffect(() => {
    const panes = panesRef.current
    if (panes) panes.scrollTop = 0
  }, [snap.scrollReset])

  const paneClass = (cat: string): string => (snap.category === cat ? 'settings-pane active' : 'settings-pane')

  return (
    <div className='settings-layout'>
      <nav className='settings-nav' id='settings-nav'>
        {CATEGORIES.map(item => (
          <button
            key={item.id}
            type='button'
            className={snap.category === item.id ? 'settings-nav-item active' : 'settings-nav-item'}
            data-cat={item.id}
            onClick={() => selectCategory(item.id)}
          >
            {/* 分类图标（icons.js）：与主侧栏同款 17px 图标盒，颜色随 currentColor
                （选中态自动变主题色） */}
            <span className='ico' dangerouslySetInnerHTML={{ __html: iconHtml(item.icon, 17) }} />
            {item.label}
          </button>
        ))}
      </nav>

      <div className='settings-panes' ref={panesRef}>
        <div className={paneClass('general')} data-cat='general'>
          <GeneralPane snap={snap} />
        </div>
        {/* AI 助手：三块面板（对话模型 / 系统提示词 / 工具勾选）。它自带一份独立的
            加载与保存（/api/assistant/config），不经过 settings-state 的快照 —— 见
            assistant-settings-pane.tsx 的模块头 */}
        <div className={paneClass('assistant')} data-cat='assistant'>
          <AssistantSettingsPane />
        </div>
        <div className={paneClass('display')} data-cat='display'>
          <DisplayPane />
        </div>
        <div className={paneClass('gateway')} data-cat='gateway'>
          <GatewayPane snap={snap} />
        </div>
        <div className={paneClass('routing')} data-cat='routing'>
          <RoutingPane snap={snap} />
        </div>
        <div className={paneClass('retry')} data-cat='retry'>
          <RetryPane snap={snap} />
        </div>
        <div className={paneClass('timeout')} data-cat='timeout'>
          <TimeoutPane snap={snap} />
        </div>
        <div className={paneClass('security')} data-cat='security'>
          <SecurityPane snap={snap} />
        </div>
        <div className={paneClass('data')} data-cat='data'>
          <DataPane snap={snap} />
        </div>
        <div className={paneClass('feedback')} data-cat='feedback'>
          <FeedbackPane />
        </div>
        {/*
          「更新」的面板（原「关于」，id 仍是 about）由另一个岛（update-panel.tsx）接管：
          它按这个选择器找挂载点，找到就把 React root 建在这个 div 上。所以这里必须是
          **空的**、且永远保持同一个元素（不给 children、不改它在兄弟中的位置、不条件渲染）——
          React 对没有 children 的宿主元素不会去动它的 DOM 子树，那个岛的渲染结果才留得住。
          显隐照旧：className 上的 active 由本文件按当前分类切。
        */}
        <div className={paneClass('about')} data-cat='about' />
      </div>

      <RetentionConfirmDialog confirm={snap.retentionConfirm} />
      <DbBackupConfirmDialog confirm={snap.dbBackupConfirm} />
      <LanDialog confirm={snap.lanConfirm} register={snap.lanRegister} />
    </div>
  )
}

/* ─── 挂载：接管 index.html 里既有的设置页 section ─── */

const PAGE_SELECTOR = '.page[data-page="settings"]'

let pageRoot: ReturnType<typeof createRoot> | null = null

/**
 * 把 React root 直接建在 `.page[data-page="settings"]` 上（不套宿主 div：页面 CSS 用
 * `.page[data-page="settings"]` 的直接子选择器分配高度与滚动归属）。
 *
 * ── 为什么必须 flushSync ──────────────────────
 * `.settings-pane[data-cat="about"]` 是 update-panel.tsx 的挂载点，它在**自己的模块加载期**
 * 就 `document.querySelector` 这个选择器，找到才建 root。React 19 的 `createRoot().render()`
 * 是并发调度，提交可能落在下一个宏任务 —— 那一刻这个 pane 还没进 DOM，update-panel 只会
 * 退化成等 DOMContentLoaded，而那时它早已过了注册窗口，「设置 → 更新」会整块空白。
 * 两个岛的求值顺序由 glob 的文件名字典序决定（settings-page.tsx 排在 update-panel.tsx 前），
 * 所以这里同步提交之后，它一定能查到。
 *
 * 先 replaceChildren()：React 不替我们清容器，留着静态骨架会与它的接管打架。
 */
function mount(): void {
  if (pageRoot) return
  const section = document.querySelector<HTMLElement>(PAGE_SELECTOR)
  if (!section) return
  section.replaceChildren()
  pageRoot = createRoot(section)
  flushSync(() => { pageRoot?.render(<SettingsPage />) })
}

// 首屏就按上次的选择展开（旧实现在模块加载期做同一件事），不必等 load() 回来；
// load() 里还会再校准一次，覆盖「页面切回来时状态被重置」的情况。
restoreCategory()

// 脚本排在页面骨架之后（index.html 里 islands/ui.js 在各 section 之后），正常直接挂；
// 万一将来被挪到前面，退化成等 DOM 解析完再挂。
if (document.querySelector(PAGE_SELECTOR)) mount()
else document.addEventListener('DOMContentLoaded', mount, { once: true })

/* ─── 注册：对外契约 ─────────────────────────── */

declare global {
  interface Window {
    /**
     * 设置页（替换 ui/settings-panel.js，九个方法与原实现逐字一致）。
     * 调用点：app.js:135 切到设置页时 load()；upgrade-panel.js:74 迁移完成后 load()；
     * update-panel.tsx:972 的「去更新」showCategory('about')。
     */
    wbSettingsPanel?: {
      load(): Promise<void>
      /** 铺启动与托盘设置（旧实现叫 renderSettings，对外名字是 render） */
      render(data?: unknown): void
      renderRetention(data?: unknown): void
      renderRetry(data?: unknown): void
      renderDebug(data?: unknown): void
      renderSanitize(data?: unknown): void
      renderPrompt(data?: unknown): void
      renderStorage(data?: unknown): void
      showCategory(category?: string | null): void
    }
  }
}

window.wbSettingsPanel = {
  load,
  render: renderSettings,
  renderRetention,
  renderRetry,
  renderDebug,
  renderSanitize,
  renderPrompt,
  renderStorage,
  showCategory,
}

// 首屏自持加载：app.js 的 showPage 在脚本加载前已执行过，若上次停留在设置页，
// 这里补一次加载，避免徽标一直停在「检测中…」
if (shared().wbApp?.currentPage === 'settings') void load()
