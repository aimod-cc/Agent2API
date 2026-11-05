import { InputGroup, InputGroupInput, InputGroupAddon, InputGroupButton } from '@ui'
import { useMemo, useState } from 'react'
import { PageHead, IconButton, Badge, Empty, ErrorNote, SkeletonList } from '../components/ui'
import { ProviderLogo } from '../components/ProviderLogo'
import { HiddenAccountsNotice } from '../components/HiddenAccountsNotice'
import { Icon } from '../components/Icon'
import { AccountSheet } from './AccountSheet'
import { DynamicProviderSheet } from './add-account-dynamic-sheet'
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
import { useDisplayClock } from '../../../ui-shared/use-display-clock'
import { credentialExpiryRowsOf, credentialExpiryText } from '../../../ui-shared/credential-expiry'
import { upstreamCooldownOf, upstreamSuspendedReasonOf } from '../../../ui-shared/account-upstream-state'
import { providerLabel } from '../lib/providers'
import { usePolling } from '../lib/usePolling'
import { usePullToRefresh, PullIndicator } from '../components/PullToRefresh'
import { t } from '../i18n'
import { Button, Input, Card } from '@ui'
import { Switch as UiSwitch } from '@ui'
import { Toggle as UiToggle } from '@ui'

/**
 * 账号。
 *
 * ── 为什么是分组卡片而不是表格 ────────────────────────────────
 * 桌面端账号表有 10 列（顺序 / 账号 / 代理 / 连接数 / 状态 / 限流 / 有效期 /
 * 余额 / 操作…），压不进 390px。手机上只留五个关键读数：邮箱、状态、有效期、
 * 余额、代理；提供商信息上移到分组头，行里不再重复。
 *
 * ── 分组口径 ──────────────────────────────────────────────────
 * 组内使用网关的全局消耗顺序，组顺位仍由账号库的手动优先级决定。
 */

type Filter = 'all' | 'enabled' | 'limited' | 'disabled'

