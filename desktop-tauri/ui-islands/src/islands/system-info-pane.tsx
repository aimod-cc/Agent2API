import * as React from 'react'
import { createRoot } from 'react-dom/client'
import { Badge, Button, Switch } from '@ui'
import {
  formatBytes,
  formatCount,
  shared,
  toast,
  type SystemInfo,
} from './settings-model'
import { t } from '../i18n'

/**
 * 设置页「版本信息」面板（React 岛）。
 *
 * ── 它解决什么问题 ──────────────────────────────────────────
 * 用户出问题时最常见的开场是「截个图 / 说一句我用的哪版」。这个页面把
 * 「你装的是哪一版、跑在什么环境、数据与日志在哪、哪些开关是开的」一次
 * 摆全，**且能一键复制成纯文本** —— 截图只能看，文本能直接贴进 issue。
 *
 * ── 为什么环境判定读后端，不读 `platform` ─────────────────────
 * `workbuddyDesktop.platform` 只说明编译目标（windows / macos / linux / web）。
 * 桌面端开了「同时开放网页管理面板」后，别人用浏览器访问那个面板时
 * `platform` 是 `'web'`，但后端进程仍是桌面端 —— 只看前端会把它误报成
 * Docker 面板。真正的判据在后端（`core::system_info::deployment`），这里只
 * 负责把 `deployment` + `platform` 组合成一句人话。
 *
 * ── 「隐藏用户名」为什么是请求参数而不是前端替换 ───────────────
 * 这个页面的核心用途是**分享**。如果脱敏只做在渲染层，「复制全部信息」
 * 走的是另一条路（把数据序列化成文本），很容易漏掉 —— 用户以为隐藏了，
 * 实际复制出去的是完整路径。所以开关直接进请求（`?hideUsername=1`），
 * 后端返回的路径本身就已经是 `~\.agent2api`。开关**不落库**：它是「我这次
 * 要发给别人」的一次性决定，记住反而会让下次截图悄悄少信息。
 *
 * ── 布局 ────────────────────────────────────────────────────
 * 顶部是环境徽章（截图里最该被一眼看到的东西），下面按「版本 / 环境 /
 * 位置 / 状态 / 计数」分组。所有值用等宽字体 —— 版本号、路径、端口在等宽
 * 字体下更容易逐字核对，也更容易被 OCR / 人工比对。
 */

/* ─── 类型与常量 ───────────────────────────── */

type Group = { title: string; rows: Array<[string, React.ReactNode]> }

/** 运行形态 → 展示文案（后端 `deployment` 的取值集合） */
const DEPLOYMENT_LABEL: Record<string, string> = {
  desktop: t('桌面端'),
  'windows-service': t('Windows 服务'),
  docker: t('Docker 容器'),
  systemd: t('systemd 服务'),
  foreground: t('前台运行'),
}

/** 系统标识 → 展示文案 */
const OS_LABEL: Record<string, string> = {
  windows: 'Windows',
  macos: 'macOS',
  linux: 'Linux',
}

/** 安装包形态 → 展示文案 */
const INSTALLER_LABEL: Record<string, string> = {
  nsis: t('Windows 安装包（NSIS）'),
  dmg: 'macOS 安装包（dmg）',
  deb: t('Linux 安装包（deb）'),
  appimage: 'AppImage',
}

/** 当前界面形态（前端补充，后端不知道用户在哪个窗口里） */
function interfaceLabel(): string {
  const platform = shared().workbuddyDesktop?.platform
  if (platform === 'web') return t('浏览器面板')
  return t('应用窗口')
}

/**
 * 系统名 + 版本拼成一句。
 *
 * 不能直接 `${OS_LABEL[os]} ${osVersion}`：Windows 的 `ProductName`
 * 自带 "Windows"（`Windows 11 Pro`），Linux 的 `PRETTY_NAME` 也自带发行版名
 * （`Ubuntu 24.04 LTS`）—— 再拼一次前缀就成了「Windows Windows 11 Pro」。
 * 判据是「版本描述是否已经以系统名开头」，是就不加。
 */
