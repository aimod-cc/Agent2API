import * as React from 'react'
import { mountOnPage } from '../island-lazy'
import { createRoot } from 'react-dom/client'
import {
  Alert, Button, Card, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader,
  DialogTitle, Empty, EmptyDescription, EmptyTitle,
} from '@ui'
import { InstalledPluginCard, StorePluginCard } from './plugins-cards'
import { PluginNotesDialog, type PluginNotes } from './plugins-notes'
import { PluginPackSettingsDialog } from './plugins-pack-settings'
import { PluginSettingsDialog } from './plugins-settings'
import { api, bridge, errorMessage, pluginDescription, pluginName, toast } from './plugins-shared'
import { clampMaxPackBytes, LEGACY_MAX_PACK_BYTES, PluginFileInstallDialog, PluginFileInstallProgress, usePluginFileInstall } from './plugins-file-install'
import { pluginOperationNotice, type ScriptPackage, type ScriptRecommendation } from '../../../ui-shared/plugins'
import type { PackSummary, PluginMutation, PluginOperation, PluginUpdate, PluginsPayload, ReleaseState, StorePlugin } from './plugins-types'
import { t } from '../i18n'
import { useScriptPluginsController } from '../../../ui-shared/plugins-scripts-controller'
import { PluginStreamFooter, usePluginStream } from '../../../ui-shared/plugins-stream-footer'
import { isRecommendationInstalled, scriptRecommendations } from '../../../ui-shared/plugins-script-display'
import { PLUGIN_GRID_CLASS, PluginInstallButton, type PluginViewMode } from '../../../ui-shared/plugins-store'
import { PluginDetailsDialog, type PluginDetailsPane } from '../../../ui-shared/plugins-details'
import { PluginDiagnostics } from './plugins-page-tools'
import { useGatewayBase } from './port-panel'
import { PluginStartupDialog, subscribePluginDiagnostics, takePluginDiagnostics } from '../../../ui-shared/plugin-startup-view'
import { getPluginStartup, refreshPluginStartup, subscribePluginStartup } from '../../../ui-shared/plugin-startup'
import { openExternal } from './update-shared'
import { PluginPageToolbar, type PluginSortKey } from './plugins-toolbar'
import { PluginCategoryRail, pluginRailSubtitle, pluginRailTitle, type PluginRailKey } from './plugins-rail'
import { applicablePluginItems, missingPluginItems, pluginBatchItems, type PluginBatchAction, type PluginBatchItem } from '../../../ui-shared/plugins-batch-items'
import { usePluginBatch, waitScriptOperation, type PluginRequest } from '../../../ui-shared/plugins-batch-runner'
import { PluginBatchConfirm, PluginBatchProgress, PluginBatchToolbar } from '../../../ui-shared/plugins-batch-view'
import { PluginMissingBanner, PluginMissingDialog } from '../../../ui-shared/plugins-missing'
import { PluginCatalogStatus } from '../../../ui-shared/plugins-catalog-status'
import { ScriptCatalogStatus } from '../../../ui-shared/plugins-script-catalog'
import { ScriptOperationCard, scriptOperationMatches } from '../../../ui-shared/plugins-script-progress'
import { summarizeNetworkError } from '../../../ui-shared/updates'
import { buildUnifiedRows, filterAndSortRows, indexScriptPackages, type UnifiedPluginRow } from './plugins-rows'
import type { PluginStatusFilter } from '../../../ui-shared/plugins-filters'
import { TaskPluginsSection } from '../../../ui-shared/task-plugins-section'
import { PluginScheduledFilterBar, usePluginsTaskData } from './plugins-scheduled-pane'
import '../styles/plugins-page.css'

const VIEW_MODE_KEY = 'agent2api-plugins-view-mode'

const EMPTY_PACKS: PackSummary[] = []
const EMPTY_STORE: StorePlugin[] = []
const EMPTY_OPERATIONS: PluginOperation[] = []
const EMPTY_UPDATES: PluginUpdate[] = []
const EMPTY_SCRIPTS: ScriptPackage[] = []

type ConditionalPluginsResponse =
  | (PluginsPayload & { maxPackBytes?: unknown; unchanged?: false })
  | { unchanged: true; revision?: string }

function readStoredViewMode(): PluginViewMode {
  try {
    return window.localStorage.getItem(VIEW_MODE_KEY) === 'list' ? 'list' : 'cards'
  } catch {
    return 'cards'
  }
}

