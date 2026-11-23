import { useEffect, useMemo, useState } from 'react'
import {
  fetchAccounts,
  fetchCustomProviders,
  fetchUsageSnapshot,
  queryAccounts,
  refreshExpiringAccounts,
  setAccountEnabled,
  type Account,
  type AccountQueryCounts,
  type AccountQueryGroup,
  type AccountStatusFilter,
  type UsageRow,
} from '../core/api'
import { formatRelativeTime, providerDisplayName, syncProviderLabels } from '../core/format'
import { t } from '../core/i18n'
import { serverIdentity } from '../core/servers'
import { store, useStore } from '../core/store'
import {
  CommonScaffold,
  CommonSwitch,
  EmptyBlock,
  RecordCard,
  SearchField,
  SectionHeader,
  TonalChip,
  TonalIconButton,
} from '../ui/components'
import { GlyphIcon, ProviderAvatar } from '../ui/icons'
import {
  AccountsFilterSheet,
  activeAdvancedFilterCount,
  DEFAULT_ACCOUNT_FILTERS,
  type AccountFilters,
} from './AccountsFilterSheet'

const QUICK_FILTERS: ReadonlyArray<{ id: AccountStatusFilter; label: string }> = [
  { id: 'all', label: '全部' },
  { id: 'enabled', label: '已启用' },
  { id: 'limited', label: '限流中' },
  { id: 'relogin', label: '需重登' },
]

export function isAccountRateLimited(acc: Account): boolean {
  const now = Date.now()
  if (acc.upstreamCooldownUntil && acc.upstreamCooldownUntil > now) return true
  if (acc.providerCooldown?.resetAt && acc.providerCooldown.resetAt > now) return true
  const checks = [acc.rateLimits, acc.routingCooldowns]
  for (const map of checks) {
    if (!map) continue
    for (const info of Object.values(map)) {
      if (info?.resetAt && info.resetAt > now) return true
    }
  }
  return false
}

export function accountTitle(acc: Account): string {
  return (
    acc.name?.trim() ||
    acc.displayName?.trim() ||
    acc.nickname?.trim() ||
    acc.email?.trim() ||
    acc.id
  )
}

