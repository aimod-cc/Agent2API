import * as React from 'react'
import { mountOnPage } from '../island-lazy'
import { createRoot } from 'react-dom/client'
import { SegmentedControl, Spinner , Button } from '@ui'
import {
  loadCenter, loadRecords, store, subscribe,
} from './checkin-store'
import { CenterTab } from './checkin-center-tab'
import { ScheduleTab } from './checkin-schedule-tab'
import { MakeupTab } from './checkin-makeup-tab'
import { t } from '../i18n'
import { RecordsTab } from './checkin-records-tab'
import { Badge } from '@ui'

/**
 * Agent2API · 签到中心（页面岛）。
 *
 * ── 这一页为什么在网关里，而不是插件里 ──────────────────────
 * 它原先由插件包出 HTML/CSS/JS，网关用 iframe 托管。那条路的代价是**两个前端**：
 * iframe 读不到宿主的 CSS 变量、拿不到组件库实例、用不了 tooltip 与确认弹窗，
 * 于是每一项都得抄一遍（`/plugins/runtime/*` 那条资源路由就是为抄样式而生的）。
 * 抄必然漂移 —— 开关的间距、说明文字的字号、面板的圆角，一处一处都对不上。
 *
 * 现在反过来切：**「签到这件事」归网关**（页面、三个标签页、排程、记录、空态），
 * **「某家怎么签到」归插件**（走哪个接口、有哪些档位、有没有专属面板）。插件回归
 * 成纯后端能力提供者，一个 exe 只讲 RPC，不出 UI。于是这一页与账号页、模型管理页
 * 是同一份代码、同一套组件、同一套令牌 —— 样式不会再漂。
 *
 * ── 菜单项是固定的 ──────────────────────────────────────────
 * 与「模型管理」「账号」同级，永远在侧边栏里。**不随插件装没装而消失** ——
 * 签到是产品的一个功能，不是某个插件的附属品。没装插件时页面显示空态，告诉
 * 用户去哪儿装，而不是让整个菜单项凭空不见（那会让人以为功能被砍了）。
 *
 * 四个标签页共用快照；定时签到与定时补签分别管理各自的设置。
 * 保活模型链和猫猫旅行由插件申报决定是否显示。
 *
 * ── 数据流 ──────────────────────────────────────────────────
 * 一次 `GET /api/checkin-center` 拉全（见 `checkin-store.ts`），四个标签页共用。
 * 切标签页不重新请求，只切视图。
 */

const TABS = ['center', 'schedule', 'makeup', 'records'] as const
type Tab = typeof TABS[number]

function tabOptions() {
  return [
    { value: 'center', label: t('签到中心') },
    { value: 'schedule', label: t('定时签到') },
    { value: 'makeup', label: t('定时补签') },
    { value: 'records', label: t('签到记录') },
  ]
}

/** 当前标签页（组件外也能读：自动签到执行中的轮询要用） */
let activeTab: Tab = 'center'

/* ─── 订阅 store ───────────────────────────── */

/** 订阅 store 并在变化时重渲（本页数据量小，整页重渲足够） */
function useStore(): number {
  const [, bump] = React.useReducer((n: number) => n + 1, 0)
  React.useEffect(() => subscribe(() => bump()), [])
  return 0
}

/* ─── 页面 ─────────────────────────────────── */

