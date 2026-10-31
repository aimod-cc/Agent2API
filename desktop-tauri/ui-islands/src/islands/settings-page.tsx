import * as React from 'react'
import { providerPromptRows } from './settings-prompt-rows'
import { flushSync } from 'react-dom'
import { createRoot } from 'react-dom/client'
import {
  Badge,
  Button,
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Input,
  InputGroup,
  InputGroupInput,
  Label,
  NavList,
  NavItem,
  SegmentedControl,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Switch,
  CardHeader, Card, CardContent } from '@ui'
import {
  CATEGORIES,
  CAPTCHA_POOL_FIELDS,
  modelsDevStateText,
  NOTES,
  PROMPT_MODES,
  QUEUE_FIELDS,
  RETENTION_FIELDS,
  RETRY_FIELDS,
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
  type ThemeMode,
} from './settings-model'
import {
  addProviderPrompt,
  addRetryCode,
  applyUnits,
  cancelLanRegister,
  clearDegrade,
  closePasswordDialog,
  dropLastRetryCode,
  exportAccounts,
  importAccounts,
  load,
  openPasswordDialog,
  panelLogout,
  refreshDebug,
  refreshDeveloperRoleCompat,
  refreshClineHeaders,
  refreshZcodeVersion,
  refreshCors,
  refreshCaptchaPool,
  refreshModelsDev,
  refreshPrompt,
  refreshQueue,
  refreshRetention,
  refreshRetry,
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
  resolveLanConfirm,
  resolveRetentionConfirm,
  restoreCategory,
  saveCaptcha,
  saveClineHeaders,
  saveZcodeVersion,
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
  saveDeveloperRoleCompat,
  saveTimeoutField,
  saveToggle,
  selectCategory,
  showCategory,
  submitLanRegister,
  submitPasswordChange,
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
  type SettingsSnapshot,
  type StorageState,
} from './settings-state'
// 账号列表的两项偏好与账号页共用同一份 store：设置页写、账号页订阅，
// 因此两边读写必须走同一组函数（别在这里自己 localStorage.setItem 一份字面量）
import {
  readGroupTotalFiltered, readNamesHidden, setGroupTotalFiltered, setNamesHidden, subscribe,
} from './accounts-store'
import {
  GatewayTextButton,
  ProviderPromptTextButton,
  PromptTextButton,
  gatewayEditedText,
} from './settings-prompt-editor'
// 云同步面板（设置 → 数据）：独立成文件、自带一份数据流（/api/cloud-sync），
// 与设置页的快照 store 没有交集，见该文件的模块头
import { CloudSyncPanel } from './settings-cloud-sync'
import { DbRestoreDialog } from './settings-db-restore'
import { DatabaseBackupDialogs, DatabaseBackupPanel } from './settings-database-backup'
import { SanitizePanel } from './settings-sanitize'
// 侧边栏菜单面板（设置 → 显示）：显隐 / 顺序 / 分组。布局模型在
// ui/sidebar-layout.js（经典脚本，主侧边栏渲染与这里共用同一份），见该文件的模块头
import { SidebarPane } from './sidebar-pane'
import { t } from '../i18n'
import { Alert } from '@ui'

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
 * 问号的 `[data-tip]` 由公共提示桥接入组件库 Tooltip；数字框仅保留业务宽度约束。
 * 分类面板通过显隐保留各自草稿，不让切换分类重置编辑态。
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
    <CardHeader className='panel-head'>
      <h2>{title}</h2>
      {tip ? <span className='tip-q' data-tip={tip}></span> : null}
      {badge}
      {actions ? <div className='head-actions'>{actions}</div> : null}
    </CardHeader>
  )
}

/** 折叠不卸载正文，避免丢失尚未提交的设置草稿。 */
function CollapsiblePanel({ title, tip, badge, actions, children }: PanelHeadProps & { children: React.ReactNode }) {
  return <Collapsible render={<Card />} className='panel panel-fold group/fold'>
    <CardHeader className='panel-head'>
      <CollapsibleTrigger className='h-auto flex-1 justify-start gap-2 px-0 text-left'>
        <span className='transition-transform group-data-open/fold:rotate-90' aria-hidden dangerouslySetInnerHTML={{ __html: iconHtml('caretRight', 14) }} />
        <h2>{title}</h2>
      </CollapsibleTrigger>
      {tip ? <span className='tip-q' data-tip={tip} /> : null}
      {badge}
      {actions ? <div className='head-actions'>{actions}</div> : null}
    </CardHeader>
    <CollapsibleContent keepMounted>{children}</CollapsibleContent>
  </Collapsible>
}

type SwitchRowProps = {
  id: string
  label: string
  checked: boolean
  disabled?: boolean
  /** 行内问号提示（与 PanelHead 同款 `[data-tip]`，desktop-tooltip.tsx 统一服务） */
  tip?: string
  onCheckedChange: (next: boolean) => void
}

/**
 * 一行开关。开关与文字同在一个 `<label class="switch">` 里（与旧模板同构）：Base UI 的
 * Switch 会渲染一个隐藏 checkbox，label 的原生激活行为照样把点击转给它 —— 点文字也能拨动。
 */
