import * as React from 'react'
import type { AccountListPage, AccountListQuery } from '../../../ui-shared/accounts-list'
import { accountDetailsCurrent, queryAccountPage } from './accounts-list-api'
import { adoptAccountSorting } from './accounts-sort'
import { shared, type AccountRecord } from './accounts-shared'
import { patch } from './accounts-store'
import { mergeAccountPages, previewAccountPage, reflowAccountPage, type AccountListView } from './accounts-list-view'

type ListState = {
  key: string
  contentKey: string
  revision: unknown
  data: AccountListPage<AccountRecord> | null
  loading: boolean
  error: string
}

export function useAccountsList(query: AccountListQuery, continuous: boolean, revision: unknown,
  view: AccountListView, balanceRevision = 0) {
  const key = JSON.stringify({ ...query, continuous })
  const contentKey = JSON.stringify({ ...query, collapsed: undefined, offset: undefined, continuous })
  const [state, setState] = React.useState<ListState>({ key: '', contentKey: '', revision: null, data: null, loading: false, error: '' })
  const generation = React.useRef(0)
  const loadingMore = React.useRef(false)
  const pages = React.useRef(new Map<string, { revision: unknown; data: AccountListPage<AccountRecord> }>())
  const requestedSearch = React.useRef(query.q?.trim() || '')
  const current = React.useRef({ key, contentKey, query, state, view, revision })
  current.current = { key, contentKey, query, state, view, revision }

  const remember = React.useCallback((contentKey: string, page: AccountListPage<AccountRecord>) => {
    pages.current.delete(contentKey)
    pages.current.set(contentKey, { revision: current.current.revision, data: page })
    if (pages.current.size > 12) pages.current.delete(pages.current.keys().next().value!)
  }, [])

  const data = React.useMemo(() => {
    if (state.contentKey === contentKey && state.data && state.revision === revision
      && (!state.loading || state.key === key)) {
      return reflowAccountPage({ ...state.data, sorting: view.sorting || state.data.sorting }, query, continuous, view.snapshot)
    }
    const saved = pages.current.get(contentKey)
    return previewAccountPage(query, continuous, view, saved?.data || state.data,
      saved ? saved.revision === revision : state.revision === revision)
  }, [state.data, state.key, state.contentKey, state.revision, state.loading, key, contentKey, continuous, revision, view.snapshot, view.groups, view.counts, view.sorting])
  const visible = React.useRef(data)
  visible.current = data

  React.useEffect(() => {
    if (shared().wbApp?.currentPage !== 'accounts' || document.hidden) return
    const run = ++generation.current
    const search = query.q?.trim() || ''
    const delay = search && requestedSearch.current !== search ? 200 : 0
    requestedSearch.current = search
    const background = current.current.state.key === key && Boolean(current.current.state.data)
    setState({ key, contentKey, revision, data: visible.current, loading: true, error: '' })
    const timer = window.setTimeout(() => { void queryAccountPage({ ...query, metadataOnly: background }).then(first => {
      if (run !== generation.current) return
      adoptAccountSorting(first.sorting)
      const snapshot = current.current.view.snapshot
      if (snapshot) shared().wbApp?.applyAccounts?.({ ...snapshot, summary: first.summary })
      setState(previous => {
        const data = background && visible.current
          ? reflowAccountPage({ ...first, accounts: visible.current.accounts }, query, continuous, current.current.view.snapshot)
          : mergeAccountPages(first, previous.key === key ? previous.data : null, continuous, current.current.view.snapshot)
        const previousAccounts = new Map((visible.current?.accounts || []).map(account => [account.id, account]))
        const stableAccounts = data.accounts.map(account => {
          const old = previousAccounts.get(account.id)
          return old && JSON.stringify(old) === JSON.stringify(account) ? old : account
        })
        remember(contentKey, { ...data, accounts: stableAccounts })
        return { key, contentKey, revision, data: { ...data, accounts: stableAccounts }, loading: false, error: '' }
      })
    }).catch(error => {
      if (run === generation.current) setState(previous => ({ ...previous, key, loading: false, error: error instanceof Error ? error.message : String(error) }))
    }) }, delay)
    return () => { generation.current++; window.clearTimeout(timer) }
  }, [key, revision, balanceRevision])

  const staleReads = React.useRef(new Set<string>())
  React.useEffect(() => {
    const indexes = new Map((current.current.view.snapshot?.accounts || []).map(account => [account.id, account]))
    const ids = visible.current?.accounts.filter(account => !accountDetailsCurrent(indexes.get(account.id) || account)).map(account => account.id) || []
    if (!ids.length || state.loading || shared().wbApp?.currentPage !== 'accounts' || document.hidden) return
    const run = generation.current
    for (const key of staleReads.current) if (!key.startsWith(`${run}:`)) staleReads.current.delete(key)
    const requestKey = `${run}:${ids.slice(0, 200).join('|')}`
    if (staleReads.current.has(requestKey)) return
    staleReads.current.add(requestKey)
    void queryAccountPage({ ids: ids.slice(0, 200), pageSize: 200 }).then(page => {
      if (run !== generation.current) return
      const byId = new Map(page.accounts.map(account => [account.id, account]))
      patch({})
      setState(previous => {
        if (!previous.data) return previous
        const data = { ...previous.data, accounts: (visible.current?.accounts || previous.data.accounts).map(account => byId.get(account.id) || account) }
        remember(previous.contentKey, data)
        return { ...previous, data }
      })
    }).catch(() => {})
  }, [data?.accounts, state.loading, revision])

  const loadMore = React.useCallback(async () => {
    const { key, contentKey, query, state } = current.current
    const page = state.data
    if (!continuous || loadingMore.current || shared().wbApp?.currentPage !== 'accounts' || document.hidden || state.loading || state.key !== key || !page || page.accounts.length >= page.displayTotal) return
    loadingMore.current = true
    const run = generation.current
    try {
      const next = await queryAccountPage({ ...query, offset: page.offset })
      if (run !== generation.current || current.current.key !== key) return
      setState(previous => {
        if (!previous.data || previous.key !== key) return previous
        const data = mergeAccountPages(next, previous.data, true, current.current.view.snapshot)
        remember(contentKey, data)
        return { ...previous, error: '', data }
      })
    } catch (error) {
      if (run === generation.current) setState(previous => ({ ...previous, error: error instanceof Error ? error.message : String(error) }))
    } finally { loadingMore.current = false }
  }, [continuous, remember])

  return { ...state, data, loadMore, loading: state.loading || state.key !== key,
    blocking: state.key !== key || !data && state.loading }
}
