import * as React from 'react'
import { createRoot } from 'react-dom/client'
import '../../ui/css/tokens.css'
import '../src/styles/islands.css'
import '../../ui/css/layout.css'
import '../../ui/css/page-checkin.css'
import './proto.css'

/**
 * 签到中心预览：用真实实现（`checkin-center-tab.tsx` 等）配一份假快照渲染。
 *
 * 这样看到的排版、滚动、筛选与左栏联动都是上线后的真实行为，不是另画一版。
 * 假数据只在 `window` 上替换壳侧接口，组件本身不改。
 */

type Row = Record<string, unknown>

const DAY = 24 * 3600 * 1000

function provider(id: string, label: string, accounts: Row[], done: number, extra: Row = {}) {
  return {
    id, label, accounts, doneCount: done, totalCount: accounts.length,
    description: null, modes: null, uiHooks: null, manualPlans: false, ...extra,
  }
}

function account(id: string, name: string, done: boolean, hour: number, streak = 0): Row {
  return {
    id, name, available: true, checkedInToday: done,
    checkinAt: done ? Date.now() - (24 - hour) * 3600 * 1000 : null,
    edition: null, streakDays: streak,
    dailyCheckin: done ? { at: Date.now(), completed: true, completedCount: 1, totalCount: 1, sourcesComplete: true } : null,
  }
}

const CENTER = {
  daily: {
    // 与 providers 逐家相加一致（后端 todayDone / todayEligible 就是各家之和）
    todayDone: 38,
    todayEligible: 44,
    outOfScope: [{ label: '智谱清言', reason: '访客账号没有积分体系', count: 16 }],
    providers: [
      provider('workbuddy', 'WorkBuddy 国内版', [
        account('w1', '138****2048', true, 8, 12), account('w2', '139****7781', true, 8, 5),
        account('w3', '186****3092', true, 9, 3), account('w4', '137****5520', true, 9, 1),
      ], 4, { description: '腾讯每日签到接口 · 仅国内版', modes: ['keepalive', 'claim'] }),
      provider('workbuddy-intl', 'WorkBuddy 国际版', Array.from({ length: 12 }, (_, i) =>
        account(`wi${i}`, `user${i + 1}@example.com`, true, 8 + (i % 4), i % 7)), 12, {
        description: '每日活跃任务 · 网页会话保活点亮日活，领取才加分',
        modes: ['keepalive', 'claim'],
      }),
      provider('raccoon', '小浣熊', [
        account('r1', 'raccoon_01', true, 8), account('r2', 'raccoon_02', true, 8),
        account('r3', 'raccoon_03', true, 9), account('r4', 'raccoon_04', true, 9),
      ], 4),
      provider('autoclaw', 'AutoClaw 国内版', [account('a1', 'claw_01', true, 8), account('a2', 'claw_02', true, 8)], 2),
      provider('autoclaw-intl', 'AutoClaw 国际版', [account('a3', 'claw_intl_01', true, 9)], 1),
      provider('qoder', 'Qoder 中国版', [
        account('q1', 'dev@example.com', true, 7), account('q2', 'ops@example.com', true, 8),
        account('q3', 'qa@example.com', true, 9),
      ], 3),
      provider('chatglm', '智谱清言', [
        account('g1', 'glm_01', true, 8), account('g2', 'glm_02', true, 8),
        account('g3', 'glm_03', true, 9), account('g4', 'glm_04', true, 9),
      ], 4),
      provider('cline-free', 'Cline 免费池', [
        ...Array.from({ length: 8 }, (_, i) => account(`c${i}`, `cline_0${i + 1}`, true, 8)),
        ...Array.from({ length: 6 }, (_, i) => account(`ct${i}`, `cline_${i + 9}`, false, 0)),
      ], 8, { description: '免费额度池 · 今日还有 6 个账号未签' }),
    ],
  },
  extras: {
    onboarding: [
      { id: 'w1', name: '138****2048', provider: 'workbuddy', settled: null },
      { id: 'wi0', name: 'user1@example.com', provider: 'workbuddy-intl', settled: null },
      { id: 'g1', name: 'glm_01', provider: 'chatglm', settled: null },
    ],
    plans: [
      { id: 'q1', name: 'dev@example.com', provider: 'qoder', claimAt: null, claimPlans: null },
    ],
    campaigns: [
      { id: 'wi1', name: 'user2@example.com', provider: 'workbuddy-intl', available: true },
      { id: 'a1', name: 'claw_01', provider: 'autoclaw', available: true },
    ],
    growth: [
      { id: 'w1', name: '138****2048', provider: 'workbuddy' },
      { id: 'w2', name: '139****7781', provider: 'workbuddy' },
      { id: 'wi0', name: 'user1@example.com', provider: 'workbuddy-intl' },
      { id: 'r1', name: 'raccoon_01', provider: 'raccoon' },
      { id: 'q1', name: 'dev@example.com', provider: 'qoder' },
    ],
  },
  auto: { enabled: true, running: false, schedules: [{ id: 's1', enabled: true, nextRunAt: Date.now() + 6 * 3600 * 1000, providerRows: [] }], defaultRetryTime: '00:10' },
  keepalive: { models: [], times: null },
  travel: { times: null, enabled: false },
  history: [],
  adminPanels: [],
  checkinFeatures: { workbuddy: { description: '腾讯每日签到接口' } },
}

/* 壳侧接口：组件只读快照与几个动作，这里给最小实现 */
const w = window as unknown as Record<string, unknown>
/* 预览没有 ui/i18n.js，补一个最小的 t()：中文即键，只需替换 {name} 占位符 */
w.wbI18n = {
  t: (key: string, params?: Record<string, string | number>) => key.replace(/\{(\w+)\}/g, (_, name: string) =>
    params && name in params ? String(params[name]) : `{${name}}`),
  locale: () => 'zh-Hans',
  rawLocale: () => 'zh-Hans',
  setLocale: () => undefined,
  sweep: () => undefined,
  LOCALES: [],
}
w.wbApp = { showPage: () => undefined, toast: () => undefined, currentPage: 'checkin', getState: () => ({ accounts: { accounts: [] } }) }
w.wbProviders = {
  labelOf: (id: string) => ({
    workbuddy: 'WorkBuddy 国内版', 'workbuddy-intl': 'WorkBuddy 国际版', raccoon: '小浣熊',
    autoclaw: 'AutoClaw 国内版', 'autoclaw-intl': 'AutoClaw 国际版', qoder: 'Qoder 中国版',
    chatglm: '智谱清言', 'cline-free': 'Cline 免费池',
  } as Record<string, string>)[id] || id,
}
w.workbuddyDesktop = {
  getCheckinCenter: async () => CENTER,
  getCheckinRecords: async () => ({ records: [], total: 1700 }),
  getOnboardingTasks: async () => ({ tasks: [], earned: 0 }),
  getGrowthTasks: async () => ({ tasks: [] }),
  getCampaignRewards: async () => ({ campaigns: [], riskNote: null }),
}

/* 预览外壳：左侧主导航 + 顶部标签 + 页面容器 */
const App = React.lazy(() => import('./shell'))

createRoot(document.getElementById('root')!).render(
  <React.Suspense fallback={null}><App /></React.Suspense>,
)
