import { useMemo, useState } from 'react'
import { PageHead, IconButton, Badge, Empty, ErrorNote, SkeletonList } from '../components/ui'
import { ProviderLogo } from '../components/ProviderLogo'
import { Icon } from '../components/Icon'
import { AccountSheet } from './AccountSheet'
import {
  AccountsFilterSheet,
  DEFAULT_FILTERS,
  activeFilterCount,
  type AccountFilters,
} from '../components/AccountsFilterSheet'
import { useStore, store } from '../state/store'
import {
  fetchUsageSnapshot,
  setAccountEnabled,
  type Account,
  type UsageRow,
  type UsageSnapshot,
} from '../api/gateway'
import { accountName, formatExpiry, formatRelative, isRateLimited } from '../lib/format'
import { expiryOf, hasExpiry, isLongLivedCredential, supportsUsage } from '../lib/providerFacts'
import { usageIndex, usageText } from '../lib/usage'
import { providerLabel } from '../lib/providers'
import { usePolling } from '../lib/usePolling'
import { usePullToRefresh, PullIndicator } from '../components/PullToRefresh'
import { t } from '../i18n'

/**
 * 账号。
 *
 * ── 为什么是分组卡片而不是表格 ────────────────────────────────
 * 桌面端账号表有 10 列（顺序 / 账号 / 代理 / 连接数 / 状态 / 限流 / 有效期 /
 * 余额 / 操作…），压不进 390px。手机上只留五个关键读数：邮箱、状态、有效期、
 * 余额、代理；提供商信息上移到分组头，行里不再重复。
 *
 * ── 分组口径 ──────────────────────────────────────────────────
 * 与后端一致：同一提供商的账号是一组，组内按 priority 升序（= 转发尝试顺序）。
 * 组本身的先后也按最小 priority 排 —— 这就是全局队列在手机上的投影。
 */

type Filter = 'all' | 'enabled' | 'limited' | 'disabled'

