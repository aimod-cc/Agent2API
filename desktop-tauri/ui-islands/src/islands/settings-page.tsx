import * as React from 'react'
import { mountOnPage } from '../island-lazy'
import { flushSync } from 'react-dom'
import { createRoot } from 'react-dom/client'
import {
  Avatar,
  AvatarFallback,
  Badge,
  BadgeDot,
  Button,
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Field,
  FieldLabel,
  Input,
  InputGroup,
  InputGroupInput,
  Item,
  ItemActions,
  ItemContent,
  ItemDescription,
  ItemMedia,
  ItemTitle,
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
  CardHeader,
  Card,
  CardContent,
  CardTitle,
  cn,
} from '@ui'
import { ChevronDownIcon, KeyIcon } from './settings-account-ui'
import {
  CATEGORIES,
  CAPTCHA_POOL_FIELDS,
  IMAGE_COMPRESS_FIELDS,
  modelsDevStateText,
  NOTES,
  QUEUE_FIELDS,
  RETENTION_FIELDS,
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
  toast,
  type NumberField,
  type ThemeMode,
} from './settings-model'
import {
  refreshExchangeRate,
  switchDisplayCurrency,
  usePricingDisplay,
} from './model-pricing'
import {
  applyUnits,
  closePasswordDialog,
  exportAccounts,
  importAccounts,
  load,
  openPasswordDialog,
  panelLogout,
  refreshDebug,
  refreshDeveloperRoleCompat,
  refreshReasoningCollapse,
  refreshImageCompress,
  refreshClineHeaders,
  refreshCors,
  refreshCaptchaPool,
  refreshModelsDev,
  refreshQueue,
  refreshRetention,
  refreshStorage,
  refreshTimeouts,
  renderDebug,
  renderCors,
  renderPrompt,
  renderRetention,
  renderRetry,
  renderSanitize,
  renderSettings,
  renderStorage,
  resolveRetentionConfirm,
  restoreCategory,
  saveClineHeaders,
  saveCors,
  saveCaptchaPoolField,
  saveDebug,
  saveImageCompressEnabled,
  saveImageCompressField,
  saveModelsDev,
  saveQueueField,
  saveRetentionField,
  saveDeveloperRoleCompat,
  saveReasoningCollapse,
  saveTimeoutField,
  saveToggle,
  selectCategory,
  showCategory,
  submitPasswordChange,
  useSettings,
  type ClineHeadersState,
  type DebugState,
  type LoadStatus,
  type NumericState,
  type SettingsSnapshot,
  type StorageState,
} from './settings-state'
// 账号列表的两项偏好与账号页共用同一份 store：设置页写、账号页订阅，
// 因此两边读写必须走同一组函数（别在这里自己 localStorage.setItem 一份字面量）
import {
  readGroupTotalFiltered, readNamesHidden, setGroupTotalFiltered, setNamesHidden, subscribe,
} from './accounts-store'
import { useLoginNoAutoOpen } from '../../../ui-shared/login-browser-prefs'
import { PromptSettingsPanel } from '../../../ui-shared/settings-prompt-panel'
import { promptRulesApi } from './settings-prompt-api'
// 云同步面板（设置 → 数据）：独立成文件、自带一份数据流（/api/cloud-sync），
// 与设置页的快照 store 没有交集，见该文件的模块头
import { CloudSyncPanel } from './settings-cloud-sync'
import { CaptchaSettingsPanel } from './settings-captcha'
import { NotificationSettingsPanel } from './settings-notifications'
import { OauthSettingsPanel } from './settings-oauth'
import { BrandingSettingsPanel } from './settings-branding'
import { NetworkAccessDialog, NetworkAccessPanel } from './settings-network-access'
import { GraphicsSettingsPanel } from './settings-graphics'
import { DbRestoreDialog } from './settings-db-restore'
import { DatabaseBackupDialogs, DatabaseBackupPanel } from './settings-database-backup'
import { PrivacyPanel } from './settings-privacy'
import { SanitizePanel } from './settings-sanitize'
import { WebToolsPanel } from './settings-web-tools'
import { ClientIdentificationPanel } from '../../../ui-shared/client-identification-panel'
import { ProviderSettingsPane } from './settings-providers'
import { clientIdentificationApi } from './settings-client-identification'
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

