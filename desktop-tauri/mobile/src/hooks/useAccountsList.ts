import { useCallback, useEffect, useRef, useState } from 'react'
import { queryAccounts, type Account } from '../api/gateway'
import { serverIdentity } from '../lib/servers'
import { store } from '../state/store'
import { t } from '../i18n'
import type { AccountListPage, AccountListQuery } from '../../../ui-shared/accounts-list'

export function useAccountsList(query: AccountListQuery, continuous: boolean, revision: unknown, balanceRevision = '') {
  const key = JSON.stringify(query)
  const identity = serverIdentity()
  const [state, setState] = useState<{ key: string; data: AccountListPage<Account> | null; loading: boolean; error: string }>({ key: '', data: null, loading: false, error: '' })
  const generation = useRef(0)
  const loadingMore = useRef(false)
  const current = useRef({ key, query, state })
  current.current = { key, query, state }
  useEffect(() => {
    const run = ++generation.current
    const controller = new AbortController()
    const background = current.current.state.key === key && Boolean(current.current.state.data)
    setState(previous => ({ key, data: previous.key === key ? previous.data : null, loading: true, error: '' }))
    const timer = window.setTimeout(() => { void queryAccounts({ ...query, metadataOnly: background }, controller.signal).then(data => {
      if (run !== generation.current || serverIdentity() !== identity) return
      if (!Array.isArray(data.accounts) || !Number.isFinite(data.total)) throw new Error(t('账号分页响应异常，请刷新后重试'))
      store.applyAccounts({ ...store.get().session?.accounts, summary: data.summary, sorting: data.sorting })
      setState(previous => {
        if (background && previous.data) {
          const indexes = new Map(store.accounts().map(account => [account.id, account]))
          const saved = new Map(previous.data.accounts.map(account => [account.id, account]))
          const displayIds = data.displayIds || [...saved.keys()]
          const wanted = new Set(displayIds.slice(continuous ? 0 : data.offset, (continuous ? 0 : data.offset) + data.pageSize))
          if (continuous) for (const id of saved.keys()) wanted.add(id)
          const accounts = displayIds.filter(id => wanted.has(id)).flatMap(id => {
            const old = saved.get(id)
            const index = indexes.get(id)
            if (!old) return index ? [{ ...index, detailsLoaded: false }] : []
            if (!index) return []
            const changed = JSON.stringify([index.updatedAt, index.enabled, index.priority, index.rateLimits, index.needsRelogin, index.upstreamCooldownUntil])
              !== JSON.stringify([old.updatedAt, old.enabled, old.priority, old.rateLimits, old.needsRelogin, old.upstreamCooldownUntil])
            if (!changed && index.listLimited === old.listLimited) return [old]
            const merged: Account = { ...old, ...index, detailsLoaded: !changed }
            for (const field of ['rateLimits', 'upstreamCooldownUntil', 'upstreamBlockedReason']) {
              if (!Object.prototype.hasOwnProperty.call(index, field)) delete merged[field]
            }
            return [merged]
          })
          const loaded = new Set(accounts.map(account => account.id))
          const missing = displayIds.findIndex(id => !loaded.has(id))
          return { key, data: { ...data, accounts,
            offset: continuous ? missing === -1 ? data.displayTotal : missing : data.offset }, loading: false, error: '' }
        }
        const keep = continuous && previous.key === key && previous.data
        const ids = new Set(data.accounts.map(account => account.id))
        const valid = new Set(data.displayIds || store.accounts().map(account => account.id))
        const indexes = new Map(store.accounts().map(account => [account.id, account]))
        const tail = keep ? previous.data!.accounts.filter(account => !ids.has(account.id) && valid.has(account.id))
          .map(account => {
            const index = indexes.get(account.id)
            if (!index) return account
            const changed = JSON.stringify([index.updatedAt, index.enabled, index.priority, index.rateLimits, index.needsRelogin])
              !== JSON.stringify([account.updatedAt, account.enabled, account.priority, account.rateLimits, account.needsRelogin])
            const merged: Account = { ...account, ...index, detailsLoaded: !changed }
            for (const key of ['rateLimits', 'upstreamCooldownUntil', 'upstreamBlockedReason']) {
              if (!Object.prototype.hasOwnProperty.call(index, key)) delete merged[key]
            }
            return merged
          }) : []
        const positions = new Map((data.sorting?.providerOrder || []).flatMap(provider => data.sorting?.accountOrder[provider] || [])
          .map((id, index) => [id, index]))
        const accounts = [...data.accounts, ...tail].sort((left, right) => (positions.get(left.id) ?? Infinity) - (positions.get(right.id) ?? Infinity))
        const loaded = new Set(accounts.map(account => account.id))
        const missing = data.displayIds?.findIndex(id => !loaded.has(id))
        return { key, data: { ...data, accounts, offset: continuous ? missing === -1 ? data.displayTotal : missing ?? accounts.length : data.offset }, loading: false, error: '' }
      })
    }).catch(error => {
      if (!controller.signal.aborted && run === generation.current) setState(previous => ({ ...previous, key, loading: false, error: error instanceof Error ? error.message : String(error) }))
    }) }, query.q?.trim() ? 200 : 0)
    return () => { generation.current++; window.clearTimeout(timer); controller.abort() }
  }, [key, revision, balanceRevision, identity])

  const staleReads = useRef(new Set<string>())
  useEffect(() => {
    const ids = current.current.state.data?.accounts.filter(account => account.detailsLoaded === false).map(account => account.id) || []
    if (!ids.length || current.current.state.loading) return
    const run = generation.current
    for (const key of staleReads.current) if (!key.startsWith(`${run}:`)) staleReads.current.delete(key)
    const requestKey = `${run}:${ids.slice(0, 200).join('|')}`
    if (staleReads.current.has(requestKey)) return
    staleReads.current.add(requestKey)
    const controller = new AbortController()
    void queryAccounts({ ids: ids.slice(0, 200), pageSize: 200 }, controller.signal).then(page => {
      if (run !== generation.current || controller.signal.aborted) return
      const byId = new Map(page.accounts.map(account => [account.id, account]))
      setState(previous => {
        if (!previous.data || !previous.data.accounts.some(account => byId.has(account.id))) return previous
        return { ...previous, data: { ...previous.data,
          accounts: previous.data.accounts.map(account => byId.get(account.id) || account) } }
      })
    }).catch(() => {})
    return () => { controller.abort(); staleReads.current.delete(requestKey) }
  }, [state.data?.accounts, state.loading])

  const loadMore = useCallback(async () => {
    const { key, query, state } = current.current
    const page = state.data
    if (!continuous || loadingMore.current || state.loading || state.key !== key || !page || page.accounts.length >= page.displayTotal) return
    loadingMore.current = true
    const run = generation.current
    try {
      const next = await queryAccounts({ ...query, offset: page.offset })
      if (run !== generation.current || current.current.key !== key || serverIdentity() !== identity) return
      setState(previous => {
        if (!previous.data || previous.key !== key) return previous
        const ids = new Set(previous.data.accounts.map(account => account.id))
        const refreshed = new Map(next.accounts.map(account => [account.id, account]))
        const accounts = [...previous.data.accounts.map(account => refreshed.get(account.id) || account),
          ...next.accounts.filter(account => !ids.has(account.id))]
        const positions = new Map((next.displayIds || accounts.map(account => account.id)).map((id, index) => [id, index]))
        const ordered = accounts.filter(account => positions.has(account.id)).sort((left, right) => positions.get(left.id)! - positions.get(right.id)!)
        const loaded = new Set(ordered.map(account => account.id))
        const missing = next.displayIds?.findIndex(id => !loaded.has(id))
        return { ...previous, error: '', data: { ...next, accounts: ordered,
          offset: missing === -1 ? next.displayTotal : missing ?? ordered.length } }
      })
    } catch (error) {
      if (run === generation.current) setState(previous => ({ ...previous, error: error instanceof Error ? error.message : String(error) }))
    } finally { loadingMore.current = false }
  }, [continuous, identity])
  return { ...state, data: state.key === key ? state.data : null, loadMore,
    blocking: state.key !== key || !state.data && state.loading,
    loading: state.loading || state.key !== key }
}