export function AccountsPage() {
  const state = useStore()
  const [filters, setFilters] = useState<AccountFilters>(DEFAULT_FILTERS)
  const [filterOpen, setFilterOpen] = useState(false)
  const [query, setQuery] = useState('')
  const [openId, setOpenId] = useState<string | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [usage, setUsage] = useState<UsageSnapshot | null>(null)
  const accounts = state.session?.accounts?.accounts || []

  const load = async () => {
    // 余额快照与账号刷新并行：快照失败不影响账号列表（余额查不到是常态，
    // 静默降级成「未查询」，不弹 toast 也不进页内错误条）
    void fetchUsageSnapshot()
      .then(setUsage)
      .catch(() => {})
    try {
      await store.refresh(true)
      setError('')
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
    }
  }

  usePolling(load, 30_000)

  const pull = usePullToRefresh({ onRefresh: load })

  // 快照行按 id 索引：每次 render 重建 Map 太浪费，且行组件会因此白白重渲
  const usageById = useMemo(() => usageIndex(usage), [usage])

  /** 这家有没有「有效期」概念、是不是长期有效、到期时刻 —— 筛选与行渲染共用 */
  const expiryInfo = (account: Account) => {
    const at = expiryOf(account)
    return {
      at,
      has: hasExpiry(account.provider),
      longLived: isLongLivedCredential(account.provider),
      expired: at > 0 && at <= Date.now(),
    }
  }

  /** 余额读数归成三档（未查询 / 失败 / 有数）—— 与 usageText 的 kind 对应 */
  const usageState = (account: Account): 'has' | 'unqueried' | 'failed' => {
    if (!supportsUsage(account)) return 'unqueried'
    const row = usageById.get(account.id)
    if (!row) return 'unqueried'
    if (row.error) return 'failed'
    return 'has'
  }

  /** 各维度选项的计数：基于**搜索词过滤后**的集合，数字与点进去看到的一致 */
  const counts = useMemo(() => {
    const keyword = query.trim().toLowerCase()
    const base = accounts.filter(item => {
      if (!keyword) return true
      const haystack = [item.name, item.nickname, item.email, item.id, item.provider]
        .filter(Boolean)
        .join(' ')
        .toLowerCase()
      return haystack.includes(keyword)
    })
    const expiryOfAccount = (account: Account) => {
      const at = expiryOf(account)
      return {
        at,
        longLived: isLongLivedCredential(account.provider),
        expired: at > 0 && at <= Date.now(),
        soon: at > Date.now() && at - Date.now() <= 7 * 86_400_000,
      }
    }
    const usageOfAccount = (account: Account): 'has' | 'unqueried' | 'failed' => {
      if (!supportsUsage(account)) return 'unqueried'
      const row = usageById.get(account.id)
      if (!row) return 'unqueried'
      if (row.error) return 'failed'
      return 'has'
    }
    return {
      status: {
        all: base.length,
        enabled: base.filter(item => item.enabled).length,
        disabled: base.filter(item => !item.enabled).length,
        limited: base.filter(item => item.enabled && isRateLimited(item)).length,
        relogin: base.filter(item => item.needsRelogin).length,
        expired: base.filter(item => expiryOfAccount(item).expired).length,
      },
      expiry: {
        all: base.length,
        soon: base.filter(item => expiryOfAccount(item).soon).length,
        expired: base.filter(item => expiryOfAccount(item).expired).length,
        longlived: base.filter(item => expiryOfAccount(item).longLived).length,
      },
      usage: {
        all: base.length,
        has: base.filter(item => usageOfAccount(item) === 'has').length,
        unqueried: base.filter(item => usageOfAccount(item) === 'unqueried').length,
        failed: base.filter(item => usageOfAccount(item) === 'failed').length,
      },
    }
    // usageById 变化（余额刷新）要重算余额那组计数
  }, [accounts, query, usageById])

  /** 可选的提供商（按当前账号里出现过的去重，保持后端给的先后） */
  const providerOptions = useMemo(() => {
    const seen = new Set<string>()
    const list: Array<{ id: string; label: string }> = []
    accounts.forEach(item => {
      const id = String(item.provider || 'unknown')
      if (seen.has(id)) return
      seen.add(id)
      list.push({ id, label: providerLabel(id) })
    })
    return list
  }, [accounts])

  const filtered = useMemo(() => {
    const keyword = query.trim().toLowerCase()
    return accounts.filter(item => {
      // ① 状态（单选）
      if (filters.status === 'enabled' && !item.enabled) return false
      if (filters.status === 'disabled' && item.enabled) return false
      if (filters.status === 'limited' && !(item.enabled && isRateLimited(item))) return false
      if (filters.status === 'relogin' && !item.needsRelogin) return false
      if (filters.status === 'expired' && !expiryInfo(item).expired) return false
      // ② 提供商（多选，空 = 不限）
      if (filters.providers.length && !filters.providers.includes(String(item.provider || 'unknown'))) {
        return false
      }
      // ③ 有效期
      if (filters.expiry !== 'all') {
        const info = expiryInfo(item)
        if (filters.expiry === 'longlived' && !info.longLived) return false
        if (filters.expiry === 'expired' && !info.expired) return false
        if (filters.expiry === 'soon') {
          if (!(info.at > Date.now() && info.at - Date.now() <= 7 * 86_400_000)) return false
        }
      }
      // ④ 余额
      if (filters.usage !== 'all' && usageState(item) !== filters.usage) return false
      // ⑤ 搜索词
      if (!keyword) return true
      const haystack = [item.name, item.nickname, item.email, item.id, item.provider]
        .filter(Boolean)
        .join(' ')
        .toLowerCase()
      return haystack.includes(keyword)
    })
    // expiryInfo / usageState 是纯函数（只读 Date.now 与 props），不必进依赖
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [accounts, filters, query, usageById])

  const groups = useMemo(() => {
    const map = new Map<string, Account[]>()
    filtered.forEach(item => {
      const provider = String(item.provider || 'unknown')
      const list = map.get(provider) || []
      list.push(item)
      map.set(provider, list)
    })
    return [...map.entries()]
      .map(([provider, list]) => ({
        provider,
        list: [...list].sort((a, b) => (a.priority ?? 999) - (b.priority ?? 999)),
      }))
      .sort((a, b) => (a.list[0]?.priority ?? 999) - (b.list[0]?.priority ?? 999))
  }, [filtered])

  const enabledCount = accounts.filter(item => item.enabled).length
  const limitedCount = accounts.filter(item => item.enabled && isRateLimited(item)).length

  const toggle = async (account: Account, next: boolean) => {
    setBusy(true)
    try {
      await setAccountEnabled(account.id, next)
      store.toast(next ? t('已启用 {name}', { name: accountName(account) }) : t('已禁用 {name}', { name: accountName(account) }))
      await store.refresh(true)
    } catch (failure) {
      store.toast(failure instanceof Error ? failure.message : String(failure), 'err')
    } finally {
      setBusy(false)
    }
  }

  const openAccount = openId ? accounts.find(item => item.id === openId) || null : null

  return (
    <div className='m-page m-fixed' ref={pull.bindRef}>
      <PullIndicator pull={pull.pull} refreshing={pull.refreshing} />
      <div className='m-sticky'>
      <PageHead
        title={t('账号')}
        subtitle={t('{n} 个账号 · {e} 启用 · {l} 限流', { n: accounts.length, e: enabledCount, l: limitedCount })}
        actions={
          <>
            <button
              type='button'
              className='m-icon-btn'
              aria-label={t('筛选')}
              title={t('筛选')}
              onClick={() => setFilterOpen(true)}
              style={{ position: 'relative' }}
            >
              <Icon name='filter' />
              {activeFilterCount(filters) > 0 ? (
                <span
                  style={{
                    position: 'absolute',
                    top: -2,
                    right: -2,
                    minWidth: 16,
                    height: 16,
                    padding: '0 4px',
                    borderRadius: 8,
                    background: 'var(--m-primary)',
                    color: '#fff',
                    fontSize: 10,
                    fontWeight: 700,
                    display: 'grid',
                    placeItems: 'center',
                  }}
                >
                  {activeFilterCount(filters)}
                </span>
              ) : null}
            </button>
            <IconButton
              name='refresh'
              label={t('刷新')}
              onClick={() => {
                void store.refresh(true)
                store.toast(t('已刷新'))
              }}
            />
          </>
        }
      />

      <div className='m-section' style={{ marginTop: 0 }}>
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 8,
            height: 38,
            padding: '0 12px',
            borderRadius: 11,
            background: 'var(--m-surface)',
            border: '1px solid var(--m-line)',
            color: 'var(--m-text-3)',
          }}
        >
          <Icon name='search' style={{ width: 16, height: 16, flex: 'none' }} />
          <input
            value={query}
            onChange={event => setQuery(event.target.value)}
            placeholder={t('搜索账号 / 邮箱 / 备注')}
            style={{
              flex: 1,
              minWidth: 0,
              border: 0,
              outline: 'none',
              background: 'transparent',
              color: 'var(--m-text)',
              fontSize: 16,
            }}
          />
          {query ? (
            <button type='button' onClick={() => setQuery('')} aria-label={t('清除')} style={{ display: 'flex', border: 0, background: 'none', color: 'var(--m-text-3)' }}>
              <Icon name='close' style={{ width: 15, height: 15 }} />
            </button>
          ) : null}
        </div>
      </div>

      {/* 生效中的条件用一条摘要显示：横排 chip 只留「全部」与当前选中的，
          避免四颗常驻 chip 与面板里的选项各说一套 */}
      <div className='m-chips' style={{ padding: '10px 16px 12px' }}>
        <button
          type='button'
          className='m-chip'
          data-on={activeFilterCount(filters) === 0}
          onClick={() => setFilters(DEFAULT_FILTERS)}
        >
          {t('全部')}
          <span className='m-n'>{accounts.length}</span>
        </button>
        {activeFilterCount(filters) > 0 ? (
          <button type='button' className='m-chip' data-on onClick={() => setFilterOpen(true)}>
            <Icon name='filter' style={{ width: 13, height: 13 }} />
            {t('已筛选 {n} 项', { n: filtered.length })}
          </button>
        ) : null}
      </div>
      </div>

      {/* 只有这一块跟着手指滚：筛选条常驻，换筛选不用先滑回顶部 */}
      <div className='m-body'>
      {error ? <ErrorNote text={error} /> : null}
      {!state.session && state.loading ? <SkeletonList rows={4} height={72} /> : null}

      {state.session && !groups.length ? (
        <div className='m-section'>
          <div className='m-card'>
            <Empty
              text={
                query
                  ? t('没有匹配「{kw}」的账号', { kw: query })
                  : activeFilterCount(filters) > 0
                    ? t('当前筛选下没有账号')
                    : t('还没有账号，去桌面端添加')
              }
            />
          </div>
        </div>
      ) : null}

      {groups.map(group => {
        const enabled = group.list.filter(item => item.enabled).length
        const limited = group.list.filter(item => item.enabled && isRateLimited(item)).length
        return (
          <div key={group.provider} className='m-section' style={{ marginTop: 12 }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 9, padding: '0 3px 8px' }}>
              <ProviderLogo id={group.provider} />
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontSize: 13, fontWeight: 600 }}>{providerLabel(group.provider)}</div>
                <div style={{ fontSize: 11.5, color: 'var(--m-text-3)', marginTop: 2 }}>
                  {t('{n} 个账号 · 启用 {e}', { n: group.list.length, e: enabled })}
                  {limited ? t(' · 限流 {n}', { n: limited }) : ''}
                </div>
              </div>
            </div>
            <div className='m-card'>
              {group.list.map((account, index) => (
                <AccountRow
                  key={account.id}
                  account={account}
                  rank={index + 1}
                  usageRow={usageById.get(account.id)}
                  onOpen={() => setOpenId(account.id)}
                  onToggle={next => void toggle(account, next)}
                  busy={busy}
                />
              ))}
            </div>
          </div>
        )
      })}

      </div>

      <AccountSheet
        account={openAccount}
        onClose={() => setOpenId(null)}
        onChanged={() => void store.refresh(true)}
      />
      <AccountsFilterSheet
        open={filterOpen}
        onClose={() => setFilterOpen(false)}
        filters={filters}
        onChange={setFilters}
        providers={providerOptions}
        counts={counts}
      />
    </div>
  )
}