function PluginsPage(): React.ReactElement {
  const startupView = React.useSyncExternalStore(subscribePluginStartup, getPluginStartup)
  const gatewayBase = useGatewayBase()
  const [payload, setPayload] = React.useState<PluginsPayload | null>(null)
  const [maxPackBytes, setMaxPackBytes] = React.useState(LEGACY_MAX_PACK_BYTES)
  const [error, setError] = React.useState('')
  const [syncError, setSyncError] = React.useState('')
  const [busy, setBusy] = React.useState('')
  const [settingsOpen, setSettingsOpen] = React.useState(false)
  const [settingsPackId, setSettingsPackId] = React.useState<string | null>(null)
  const [detailsPane, setDetailsPane] = React.useState<PluginDetailsPane>('settings')
  const [detailsStore, setDetailsStore] = React.useState<StorePlugin | null>(null)
  const [confirmRemove, setConfirmRemove] = React.useState<PackSummary | null>(null)
  const [notes, setNotes] = React.useState<PluginNotes | null>(null)
  const [rail, setRail] = React.useState<PluginRailKey>('installed')
  const [railTouched, setRailTouched] = React.useState(false)
  const [search, setSearch] = React.useState('')
  const deferredSearch = React.useDeferredValue(search)
  const [sortBy, setSortBy] = React.useState<PluginSortKey>('default')
  const [viewMode, setViewModeState] = React.useState<PluginViewMode>(readStoredViewMode)
  const [diagnosticsOpen, setDiagnosticsOpen] = React.useState(false)
  const [startupId, setStartupId] = React.useState<string | null>(null)
  React.useEffect(() => {
    const open = () => { const id = takePluginDiagnostics(); if (id) setStartupId(id) }
    open()
    return subscribePluginDiagnostics(open)
  }, [])
  const [scriptBusy, setScriptBusy] = React.useState(false)
  const [scriptInstallRequest, setScriptInstallRequest] = React.useState(0)
  const [taskStatus, setTaskStatus] = React.useState<PluginStatusFilter>('all')
  const [taskImportRequest, setTaskImportRequest] = React.useState(0)
  const [taskCreateRequest, setTaskCreateRequest] = React.useState(0)
  const [selecting, setSelecting] = React.useState(false)
  const [selected, setSelected] = React.useState(new Set<string>())
  const [batchConfirm, setBatchConfirm] = React.useState<{ action: PluginBatchAction; items: PluginBatchItem[] } | null>(null)
  const [missingOpen, setMissingOpen] = React.useState(false)
  const listRef = React.useRef<HTMLDivElement>(null)
  const taskData = usePluginsTaskData()
  const seenOperations = React.useRef(new Set<string>())
  const previousSignature = React.useRef('')
  const lastRevision = React.useRef('')
  const lastPacksSignature = React.useRef('')
  const mounted = React.useRef(true)
  const fileOperationLocked = React.useRef(false)
  const mutationLocked = React.useRef(false)
  const busyRef = React.useRef(busy)
  busyRef.current = busy
  const scriptBusyRef = React.useRef(scriptBusy)
  scriptBusyRef.current = scriptBusy
  const targetRef = React.useRef(payload?.target)
  targetRef.current = payload?.target
  const loading = React.useRef<Promise<void> | null>(null)

  const setViewMode = React.useCallback((next: PluginViewMode) => {
    setViewModeState(next)
    try { window.localStorage.setItem(VIEW_MODE_KEY, next) } catch { /* ignore */ }
  }, [])

  const selectRail = React.useCallback((next: PluginRailKey) => {
    setRailTouched(true)
    setRail(next)
    if (next === 'scheduled') {
      setSelecting(false)
      setSelected(new Set())
      void taskData.load()
    }
  }, [taskData.load])

  const synchronize = React.useCallback(async (result: PluginMutation): Promise<void> => {
    const app = bridge().wbApp
    const [refreshed] = await Promise.all([app?.refresh?.(), app?.refreshPluginPages?.(result)])
    if (mounted.current) {
      setSyncError(refreshed === true ? ''
        : t('插件状态已变更，但账号列表同步失败。请回到账号页点击「刷新」重试。'))
    }
  }, [])

  const load = React.useCallback(async (options?: { force?: boolean }): Promise<void> => {
    if (loading.current) {
      if (!options?.force) return loading.current
      await loading.current.catch(() => undefined)
    }
    const request = (async () => {
      try {
        const path = !options?.force && lastRevision.current
          ? `/api/plugins?rev=${encodeURIComponent(lastRevision.current)}`
          : '/api/plugins'
        const response = await api<ConditionalPluginsResponse>('GET', path)
        if (!mounted.current) return
        if ('unchanged' in response && response.unchanged === true) {
          if (typeof response.revision === 'string' && response.revision) {
            lastRevision.current = response.revision
          }
          setError('')
          return
        }
        const next = response as PluginsPayload & { maxPackBytes?: unknown }
        lastRevision.current = typeof next.revision === 'string' ? next.revision : ''
        const signature = JSON.stringify({
          packs: next.packs.map(pack => ({ id: pack.id, version: pack.version, enabled: pack.enabled, providers: pack.providers })),
          running: next.host?.running,
          scripts: next.scripts?.packages.map(pack => ({
            id: pack.id, version: pack.version, enabled: pack.enabled, running: pack.running, providers: pack.providers, error: pack.error,
          })),
        })
        const changed = Boolean(previousSignature.current && previousSignature.current !== signature)
        previousSignature.current = signature
        const packsSig = JSON.stringify(next.packs.map(pack => ({
          id: pack.id, enabled: pack.enabled, ui: (pack as PackSummary & { ui?: unknown }).ui,
        })))
        const packsUiChanged = packsSig !== lastPacksSignature.current
        lastPacksSignature.current = packsSig
        setMaxPackBytes(clampMaxPackBytes(next.maxPackBytes))
        setPayload(next)
        if (packsUiChanged) void bridge().wbApp?.refreshPluginPages?.(next)
        setError('')
        if (changed) await synchronize(next)
        for (const operation of next.operations || []) {
          if (operation.active) continue
          const key = `${operation.id}:${operation.updatedAt}`
          if (seenOperations.current.has(key)) continue
          seenOperations.current.add(key)
          if (Date.now() - operation.updatedAt > 30_000) continue
          const notice = pluginOperationNotice(next, operation, t)
          if (notice) toast(notice.message, notice.kind)
        }
      } catch (cause) {
        if (mounted.current) setError(errorMessage(cause))
      }
    })()
    loading.current = request
    try { await request } finally { loading.current = null }
  }, [synchronize])

  const forceLoad = React.useCallback(async () => {
    await Promise.all([load({ force: true }), taskData.load()])
  }, [load, taskData.load])

  const pageLoad = React.useCallback(async () => {
    await Promise.all([load(), taskData.load()])
  }, [load, taskData.load])

  React.useEffect(() => {
    mounted.current = true
    void load()
    return () => { mounted.current = false }
  }, [load])

  React.useEffect(() => {
    currentLoad = pageLoad
    return () => { if (currentLoad === pageLoad) currentLoad = null }
  }, [pageLoad])

  const run = React.useCallback(async (key: string, action: () => Promise<void>): Promise<void> => {
    if (busyRef.current || scriptBusyRef.current || fileOperationLocked.current || mutationLocked.current) return
    mutationLocked.current = true
    setBusy(key)
    try { await action() }
    catch (cause) { toast(cause instanceof Error ? cause.message : String(cause), 'err') }
    finally { mutationLocked.current = false; if (mounted.current) setBusy('') }
  }, [])

  const check = React.useCallback(async (): Promise<void> => {
    await run('check', async () => {
      const [result] = await Promise.all([
        api<{ updates: PluginUpdate[]; error: boolean; reason?: string }>('GET', '/api/plugins/update'),
        api('GET', '/api/plugins/scripts/check').catch(() => undefined),
      ])
      await forceLoad()
      if (result.error) toast(summarizeNetworkError(result.reason, t).summary || t('检查更新失败'), 'err')
    })
  }, [run, forceLoad])

  const refreshStore = React.useCallback(async (): Promise<void> => {
    await run('refresh', async () => {
      const result = await api<ReleaseState>('GET', '/api/plugins/release')
      await forceLoad()
      if (result.error) toast(summarizeNetworkError(result.reason, t).summary || t('检查更新失败'), 'err')
    })
  }, [run, forceLoad])

  const accept = React.useCallback(async (result: PluginMutation): Promise<void> => {
    setPayload(previous => previous ? { ...previous, ...result } : previous)
    await synchronize(result)
    if (result.hostError) toast(result.hostError, 'err')
    else if (result.installNotice) toast(t(result.installNotice), 'ok')
    await forceLoad()
  }, [synchronize, forceLoad])

  const mutation = React.useCallback(async (key: string, path: string, body: unknown): Promise<void> => {
    await run(key, async () => { await accept(await api<PluginMutation>('POST', path, body)) })
  }, [run, accept])

  const start = React.useCallback(async (id: string, update: boolean): Promise<void> => {
    await run(`start:${id}`, async () => {
      const result = await api<{ operation: PluginOperation }>('POST', update
        ? '/api/plugins/update' : '/api/plugins/install-from-release', { id, target: targetRef.current })
      setPayload(previous => previous ? {
        ...previous, operations: [...(previous.operations || []).filter(item => item.id !== id), result.operation],
      } : previous)
    })
  }, [run])

  const batchSynchronize = React.useCallback(async () => {
    const next = await api<PluginsPayload>('GET', '/api/plugins')
    lastRevision.current = typeof next.revision === 'string' ? next.revision : ''
    setPayload(next)
    await synchronize(next)
  }, [synchronize])

  const batchCompleted = React.useCallback((items: PluginBatchItem[]) => {
    setSelected(previous => {
      const next = new Set(previous)
      items.forEach(item => next.delete(item.key))
      return next
    })
  }, [])

  const batch = usePluginBatch({
    request: api,
    refresh: forceLoad,
    target: payload?.target || '',
    t,
    disabled: Boolean(busy) || scriptBusy || fileOperationLocked.current,
    synchronize: batchSynchronize,
    completed: batchCompleted,
  })

  const onTaskPluginInstalled = React.useCallback(() => taskData.refresh(), [taskData.refresh])
  const rememberFileOp = React.useCallback((operation: PluginOperation) => {
    if (!operation.active) seenOperations.current.add(`${operation.id}:${operation.updatedAt}`)
    setPayload(previous => previous ? {
      ...previous,
      operations: [...(previous.operations || []).filter(item => item.id !== operation.id), operation],
    } : previous)
  }, [])

  const fileInstall = usePluginFileInstall({
    request: api,
    t,
    maxPackBytes,
    previewAvailable: payload?.supportsPackPreview === true,
    disabled: Boolean(busy) || scriptBusy || batch.running,
    accept,
    onTaskPluginInstalled,
    refresh: forceLoad,
    remember: rememberFileOp,
  })
  fileOperationLocked.current = fileInstall.running
  mutationLocked.current = Boolean(busy) || batch.running

  const packs = payload?.packs || EMPTY_PACKS
  const host = payload?.host
  const catalog = payload?.catalog
  const store = React.useMemo(
    () => (catalog?.plugins || EMPTY_STORE).filter(item => item.category !== 'scheduled_task'),
    [catalog?.plugins],
  )
  const storeById = React.useMemo(() => new Map(store.map(item => [item.id, item])), [store])
  const detailsItem = detailsStore ? storeById.get(detailsStore.id) || detailsStore : null
  const detailsInstallable = Boolean(detailsItem?.version && !detailsItem.error
    && detailsItem.platforms.some(platform => platform.target === 'wasm32-unknown-unknown'))
  const installedIds = React.useMemo(() => new Set(packs.map(pack => pack.id)), [packs])
  const unpublishedCount = React.useMemo(
    () => store.filter(item => !installedIds.has(item.id) && !item.version).length,
    [store, installedIds],
  )
  const storePlugins = React.useMemo(
    () => store.filter(item => !installedIds.has(item.id) && Boolean(item.version)),
    [store, installedIds],
  )
  const operations = payload?.operations || EMPTY_OPERATIONS
  const operationById = React.useMemo(() => new Map(operations.map(item => [item.id, item])), [operations])
  const active = operations.some(operation => operation.active)
  const target = payload?.target || ''
  const locked = Boolean(busy) || scriptBusy || batch.running || fileInstall.running
  const settingsPack = packs.find(pack => pack.id === settingsPackId)
  const settingsOperation = settingsPackId ? operationById.get(settingsPackId) : undefined

  const updates = payload?.updates || EMPTY_UPDATES
  const updatesById = React.useMemo(() => new Map(updates.map(item => [item.id, item])), [updates])
  const scripts = payload?.scripts
  const scriptItems = scripts?.packages || EMPTY_SCRIPTS
  const recommendations = React.useMemo(
    () => (scripts ? scriptRecommendations(scripts) : ([] as ScriptRecommendation[])),
    [scripts],
  )
  const missingScripts = React.useMemo(() => {
    const scriptsByName = indexScriptPackages(scriptItems)
    return recommendations.filter(item => {
      const sameName = scriptsByName.get(item.name)
      return !sameName || !isRecommendationInstalled(item, sameName)
    })
  }, [recommendations, scriptItems])
  const resolveIcon = React.useCallback((path: string) => `${gatewayBase.replace(/\/+$/, '')}${path}`, [gatewayBase])
  const scriptRequest = React.useCallback((path: string, body: Record<string, unknown>) => api('POST', path, body), [])

  const scriptCtrl = useScriptPluginsController({
    state: scripts,
    request: scriptRequest,
    refresh: forceLoad,
    t,
    disabled: locked,
    installRequest: scriptInstallRequest,
    onBusyChange: setScriptBusy,
    resolveIcon,
    onOpenExternal: openExternal,
  })

  const { allRows, railCounts } = React.useMemo(() => buildUnifiedRows({
    packs,
    storeById,
    storePlugins,
    host,
    updatesById,
    scripts,
    scriptItems,
    recommendations,
    missingScripts,
    startupSnapshot: startupView.value,
    t,
  }), [packs, storeById, storePlugins, host, updatesById, scripts, scriptItems, recommendations, missingScripts, startupView.value])

  // 新装软件无任何已装插件时，自动定位到「全部插件」方便直接挑选安装
  React.useEffect(() => {
    if (!payload || railTouched) return
    if (railCounts.installed === 0 && railCounts.all > 0 && rail === 'installed') {
      setRail('all')
    }
  }, [payload, railTouched, railCounts.installed, railCounts.all, rail])

  // 异常清零后自动切回已安装
  React.useEffect(() => {
    if (rail === 'error' && railCounts.error === 0) setRail('installed')
  }, [rail, railCounts.error])

  const deferredKeyword = deferredSearch.trim().toLocaleLowerCase()
  const filteredRows = React.useMemo(
    () => filterAndSortRows(allRows, rail, deferredKeyword, sortBy),
    [allRows, rail, deferredKeyword, sortBy],
  )

  const streamResetKey = React.useMemo(() => JSON.stringify([rail, deferredKeyword, sortBy]), [rail, deferredKeyword, sortBy])
  const stream = usePluginStream(filteredRows.length, streamResetKey)
  const visibleRows = React.useMemo(() => stream.slice(filteredRows), [stream, filteredRows])

  const unmatchedScriptOperation = Boolean(
    scripts && scriptCtrl.operation.active
    && !visibleRows.some(row => (row.kind === 'script-installed' && scriptOperationMatches(scriptCtrl.operation, row.pack))
      || (row.kind === 'script-recommended' && scriptOperationMatches(scriptCtrl.operation, row.item)))
  )

  const pollBusy = active || scripts?.operation.active === true || catalog?.checking === true
    || scripts?.catalog?.refreshing === true || fileInstall.running || batch.running || Boolean(busy) || scriptBusy
  React.useEffect(() => {
    let activeTimer = true
    let timer = 0
    const interval = pollBusy ? 1500 : 8000
    const schedule = () => {
      timer = window.setTimeout(async () => {
        if (!activeTimer || !mounted.current) return
        const page = bridge().wbApp?.currentPage
        if (!document.hidden && (!page || page === 'plugins')) await load()
        if (activeTimer && mounted.current) schedule()
      }, interval)
    }
    schedule()
    return () => { activeTimer = false; window.clearTimeout(timer) }
  }, [load, pollBusy])

  const hasAny = allRows.length > 0
  const batchItems = React.useMemo(() => pluginBatchItems(payload, t, resolveIcon), [payload, resolveIcon])
  const batchItemByKey = React.useMemo(() => new Map(batchItems.map(item => [item.key, item])), [batchItems])
  const pendingUpdates = React.useMemo(() => applicablePluginItems(batchItems, 'update'), [batchItems])
  const missing = React.useMemo(() => missingPluginItems(payload, batchItems, t, resolveIcon), [payload, batchItems, resolveIcon])
  const missingAccounts = React.useMemo(() => (
    missing.required.reduce((total, item) => total + item.accounts, 0)
    + missing.bindable.reduce((total, item) => total + item.provider.count, 0)
    + missing.unresolved.reduce((total, item) => total + item.count, 0)
  ), [missing])
  const selectableVisibleKeys = React.useMemo(
    () => filteredRows.map(row => row.selectionKey).filter(key => !batchItemByKey.get(key)?.active),
    [filteredRows, batchItemByKey],
  )

  const changeSelection = React.useCallback((keys: string[], checked: boolean): void => {
    setSelected(previous => {
      const next = new Set(previous)
      keys.forEach(key => checked ? next.add(key) : next.delete(key))
      return next
    })
  }, [])
  const handleToggleSelect = React.useCallback((key: string, checked: boolean): void => {
    changeSelection([key], checked)
  }, [changeSelection])

  const batchItemsRef = React.useRef(batchItems)
  batchItemsRef.current = batchItems
  const missingRef = React.useRef(missing)
  missingRef.current = missing
  const selectedRef = React.useRef(selected)
  selectedRef.current = selected
  const pendingUpdatesRef = React.useRef(pendingUpdates)
  pendingUpdatesRef.current = pendingUpdates

  const confirmBatch = React.useCallback((action: PluginBatchAction, items?: PluginBatchItem[]): void => {
    const picked = items ?? batchItemsRef.current.filter(item => selectedRef.current.has(item.key))
    const current = picked.flatMap(item => {
      const row = item.required
        ? missingRef.current.required.find(r => r.key === item.key) || batchItemsRef.current.find(r => r.key === item.key)
        : batchItemsRef.current.find(r => r.key === item.key)
      return row ? [{ ...row, required: item.required }] : []
    })
    const targets = applicablePluginItems(current, action)
    if (targets.length) setBatchConfirm({ action, items: targets })
  }, [])

  React.useEffect(() => {
    if (!payload) return
    const valid = new Set(batchItems.map(item => item.key))
    setSelected(previous => {
      const next = new Set([...previous].filter(key => valid.has(key)))
      return next.size === previous.size ? previous : next
    })
  }, [payload, batchItems])

  React.useEffect(() => {
    if (listRef.current) listRef.current.scrollTop = 0
  }, [rail, deferredKeyword, sortBy, taskStatus])

  React.useEffect(() => {
    if (settingsPackId && payload && !settingsPack) setSettingsPackId(null)
    if (detailsStore && installedIds.has(detailsStore.id)) setDetailsStore(null)
  }, [payload, settingsPack, settingsPackId, detailsStore, installedIds])

  const openPackDetails = React.useCallback((id: string, pane: PluginDetailsPane): void => {
    setDetailsPane(pane)
    setSettingsPackId(id)
  }, [])
  const handleEnablePack = React.useCallback((id: string, enabled: boolean) => {
    void mutation(`enable:${id}`, '/api/plugins/enable', { id, enabled })
  }, [mutation])
  const handlePackSettings = React.useCallback((id: string) => openPackDetails(id, 'settings'), [openPackDetails])
  const handlePackAbout = React.useCallback((id: string) => openPackDetails(id, 'about'), [openPackDetails])
  const handleInstallStore = React.useCallback((id: string) => { void start(id, false) }, [start])
  const handleOpenStoreDetails = React.useCallback((item: StorePlugin) => setDetailsStore(item), [])
  const handleShowStoreNotes = React.useCallback((item: StorePlugin) => {
    setNotes({
      name: pluginName(item.id, item.name),
      version: item.version || '',
      notes: item.notes,
      page: item.page,
      prerelease: item.prerelease,
    })
  }, [])

  const handleToggleSelectMode = React.useCallback(() => {
    setSelecting(value => !value)
    setSelected(new Set())
  }, [])
  const handleUpdateAll = React.useCallback(() => confirmBatch('update', pendingUpdatesRef.current), [confirmBatch])
  const handleOpenMissing = React.useCallback(() => setMissingOpen(true), [])
  const handleRefreshStore = React.useCallback(() => { void refreshStore() }, [refreshStore])
  const handleCheck = React.useCallback(() => { void check() }, [check])
  const handleOpenSettings = React.useCallback(() => setSettingsOpen(true), [])
  const handleOpenScript = React.useCallback(() => setScriptInstallRequest(value => value + 1), [])
  const handleOpenDiagnostics = React.useCallback(() => setDiagnosticsOpen(true), [])
  const handleExitBatch = React.useCallback(() => { setSelecting(false); setSelected(new Set()) }, [])
  const handleTaskImport = React.useCallback(() => setTaskImportRequest(value => value + 1), [])
  const handleTaskCreate = React.useCallback(() => setTaskCreateRequest(value => value + 1), [])
  const handleTaskNotify = React.useCallback((message: string, kind?: 'ok' | 'err') => {
    toast(message, kind === 'err' ? 'err' : 'ok')
  }, [])
  const handleOpenScheduledPage = React.useCallback((pluginId?: string) => {
    bridge().wbApp?.showPage?.('scheduled')
    if (pluginId) window.wbScheduledPage?.openCreateForPlugin?.(pluginId)
  }, [])
  const scrollListToTop = React.useCallback(() => { listRef.current?.scrollTo({ top: 0, behavior: 'smooth' }) }, [])

  const isScheduledRail = rail === 'scheduled'
  const mergedRailCounts = React.useMemo(() => ({
    ...railCounts,
    scheduled: taskData.totalCount,
    scheduledUpdatable: taskData.updatableCount,
  }), [railCounts, taskData.totalCount, taskData.updatableCount])
  const toolbarCount = isScheduledRail ? taskData.countMatching(taskStatus, deferredKeyword) : filteredRows.length
  const showCommunityCatalogStatus = (rail === 'all' || rail === 'community') && Boolean(catalog?.error)
  const isMagpieRail = rail === 'magpie-official' || rail === 'magpie-recommended' || rail === 'magpie-third-party'
  const showScriptCatalogStatus = Boolean(
    scripts?.catalog && (isMagpieRail || (rail === 'all' && (scripts.catalog.refreshing || scripts.catalog.marketError || scripts.catalog.githubError)))
  )
  const topError = isScheduledRail ? taskData.error : (error || syncError || (typeof scriptCtrl.failure === 'string' ? scriptCtrl.failure : ''))

  const renderRow = (row: UnifiedPluginRow) => {
    const selectionDisabled = locked || batchItemByKey.get(row.selectionKey)?.active === true
    const rowSelected = selected.has(row.selectionKey)
    if (row.kind === 'native-installed') {
      const pack = row.pack
      const operation = operationById.get(pack.id)
      return <InstalledPluginCard key={row.key} viewMode={viewMode} pack={pack} store={row.store}
        host={host || null} update={updatesById.get(pack.id)} operation={operation} disabled={locked || operation?.active === true}
        selecting={selecting} selected={rowSelected} selectionDisabled={selectionDisabled}
        selectionKey={row.selectionKey} onToggleSelect={handleToggleSelect}
        onEnabled={handleEnablePack} onSettings={handlePackSettings} onDetails={handlePackAbout} />
    }
    if (row.kind === 'native-store') {
      const item = row.item
      const operation = operationById.get(item.id)
      return <StorePluginCard key={row.key} viewMode={viewMode} item={item} target={target}
        operation={operation} disabled={locked}
        selecting={selecting} selected={rowSelected} selectionDisabled={selectionDisabled}
        selectionKey={row.selectionKey} onToggleSelect={handleToggleSelect}
        onInstall={handleInstallStore} onDetails={handleOpenStoreDetails} onNotes={handleShowStoreNotes} />
    }
    if (row.kind === 'script-installed') {
      return scriptCtrl.renderInstalled(row.pack, {
        viewMode,
        selecting,
        selected: rowSelected,
        selectionDisabled,
        selectionKey: row.selectionKey,
        onToggleSelect: handleToggleSelect,
      })
    }
    return scriptCtrl.renderRecommended(row.item, {
      viewMode,
      selecting,
      selected: rowSelected,
      selectionDisabled,
      selectionKey: row.selectionKey,
      onToggleSelect: handleToggleSelect,
    })
  }

  return <>
    <div className='plugins-page-layout'>
      <PluginCategoryRail value={rail} counts={mergedRailCounts} scriptsAvailable={Boolean(scripts)} onChange={selectRail} />

      <div className='plugins-main-pane'>
        <div className='plugins-fixed-area'>
          <PluginPageToolbar
            title={pluginRailTitle(rail)}
            count={toolbarCount}
            subtitle={pluginRailSubtitle(rail)}
            search={search}
            sort={sortBy}
            viewMode={viewMode}
            selecting={selecting}
            locked={locked}
            checking={busy === 'check' || catalog?.checking === true}
            scriptsAvailable={Boolean(scripts)}
            scriptActive={scripts?.operation.active === true}
            scheduledMode={isScheduledRail}
            updateCount={pendingUpdates.length}
            updateDisabled={locked || active || scripts?.operation.active === true}
            onSearch={setSearch}
            onSort={setSortBy}
            onViewMode={setViewMode}
            onSelect={handleToggleSelectMode}
            onUpdateAll={handleUpdateAll}
            onTaskImport={handleTaskImport}
            onTaskCreate={handleTaskCreate}
            onFile={fileInstall.show}
            onMissing={handleOpenMissing}
            onRefresh={handleRefreshStore}
            onCheck={handleCheck}
            onSettings={handleOpenSettings}
            onScript={handleOpenScript}
            onDiagnostics={handleOpenDiagnostics}
          />
          {topError ? <Alert variant='destructive' className='whitespace-pre-wrap break-words'>{topError}</Alert> : null}
          {!isScheduledRail && (missing.required.length || missing.bindable.length || missing.unresolved.length) ? <PluginMissingBanner
            count={missing.required.length + missing.bindable.length + missing.unresolved.length}
            accounts={missingAccounts} onOpen={handleOpenMissing} t={t} /> : null}
          {isScheduledRail ? <PluginScheduledFilterBar
            status={taskStatus}
            totalCount={taskData.totalCount}
            installedCount={taskData.installedCount}
            storeUninstalledCount={taskData.storeUninstalledCount}
            updatableCount={taskData.updatableCount}
            onStatusChange={setTaskStatus}
            t={t}
          /> : null}
          {!isScheduledRail && selecting ? <PluginBatchToolbar items={batchItems}
            visibleKeys={selectableVisibleKeys}
            selected={selected} disabled={locked} onSelection={changeSelection} onAction={confirmBatch}
            onExit={handleExitBatch} t={t} /> : null}
          {batch.task ? <PluginBatchProgress task={batch.task} t={t} onClose={batch.clear}
            onRetry={items => confirmBatch(batch.task!.action, items)} /> : null}
          {fileInstall.backgroundVisible ? <PluginFileInstallProgress model={fileInstall} t={t} /> : null}
        </div>

        <div ref={listRef} className='plugins-scroll-list'>
          {isScheduledRail && !taskData.loaded && !taskData.payload ? (
            <Card className='shrink-0'><Empty>
              <EmptyTitle>{t('正在读取插件列表…')}</EmptyTitle>
            </Empty></Card>
          ) : null}
          <TaskPluginsSection
            payload={taskData.payload}
            status={taskStatus}
            query={deferredSearch}
            request={api}
            refresh={taskData.refresh}
            jobCounts={taskData.jobCounts}
            embedded
            hideContent={!isScheduledRail || (!taskData.loaded && !taskData.payload)}
            scrollRef={listRef}
            importRequest={taskImportRequest}
            createRequest={taskCreateRequest}
            t={t}
            notify={handleTaskNotify}
            onOpenScheduledPage={handleOpenScheduledPage}
          />

          {!isScheduledRail ? <>
            {showCommunityCatalogStatus ? (
              <PluginCatalogStatus catalog={catalog} refreshing={busy === 'refresh' || busy === 'check' || catalog?.checking === true}
                disabled={locked} onRetry={refreshStore} t={t} />
            ) : null}
            {showScriptCatalogStatus && scripts?.catalog ? (
              <ScriptCatalogStatus catalog={scripts.catalog} request={scriptRequest}
                refresh={forceLoad} disabled={locked} t={t} />
            ) : null}

            {!payload ? (
              <Card className='shrink-0'><Empty>
                <EmptyTitle>{t('正在读取插件列表…')}</EmptyTitle>
              </Empty></Card>
            ) : filteredRows.length === 0 && !unmatchedScriptOperation ? (
              <Card className='shrink-0'><Empty>
                <EmptyTitle>
                  {!hasAny && catalog?.checking ? t('正在读取插件列表…')
                    : rail === 'updatable' && !deferredKeyword ? t('所有已安装插件均为最新版本')
                      : rail === 'installed' && !deferredKeyword ? t('还没有安装任何插件')
                        : hasAny ? t('没有符合筛选条件的插件') : t('暂无可安装的插件')}
                </EmptyTitle>
                <EmptyDescription>
                  {rail === 'installed' && !deferredKeyword && hasAny
                    ? t('可以从左侧「插件商店」挑选并安装需要的提供商插件。')
                    : rail === 'updatable' && !deferredKeyword
                      ? t('可在右上角更多操作中随时检查更新。')
                      : hasAny ? t('试试更换搜索词，或切换到其他插件分类查看。')
                        : summarizeNetworkError(catalog?.reason, t).summary || (unpublishedCount > 0
                          ? t('插件发布后会显示在这里。')
                          : t('可以点击「刷新」读取插件仓库，或从本地 zip 安装插件。'))}
                </EmptyDescription>
                {search.trim() ? (
                  <Button variant='outline' size='sm' className='mt-2' onClick={() => setSearch('')}>{t('清除筛选')}</Button>
                ) : rail !== 'all' && hasAny ? (
                  <Button variant='outline' size='sm' className='mt-2' onClick={() => selectRail('all')}>{t('浏览全部插件')}</Button>
                ) : !hasAny && catalog?.error ? (
                  <Button variant='outline' size='sm' className='mt-2' disabled={locked || catalog.checking}
                    onClick={() => void refreshStore()}>{t('刷新重试')}</Button>
                ) : null}
              </Empty></Card>
            ) : (
              <section className='@container/plugin-gallery min-w-0 shrink-0' aria-label={pluginRailTitle(rail)}>
                {viewMode === 'cards' ? (
                  <div className={PLUGIN_GRID_CLASS}>
                    {unmatchedScriptOperation ? <ScriptOperationCard operation={scriptCtrl.operation} t={t} /> : null}
                    {visibleRows.map(renderRow)}
                  </div>
                ) : (
                  <>
                    {unmatchedScriptOperation ? <div className='mb-3'><ScriptOperationCard operation={scriptCtrl.operation} t={t} /></div> : null}
                    <div className='plugins-list-stack'>
                      {visibleRows.map(renderRow)}
                    </div>
                  </>
                )}
                <PluginStreamFooter
                  stream={stream}
                  scrollRef={listRef}
                  onScrollTop={scrollListToTop}
                  t={t}
                />
              </section>
            )}

            <p className='m-0 shrink-0 text-[11.5px] leading-relaxed text-muted-foreground'>
              {t('插件独立安装与更新。卸载后账号数据仍会保留；脚本插件共用一个按需下载的运行时。')}
            </p>
          </> : null}
        </div>
      </div>
    </div>

    {scriptCtrl.dialogs}
    <PluginFileInstallDialog
      model={fileInstall}
      maxPackBytes={maxPackBytes}
      onOpenScheduledTasks={() => { fileInstall.close(); bridge().wbApp?.showPage?.('scheduled') }}
      t={t}
    />
    {batchConfirm ? <PluginBatchConfirm key={`${batchConfirm.action}:${batchConfirm.items.map(item => item.key).join('|')}`}
      action={batchConfirm.action} items={batchConfirm.items} disabled={locked} t={t} onClose={() => setBatchConfirm(null)}
      onConfirm={() => { const { action, items } = batchConfirm; setBatchConfirm(null); void batch.execute(action, items) }} /> : null}
    {missingOpen ? <PluginMissingDialog required={missing.required} bindable={missing.bindable} unresolved={missing.unresolved}
      disabled={locked} t={t} onClose={() => setMissingOpen(false)}
      onInstall={items => { setMissingOpen(false); void batch.execute('install', items) }}
      onBindAlias={async ({ packageId, original, alias }) => {
        const response = await api<{ operation?: import('../../../ui-shared/plugins').ScriptOperation }>('POST', '/api/plugins/scripts/change', {
          id: packageId, action: 'bind_alias', original, alias,
        })
        if (response?.operation) {
          const finalOp = await waitScriptOperation(response.operation, api as PluginRequest, t)
          if (finalOp.error || finalOp.stage === 'failed') throw new Error(finalOp.error || t('插件操作失败'))
        }
        refreshPluginStartup()
        const next = await api<PluginsPayload>('GET', '/api/plugins')
        lastRevision.current = typeof next.revision === 'string' ? next.revision : ''
        setPayload(next)
        await synchronize(next)
        await forceLoad()
      }} /> : null}
    <PluginSettingsDialog open={settingsOpen} onClose={() => setSettingsOpen(false)} onChanged={check} />
    <PluginDiagnostics open={diagnosticsOpen} payload={payload} onClose={() => setDiagnosticsOpen(false)} />
    {startupId ? <PluginStartupDialog id={startupId}
      name={packs.find(pack => pack.id === startupId)?.name || payload?.scripts?.packages.find(pack => pack.id === startupId)?.name || startupId}
      status={packs.find(pack => pack.id === startupId)?.startup || payload?.scripts?.packages.find(pack => pack.id === startupId)?.startup}
      script={startupId.startsWith('script-')} t={t} onClose={() => setStartupId(null)}
      retry={async () => {
        if (startupId.startsWith('script-')) await api('POST', '/api/plugins/scripts/change', { id: startupId, action: 'reload' })
        else await api('POST', '/api/plugins/reload', { id: startupId })
        refreshPluginStartup()
        await forceLoad()
      }} /> : null}
    {detailsItem ? <PluginDetailsDialog name={pluginName(detailsItem.id, detailsItem.name)}
      description={pluginDescription(detailsItem.id, detailsItem.description)} version={detailsItem.version}
      sourceLabel={t('社区插件')} icon={detailsItem.icon} artwork={detailsItem.id === 'agent2api-web-providers' ? 'web' : 'providers'}
      t={t} onClose={() => setDetailsStore(null)}
      metadata={detailsItem.error ? <div className='text-xs text-warning'>{errorMessage(detailsItem.reason)}</div>
        : detailsItem.version && !detailsInstallable ? <p className='m-0 text-xs text-warning'>{t('当前平台暂无包')}</p> : null}
      aboutActions={detailsItem.version ? <Button variant='link' size='sm' onClick={() => {
        setDetailsStore(null)
        setNotes({ name: pluginName(detailsItem.id, detailsItem.name), version: detailsItem.version || '',
          notes: detailsItem.notes, page: detailsItem.page, prerelease: detailsItem.prerelease })
      }}>{t('更新日志')}</Button> : null}
      primaryAction={<PluginInstallButton t={t} disabled={locked || !detailsInstallable || operations.some(item => item.id === detailsItem.id && item.active)}
        onClick={() => { setDetailsStore(null); void start(detailsItem.id, false) }} />} /> : null}
    {settingsPack ? <PluginPackSettingsDialog key={settingsPack.id} pack={settingsPack} initialPane={detailsPane}
      store={storeById.get(settingsPack.id)} update={updatesById.get(settingsPack.id)} operation={settingsOperation}
      disabled={locked || settingsOperation?.active === true} onClose={() => setSettingsPackId(null)}
      onEnabled={enabled => void mutation(`enable:${settingsPack.id}`, '/api/plugins/enable', { id: settingsPack.id, enabled })}
      onProvider={(provider, enabled) => void mutation(`provider:${provider.id}`, '/api/plugins/provider', {
        id: settingsPack.id, provider: provider.id, enabled,
      })}
      onUpdate={() => void start(settingsPack.id, true)}
      onReload={async () => { await api('POST', '/api/plugins/reload', { id: settingsPack.id }); await forceLoad() }}
      onNotes={() => {
        const update = updatesById.get(settingsPack.id)
        setSettingsPackId(null)
        setNotes({ name: pluginName(settingsPack.id, settingsPack.name), version: update?.latest || settingsPack.version,
          notes: update?.notes, page: update?.page, prerelease: update?.prerelease })
      }}
      onRemove={() => { setConfirmRemove(settingsPack); setSettingsPackId(null) }} /> : null}
    <PluginNotesDialog value={notes} onClose={() => setNotes(null)} />
    <Dialog open={confirmRemove !== null} onOpenChange={next => { if (!next) setConfirmRemove(null) }}>
      <DialogContent className='text-foreground'>
        <DialogHeader><DialogTitle>{t('卸载插件包')}</DialogTitle></DialogHeader>
        <DialogBody><p className='text-[13px]'>
          {t('确定卸载「{name}」吗？卸载后这个包里的提供商将无法转发，也无法添加账号。账号数据本身不会被删除，重新安装后可以继续使用。', {
            name: confirmRemove ? pluginName(confirmRemove.id, confirmRemove.name) : '',
          })}
        </p></DialogBody>
        <DialogFooter>
          <span className='flex-1' /><Button variant='outline' onClick={() => setConfirmRemove(null)}>{t('取消')}</Button>
          <Button variant='destructive' disabled={locked} onClick={() => {
            const pack = confirmRemove
            setConfirmRemove(null)
            if (pack) void mutation(`remove:${pack.id}`, '/api/plugins/uninstall', { id: pack.id })
          }}>{t('卸载')}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </>
}

const PAGE_SELECTOR = '.page[data-page="plugins"]'
let mounted = false
let currentLoad: (() => Promise<void>) | null = null

function mount(): void {
  if (mounted) return
  const section = document.querySelector<HTMLElement>(PAGE_SELECTOR)
  if (!section) return
  mounted = true
  section.replaceChildren()
  createRoot(section).render(<PluginsPage />)
}

mountOnPage('plugins', PAGE_SELECTOR, mount)

declare global {
  interface Window {
    wbPluginsPage?: { load(): Promise<void> }
    wbScheduledPage?: { load(): Promise<void>; openCreateForPlugin?(pluginId: string): void }
  }
}

window.wbPluginsPage = { async load() { await currentLoad?.() } }