function SwitchRow({ id, label, checked, disabled, tip, onCheckedChange }: SwitchRowProps) {
  return (
    <Label className='switch'>
      <Switch id={id} checked={checked} disabled={disabled} onCheckedChange={next => onCheckedChange(next)} />
      <span>{label}</span>
      {tip ? <span className='tip-q' data-tip={tip}></span> : null}
    </Label>
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
      <Label htmlFor={field.id}>{field.label}</Label>
      <span className='retention-input'>
        <Input
          id={field.id}
          type='number'
          min={field.min}
          max={field.max}
          step={1}
          inputMode='numeric'
          className={field.max > 99999
            ? 'w-[132px] max-w-[132px] text-center tabular-nums'
            : 'w-[92px] max-w-[92px] text-center tabular-nums'}
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
      <Card render={<section />} className='panel'>
        <PanelHead
          title={t('启动与托盘')}
          tip={TIPS.tray}
          badge={app.status === 'ready'
            ? <StatusBadge tone='ok'>{t('已应用')}</StatusBadge>
            : app.status === 'unavailable'
              ? <StatusBadge tone='bad'>{t('不可用')}</StatusBadge>
              : <StatusBadge tone='idle'>{t('检测中…')}</StatusBadge>}
        />
        <CardContent className='panel-body'>
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
        </CardContent>
      </Card>

      {!snap.panelLogin && (
        <Card render={<section />} className='panel'>
          <PanelHead title={t('局域网访问')} tip={TIPS.lan} />
          <CardContent className='panel-body'>
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
          </CardContent>
        </Card>
      )}

      <Card render={<section />} className='panel'>
        <PanelHead title={t('计量单位')} tip={TIPS.units} />
        <CardContent className='panel-body'>
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
        </CardContent>
      </Card>
    </>
  )
}

/* ─── 显示分类 ─────────────────────────────── */