function GeneralPane({ snap }: { snap: SettingsSnapshot }) {
  const app = snap.app
  const [loginNoAutoOpen, setLoginNoAutoOpen] = useLoginNoAutoOpen()
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
      <BrandingSettingsPanel disabled={snap.busy !== null} />

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

      <Card render={<section />} className='panel'>
        <PanelHead title={t('账号登录')} />
        <CardContent className='panel-body'>
          <div className='settings-switches'>
            <SwitchRow
              id='settings-login-no-auto-open'
              label={t('使用系统浏览器时不自动打开')}
              checked={loginNoAutoOpen}
              onCheckedChange={setLoginNoAutoOpen}
            />
          </div>
          <div className='settings-state'>
            {loginNoAutoOpen
              ? t('使用系统浏览器登录时，仅在弹窗内生成只读授权链接供复制，不自动拉起默认浏览器。')
              : t('使用系统浏览器登录时，自动拉起默认浏览器，同时在弹窗内保留授权链接供复制。')}
          </div>
        </CardContent>
      </Card>

      <UnitsPanel snap={snap} />
    </>
  )
}

const CURRENCY_OPTIONS = [
  { value: 'usd', label: t('美元 ($)') },
  { value: 'cny', label: t('人民币 (¥)') },
] as const

function UnitsPanel({ snap }: { snap: SettingsSnapshot }) {
  const pricing = usePricingDisplay()
  const [currencyBusy, setCurrencyBusy] = React.useState(false)
  const [fxBusy, setFxBusy] = React.useState(false)

  return (
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
        <div className='retention-list'>
          <div className='retention-row'>
            <Label>{t('计价货币')}</Label>
            <div className='flex flex-wrap items-center gap-2.5'>
              <SegmentedControl<'usd' | 'cny'>
                aria-label={t('计价货币')}
                value={pricing.currency}
                options={CURRENCY_OPTIONS}
                onValueChange={next => {
                  if (next === pricing.currency || currencyBusy) return
                  setCurrencyBusy(true)
                  void switchDisplayCurrency(next)
                    .then(() => {
                      toast(next === 'cny' ? t('✅ 已切换为人民币 (¥)') : t('✅ 已切换为美元 ($)'))
                    })
                    .finally(() => setCurrencyBusy(false))
                }}
              />
              <span className='text-[12px] text-subtle tabular-nums'>
                {t('1 USD ≈ ¥{rate}', { rate: pricing.exchangeRate.toFixed(4) })}
                {pricing.fx?.fallback ? t('（离线默认）') : ''}
              </span>
              <Button
                id='settings-refresh-fx'
                variant='outline'
                size='sm'
                disabled={fxBusy}
                onClick={() => {
                  setFxBusy(true)
                  void refreshExchangeRate()
                    .then(fx => {
                      if (fx) toast(t('✅ 汇率已刷新：1 USD ≈ ¥{rate}', { rate: fx.rate.toFixed(4) }))
                      else toast(t('当前环境不支持刷新汇率'), 'err')
                    })
                    .catch(err => {
                      toast(t('刷新汇率失败：{message}', { message: err instanceof Error ? err.message : String(err) }), 'err')
                    })
                    .finally(() => setFxBusy(false))
                }}
              >
                {fxBusy ? t('刷新中…') : t('刷新汇率')}
              </Button>
            </div>
          </div>
        </div>
      </CardContent>
    </Card>
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
  // 账号列表的两项「怎么看这张表」偏好：合计口径与账号名掩码。
  // 与主题 / 缩放同属纯前端偏好（localStorage），不经过 settings-state 的快照。
  const [groupTotalFiltered, setGroupTotalFilteredState] = React.useState<boolean>(readGroupTotalFiltered)
  const [namesHidden, setNamesHiddenState] = React.useState<boolean>(readNamesHidden)
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

      <GraphicsSettingsPanel />

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

function PromptPanel({ snap }: { snap: SettingsSnapshot }) {
  return <PromptSettingsPanel api={promptRulesApi} t={t} active={snap.category === 'gateway'} />
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

      <Card render={<section />} className='panel'>
        <PanelHead
          title={t('图片压缩')}
          tip={TIPS.imageCompress}
          badge={snap.imageCompress.status === 'unavailable'
            ? <StatusBadge tone='bad'>{t('不可用')}</StatusBadge>
            : <StatusBadge tone={snap.imageCompress.enabled ? 'ok' : 'idle'}>
                {snap.imageCompress.enabled ? t('已开启') : t('已关闭')}
              </StatusBadge>}
          actions={<RefreshButton id='btn-image-compress-refresh' onClick={() => void refreshImageCompress()} />}
        />
        <CardContent className='panel-body'>
          <div className='settings-switches'>
            <SwitchRow
              id='settings-image-compress'
              label={t('转发前压缩对话里内嵌的大图（只改发给上游的副本，生图与图片编辑不受影响）')}
              checked={snap.imageCompress.enabled}
              // 读到后端值之前不许切（与 developerRoleCompat 同一取舍）
              disabled={snap.imageCompress.status !== 'ready' || snap.busy === 'imageCompress'}
              onCheckedChange={next => void saveImageCompressEnabled(next)}
            />
          </div>
          <div className='retention-list'>
            {IMAGE_COMPRESS_FIELDS.map(field => (
              <NumberRow
                key={field.key}
                field={field}
                value={snap.imageCompress.values?.[field.key] ?? null}
                disabled={snap.busy === 'imageCompress'}
                onCommit={raw => saveImageCompressField(field, raw)}
              />
            ))}
          </div>
          <div className='hint retention-note'>{NOTES.imageCompress}</div>
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
            disabled={snap.modelsDev.status !== 'ready' || snap.busy === 'modelsDev' || snap.busy === 'modelsDevRefresh'}
            onCheckedChange={next => void saveModelsDev(next)}
          />
          <div className='settings-state'>{modelsDevStateText(snap.modelsDev, snap.busy === 'modelsDevRefresh')}</div>
          <div className='hint retention-note'>{NOTES.modelsDev}</div>
        </CardContent>
      </Card>

      <SanitizePanel snap={snap} />
      <WebToolsPanel />

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

      <Card render={<section />} className='panel'>
        <PanelHead
          title={t('第三方模型回答完自动折叠过程')}
          tip={TIPS.reasoningCollapse}
          badge={snap.reasoningCollapse.status === 'ready'
            ? <StatusBadge tone='ok'>{t('已生效')}</StatusBadge>
            : snap.reasoningCollapse.status === 'unavailable'
              ? <StatusBadge tone='bad'>{t('不可用')}</StatusBadge>
              : <StatusBadge tone='idle'>{t('检测中…')}</StatusBadge>}
          actions={
            <RefreshButton id='btn-reasoning-collapse-refresh' onClick={() => void refreshReasoningCollapse()} />
          }
        />
        <CardContent className='panel-body'>
          <div className='settings-switches'>
            <SwitchRow
              id='settings-reasoning-collapse'
              label={t('给 Codex 的响应补阶段标记，回答完后像官方模型一样把推理与命令过程折叠起来')}
              checked={snap.reasoningCollapse.on}
              // 读到后端值之前不许切（与 developerRoleCompat 同一取舍）
              disabled={snap.reasoningCollapse.status !== 'ready' || snap.busy === 'reasoningCollapse'}
              onCheckedChange={next => void saveReasoningCollapse(next)}
            />
          </div>
          <div className='settings-state'>
            {snap.reasoningCollapse.status === 'loading'
              ? STATES.appLoading
              : snap.reasoningCollapse.status === 'unavailable'
                ? STATES.reasoningCollapseUnavailable
                : snap.reasoningCollapse.on ? STATES.reasoningCollapseOn : STATES.reasoningCollapseOff}
          </div>
          <div className='hint retention-note'>{NOTES.reasoningCollapse}</div>
        </CardContent>
      </Card>

      {/* Cline 伪装头：实现与默认值都在插件里，插件没申报就不画。
          放在「网关」分类下是历史位置，语义上它属于插件面板。 */}
      {panels.has('upstream_headers') ? <ClineHeadersPanel snap={snap} /> : null}

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
      <PrivacyPanel snap={snap} />

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
  const [resetOpen, setResetOpen] = React.useState(false)
  const initial = (account.username?.trim().slice(0, 1) || 'A').toUpperCase()

  return (
    <>
      <Card render={<section />} className='panel'>
        <CardHeader>
          <div className='flex flex-wrap items-center gap-2'>
            <CardTitle>{t('管理员账号')}</CardTitle>
            {account.status === 'loading' ? (
              <StatusBadge tone='idle'>{t('检测中…')}</StatusBadge>
            ) : account.status === 'unavailable' ? (
              <StatusBadge tone='bad'>{t('不可用')}</StatusBadge>
            ) : account.registered ? (
              <Badge variant='success'>
                <BadgeDot />
                {t('已启用登录')}
              </Badge>
            ) : (
              <StatusBadge tone='idle'>{t('未注册')}</StatusBadge>
            )}
          </div>
        </CardHeader>

        <CardContent className='flex flex-col gap-3'>
          {account.status === 'loading' ? (
            <div className='settings-state' id='account-username'>—</div>
          ) : account.status === 'unavailable' ? (
            <Alert variant='destructive' id='account-username'>
              {t('未能读取账户信息，请稍后重试')}
            </Alert>
          ) : account.registered ? (
            <Item variant='outline' size='sm' className='flex-wrap bg-surface-2/60 px-3.5 py-2.5 sm:flex-nowrap'>
              <ItemMedia>
                <Avatar className='size-8'>
                  <AvatarFallback className='border border-primary-bd bg-primary-soft text-[12.5px] font-semibold text-primary-fg'>
                    {initial}
                  </AvatarFallback>
                </Avatar>
              </ItemMedia>
              <ItemContent>
                <ItemTitle className='text-[13.5px] font-semibold' id='account-username'>
                  {account.username || '—'}
                </ItemTitle>
                <ItemDescription>
                  {t('修改密码后其他设备会话将失效，当前会话保留')}
                </ItemDescription>
              </ItemContent>
              <ItemActions>
                <Button
                  id='btn-account-change-password'
                  variant='outline'
                  size='sm'
                  onClick={() => openPasswordDialog()}
                >
                  <KeyIcon />
                  {t('修改密码')}
                </Button>
              </ItemActions>
            </Item>
          ) : (
            <Alert id='account-username'>
              {t('尚未注册管理员。开启「网关 → 网络访问 → 允许其他设备访问网关」时会引导注册，也可以从登录页注册。')}
            </Alert>
          )}

          <Collapsible open={resetOpen} onOpenChange={setResetOpen} className='border-t border-hairline pt-2'>
            <CollapsibleTrigger
              render={<button type='button' />}
              className='flex w-full cursor-pointer items-center justify-between gap-3 rounded-md border-0 bg-transparent px-2 py-1 text-left transition-colors hover:bg-surface-2 focus-visible:shadow-focus focus-visible:outline-none'
            >
              <span className='text-[12.5px] font-medium text-subtle'>
                {t('忘记密码？查看重置管理员说明')}
              </span>
              <ChevronDownIcon
                size={14}
                className={cn('shrink-0 text-muted-foreground transition-transform duration-150', resetOpen && 'rotate-180')}
              />
            </CollapsibleTrigger>

            <CollapsibleContent keepMounted className='mt-2 flex flex-col gap-2.5 px-2'>
              <div className='hint'>
                {t('忘记密码时无法在界面里找回：需要停掉程序后，从数据库删除管理员记录（kv 表的 panelAdmin 键），重启后按首次使用重新注册。具体命令与各形态的库文件路径见 docs 目录下的「重置管理员」文档。')}
              </div>
              <Alert variant='destructive' className='danger-zone'>
                <strong>{t('重置会清空管理员账号与密码，面板将回到「未注册」状态。')}</strong>
              </Alert>
            </CollapsibleContent>
          </Collapsible>
        </CardContent>
      </Card>

      <CaptchaSettingsPanel disabled={snap.busy !== null} />

      <OauthSettingsPanel />
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
      <DialogContent className='w-[min(430px,calc(100vw-48px))]' initialFocus={cancelRef}>
        <DialogHeader>
          <div className='min-w-0 flex-1'>
            <DialogTitle>{t('修改密码')}</DialogTitle>
            <DialogDescription className='mt-0.5 text-[12px] text-subtle'>
              {t('修改密码后其他设备会话将失效，当前会话保留')}
            </DialogDescription>
          </div>
        </DialogHeader>
        <DialogBody className='gap-3.5'>
          {/* form 让浏览器的密码管理器能识别「旧密码 → 新密码」这一组 */}
          <form
            className='flex flex-col gap-3.5'
            onSubmit={event => { event.preventDefault(); submit() }}
            autoComplete='on'
          >
            <Field>
              <FieldLabel htmlFor='account-old-password' className='text-[12.5px] font-medium text-foreground'>
                {t('当前密码')}
              </FieldLabel>
              <Input
                id='account-old-password'
                type='password'
                autoComplete='current-password'
                placeholder={t('输入当前登录用的密码')}
                value={oldValue}
                disabled={busy}
                onChange={event => setOldValue(event.target.value)}
              />
            </Field>
            <Field>
              <FieldLabel htmlFor='account-new-password' className='text-[12.5px] font-medium text-foreground'>
                {t('新密码')}
              </FieldLabel>
              <Input
                id='account-new-password'
                type='password'
                autoComplete='new-password'
                placeholder={t('至少 8 位')}
                value={newValue}
                disabled={busy}
                onChange={event => setNewValue(event.target.value)}
              />
            </Field>
            <Field>
              <FieldLabel htmlFor='account-repeat-password' className='text-[12.5px] font-medium text-foreground'>
                {t('确认新密码')}
              </FieldLabel>
              <Input
                id='account-repeat-password'
                type='password'
                autoComplete='new-password'
                placeholder={t('再输入一次新密码')}
                value={repeat}
                disabled={busy}
                onChange={event => setRepeat(event.target.value)}
              />
            </Field>
          </form>
          {mismatch || password?.error ? (
            <div className='text-[12.5px] whitespace-pre-wrap text-destructive'>
              {mismatch ? t('两次输入的新密码不一致') : (password?.error || '')}
            </div>
          ) : null}
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
          <NetworkAccessPanel snap={snap} />
          {/* 图片兼容已搬到「路由 → 特殊请求路由 → 图片兼容」：它与账号路由是同一层
              （都在模型解析时改写请求），留在设置页会让「改哪只模型」散落在两处。 */}
          <ClientIdentificationPanel api={clientIdentificationApi} t={t} active={snap.category === 'gateway'} />
          <GatewayPane snap={snap} panels={adminPanels} />
        </div>
        <div className={paneClass('providers')} data-cat='providers'>
          <ProviderSettingsPane active={snap.category === 'providers'} onOpenDetail={() => setMobileDetail(true)} />
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
        <div className={paneClass('notifications')} data-cat='notifications'>
          <NotificationSettingsPanel />
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
      <NetworkAccessDialog confirm={snap.lanConfirm} register={snap.lanRegister} />
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

// 首次切到本页时才挂载（见 island-lazy）：隐藏页不在首屏渲染，也不拉数据。
mountOnPage('settings', PAGE_SELECTOR, mount)

/* ─── 注册：对外契约 ─────────────────────────── */

declare global {
  interface Window {
    /**
     * 设置页（替换 ui/settings-panel.js，九个方法与原实现逐字一致）。
     * 调用点：app.js:135 切到设置页时 load()；upgrade-panel.js:74 迁移完成后 load()；
     * update-panel.tsx:972 的「去更新」showCategory('about')。
     */
    wbSettingsPanel?: {
      load: typeof load
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