export function AccountsView() {
  const { server, session, revision, navTabs } = useStore()
  const [accounts, setAccounts] = useState<Account[]>(() => session?.accounts?.accounts || [])
  const [counts, setCounts] = useState<AccountQueryCounts | null>(null)
  const [groups, setGroups] = useState<AccountQueryGroup[]>([])
  const [currentId, setCurrentId] = useState<string | null>(
    () => session?.accounts?.currentAccountId || null,
  )
  const [currentByProvider, setCurrentByProvider] = useState<Record<string, string | null>>(
    () => session?.accounts?.currentAccountIds || {},
  )
  const [usageMap, setUsageMap] = useState<Record<string, UsageRow>>({})
  const [keyword, setKeyword] = useState('')
  const [filters, setFilters] = useState<AccountFilters>(DEFAULT_ACCOUNT_FILTERS)
  const [filterOpen, setFilterOpen] = useState(false)
  const [busyId, setBusyId] = useState<string | null>(null)
  const [refreshingTokens, setRefreshingTokens] = useState(false)

  const loadAccounts = async (signal?: AbortSignal) => {
    const id = serverIdentity()
    if (!server?.base) return
    try {
      const [qRes, baseRes, customRes] = await Promise.all([
        queryAccounts(
          {
            q: keyword.trim() || undefined,
            status: filters.status,
            providers: filters.providers.length ? filters.providers : undefined,
            expiry: filters.expiry,
            usage: filters.usage,
          },
          signal,
        ).catch(() => null),
        fetchAccounts(signal).catch(() => null),
        fetchCustomProviders(signal).catch(() => null),
      ])
      if (serverIdentity() !== id) return
      if (baseRes?.providers) syncProviderLabels(baseRes.providers)
      if (session?.accounts?.providers) syncProviderLabels(session.accounts.providers)
      if (customRes?.providers) syncProviderLabels(customRes.providers)
      if (qRes?.accounts) {
        setAccounts(qRes.accounts)
        if (qRes.counts) {
          const st = { ...qRes.counts.status }
          if (!st.enabled && typeof qRes.summary?.enabled === 'number') st.enabled = qRes.summary.enabled
          if (!st.disabled && typeof qRes.summary?.disabled === 'number') st.disabled = qRes.summary.disabled
          setCounts({ ...qRes.counts, status: st })
        } else {
          setCounts(null)
        }
        const labelById = new Map<string, string>()
        for (const p of session?.accounts?.providers || []) {
          if (p.id && p.label) labelById.set(p.id.toLowerCase(), p.label)
        }
        for (const p of baseRes?.providers || []) {
          if (p.id && p.label) labelById.set(p.id.toLowerCase(), p.label)
        }
        for (const p of customRes?.providers || []) {
          const pname = p.label || p.name
          if (p.id && pname) labelById.set(p.id.toLowerCase(), pname)
        }
        setGroups(
          (qRes.groups || []).map(g => {
            const gid = (g.id || g.provider || '').toLowerCase()
            return {
              ...g,
              label: g.label || labelById.get(gid) || providerDisplayName(gid),
            }
          }),
        )
      } else if (baseRes?.accounts) {
        setAccounts(baseRes.accounts)
      }
      if (baseRes) {
        if (baseRes.currentAccountId !== undefined) setCurrentId(baseRes.currentAccountId)
        if (baseRes.currentAccountIds) setCurrentByProvider(baseRes.currentAccountIds)
      }
    } catch {
      /* handled by store or fallback to session */
    }
  }

  useEffect(() => {
    if (session?.accounts?.accounts?.length && accounts.length === 0 && !counts) {
      setAccounts(session.accounts.accounts)
    }
  }, [session, accounts.length, counts])

  useEffect(() => {
    const controller = new AbortController()
    void loadAccounts(controller.signal)
    fetchUsageSnapshot(controller.signal)
      .then(snap => {
        const next: Record<string, UsageRow> = {}
        for (const row of snap.results || []) {
          if (row.id) next[row.id] = row
        }
        setUsageMap(next)
      })
      .catch(() => {})
    return () => controller.abort()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [revision, server?.base, keyword, filters])

  const filtered = useMemo(() => {
    if (counts) return accounts
    const q = keyword.trim().toLowerCase()
    return accounts.filter(acc => {
      if (filters.status === 'enabled' && acc.enabled === false) return false
      if (filters.status === 'disabled' && acc.enabled !== false) return false
      if (filters.status === 'limited' && !isAccountRateLimited(acc)) return false
      if (filters.status === 'relogin' && !acc.needsRelogin) return false
      if (filters.providers.length > 0 && !filters.providers.includes((acc.provider || '').toLowerCase())) {
        return false
      }
      if (!q) return true
      const title = accountTitle(acc).toLowerCase()
      const email = (acc.email || '').toLowerCase()
      const prov = (acc.provider || '').toLowerCase()
      const provLabel = providerDisplayName(acc.provider).toLowerCase()
      return (
        title.includes(q) ||
        email.includes(q) ||
        prov.includes(q) ||
        provLabel.includes(q) ||
        acc.id.toLowerCase().includes(q)
      )
    })
  }, [accounts, counts, keyword, filters])

  const grouped = useMemo(() => {
    const map = new Map<string, Account[]>()
    for (const acc of filtered) {
      const key = (acc.provider || 'other').toLowerCase()
      const list = map.get(key) || []
      list.push(acc)
      map.set(key, list)
    }
    return Array.from(map.entries())
  }, [filtered])

  const handleToggle = async (acc: Account, next: boolean) => {
    setBusyId(acc.id)
    setAccounts(prev => prev.map(item => (item.id === acc.id ? { ...item, enabled: next } : item)))
    try {
      await setAccountEnabled(acc.id, next)
      store.toast(next ? '已启用账号' : '已停用账号', 'success')
      void store.refresh(true)
    } catch (err) {
      setAccounts(prev =>
        prev.map(item => (item.id === acc.id ? { ...item, enabled: !next } : item)),
      )
      store.toast(err instanceof Error ? err.message : '操作失败', 'danger')
    } finally {
      setBusyId(null)
    }
  }

  const handleRefreshExpiring = async () => {
    if (refreshingTokens) return
    setRefreshingTokens(true)
    try {
      await refreshExpiringAccounts()
      store.toast('已触发临期令牌刷新', 'success')
      await loadAccounts()
      void store.refresh(true)
    } catch (err) {
      store.toast(err instanceof Error ? err.message : '刷新令牌失败', 'danger')
    } finally {
      setRefreshingTokens(false)
    }
  }

  const advCount = activeAdvancedFilterCount(filters)
  const totalCount = counts?.status?.all ?? accounts.length

  const stickyHeader = (
    <>
      <SearchField
        value={keyword}
        onChange={setKeyword}
        placeholder={t('搜索账号名称、邮箱或提供商…')}
      />
      <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
        <div className='fc-filter-bar' style={{ flex: 1, minWidth: 0 }}>
          {QUICK_FILTERS.map(opt => {
            const active = filters.status === opt.id
            const c = counts?.status?.[opt.id]
            return (
              <button
                key={opt.id}
                type='button'
                className={`fc-filter-pill ${active ? 'is-active' : ''}`}
                onClick={() => setFilters(prev => ({ ...prev, status: opt.id }))}
              >
                <span>{t(opt.label)}</span>
                {typeof c === 'number' ? <span style={{ opacity: 0.72 }}>{c}</span> : null}
              </button>
            )
          })}
        </div>
        <button
          type='button'
          className={`fc-filter-pill ${advCount > 0 ? 'is-active' : ''}`}
          style={{ flex: 'none' }}
          onClick={() => setFilterOpen(true)}
        >
          <GlyphIcon name='filter' size={14} />
          <span>{t('筛选')}</span>
          {advCount > 0 ? <span className='fc-filter-badge'>{advCount}</span> : null}
        </button>
      </div>
    </>
  )

  return (
    <CommonScaffold
      title={t('账号')}
      badge={<TonalChip tone='primary'>{totalCount}</TonalChip>}
      onBack={navTabs.includes('accounts') ? undefined : () => store.returnFromSubPage()}
      stickyHeader={stickyHeader}
      actions={
        <>
          <TonalIconButton
            icon='calendar'
            onClick={() => store.setTab('checkin')}
            title={t('签到中心')}
          />
          <TonalIconButton
            icon='refresh'
            disabled={refreshingTokens}
            onClick={handleRefreshExpiring}
            title={t('刷新临期令牌')}
          />
          <TonalIconButton
            icon='plus'
            label={t('添加')}
            primary
            onClick={() => store.openSheet({ kind: 'add-account' })}
          />
        </>
      }
    >
      {grouped.length === 0 ? (
        <EmptyBlock
          icon='accounts'
          title={totalCount === 0 ? t('还没有添加任何账号') : t('没有匹配的账号')}
          description={
            totalCount === 0
              ? t('添加上游 Agent 或网页端账号后，即可通过网关统一路由与调用')
              : t('尝试更换筛选条件或搜索关键词')
          }
          action={
            totalCount === 0 ? (
              <button
                type='button'
                className='fc-btn is-primary is-pill'
                onClick={() => store.openSheet({ kind: 'add-account' })}
              >
                <GlyphIcon name='plus' size={16} />
                <span>{t('添加第一个账号')}</span>
              </button>
            ) : (
              <button
                type='button'
                className='fc-btn is-sm'
                onClick={() => {
                  setKeyword('')
                  setFilters(DEFAULT_ACCOUNT_FILTERS)
                }}
              >
                {t('重置筛选')}
              </button>
            )
          }
        />
      ) : (
        grouped.map(([provider, list]) => (
          <div
            key={provider}
            style={{ display: 'flex', flexDirection: 'column', gap: 10 }}
          >
            <SectionHeader
              title={providerDisplayName(provider)}
              trailing={<span style={{ fontSize: 12, color: 'var(--fc-text-tertiary)' }}>{list.length} 个</span>}
            />
            {list.map(acc => {
              const title = accountTitle(acc)
              const enabled = acc.enabled !== false
              const limited = isAccountRateLimited(acc)
              const isPrimary =
                currentId === acc.id ||
                (acc.provider ? currentByProvider[acc.provider] === acc.id : false)
              const usage = usageMap[acc.id]
              const usageText =
                usage?.summary ||
                (usage?.remaining !== undefined && usage?.remaining !== null
                  ? `余 ${usage.remaining}${usage.unit || ''}`
                  : usage?.balance !== undefined && usage?.balance !== null
                    ? `余额 ${usage.balance}${usage.unit || ''}`
                    : '')
              const expiresAt = acc.accessTokenExpiresAt || acc.tokenExpiresAt || acc.expiresAt

              return (
                <RecordCard
                  key={acc.id}
                  onClick={() => store.openSheet({ kind: 'account-detail', account: acc })}
                >
                  <div
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      justifyContent: 'space-between',
                      gap: 12,
                    }}
                  >
                    <div
                      style={{
                        display: 'flex',
                        alignItems: 'center',
                        gap: 12,
                        minWidth: 0,
                        flex: 1,
                      }}
                    >
                      <ProviderAvatar provider={acc.provider} size={40} />
                      <div style={{ minWidth: 0, flex: 1 }}>
                        <div
                          style={{
                            fontSize: 15,
                            fontWeight: 700,
                            color: 'var(--fc-text)',
                            overflow: 'hidden',
                            textOverflow: 'ellipsis',
                            whiteSpace: 'nowrap',
                          }}
                        >
                          {title}
                        </div>
                        <div
                          style={{
                            fontSize: 12,
                            color: 'var(--fc-text-secondary)',
                            overflow: 'hidden',
                            textOverflow: 'ellipsis',
                            whiteSpace: 'nowrap',
                            marginTop: 2,
                          }}
                        >
                          {acc.email && acc.email !== title
                            ? acc.email
                            : providerDisplayName(acc.provider)}
                          {acc.editionLabel ? ` · ${acc.editionLabel}` : ''}
                        </div>
                      </div>
                    </div>

                    <CommonSwitch
                      checked={enabled}
                      disabled={busyId === acc.id}
                      onChange={next => void handleToggle(acc, next)}
                    />
                  </div>

                  <div
                    style={{
                      display: 'flex',
                      flexWrap: 'wrap',
                      alignItems: 'center',
                      gap: 6,
                    }}
                  >
                    {isPrimary ? <TonalChip tone='primary'>首选</TonalChip> : null}
                    {acc.needsRelogin ? (
                      <TonalChip tone='danger'>需重新登录</TonalChip>
                    ) : limited ? (
                      <TonalChip tone='warning'>限流冷却中</TonalChip>
                    ) : enabled ? (
                      <TonalChip tone='success'>就绪</TonalChip>
                    ) : (
                      <TonalChip>已停用</TonalChip>
                    )}
                    {usageText ? <TonalChip tone='primary'>{usageText}</TonalChip> : null}
                    {expiresAt ? (
                      <TonalChip>令牌 {formatRelativeTime(expiresAt)}</TonalChip>
                    ) : null}
                  </div>
                </RecordCard>
              )
            })}
          </div>
        ))
      )}

      <AccountsFilterSheet
        open={filterOpen}
        filters={filters}
        counts={counts}
        groups={groups}
        onChange={setFilters}
        onReset={() => setFilters(DEFAULT_ACCOUNT_FILTERS)}
        onClose={() => setFilterOpen(false)}
      />
    </CommonScaffold>
  )
}