function formatOs(os: string | null | undefined, version: string | null | undefined): string {
  const name = OS_LABEL[String(os)] || text(os)
  const detail = text(version)
  if (detail === '—') return name
  if (name !== '—' && detail.toLowerCase().startsWith(name.toLowerCase())) return detail
  return `${name} ${detail}`
}

/** 已运行时长 → 「3 天 4 小时」这种可读文本 */
function formatUptime(seconds: number | null | undefined): string {
  const total = Number(seconds)
  if (!Number.isFinite(total) || total <= 0) return '—'
  const days = Math.floor(total / 86400)
  const hours = Math.floor((total % 86400) / 3600)
  const minutes = Math.floor((total % 3600) / 60)
  if (days > 0) return t('{d} 天 {h} 小时', { d: days, h: hours })
  if (hours > 0) return t('{h} 小时 {m} 分钟', { h: hours, m: minutes })
  if (minutes > 0) return t('{m} 分钟', { m: minutes })
  return t('不到 1 分钟')
}

/** 时间戳（毫秒）→ 本地时间串；0 / 非法给「—」 */
function formatTime(ms: number | null | undefined): string {
  const value = Number(ms)
  if (!Number.isFinite(value) || value <= 0) return '—'
  const date = new Date(value)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} `
    + `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`
}

/** 布尔开关 → 「开 / 关」；未知给「—」 */
function onOff(value: boolean | null | undefined): string {
  if (value === true) return t('开')
  if (value === false) return t('关')
  return '—'
}

/** 任意值 → 可展示文本（空 / null 一律「—」） */
function text(value: unknown): string {
  if (value === null || value === undefined) return '—'
  const result = String(value).trim()
  return result || '—'
}

/* ─── 数据读取 ─────────────────────────────── */

type State = {
  status: 'loading' | 'ready' | 'unavailable'
  info: SystemInfo | null
  /** 本次读取的失败原因（空串 = 没有失败） */
  error: string
  /** 「隐藏用户名」：改了立即重拉一次（脱敏在后端） */
  hideUsername: boolean
}

const EMPTY: State = { status: 'loading', info: null, error: '', hideUsername: false }

let snapshot: State = EMPTY
const listeners = new Set<() => void>()

function publish(patch: Partial<State>): void {
  snapshot = { ...snapshot, ...patch }
  listeners.forEach(listener => listener())
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

function getSnapshot(): State {
  return snapshot
}

/**
 * 拉一次读数。
 *
 * 失败不清空已有的 `info`：与更新面板同一个取向 —— 环境信息不会因为一次
 * 网络抖动就变成未知，留着上一次的值 + 一句失败原因比整页「—」有用。
 */
async function load(): Promise<void> {
  const api = shared().workbuddyDesktop
  if (!api?.getSystemInfo) {
    publish({ status: 'unavailable', error: t('当前环境不支持读取版本信息（桥接方法缺失）') })
    return
  }
  if (snapshot.status !== 'ready') publish({ status: 'loading' })
  try {
    const info = await api.getSystemInfo({ hideUsername: snapshot.hideUsername })
    if (!info) {
      publish({ status: 'unavailable', error: t('后端没有返回版本信息') })
      return
    }
    publish({ status: 'ready', info, error: '' })
  } catch (error) {
    publish({
      status: 'unavailable',
      error: error instanceof Error ? error.message : String(error),
    })
  }
}

/** 拨「隐藏用户名」：先更新开关再重拉（让用户看到路径立刻变化） */
function toggleHideUsername(next: boolean): void {
  publish({ hideUsername: next })
  void load()
}

/* ─── 复制成纯文本 ─────────────────────────── */

/**
 * 把当前读数摊平成「分组 + 键值」的纯文本。
 *
 * 用的是**已经脱敏**的 `info`（开关走请求参数，见文件头），所以这里不需要
 * 再处理路径。行格式刻意用 `键: 值` 而不是表格：贴进 GitHub issue 后，
 * 表格语法在移动端会挤成一团，纯文本行反而是最好读的。
 */
function toPlainText(info: SystemInfo): string {
  const app = info.app || {}
  const runtime = info.runtime || {}
  const deployment = info.deployment || {}
  const security = info.security || {}
  const features = info.features || {}
  const stats = info.stats || {}

  const lines: string[] = []
  const section = (title: string, rows: Array<[string, string]>) => {
    lines.push(`[${title}]`)
    rows.forEach(([key, value]) => lines.push(`${key}: ${value}`))
    lines.push('')
  }

  section(t('版本'), [
    [t('软件版本'), text(app.version)],
    [t('更新通道'), app.channel === 'beta' ? t('beta 版') : t('正式版')],
    [t('数据库 schema'), text(app.schemaVersion)],
    [t('库中 schema'), text(app.dbSchemaVersion)],
    [t('上游客户端版本'), text(app.upstreamClientVersion)],
    [t('上游 CLI 版本'), text(app.upstreamCliVersion)],
  ])
  section(t('环境'), [
    [t('运行形态'), DEPLOYMENT_LABEL[String(runtime.deployment)] || text(runtime.deployment)],
    [t('当前界面'), interfaceLabel()],
    [t('操作系统'), formatOs(runtime.os, runtime.osVersion)],
    [t('架构'), text(runtime.arch)],
    [t('WebView / 浏览器内核'), text(runtime.webviewVersion)],
    [t('进程 PID'), text(runtime.pid)],
    [t('启动时间'), formatTime(runtime.startedAt)],
    [t('已运行'), formatUptime(runtime.uptimeSeconds)],
    [t('开发构建'), onOff(runtime.debugBuild)],
  ])
  section(t('位置与端口'), [
    [t('监听地址'), `${text(deployment.host)}:${text(deployment.port)}`],
    [t('面板端口'), text(deployment.panelPort)],
    [t('托管网页界面'), onOff(deployment.uiHosted)],
    [t('数据目录'), text(deployment.configDir)],
    [t('数据库文件'), `${text(deployment.dbFile)}（${formatBytes(Number(deployment.dbBytes) || 0)}）`],
    [t('日志文件'), text(deployment.logFile)],
    [t('程序路径'), text(deployment.exePath)],
    [t('安装包形态'), INSTALLER_LABEL[String(deployment.installerKind)] || text(deployment.installerKind)],
    [t('容器内更新'), onOff(deployment.selfUpdate)],
  ])
  section(t('安全'), [
    [t('管理员已注册'), onOff(security.adminRegistered)],
    [t('面板登录'), onOff(security.panelAuth)],
    [t('网关 Key 数量'), text(security.apiKeyCount)],
    [t('/v1 拒绝无 Key 请求'), onOff(security.v1FailClosed)],
    [t('未注册闸门'), onOff(security.panelGate)],
    [t('允许无 Key 裸跑'), onOff(security.allowNoKey)],
    [t('GitHub 令牌'), security.githubTokenFilled ? t('已填写') : t('未填写')],
    [t('更新出网线路'), security.updateProxy ? t('已指定出口') : t('直连')],
  ])
  section(t('开关'), [
    [t('调试模式'), onOff(features.debugMode)],
    [t('指纹脱敏'), onOff(features.sanitizeFingerprints)],
    [t('跨域访问'), onOff(features.cors)],
    [t('开发者角色兼容'), onOff(features.developerRoleCompat)],
    [t('保活换行丢弃'), onOff(features.keepaliveStrip)],
    [t('自动续写'), onOff(features.autoContinue)],
    [t('models.dev 元数据'), onOff(features.modelsDev)],
    [t('登录人机验证'), onOff(features.captchaEnabled)],
  ])
  section(t('计数'), [
    [t('账号'), `${text(stats.enabledAccounts)} / ${text(stats.accounts)} ${t('启用')}`],
    [t('限流中账号'), text(stats.limitedAccounts)],
    [t('模型'), text(stats.models)],
    [t('在途请求'), text(stats.inFlight)],
    [t('累计请求'), formatCount(stats.requests)],
    [t('事件日志'), formatCount(stats.logs)],
    [t('待迁移旧数据'), onOff(stats.upgradePending)],
  ])
  return lines.join('\n').trimEnd()
}

/** 复制（与 clipboard.js 同一套兜底：非安全上下文下 execCommand） */
async function copyPlainText(): Promise<void> {
  const info = snapshot.info
  if (!info) {
    toast(t('还没有读到版本信息'), 'err')
    return
  }
  const payload = toPlainText(info)
  try {
    await navigator.clipboard.writeText(payload)
    toast(t('已复制全部版本信息'))
    return
  } catch {
    /* 落到下面的兜底 */
  }
  try {
    const area = document.createElement('textarea')
    area.value = payload
    area.style.position = 'fixed'
    area.style.opacity = '0'
    document.body.appendChild(area)
    area.select()
    const ok = document.execCommand('copy')
    area.remove()
    if (ok) toast(t('已复制全部版本信息'))
    else toast(t('复制失败，请手动选择复制'), 'err')
  } catch {
    toast(t('复制失败，请手动选择复制'), 'err')
  }
}

/* ─── 视图 ─────────────────────────────────── */

/** 一行「键 —— 值」：键定宽、值等宽可折行（长路径不撑出横向滚动） */
function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className='sysinfo-row'>
      <span className='sysinfo-key'>{label}</span>
      <span className='sysinfo-value'>{children}</span>
    </div>
  )
}

function GroupCard({ title, rows }: { title: string; rows: Array<[string, React.ReactNode]> }) {
  if (!rows.length) return null
  return (
    <section className='panel'>
      <div className='panel-head'>
        <h2>{title}</h2>
      </div>
      <div className='panel-body'>
        {rows.map(([label, value], index) => (
          <Row key={`${label}-${index}`} label={label}>
            {value}
          </Row>
        ))}
      </div>
    </section>
  )
}

function SystemInfoPanel() {
  const state = React.useSyncExternalStore(subscribe, getSnapshot)

  // 面板常驻（root 随应用生命周期），进设置页时由 wbSystemInfoPane.load() 重拉
  React.useEffect(() => { void load() }, [])

  const info = state.info
  const app = info?.app || {}
  const runtime = info?.runtime || {}
  const deployment = info?.deployment || {}
  const security = info?.security || {}
  const features = info?.features || {}
  const stats = info?.stats || {}

  const deploymentLabel = DEPLOYMENT_LABEL[String(runtime.deployment)] || text(runtime.deployment)
  // 系统描述本身可能已带系统名（Windows 的 ProductName / Linux 的 PRETTY_NAME），
  // 拼之前先判一次，避免「Windows Windows 11」这种重复（见 formatOs 的说明）
  const osFull = formatOs(runtime.os, runtime.osVersion)
  const schemaMismatch = app.schemaVersion != null
    && app.dbSchemaVersion != null
    && app.schemaVersion !== app.dbSchemaVersion

  const groups: Group[] = [
    {
      title: t('版本'),
      rows: [
        [t('软件版本'), <span className='sysinfo-mono'>{text(app.version)}</span>],
        [t('更新通道'), app.channel === 'beta'
          ? <Badge variant='warning' shape='tag'>{t('beta 版')}</Badge>
          : <Badge variant='outline' shape='tag'>{t('正式版')}</Badge>],
        [t('数据库 schema'), (
          <span className='sysinfo-mono'>
            {text(app.schemaVersion)}
            {app.dbSchemaVersion != null ? ` / ${app.dbSchemaVersion}` : ''}
            {schemaMismatch
              ? <Badge variant='destructive' shape='tag' className='ml-2'>{t('不一致')}</Badge>
              : null}
          </span>
        )],
        [t('上游客户端版本'), <span className='sysinfo-mono'>{text(app.upstreamClientVersion)}</span>],
        [t('上游 CLI 版本'), <span className='sysinfo-mono'>{text(app.upstreamCliVersion)}</span>],
      ],
    },
    {
      title: t('环境'),
      rows: [
        [t('运行形态'), <Badge variant='brand'>{deploymentLabel}</Badge>],
        [t('当前界面'), text(interfaceLabel())],
        [t('操作系统'), osFull],
        [t('架构'), <span className='sysinfo-mono'>{text(runtime.arch)}</span>],
        [t('WebView / 浏览器内核'), <span className='sysinfo-mono'>{text(runtime.webviewVersion)}</span>],
        [t('进程 PID'), <span className='sysinfo-mono'>{text(runtime.pid)}</span>],
        [t('启动时间'), <span className='sysinfo-mono'>{formatTime(runtime.startedAt)}</span>],
        [t('已运行'), formatUptime(runtime.uptimeSeconds)],
        [t('开发构建'), onOff(runtime.debugBuild)],
        [t('详细日志'), onOff(runtime.verbose)],
      ],
    },
    {
      title: t('位置与端口'),
      rows: [
        [t('监听地址'), <span className='sysinfo-mono'>{`${text(deployment.host)}:${text(deployment.port)}`}</span>],
        [t('面板端口'), <span className='sysinfo-mono'>{text(deployment.panelPort)}</span>],
        [t('托管网页界面'), onOff(deployment.uiHosted)],
        [t('数据目录'), <span className='sysinfo-mono'>{text(deployment.configDir)}</span>],
        [t('数据库文件'), (
          <span className='sysinfo-mono'>
            {text(deployment.dbFile)}
            <span className='sysinfo-dim'>{`（${formatBytes(Number(deployment.dbBytes) || 0)}）`}</span>
            {deployment.dbAvailable === false
              ? <Badge variant='destructive' shape='tag' className='ml-2'>{t('不可用')}</Badge>
              : null}
          </span>
        )],
        [t('日志文件'), <span className='sysinfo-mono'>{text(deployment.logFile)}</span>],
        [t('程序路径'), <span className='sysinfo-mono'>{text(deployment.exePath)}</span>],
        [t('安装包形态'), INSTALLER_LABEL[String(deployment.installerKind)] || text(deployment.installerKind)],
        [t('容器内更新'), onOff(deployment.selfUpdate)],
        [t('可回滚到上一版本'), onOff(deployment.backupAvailable)],
      ],
    },
    {
      title: t('安全'),
      rows: [
        [t('管理员已注册'), onOff(security.adminRegistered)],
        [t('面板登录'), onOff(security.panelAuth)],
        [t('网关 Key 数量'), <span className='sysinfo-mono'>{text(security.apiKeyCount)}</span>],
        [t('/v1 拒绝无 Key 请求'), onOff(security.v1FailClosed)],
        [t('未注册闸门'), onOff(security.panelGate)],
        [t('允许无 Key 裸跑'), onOff(security.allowNoKey)],
        [t('GitHub 令牌'), security.githubTokenFilled ? t('已填写') : t('未填写')],
        [t('更新出网线路'), security.updateProxy ? t('已指定出口') : t('直连')],
      ],
    },
    {
      title: t('开关'),
      rows: [
        [t('调试模式'), onOff(features.debugMode)],
        [t('指纹脱敏'), onOff(features.sanitizeFingerprints)],
        [t('跨域访问'), onOff(features.cors)],
        [t('开发者角色兼容'), onOff(features.developerRoleCompat)],
        [t('保活换行丢弃'), onOff(features.keepaliveStrip)],
        [t('自动续写'), features.autoContinue
          ? t('开（最多 {n} 轮）', { n: text(features.autoContinueMaxRounds) })
          : t('关')],
        [t('models.dev 元数据'), onOff(features.modelsDev)],
        [t('登录人机验证'), onOff(features.captchaEnabled)],
      ],
    },
    {
      title: t('计数'),
      rows: [
        [t('账号'), <span className='sysinfo-mono'>{`${text(stats.enabledAccounts)} / ${text(stats.accounts)}`}</span>],
        [t('限流中账号'), <span className='sysinfo-mono'>{text(stats.limitedAccounts)}</span>],
        [t('模型'), <span className='sysinfo-mono'>{text(stats.models)}</span>],
        [t('在途请求'), <span className='sysinfo-mono'>{text(stats.inFlight)}</span>],
        [t('累计请求'), <span className='sysinfo-mono'>{formatCount(stats.requests)}</span>],
        [t('事件日志'), <span className='sysinfo-mono'>{formatCount(stats.logs)}</span>],
        [t('待迁移旧数据'), onOff(stats.upgradePending)],
      ],
    },
  ]

  return (
    <>
      {/* 头部：环境徽章 + 三个动作。徽章放在最上面是刻意的 —— 截图里
          最该被一眼看到的就是「他跑在什么环境」。 */}
      <section className='panel'>
        <div className='panel-head'>
          <h2>{t('版本信息')}</h2>
          <div className='head-actions'>
            <Button variant='outline' onClick={() => void load()} disabled={state.status === 'loading'}>
              {state.status === 'loading' ? t('读取中…') : t('刷新')}
            </Button>
            <Button variant='default' onClick={() => void copyPlainText()} disabled={!info}>
              {t('复制全部信息')}
            </Button>
          </div>
        </div>
        <div className='panel-body'>
          <div className='sysinfo-headline'>
            <Badge variant='brand'>{deploymentLabel}</Badge>
            <span className='sysinfo-headline-version'>
              {app.version ? `v${app.version}` : '—'}
            </span>
            <span className='sysinfo-headline-os'>
              {OS_LABEL[String(runtime.os)] || text(runtime.os)} {text(runtime.arch)}
            </span>
          </div>
          <div className='detail'>
            {t('出问题时把这一页截图，或点「复制全部信息」把文本贴进反馈，一般就能定位到大致原因。')}
          </div>

          {/* 路径里的用户名可以隐藏：截图会发到公开的地方，这是给用户的选择 */}
          <div className='sysinfo-toggle'>
            <div className='sysinfo-toggle-text'>
              <span>{t('隐藏用户名')}</span>
              <span className='hint'>
                {t('把路径里的主目录（如 C:\\Users\\你的用户名）显示成 ~，截图分享时不暴露用户名。只影响本页显示与复制，不改任何文件。')}
              </span>
            </div>
            <Switch
              checked={state.hideUsername}
              onCheckedChange={toggleHideUsername}
              aria-label={t('隐藏用户名')}
            />
          </div>

          {state.error ? (
            <div className='detail' style={{ color: 'var(--danger)' }}>
              {t('读取失败：{message}', { message: state.error })}
            </div>
          ) : null}
        </div>
      </section>

      {/* 分组卡片：宽屏左右两栏、窄屏自动退回一栏（见 CSS 的 .sysinfo-groups）。
          单栏时这一页有六张卡片、四十多行，要滚两屏多；两栏之后一屏能扫完，
          而且浏览器的自动平衡恰好把「版本 / 环境 / 位置与端口」放在左栏、
          「安全 / 开关 / 计数」放在右栏 —— 正好是「这台机器是什么」与
          「它现在怎么跑」两组语义。 */}
      <div className='sysinfo-groups'>
        {groups.map(group => (
          <GroupCard key={group.title} title={group.title} rows={group.rows} />
        ))}
      </div>
    </>
  )
}

/* ─── 挂载：接管设置页 `.settings-pane[data-cat="system"]` ─── */

const PANE_SELECTOR = '.settings-pane[data-cat="system"]'

let paneRoot: ReturnType<typeof createRoot> | null = null

/**
 * 把 React root 直接建在 pane 元素上（照 update-panel 的手法：不套宿主，
 * 布局与其它分类完全一致）。容器不存在（骨架变了）就静默跳过。
 */
function mountPanel(): void {
  if (paneRoot) return
  const pane = document.querySelector<HTMLElement>(PANE_SELECTOR)
  if (!pane) return
  pane.replaceChildren()
  paneRoot = createRoot(pane)
  paneRoot.render(<SystemInfoPanel />)
}

if (document.querySelector(PANE_SELECTOR)) mountPanel()
else document.addEventListener('DOMContentLoaded', mountPanel, { once: true })

/* ─── 对外契约 ─────────────────────────────── */

declare global {
  interface Window {
    /** 设置页「版本信息」面板：切到该分类时由 settings-state 调 load() 重拉一次 */
    wbSystemInfoPane?: {
      load(): Promise<void>
    }
  }
}

window.wbSystemInfoPane = { load }
