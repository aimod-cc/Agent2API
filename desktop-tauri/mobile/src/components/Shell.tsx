import { useEffect, useState } from 'react'
import { Icon, type IconName } from './Icon'
import { useStore, store } from '../state/store'
import { LoginPage } from '../pages/LoginPage'
import { t } from '../i18n'

/**
 * 应用外壳：页面容器 + 底部标签栏 + 轻提示 + 凭证缺失层。
 *
 * ── 标签栏的显示规则 ─────────────────────────────────────────
 * 一级页显示，二级页隐藏（把高度让给内容）。助手页要撑满视口，所以它的
 * 滚动容器自己管（见 pages/AssistantPage 的 as-mode 处理）。
 */

export type TabDef = { id: string; label: string; icon: IconName }

const TABS: TabDef[] = [
  { id: 'home', label: t('首页'), icon: 'home' },
  { id: 'report', label: t('报表'), icon: 'chart' },
  { id: 'accounts', label: t('账号'), icon: 'users' },
  { id: 'assistant', label: t('助手'), icon: 'spark' },
  { id: 'more', label: t('更多'), icon: 'grid' },
]

/** 一级页集合：这些 screen 显示底部标签栏 */
export const PRIMARY_SCREENS = new Set(TABS.map(tab => tab.id))

export function TabBar() {
  const state = useStore()
  const active = state.stack.length === 0 ? state.route.screen : ''
  if (!PRIMARY_SCREENS.has(state.route.screen) || state.stack.length > 0) return null
  const limited = state.session?.accounts?.accounts?.filter(a => a.enabled && hasRateLimit(a)).length || 0
  return (
    <nav className='m-tabbar' aria-label={t('主导航')}>
      {TABS.map(tab => (
        <button
          key={tab.id}
          type='button'
          className='m-tab'
          data-active={active === tab.id}
          onClick={() => store.go(tab.id)}
          aria-current={active === tab.id ? 'page' : undefined}
        >
          <Icon name={tab.icon} />
          <span>{tab.label}</span>
          {tab.id === 'more' && limited > 0 ? <i className='m-dot' /> : null}
        </button>
      ))}
    </nav>
  )
}

function hasRateLimit(account: { rateLimits?: Record<string, unknown> }): boolean {
  return Boolean(account.rateLimits && Object.keys(account.rateLimits).length)
}

/** 轻提示 */
export function Toast() {
  const state = useStore()
  const toast = state.toast
  return (
    <div className='m-toast' data-show={Boolean(toast)} role='status' aria-live='polite'>
      {toast?.text || ''}
    </div>
  )
}

/**
 * 凭证缺失层。
 *
 * 两种来路，文案与动作都不同：
 *   · `login` —— 管理员已注册、会话过期：APK 弹应用内登录表单；
 *     浏览器（PWA）跳服务端登录页，带回跳。
 *   · `key`   —— 兼容模式（没注册管理员）没配 Key：就地输入，存 localStorage
 *
 * 为什么 APK 不能跳 `/login`：那个页面由网关出，而 APK 的界面在包里 ——
 * 跳过去等于离开应用、进一个风格不一致的网页，而且跨源之后服务端种的
 * cookie 也带不回来（见 lib/servers.ts 的说明）。
 */
export function AuthGate() {
  const state = useStore()
  const [key, setKey] = useState('')
  const [busy, setBusy] = useState(false)
  if (!state.authLost) return null

  // 打包形态走应用内登录页（login / key 两种模式它都覆盖）
  if (state.packaged) return <LoginPage mode={state.authLost} />

  const submit = async () => {
    const value = key.trim()
    if (!value) return
    setBusy(true)
    const { storeApiKey } = await import('../api/client')
    storeApiKey(value)
    store.setAuthLost(null)
    await store.refresh(true)
    setBusy(false)
  }

  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 80,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        padding: 20,
        background: 'color-mix(in srgb, black 46%, transparent)',
        backdropFilter: 'blur(4px)',
      }}
    >
      <div
        style={{
          width: '100%',
          maxWidth: 340,
          padding: 20,
          borderRadius: 18,
          background: 'var(--m-surface)',
          border: '1px solid var(--m-line)',
          boxShadow: 'var(--m-shadow-2)',
        }}
      >
        <h2 style={{ margin: 0, fontSize: 17 }}>
          {state.authLost === 'login' ? t('需要重新登录') : t('需要网关 API Key')}
        </h2>
        <p style={{ margin: '8px 0 0', fontSize: 12.5, lineHeight: 1.65, color: 'var(--m-text-2)' }}>
          {state.authLost === 'login'
            ? t('会话已过期。重新登录后会自动回到手机端。')
            : t('这个面板没有管理员，用一把已启用的 API Key 访问（在桌面端「网关 Key」页创建）。')}
        </p>
        {state.authLost === 'key' ? (
          <input
            type='password'
            value={key}
            onChange={event => setKey(event.target.value)}
            placeholder='sk-…'
            autoComplete='off'
            style={{
              width: '100%',
              boxSizing: 'border-box',
              marginTop: 14,
              height: 42,
              padding: '0 12px',
              borderRadius: 11,
              border: '1px solid var(--m-line-strong)',
              background: 'var(--m-surface-2)',
              color: 'var(--m-text)',
              fontFamily: 'var(--m-mono)',
              // 16px 是 iOS 的阈值：小于它聚焦时 Safari 会自动放大整页
              fontSize: 16,
            }}
          />
        ) : null}
        <div style={{ display: 'flex', gap: 8, marginTop: 16 }}>
          {state.authLost === 'login' ? (
            <button
              type='button'
              className='m-btn'
              onClick={() => {
                window.location.href = '/login?next=%2Fm'
              }}
            >
              {t('去登录')}
            </button>
          ) : (
            <button type='button' className='m-btn' onClick={() => void submit()} disabled={busy || !key.trim()}>
              {busy ? t('验证中…') : t('保存并继续')}
            </button>
          )}
        </div>
      </div>
    </div>
  )
}

/**
 * 主题同步：把 store 的主题写到 <html>。
 *
 * 单独一个组件而不是在 store 构造时做，是为了让 React 的首帧与 DOM 属性
 * 一致 —— 首帧闪烁（先浅色再切深色）在手机上是可见的。
 */
export function ThemeSync() {
  const { theme } = useStore()
  useEffect(() => {
    document.documentElement.dataset.theme = theme
  }, [theme])
  return null
}