export function AccountsPage() {
  const state = useStore()
  const [filters, setFilters] = useState<AccountFilters>(DEFAULT_FILTERS)
  const [filterOpen, setFilterOpen] = useState(false)
  const [query, setQuery] = useState('')
  const [openId, setOpenId] = useState<string | null>(null)
  const [adding, setAdding] = useState(false)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [usage, setUsage] = useState<UsageSnapshot | null>(null)
  const accounts = state.session?.accounts?.accounts || []
  const sessionSorting = state.session?.accounts?.sorting
  const sorting = (usage?.sorting?.updatedAt || 0) > (sessionSorting?.updatedAt || 0) ? usage?.sorting : sessionSorting

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
      list.push({ id, label: providerLabel(id, state.session?.accounts?.providers?.find(provider => provider.id === id)?.label) })
    })
    return list
  }, [accounts, state.session?.accounts?.providers])

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
      .map(([provider, list]) => {
        const positions = new Map((sorting?.accountOrder[provider] || []).map((id, index) => [id, index]))
        return {
          provider,
          list: [...list].sort((a, b) => positions.size
            ? (positions.get(a.id) ?? Infinity) - (positions.get(b.id) ?? Infinity)
            : (a.priority ?? 999) - (b.priority ?? 999)),
        }
      })
      .sort((a, b) => sorting
        ? (sorting.providerOrder.indexOf(a.provider) < 0 ? Infinity : sorting.providerOrder.indexOf(a.provider))
          - (sorting.providerOrder.indexOf(b.provider) < 0 ? Infinity : sorting.providerOrder.indexOf(b.provider))
        : Math.min(...a.list.map(account => account.priority ?? 999)) - Math.min(...b.list.map(account => account.priority ?? 999)))
  }, [filtered, sorting])

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
            <Button variant='outline' size='sm' onClick={() => setAdding(true)}>{t('添加账号')}</Button>
            <Button size='icon-touch' variant='outline'
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
            </Button>
            <IconButton
              name='refresh'
              label={t('刷新')}
              onClick={() => void load()}
            />
          </>
        }
      />

      <div className='m-section' style={{ marginTop: 0 }}>
        <InputGroup>
          <InputGroupInput value={query} onChange={event => setQuery(event.target.value)} placeholder={t('搜索账号 / 邮箱 / 备注')} />
          <InputGroupAddon><Icon name='search' style={{ width: 16, height: 16 }} /></InputGroupAddon>
          {query ? <InputGroupAddon align='inline-end'><InputGroupButton size='icon-xs' onClick={() => setQuery('')} aria-label={t('清除')}><Icon name='close' /></InputGroupButton></InputGroupAddon> : null}
        </InputGroup>
      </div>

      {/* 生效中的条件用一条摘要显示：横排 chip 只留「全部」与当前选中的，
          避免四颗常驻 chip 与面板里的选项各说一套 */}
      <div className='m-chips' style={{ padding: '10px 16px 12px' }}>
        <UiToggle size='default' variant='outline'
          type='button'
          className='m-chip rounded-pill'
          pressed={activeFilterCount(filters) === 0}
          onClick={() => setFilters(DEFAULT_FILTERS)}
        >
          {t('全部')}
          <span className='m-n'>{accounts.length}</span>
        </UiToggle>
        {activeFilterCount(filters) > 0 ? (
          <UiToggle size='default' variant='outline' type='button' className='m-chip rounded-pill' pressed onClick={() => setFilterOpen(true)}>
            <Icon name='filter' style={{ width: 13, height: 13 }} />
            {t('已筛选 {n} 项', { n: filtered.length })}
          </UiToggle>
        ) : null}
      </div>
      </div>

      {/* 只有这一块跟着手指滚：筛选条常驻，换筛选不用先滑回顶部 */}
      <div className='m-body'>
      {error || state.sessionError ? <ErrorNote text={error || state.sessionError} /> : null}
      <HiddenAccountsNotice hidden={state.session?.accounts?.hiddenAccounts} />
      {!state.session && state.loading ? <SkeletonList rows={4} height={72} /> : null}

      {state.session && !groups.length && (!state.session.accounts?.hiddenAccounts?.total || activeFilterCount(filters) > 0 || query) ? (
        <div className='m-section'>
          <Card className='m-card'>
            <Empty
              text={
                query
                  ? t('没有匹配「{kw}」的账号', { kw: query })
                  : activeFilterCount(filters) > 0
                    ? t('当前筛选下没有账号')
                    : t('还没有账号，请点击「添加账号」')
              }
            />
          </Card>
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
                <div style={{ fontSize: 13, fontWeight: 600 }}>{providerLabel(group.provider, state.session?.accounts?.providers?.find(provider => provider.id === group.provider)?.label)}</div>
                <div style={{ fontSize: 11.5, color: 'var(--m-text-3)', marginTop: 2 }}>
                  {t('{n} 个账号 · 启用 {e}', { n: group.list.length, e: enabled })}
                  {limited ? t(' · 限流 {n}', { n: limited }) : ''}
                </div>
              </div>
            </div>
            <Card className='m-card'>
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
            </Card>
          </div>
        )
      })}

      </div>

      {adding ? <DynamicProviderSheet onClose={() => setAdding(false)} onAdded={() => {
        setAdding(false)
        void load()
      }} /> : null}
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
  const now = useDisplayClock()
  const limited = isRateLimited(account)
  const cooldown = upstreamCooldownOf(account)
  const suspendedReason = upstreamSuspendedReasonOf(account)
  const expiresAt = expiryOf(account)
  const expired = expiresAt > 0 && expiresAt <= Date.now()
  const tone = account.needsRelogin || suspendedReason
    ? 'bad'
    : !account.enabled
      ? 'mute'
      : limited
        ? 'warn'
        : expired
          ? 'bad'
          : 'ok'
  const badgeText = account.needsRelogin
    ? t('需重登')
    : suspendedReason
      ? t('上游限制 · 已禁用')
      : !account.enabled
        ? t('已禁用')
        : cooldown
          ? t('账号冷却')
          : limited
            ? t('限流')
            : expired
              ? t('已过期')
              : t('正常')
  // 有效期：这家没有这个概念时不显示，而不是显示「—」（少一行噪音）；
  // 长期有效的家写「长期有效」—— 与「—」（读不到到期时间）是两件事
  const credentialRows = credentialExpiryRowsOf(account, expiresAt)
  const expiryText = credentialRows.length ? '' : isLongLivedCredential(account.provider)
    ? t('长期有效')
    : hasExpiry(account.provider) ? formatExpiry(expiresAt) : ''
  // 余额：只有这家有查询能力时才占一行；快照里没有这一行 = 还没查过
  const usageView = usageText(usageRow, now)
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
      <Button size='default' variant='ghost'
        type='button'
        onClick={onOpen}
        style={{ flex: 1, minWidth: 0, height: 'auto', display: 'flex', alignItems: 'center', gap: 11, background: 'none', border: 0, padding: 0, textAlign: 'left' }}
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
              <span style={{ flex: 'none', color: usageView.failureStatus ? 'var(--m-text-3)' : usageColor }} title={usageView.detail}>· {usageView.text}</span>
            ) : null}
          </div>
          {showUsage && usageView.expiry ? (
            <div className='mt-1.5 text-xs text-warning whitespace-normal break-words' title={usageView.expiry.detail}>
              <Badge tone='warn'>{usageView.expiry.badge}</Badge>
              {usageView.expiry.today ? <span className='ml-1.5'>{usageView.expiry.today}</span> : null}
            </div>
          ) : null}
          {showUsage && usageView.failureStatus ? (
            <div className='text-xs text-destructive whitespace-normal break-words mt-1' title={usageView.failureDetail}>
              {usageView.failureStatus}
            </div>
          ) : null}
          {credentialRows.length ? <div className='flex flex-col gap-1 mt-1.5 text-[11px]'>
            {credentialRows.map(row => <div key={row.label} className='flex items-center gap-2'>
              <span className='text-subtle' translate='no'>{row.label}</span>
              <span className={row.expiresAt !== null && row.expiresAt <= Date.now() ? 'text-destructive' : 'text-subtle'}>
                {credentialExpiryText(row, t)}
              </span>
            </div>)}
          </div> : null}
          {suspendedReason || cooldown ? (
            <div className={`text-xs ${suspendedReason ? 'text-destructive' : 'text-warning'} whitespace-normal break-words mt-1`}>
              {suspendedReason || [t('{time} 恢复', { time: formatRelative(cooldown?.resetAt) }), cooldown?.message].filter(Boolean).join(' · ')}
            </div>
          ) : null}
          {account.needsRelogin ? (
            <div className='text-xs text-destructive whitespace-normal break-words mt-1'>
              {account.needsReloginReason || t('登录授权已失效，请重新登录；自动续期无法恢复已过期或已撤销的刷新令牌。')}
            </div>
          ) : null}
        </div>
      </Button>
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
    <UiSwitch size='touch' variant='success' checked={checked} aria-label={label} disabled={disabled} onCheckedChange={onChange} />
  )
}