function AccountRow({
  account,
  rank,
  usageRow,
  onOpen,
  onToggle,
  busy,
}: {
  account: Account
  /** 组内序号（1 起）：后端 priority 是全局唯一队列里的绝对位次，手机上按组内序号读 */
  rank: number
  /** 该账号在余额快照里的那一行；快照没有这个账号时是 undefined（= 未查询） */
  usageRow?: UsageRow
  onOpen: () => void
  onToggle: (next: boolean) => void
  busy: boolean
}) {
  const limited = isRateLimited(account)
  const expiresAt = expiryOf(account)
  const expired = expiresAt > 0 && expiresAt <= Date.now()
  const tone = !account.enabled
    ? 'mute'
    : account.needsRelogin
      ? 'bad'
      : limited
        ? 'warn'
        : expired
          ? 'bad'
          : 'ok'
  const badgeText = !account.enabled
    ? t('已禁用')
    : account.needsRelogin
      ? t('需重登')
      : limited
        ? t('限流')
        : expired
          ? t('已过期')
          : t('正常')
  // 有效期：这家没有这个概念时不显示，而不是显示「—」（少一行噪音）；
  // 长期有效的家写「长期有效」—— 与「—」（读不到到期时间）是两件事
  const expiryText = isLongLivedCredential(account.provider)
    ? t('长期有效')
    : hasExpiry(account.provider) ? formatExpiry(expiresAt) : ''
  // 余额：只有这家有查询能力时才占一行；快照里没有这一行 = 还没查过
  const usageView = usageText(usageRow)
  const showUsage = supportsUsage(account)
  const usageColor =
    usageView.kind === 'bad'
      ? 'var(--m-danger)'
      : usageView.kind === 'warn'
        ? 'var(--m-warn)'
        : 'var(--m-text-3)'

  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 11,
        padding: '12px 13px',
        borderBottom: '1px solid var(--m-line)',
        opacity: account.enabled ? 1 : 0.62,
      }}
    >
      <button
        type='button'
        onClick={onOpen}
        style={{ flex: 1, minWidth: 0, display: 'flex', alignItems: 'center', gap: 11, background: 'none', border: 0, padding: 0, textAlign: 'left' }}
      >
        <span style={{ width: 20, flex: 'none', fontSize: 11, color: 'var(--m-text-3)', fontVariantNumeric: 'tabular-nums', textAlign: 'center' }}>
          #{rank}
        </span>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ fontSize: 13.5, fontWeight: 550, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
            {accountName(account)}
          </div>
          <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginTop: 4, fontSize: 11.5, color: 'var(--m-text-3)', minWidth: 0 }}>
            {/* 状态 + 有效期这一截可以截断，余额读数不能被挤掉 */}
            <span
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 6,
                minWidth: 0,
                overflow: 'hidden',
                textOverflow: 'ellipsis',
                whiteSpace: 'nowrap',
              }}
            >
              <Badge tone={tone} dot>
                {badgeText}
              </Badge>
              {expiryText ? (
                <>
                  <span>·</span>
                  {/* 有效期这一截允许吃掉省略号，余额读数（flex:none）始终完整 */}
                  <span style={{ minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                    {expiryText}
                  </span>
                </>
              ) : null}
            </span>
            {showUsage ? (
              <span style={{ flex: 'none', color: usageColor }}>· {usageView.text}</span>
            ) : null}
          </div>
        </div>
      </button>
      <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'flex-end', gap: 5, flex: 'none' }}>
        <SwitchSmall checked={Boolean(account.enabled)} onChange={onToggle} disabled={busy} label={accountName(account)} />
        <span style={{ fontSize: 10.5, color: 'var(--m-text-3)' }}>
          {account.editionLabel || formatRelative(account.updatedAt as number)}
        </span>
      </div>
    </div>
  )
}

/** 小号开关：账号行里用它，尺寸比设置页的档位小一档 */
function SwitchSmall({
  checked,
  onChange,
  disabled,
  label,
}: {
  checked: boolean
  onChange: (next: boolean) => void
  disabled?: boolean
  label: string
}) {
  return (
    <button
      type='button'
      role='switch'
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      style={{
        width: 40,
        height: 24,
        borderRadius: 999,
        border: checked ? '0' : '1px solid var(--m-line-strong)',
        background: checked ? 'var(--m-ok)' : 'var(--m-surface-3)',
        position: 'relative',
        transition: 'background .18s, border-color .18s',
        padding: 0,
        flex: 'none',
      }}
    >
      <span
        style={{
          position: 'absolute',
          top: checked ? 2 : 2,
          left: checked ? 18 : 2,
          width: 20,
          height: 20,
          borderRadius: '50%',
          background: '#fff',
          boxShadow: '0 1px 3px rgba(0,0,0,.25)',
          transition: 'left .18s cubic-bezier(.3,.9,.4,1)',
        }}
      />
    </button>
  )
}