/** 显示模式五档的状态行文案（五条文案在 settings-model 的 STATES 里） */
function themeStateText(mode: ThemeMode): string {
  if (mode === 'light') return STATES.themeLight
  if (mode === 'dark') return STATES.themeDark
  if (mode === 'maple-light') return STATES.themeMapleLight
  if (mode === 'maple-dark') return STATES.themeMapleDark
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
 * 跟随。主题只有本页这一个入口（侧边栏底部那组三键已移除），监听事件的理由
 * 只剩下头内联脚本的抢先应用与其它窗口的写入。
 *
 * 网页端：主题照常可用（data-theme 是纯 CSS 生效，窗口主题在 shim 里是空实现）；
 * 界面缩放禁用 —— 浏览器里的缩放归浏览器自己的 Ctrl +/- 管。
 */
function DisplayPane() {
  const [theme, setTheme] = React.useState<ThemeMode>(readThemeMode)
  const [zoom, setZoom] = React.useState<number>(readZoomPercent)
  // 账号列表的两项「怎么看这张表」偏好：合计口径与账号名掩码。
  // 与主题 / 缩放同属纯前端偏好（localStorage），不经过 settings-state 的快照。
  const [groupTotalFiltered, setGroupTotalFilteredState] = React.useState<boolean>(readGroupTotalFiltered)
  const [namesHidden, setNamesHiddenState] = React.useState<boolean>(readNamesHidden)
  // 端别取自桥（platform）而不是 navigator：与设置页其它端别判断同一口径
  const isWeb = shared().workbuddyDesktop?.platform === 'web'

  // 跟随别的入口的改动（index.html 头部内联脚本的抢先应用、其它窗口的写入）：
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

  // 账号列表的两项偏好：设置页写、账号页订阅同一份 store。这里同时听两路变化 ——
  //   · store 订阅：账号页表头那颗眼睛（toggleNamesHidden）/ 另一个入口写入时，
  //     本页的开关要跟着拨过去（同文档内的 localStorage 写入不触发 storage 事件，
  //     只靠那一路会漏）；
  //   · storage 事件：另一个窗口 / 另一份面板改了偏好。
  React.useEffect(() => {
    const sync = () => {
      setGroupTotalFilteredState(readGroupTotalFiltered())
      setNamesHiddenState(readNamesHidden())
    }
    window.addEventListener('storage', sync)
    const off = subscribe(sync)
    return () => {
      window.removeEventListener('storage', sync)
      off()
    }
  }, [])

  /** 改合计口径：落盘 + 通知账号页（同一个 store，订阅者一起重画） */
  function applyGroupTotalFiltered(next: boolean): void {
    setGroupTotalFiltered(next)
    setGroupTotalFilteredState(next)
  }

  /** 改账号名掩码：与账号页表头那颗眼睛走同一个写入函数 */
  function applyNamesHidden(next: boolean): void {
    setNamesHidden(next)
    setNamesHiddenState(next)
  }

  const zoomLabel = zoom === 100 ? t('100%（默认）') : `${zoom}%`

  return (
    <>
      <Card render={<section />} className='panel'>
        <PanelHead title={t('显示模式')} tip={TIPS.displayTheme} />
        <CardContent className='panel-body'>
          <div>
            <SegmentedControl<ThemeMode>
              aria-label={t('显示模式')}
              options={THEME_MODES}
              value={theme}
              onValueChange={next => shared().wbApp?.applyTheme?.(next)}
            />
          </div>
          <div className='settings-state'>{themeStateText(theme)}</div>
        </CardContent>
      </Card>

      <Card render={<section />} className='panel'>
        <PanelHead title={t('界面缩放')} tip={TIPS.displayZoom} />
        <CardContent className='panel-body'>
          <div className='retention-list'>
            <div className='retention-row'>
              <Label htmlFor='settings-zoom'>{t('缩放比例')}</Label>
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
        </CardContent>
      </Card>

      <Card render={<section />} className='panel'>
        <PanelHead title={t('语言设置')} tip={TIPS.displayLanguage} />
        <CardContent className='panel-body'>
          <LanguageRow />
          <div className='settings-state'>{STATES.languageHint}</div>
        </CardContent>
      </Card>

      {/* 账号列表：合计口径 / 账号名掩码。与上面三项同属「怎么看这张表」的纯前端
          偏好（localStorage，见 accounts-store 的 GROUP_TOTAL_FILTERED_KEY 与
          NAMES_HIDDEN_KEY）—— 不碰转发、不碰账号数据，所以也不进 settings-state
          的后端快照。账号页有自己的入口改这两项（表头那颗眼睛、合计徽标本身），
          两边读写的是同一份键。 */}
      <Card render={<section />} className='panel'>
        <PanelHead title={t('账号列表')} tip={TIPS.accountList} />
        <CardContent className='panel-body'>
          <div className='settings-switches'>
            <SwitchRow
              id='settings-group-total-filtered'
              label={t('积分合计只统计当前筛选出来的账号')}
              checked={groupTotalFiltered}
              onCheckedChange={applyGroupTotalFiltered}
            />
            <SwitchRow
              id='settings-names-hidden'
              label={t('隐藏账号名（显示成星号）')}
              checked={namesHidden}
              onCheckedChange={applyNamesHidden}
            />
          </div>
          <div className='settings-state'>
            {groupTotalFiltered ? STATES.groupTotalFilteredOn : STATES.groupTotalFilteredOff}
          </div>
        </CardContent>
      </Card>

      {/* 侧边栏菜单：显隐 / 顺序 / 分组。权威副本在库里（kv 的 sidebarLayout
          键，经 /api/sidebar-layout 读写），localStorage 只是首帧缓存 ——
          它走自己的键、不经过 settings-state 的快照，所以放在「显示」分类里。
          布局的唯一真相在 ui/sidebar-layout.js 那个经典脚本里 ——
          app.js 渲染主侧边栏时读的是同一份。 */}
      <SidebarPane />
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
        <Label htmlFor='settings-language'>{t('界面语言')}</Label>
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
      <Label htmlFor='settings-retry-no-codes-input'>{t('指定错误码直接换号')}</Label>
      <div className='retention-input'>
        {/* 点框体空白处 = 聚焦输入框（整框是一个输入控件的观感，旧实现同） */}
        <InputGroup
          className={`h-auto w-[320px] max-w-full flex-wrap gap-1.5 px-2 py-1${locked ? ' opacity-55' : ''}`}
          onClick={() => inputRef.current?.focus()}
        >
          {(codes || []).map(code => (
            <Badge variant='brand' className='tag-chip' key={code}>
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
            </Badge>
          ))}
          <InputGroupInput
            ref={inputRef}
            id='settings-retry-no-codes-input'
            type='text'
            className='h-6 min-w-[120px] flex-1 px-0'
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
        </InputGroup>
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
      <Label htmlFor='settings-prompt-file'>{t('提示词文件')}</Label>
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
      <Label className='prompt-gateway-label'>{t('网关自带')}</Label>
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

/** 只列已保存的模式、网关开关或正文覆盖；未配置的提供商从下拉手动添加。 */
function ProviderPromptRows({ prompt, locked, busy }: {
  prompt: PromptState
  locked: boolean
  busy: boolean
}) {
  const options = prompt.options
  const rows = providerPromptRows(prompt)
  const listed = new Set(rows.map(row => row.id))
  const candidates = options.filter(item => !listed.has(item.id))
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
              <Label htmlFor={`settings-prompt-mode-${item.id}`}>{label}</Label>
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
        <Label htmlFor='settings-prompt-provider-add'>{t('添加提供商')}</Label>
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
    <Card render={<section />} className='panel'>
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
      <CardContent className='panel-body'>
        <div className='retention-list'>
          {/* 两个分组条把「默认」与「某家的例外」分开：上面那几行是所有未单独配置的
              家共用的默认值，下面那张表是逐家的例外 —— 不分开时它们是一串同构的行，
              用户读不出哪几行管全部、哪几行只管一家（见 page-settings.css 的 .prompt-group）。 */}
          <div className='prompt-group'>
            {t('全局配置')}<span className='note'>{t('所有未单独配置的提供商都用这一份')}</span>
          </div>

          <div className='retention-row'>
            <Label htmlFor='settings-prompt-mode'>{t('模式')}</Label>
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
            <Label htmlFor='btn-prompt-edit-body'>{t('提示词正文')}</Label>
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
              <Label>{t('内容拦截降级')}</Label>
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
      </CardContent>
    </Card>
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
    <Card render={<section />} className='panel'>
      <PanelHead
        title={t('请求超时')}
        tip={TIPS.timeouts}
        badge={numericBadge(snap.timeouts)}
        actions={<RefreshButton id='btn-timeouts-refresh' onClick={() => void refreshTimeouts()} />}
      />
      <CardContent className='panel-body'>
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
      </CardContent>
    </Card>
  )
}

/** 请求重试：数字参数 + 「指定错误码」名单（两段共用同一份保存流程） */
function RetryPane({ snap }: { snap: SettingsSnapshot }) {
  return (
    <Card render={<section />} className='panel'>
      <PanelHead
        title={t('请求重试')}
        tip={TIPS.retry}
        badge={numericBadge(snap.retry)}
        actions={<RefreshButton id='btn-retry-refresh' onClick={() => void refreshRetry()} />}
      />
      <CardContent className='panel-body'>
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
      </CardContent>
    </Card>
  )
}

/* ─── 网关分类 ─────────────────────────────── */

function GatewayPane({ snap, panels }: { snap: SettingsSnapshot; panels: Set<string> }) {
  return (
    <>
      <Card render={<section />} className='panel'>
        <PanelHead
          title={t('排队等待')}
          tip={TIPS.queue}
          badge={numericBadge(snap.queue)}
          actions={<RefreshButton id='btn-queue-refresh' onClick={() => void refreshQueue()} />}
        />
        <CardContent className='panel-body'>
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
        </CardContent>
      </Card>

      {/* 验证码令牌池：池子本体在 ZCode 插件里，网关只有节奏设置。
          插件没申报（没装 / 没装 ZCode）时这块不出现。 */}
      {panels.has('captcha_pool') ? (
        <Card render={<section />} className='panel'>
          <PanelHead
            title={t('验证码令牌池')}
            tip={TIPS.captchaPool}
            badge={numericBadge(snap.captchaPool)}
            actions={
              <RefreshButton id='btn-captcha-pool-refresh' onClick={() => void refreshCaptchaPool()} />
            }
          />
          <CardContent className='panel-body'>
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
          </CardContent>
        </Card>
      ) : null}

      <Card render={<section />} className='panel'>
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
        <CardContent className='panel-body'>
          <SwitchRow
            id='settings-modelsdev'
            label={t('用 models.dev 补齐上游没声明的模型能力与思考档位（联网刷新，内嵌快照始终生效）')}
            checked={snap.modelsDev.enabled}
            disabled={snap.modelsDev.status !== 'ready' || snap.busy === 'modelsDev'}
            onCheckedChange={next => void saveModelsDev(next)}
          />
          <div className='settings-state'>{modelsDevStateText(snap.modelsDev)}</div>
          <div className='hint retention-note'>{NOTES.modelsDev}</div>
        </CardContent>
      </Card>

      <SanitizePanel snap={snap} />

      <Card render={<section />} className='panel'>
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
        <CardContent className='panel-body'>
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
        </CardContent>
      </Card>

      {/* Cline 伪装头：实现与默认值都在插件里，插件没申报就不画。
          放在「网关」分类下是历史位置，语义上它属于插件面板。 */}
      {panels.has('upstream_headers') ? <ClineHeadersPanel snap={snap} /> : null}

      {/* ZCode 客户端版本号：上游按它决定活动套餐下不下发。与上面那块不同，
          这一格**不依赖插件申报** —— 它是网关自己的配置（见
          api::zcode_version 的模块头），插件没装也要能改。 */}
      <ZcodeVersionPanel snap={snap} />

      <PromptPanel snap={snap} />

      <Card render={<section />} className='panel'>
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
        <CardContent className='panel-body'>
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
        </CardContent>
      </Card>
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
      <CardContent className='panel-body'>
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
      </CardContent>
    </CollapsiblePanel>
  )
}

/**
 * ZCode 客户端版本号（上游按它决定活动套餐下不下发）。
 *
 * ── 为什么需要用户自己填 ────────────────────────────────────
 * 活动套餐的探测接口只在 `app_version` 达到当期门槛时才返回套餐列表；
 * 版本落后时上游回的是一份**空列表**（`code: 0`，不是错误）。于是症状是
 * 「活动明明在，界面上一片空白」—— 看起来像上游没这活动，实际是客户端
 * 被判太旧（2026-10 实测：3.16.0 及以下全空，3.16.1 起才有 1024 礼包）。
 *
 * 插件里那份默认值是编译期常量，每期活动都要跟着改、跟着发版。这一格是
 * 那条链的旁路：用户在这里填一个更新的版本号即可，不必等我们发版。
 */
function ZcodeVersionPanel({ snap }: { snap: SettingsSnapshot }) {
  const state = snap.zcodeVersion
  const [draft, setDraft] = React.useState(state.version)
  React.useEffect(() => { setDraft(state.version) }, [state.version])

  const busy = snap.busy === 'zcodeVersion'
  const ready = state.status === 'ready'
  const dirty = ready && draft.trim() !== state.version
  // 环境变量优先级最高（见后端模块头）：它压着时界面上的值不生效，
  // 必须说出来，否则用户会对着一个「改了没反应」的输入框反复重试。
  const envShadowed = state.envOverride !== ''

  return (
    <CollapsiblePanel
      title={t('ZCode 客户端版本号')}
      tip={TIPS.zcodeVersion}
      badge={state.status === 'ready'
        ? <StatusBadge tone={envShadowed ? 'idle' : 'ok'}>
          {envShadowed ? t('被环境变量覆盖') : t('已生效')}
        </StatusBadge>
        : state.status === 'unavailable'
          ? <StatusBadge tone='bad'>{t('不可用')}</StatusBadge>
          : <StatusBadge tone='idle'>{t('检测中…')}</StatusBadge>}
      actions={<RefreshButton id='btn-zcode-version-refresh' onClick={() => void refreshZcodeVersion()} />}
    >
      <CardContent className='panel-body'>
        <div className='retention-list'>
          <div className='flex items-center gap-2'>
            <Input
              id='settings-zcode-version'
              className='w-[190px] shrink-0 font-mono text-[13px]'
              value={draft}
              disabled={busy || !ready}
              placeholder='3.16.2'
              onChange={event => setDraft(event.target.value)}
            />
            <div className='flex-1' />
            <Button
              variant='ghost'
              disabled={busy || !ready || draft === ''}
              onClick={() => void saveZcodeVersion('')}
            >
              {t('清空（用内置默认值）')}
            </Button>
            <Button disabled={busy || !ready || !dirty} onClick={() => void saveZcodeVersion(draft)}>
              {t('保存')}
            </Button>
          </div>
        </div>
        <div className='settings-state'>
          {state.status === 'loading'
            ? STATES.appLoading
            : state.status === 'unavailable'
              ? t('未能读取 ZCode 版本号设置，请稍后重试')
              : envShadowed
                ? t('环境变量 ZCODE_APP_VERSION={value} 正在生效，上面填的值暂不生效', { value: state.envOverride })
                : draft.trim()
                  ? t('探测与领取时会用 {version} 作为客户端版本号', { version: draft.trim() })
                  : t('未设置，使用插件内置的默认版本号')}
        </div>
        <div className='hint retention-note'>{NOTES.zcodeVersion}</div>
      </CardContent>
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
      <Card render={<section />} className='panel'>
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
        <CardContent className='panel-body'>
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
        </CardContent>
      </Card>

      {/* 机器人校验搬到「账户」分类（与管理员账号、登录开关同层）。
          这里保留「退出登录」—— 它属于「当前这一端的会话」，与安全分类的
          其它项（跨域、脱敏）不是一类。 */}
      {/* 面板登录：仅网页端（桌面壳的面板跟着应用走，没有「登录面板」的概念） */}
      {snap.panelLogin ? (
        <Card render={<section />} className='panel' id='panel-login-section'>
          <PanelHead title={t('面板登录')} />
          <CardContent className='panel-body'>
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
          </CardContent>
        </Card>
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

/**
 * 「账户」分类：管理员账号（改密码）、机器人校验（两端各一张开关）、重置说明。
 *
 * ── 为什么登录开关做成只读 ────────────────────────────────────
 * 「是否要求登录」由「库里有没有管理员」决定（见 `access::panel_auth_enabled`），
 * 不是一个独立开关。做成可拨的话，用户关掉它就是在删除管理员记录 —— 那与
 * 「重置管理员」是同一个动作，只是换了名字，却会让人以为「只是暂时关掉」。
 * 所以这里只显示状态，要关就按下面的重置说明走，语义清楚、后果可见。
 */
function AccountPane({ snap }: { snap: SettingsSnapshot }) {
  const account = snap.account
  const captcha = snap.captcha
  const switchDisabled = !captcha.available || snap.busy === 'captcha'
  // 本进程是哪一端：用于给两张开关加「正在使用」标记
  const here = captcha.localPanel ? 'desktop' : 'web'

  return (
    <>
      <Card render={<section />} className='panel'>
        <PanelHead
          title={t('管理员账号')}
          badge={
            account.status === 'loading'
              ? <StatusBadge tone='idle'>{t('检测中…')}</StatusBadge>
              : account.status === 'unavailable'
                ? <StatusBadge tone='bad'>{t('不可用')}</StatusBadge>
                : account.registered
                  ? <StatusBadge tone='ok'>{t('已启用登录')}</StatusBadge>
                  : <StatusBadge tone='idle'>{t('未注册')}</StatusBadge>
          }
          actions={
            account.registered ? (
              <Button
                id='btn-account-change-password'
                variant='outline'
                onClick={() => openPasswordDialog()}
              >
                {t('修改密码')}
              </Button>
            ) : null
          }
        />
        <CardContent className='panel-body'>
          <div className='settings-state' id='account-username'>
            {account.status === 'unavailable'
              ? t('未能读取账户信息，请稍后重试')
              : account.registered
                ? t('当前管理员：{name}', { name: account.username || '—' })
                : t('尚未注册管理员：管理接口不需要登录。开启「允许局域网访问」时会引导注册，也可以从登录页注册。')}
          </div>
          {account.registered ? (
            <div className='hint'>
              {t('修改密码需要输入当前密码；修改成功后，其他设备 / 浏览器上的登录会立即失效，当前这条会话保留。')}
            </div>
          ) : null}
        </CardContent>
      </Card>

      <Card render={<section />} className='panel'>
        <PanelHead
          title={t('机器人校验')}
          tip={TIPS.captchaSides}
          badge={!captcha.available
            ? <StatusBadge tone='bad'>{t('不可用')}</StatusBadge>
            : null}
        />
        <CardContent className='panel-body'>
          <div className='settings-switches'>
            <SwitchRow
              id='settings-captcha-desktop'
              label={here === 'desktop'
                ? t('桌面端：登录 / 注册需要 ALTCHA 人机验证（当前正在使用）')
                : t('桌面端：登录 / 注册需要 ALTCHA 人机验证')}
              checked={captcha.desktop}
              disabled={switchDisabled}
              onCheckedChange={next => void saveCaptcha('desktop', next)}
            />
            <SwitchRow
              id='settings-captcha-web'
              label={here === 'web'
                ? t('网页端：登录 / 注册需要 ALTCHA 人机验证（当前正在使用）')
                : t('网页端：登录 / 注册需要 ALTCHA 人机验证')}
              checked={captcha.web}
              disabled={switchDisabled}
              onCheckedChange={next => void saveCaptcha('web', next)}
            />
          </div>
          <div className='hint'>{NOTES.captcha}</div>
        </CardContent>
      </Card>

      <Card render={<section />} className='panel'>
        <PanelHead title={t('重置管理员')} />
        <CardContent className='panel-body'>
          <div className='hint'>
            {t('忘记密码时无法在界面里找回：需要停掉程序后，从数据库删除管理员记录（kv 表的 panelAdmin 键），重启后按首次使用重新注册。具体命令与各形态的库文件路径见 docs 目录下的「重置管理员」文档。')}
          </div>
          <Alert variant='destructive' className='danger-zone'>
            <strong>{t('重置会清空管理员账号与密码，面板将回到「未注册」状态。')}</strong>
          </Alert>
        </CardContent>
      </Card>
    </>
  )
}

function DataPane({ snap }: { snap: SettingsSnapshot }) {
  const io = snap.busy
  const storage = snap.storage
  // 库打不开时各计数都是后端回落出来的 0，与「真的没有数据」在数字上无法区分 ——
  // 整排显示「—」，不误导用户以为数据丢了；路径仍然照显（文件位置是已知的）
  const ready = storage.status === 'ready'
  const counts = ready && storage.available
  const path = ready && storage.file ? storage.file : '—'
  const count = (value: number | null): string => (counts && value !== null ? formatCount(value) : '—')

  return (
    <>
      {/* 云同步（对标 cc-switch 的 WebDAV / S3 同步区块）：独立成文件、自带
          数据流（它只读写 /api/cloud-sync，与设置页其它面板没有状态交集）。
          排在「整库备份 / 恢复」之前 —— 两者都是「整份数据搬来搬去」的语义，
          云同步更常用，放上面。 */}
      <CloudSyncPanel />

      <DatabaseBackupPanel />

      <Card render={<section />} className='panel'>
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
        <CardContent className='panel-body'>
          <Alert variant='destructive' className='danger-zone'>
            <strong>{t('导出文件内含 accessToken / refreshToken / apiKey 等凭证与自定义提供商定义')}</strong>
            {t('，可直接用于登录。请妥善保管，不要外传或上传到公共位置。')}
          </Alert>
          {/* 失败明细（旧实现写 innerHTML 并 display:none 收起空结果，这里条件渲染） */}
          {snap.ioFailure ? (
            <div className='io-result'>
              <span className='text-destructive'>
                {t('失败 {n} 个：{detail}', { n: snap.ioFailure.failed, detail: snap.ioFailure.detail })}
                {snap.ioFailure.more ? t(' 等') : ''}
              </span>
            </div>
          ) : null}
        </CardContent>
      </Card>

      <Card render={<section />} className='panel'>
        <PanelHead
          title={t('数据保留')}
          tip={TIPS.retention}
          badge={numericBadge(snap.retention)}
          actions={<RefreshButton id='btn-retention-refresh' onClick={() => void refreshRetention()} />}
        />
        <CardContent className='panel-body'>
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
        </CardContent>
      </Card>

      <Card render={<section />} className='panel'>
        <PanelHead
          title={t('数据存储')}
          tip={TIPS.storage}
          badge={storageBadge(storage)}
          actions={<RefreshButton id='btn-storage-refresh' onClick={() => void refreshStorage()} />}
        />
        <CardContent className='panel-body'>
          <div className='retention-list'>
            <div className='retention-row'>
              <Label htmlFor='storage-db-path'>{t('数据库文件')}</Label>
              <span className='storage-line'>
                {/* 悬停看完整路径（元素上是折行显示的，长路径会被截成好几行） */}
                <span className='storage-path' id='storage-db-path' title={path}>{path}</span>
              </span>
              <div className='hint'>{NOTES.storageFile}</div>
            </div>
            <div className='retention-row'>
              <Label htmlFor='storage-db-size'>{t('占用大小')}</Label>
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
            <StorageCount label={t('上游调试报文')} value={count(storage.upstreamRaw)} />
            <StorageCount label={t('下游正文')} value={count(storage.downstreamRaw)} />
          </div>
          <div className='hint retention-note'>{NOTES.storage}</div>
        </CardContent>
      </Card>
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
    url: 'https://github.com/aimod-cc/Agent2API/issues/new?template=bug_report.yml',
  },
  {
    title: t('功能建议'),
    desc: t('想要的新功能或改进想法'),
    cta: t('提建议'),
    url: 'https://github.com/aimod-cc/Agent2API/issues/new?template=feature_request.yml',
  },
  {
    title: t('请求提供商 / 模型支持'),
    desc: t('希望接入新的提供商或模型'),
    cta: t('去申请'),
    url: 'https://github.com/aimod-cc/Agent2API/issues/new?template=provider_request.yml',
  },
] as const

function FeedbackPane() {
  return (
    <Card render={<section />} className='panel'>
      <PanelHead title={t('反馈与需求')} />
      <CardContent className='panel-body'>
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
      </CardContent>
    </Card>
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
          <DialogTitle>{t('确认缩减数据保留')}</DialogTitle>
        </DialogHeader>
        <DialogBody>
          <Alert variant='destructive' className='danger-zone'>
            {confirm?.head}
            <br />
            <strong>{t('超出的历史数据会被立即删除，且不可恢复。')}</strong>{t('确定继续？')}</Alert>
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
 * 「修改密码」弹窗：当前密码 + 新密码 + 确认新密码。
 *
 * 三个字段都是**非受控**再取 ref 读值？不 —— 这里用受控 state：需要在提交前
 * 比较「两次新密码是否一致」，而那一比发生在点击那一刻，读 ref 与读 state 等价，
 * 但受控写法能让「两次不一致」的提示在输入时就消失（错误文案清空）。
 * 焦点落在「取消」：改密码不该因为敲回车就提交（与危险操作确认框同一取向）。
 */
function PasswordDialog({ password }: { password: { busy: boolean; error: string } | null }) {
  const cancelRef = React.useRef<HTMLButtonElement | null>(null)
  const [oldValue, setOldValue] = React.useState('')
  const [newValue, setNewValue] = React.useState('')
  const [repeat, setRepeat] = React.useState('')
  const open = password !== null
  const busy = password?.busy === true

  // 关窗即清空三个输入：密码不该留在组件的 state 里等下一次打开
  React.useEffect(() => {
    if (!open) {
      setOldValue('')
      setNewValue('')
      setRepeat('')
    }
  }, [open])

  const mismatch = repeat.length > 0 && newValue !== repeat

  function submit(): void {
    if (busy) return
    if (mismatch) return
    void submitPasswordChange(oldValue, newValue)
  }

  return (
    <Dialog open={open} onOpenChange={next => { if (!next) closePasswordDialog() }}>
      <DialogContent className='w-[min(440px,calc(100vw-48px))]' initialFocus={cancelRef}>
        <DialogHeader>
          <DialogTitle>{t('修改密码')}</DialogTitle>
        </DialogHeader>
        <DialogBody>
          {/* form 让浏览器的密码管理器能识别「旧密码 → 新密码」这一组 */}
          <form
            onSubmit={event => { event.preventDefault(); submit() }}
            autoComplete='on'
          >
            <Label htmlFor='account-old-password' className='mb-[5px] block text-[12.5px]'>
              {t('当前密码')}
            </Label>
            <Input
              id='account-old-password'
              type='password'
              autoComplete='current-password'
              placeholder={t('输入当前登录用的密码')}
              value={oldValue}
              disabled={busy}
              onChange={event => setOldValue(event.target.value)}
            />
            <Label htmlFor='account-new-password' className='mt-3.5 mb-[5px] block text-[12.5px]'>
              {t('新密码')}
            </Label>
            <Input
              id='account-new-password'
              type='password'
              autoComplete='new-password'
              placeholder={t('至少 8 位')}
              value={newValue}
              disabled={busy}
              onChange={event => setNewValue(event.target.value)}
            />
            <Label htmlFor='account-repeat-password' className='mt-3.5 mb-[5px] block text-[12.5px]'>
              {t('确认新密码')}
            </Label>
            <Input
              id='account-repeat-password'
              type='password'
              autoComplete='new-password'
              placeholder={t('再输入一次新密码')}
              value={repeat}
              disabled={busy}
              onChange={event => setRepeat(event.target.value)}
            />
          </form>
          <div className='mt-3 min-h-5 text-[13px] whitespace-pre-wrap text-destructive'>
            {mismatch ? t('两次输入的新密码不一致') : (password?.error || '')}
          </div>
        </DialogBody>
        <DialogFooter>
          <div className='mr-auto' />
          <Button ref={cancelRef} variant='outline' onClick={() => closePasswordDialog()} disabled={busy}>
            {t('取消')}
          </Button>
          <Button onClick={submit} disabled={busy || mismatch || !oldValue || !newValue || !repeat}>
            {busy ? t('正在修改…') : t('修改密码')}
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

/**
 * 当前已装插件申报了哪些管理面板。
 *
 * ── 为什么设置页要问插件 ────────────────────────────────────
 * 伪装头编辑器这类面板的**实现**在插件里（走 `admin.action`），网关只存
 * 用户改过的键。插件没装时它既没有默认值也没有合并语义，面板点开是一屏
 * 「读取失败」。所以按插件的申报决定渲染：没申报就整块不出现。
 *
 * ── 为什么不用 useSettings 的快照 ──────────────────────────
 * 那份快照来自 `getAppSettings`（应用设置），面板声明是插件面的数据，
 * 两者来源不同。单独拉一次 `/api/plugins` 更直白，且失败时按「没有面板」
 * 处理 —— 宁可少画，也不要画一个读不出数的空面板。
 *
 * 依赖 `category`：切到「网关」分类时重拉一次，用户刚装完插件切回来
 * 就能看到新面板。
 */
function useAdminPanels(category: string): Set<string> {
  const [panels, setPanels] = React.useState<Set<string>>(() => new Set())
  React.useEffect(() => {
    let cancelled = false
    void (async () => {
      try {
        const internals = (window as unknown as {
          __TAURI_INTERNALS__?: {
            invoke?: (command: string, args?: unknown) => Promise<unknown>
          }
        }).__TAURI_INTERNALS__
        if (!internals?.invoke) return
        const result = await internals.invoke('api_request', {
          request: { method: 'GET', path: '/api/plugins', body: null },
        })
        const envelope = result as { data?: { adminPanels?: string[] } } | null
        const list = envelope?.data?.adminPanels
        if (!cancelled && Array.isArray(list)) setPanels(new Set(list))
      } catch {
        // 读不到按「没有面板」处理：隐藏比画一个空面板好
      }
    })()
    return () => { cancelled = true }
  }, [category])
  return panels
}

function SettingsPage() {
  const snap = useSettings()
  // 插件申报的管理面板（见 useAdminPanels 的说明）
  const adminPanels = useAdminPanels(snap.category)
  const panesRef = React.useRef<HTMLDivElement | null>(null)
  /**
   * 窄屏「一级菜单 / 二级页面」的当前层。
   *
   * 桌面与平板（≥768px）永远是 false：左栏导航常驻、右侧直接显示选中分类，
   * 与改动前逐像素一致 —— 这条状态只在 CSS 的 767px 断点内起作用，
   * 桌面端由 `.settings-layout.is-detail` 的规则排除，不产生任何视觉差异。
   *
   * 手机上的语义：false = 分类列表（一级），true = 某个分类的设置面板（二级）。
   * 点分类进二级、点返回回一级。初始为 false（每次进设置页都从列表开始，
   * 不沿用上次停留的分类 —— 手机上「上次看的那一页」没有上下文，列表更符合预期）。
   */
  const [mobileDetail, setMobileDetail] = React.useState(false)

  // 切换分类后把内容栏滚回顶部（旧实现是命令式写 scrollTop）：否则上一类的滚动位置会
  // 带到新分类上，打开「更新」却停在半截。scrollReset 每次 showCategory 都递增，
  // 于是「切回同一分类」（页面重入时 load → restoreCategory）同样会滚回顶部。
  React.useEffect(() => {
    const panes = panesRef.current
    if (panes) panes.scrollTop = 0
  }, [snap.scrollReset])

  const paneClass = (cat: string): string => (snap.category === cat ? 'settings-pane active' : 'settings-pane')
  const activeCategory = CATEGORIES.find(item => item.id === snap.category) || CATEGORIES[0]

  return (
    <div className={mobileDetail ? 'settings-layout is-detail' : 'settings-layout'}>
      <NavList render={<nav />} className='settings-nav' id='settings-nav'>
        {CATEGORIES.map(item => (
          <NavItem
            key={item.id}
            active={snap.category === item.id}
            className={snap.category === item.id ? 'settings-nav-item active' : 'settings-nav-item'}
            data-cat={item.id}
            icon={<span className='ico' dangerouslySetInnerHTML={{ __html: iconHtml(item.icon, 17) }} />}
            onClick={() => {
              selectCategory(item.id)
              setMobileDetail(true)
            }}
          >
            {item.label}
          </NavItem>
        ))}
      </NavList>

      {/* 二级页顶部返回栏：只在手机（≤767px）且已进入某一分类时显示，
          桌面由 CSS 排除。文案里的分类名跟着当前分类走，让用户知道自己在哪。
          图标复用 caretRight 并水平翻转（icons.js 里没有专用的 back 箭头，
          为这一个按钮新增图标不值得）。 */}
      <div className='settings-back-bar'>
        <Button size='default' variant='ghost' type='button' className='settings-back-btn' onClick={() => setMobileDetail(false)}>
          <span className='ico' dangerouslySetInnerHTML={{ __html: iconHtml('caretRight', 17) }} />
          {t('返回')}
        </Button>
        <span className='settings-back-title'>{activeCategory.label}</span>
      </div>

      <div className='settings-panes' ref={panesRef}>
        <div className={paneClass('general')} data-cat='general'>
          <GeneralPane snap={snap} />
        </div>
        {/* AI 助手的配置已搬到助手页自己的「助手设置」抽屉
            （assistant-settings-drawer.tsx）—— 换模型 / 调工具都是聊天当中会做的
            事，留在设置页意味着每次都要离开对话再切回来。 */}
        <div className={paneClass('display')} data-cat='display'>
          <DisplayPane />
        </div>
        <div className={paneClass('gateway')} data-cat='gateway'>
          <GatewayPane snap={snap} panels={adminPanels} />
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
        <div className={paneClass('account')} data-cat='account'>
          <AccountPane snap={snap} />
        </div>
        <div className={paneClass('data')} data-cat='data'>
          <DataPane snap={snap} />
        </div>
        <div className={paneClass('feedback')} data-cat='feedback'>
          <FeedbackPane />
        </div>
        {/*
          「版本信息」同样由独立岛接管（system-info-pane.tsx，选择器
          `.settings-pane[data-cat="system"]`）。约束与下面「更新」那条完全一样：
          必须是**空的**、且永远保持同一个元素 —— 那个岛在自己的模块加载期就把
          React root 建在这个 div 上，React 这边只要不去动它的子树，渲染结果就留得住。
          拆成独立岛而不是塞进本文件：本文件已经 2300 多行，而这块与设置项
          没有任何状态交集（它自己拉 `/api/system-info`）。
        */}
        <div className={paneClass('system')} data-cat='system' />
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
      <DatabaseBackupDialogs />
      {/* 恢复进度弹窗：**不可关闭**，直到恢复收口（见该文件的说明） */}
      <DbRestoreDialog />
      <PasswordDialog password={snap.account.password} />
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