function CheckinPage(): React.ReactElement {
  useStore()
  const [tab, setTab] = React.useState<Tab>(activeTab)

  // 首屏拉快照 + 记录（记录页要显示总数，进页面就拉一次）
  React.useEffect(() => {
    if (!store.loaded) void loadCenter()
    if (!store.recordsLoaded) void loadRecords()
  }, [])

  // 自动签到执行中短轮询：那是唯一「后台会自己变」的场景（与旧实现同一取舍）
  React.useEffect(() => {
    const timer = window.setInterval(() => {
      const page = (window as unknown as { wbApp?: { currentPage?: string } }).wbApp?.currentPage
      if (document.hidden || page !== 'checkin') return
      if (store.center?.auto?.running || activeTab === 'schedule' && store.center?.auto?.enabled) void loadCenter()
    }, 3000)
    return () => window.clearInterval(timer)
  }, [])

  const switchTab = (next: string) => {
    const value = (TABS as readonly string[]).includes(next) ? next as Tab : 'center'
    activeTab = value
    setTab(value)
  }

  // 总览卡点「累计签到记录」要切到记录页。走自定义事件而不是把 switchTab
  // 传给子组件：子组件（center-tab）不该反过来依赖入口层的私有函数，
  // 那会形成 page → tab → page 的循环导入。
  React.useEffect(() => {
    const onSwitch = (event: Event) => {
      const next = (event as CustomEvent<{ tab?: string }>).detail?.tab
      if (typeof next === 'string') switchTab(next)
    }
    window.addEventListener('wb:checkin-tab', onSwitch)
    return () => window.removeEventListener('wb:checkin-tab', onSwitch)
  }, [])

  const center = store.center
  const error = store.loadError

  return (
    <div className={`ck-page${tab === 'records' ? ' ck-page-records' : ''}`}>
      <div className="ck-tabs">
        <SegmentedControl
          options={tabOptions()}
          value={tab}
          onValueChange={switchTab}
          aria-label="签到中心标签页"
        />
        <span className="ck-tabs-spacer" />
        <AutoBadge />
        <Button size='default' variant='ghost' type="button" className="ck-refresh" onClick={() => { void loadCenter(); void loadRecords() }}>
          刷新
        </Button>
      </div>

      {error ? (
        <div className="ck-page-error">
          <span>{error}</span>
          <Button size='default' variant='ghost' type="button" className="ck-error-retry" onClick={() => { void loadCenter(); void loadRecords() }}>
            重试
          </Button>
        </div>
      ) : null}

      {!store.loaded ? (
        <div className="ck-loading">
          <Spinner className="size-5" />
          <span>正在读取签到数据…</span>
        </div>
      ) : (
        <>
          {tab === 'center' && center ? <CenterTab center={center} /> : null}
          {tab === 'schedule' && center ? <ScheduleTab center={center} /> : null}
          {tab === 'makeup' && center ? <MakeupTab center={center} /> : null}
          {tab === 'records' ? <RecordsTab /> : null}
        </>
      )}
    </div>
  )
}

/**
 * 顶栏的自动签到徽标。
 *
 * 三态：执行中 / 已关闭 / 已开启（再分今日是否已执行）。配色由 Badge 变体统一。
 */
function AutoBadge(): React.ReactElement | null {
  const auto = store.center?.auto
  if (!auto) return null
  if (auto.running) return <Badge variant='warning' shape='tag'>{t('执行中…')}</Badge>
  if (!auto.enabled) return <Badge variant='outline' shape='tag'>{t('自动执行已暂停')}</Badge>
  const rows = (auto.schedules ?? []).filter(entry => entry.enabled).flatMap(entry => entry.providerRows ?? [])
  const pending = rows.filter(row => !row.firedToday).length
  return (
    <Badge variant={pending ? 'brand' : 'success'} shape='tag'>
      {pending ? t('还有 {n} 个平台未执行', { n: pending }) : t('自动执行已开启')}
    </Badge>
  )
}

/* ─── 挂载 ─────────────────────────────────── */

const PAGE_SELECTOR = '.page[data-page="checkin"]'

let mounted = false

function mount(): void {
  if (mounted) return
  const section = document.querySelector<HTMLElement>(PAGE_SELECTOR)
  if (!section) return
  mounted = true
  section.replaceChildren()
  createRoot(section).render(<CheckinPage />)
}

mountOnPage('checkin', PAGE_SELECTOR, mount)

declare global {
  interface Window {
    /**
     * 签到中心（壳侧接口，与 `wbReport` / `wbAccountsView` 同一套约定）。
     *
     * `load` 由 `app.js` 在切到本页时调；`refresh` 给「签到完成后要更新账号页
     * 的用量」这类场景留一个入口。
     */
    wbCheckin?: {
      load(): Promise<void>
      refresh(): void
    }
  }
}

window.wbCheckin = {
  load: loadCenter,
  refresh: () => { void loadCenter(); void loadRecords() },
}
