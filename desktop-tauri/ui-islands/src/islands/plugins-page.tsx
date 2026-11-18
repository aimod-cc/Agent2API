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
import { clampMaxPackBytes, LEGACY_MAX_PACK_BYTES, PluginFileInstallDialog, usePluginFileInstall } from './plugins-file-install'
import type { PackSummary, PluginMutation, PluginOperation, PluginUpdate, PluginsPayload, ReleaseState, StorePlugin } from './plugins-types'
import { t } from '../i18n'
import { ScriptPluginsPanel } from '../../../ui-shared/plugins-scripts'
import { matchesPluginStatus, matchesScriptPublisher, type PluginFilterItem, type PluginStatusFilter, type ScriptPublisherFilter } from '../../../ui-shared/plugins-filters'
import { isRecommendationInstalled, matchesRecommendedPlugin, matchesScriptPlugin, scriptRecommendations, scriptUpdate } from '../../../ui-shared/plugins-script-display'
import { PLUGIN_GRID_CLASS, PluginGetButton, PluginSectionHeading } from '../../../ui-shared/plugins-store'
import { PluginDetailsDialog, type PluginDetailsPane } from '../../../ui-shared/plugins-details'
import { PluginDiagnostics } from './plugins-page-tools'
import { useGatewayBase } from './port-panel'
import { PluginStartupDialog, subscribePluginDiagnostics, takePluginDiagnostics } from '../../../ui-shared/plugin-startup-view'
import { refreshPluginStartup } from '../../../ui-shared/plugin-startup'
import { openExternal } from './update-shared'
import { PluginPageToolbar, type PluginSourceFilter } from './plugins-toolbar'
import { applicablePluginItems, missingPluginItems, packSelectionKey, pluginBatchItems, scriptSelectionKey, type PluginBatchAction, type PluginBatchItem } from '../../../ui-shared/plugins-batch-items'
import { usePluginBatch } from '../../../ui-shared/plugins-batch-runner'
import { PluginBatchConfirm, PluginBatchProgress, PluginBatchToolbar } from '../../../ui-shared/plugins-batch-view'
import { PluginMissingBanner, PluginMissingDialog } from '../../../ui-shared/plugins-missing'
import '../styles/plugins-page.css'

type SourceFilter = PluginSourceFilter

function PluginsPage(): React.ReactElement {
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
  const [source, setSource] = React.useState<SourceFilter>('all')
  const [status, setStatus] = React.useState<PluginStatusFilter>('all')
  const [scriptPublisher, setScriptPublisher] = React.useState<ScriptPublisherFilter>('all')
  const [search, setSearch] = React.useState('')
  const [diagnosticsOpen, setDiagnosticsOpen] = React.useState(false)
  const [startupId, setStartupId] = React.useState<string | null>(null)
  React.useEffect(() => {
    const open = () => { const id = takePluginDiagnostics(); if (id) setStartupId(id) }
    open()
    return subscribePluginDiagnostics(open)
  }, [])
  const [scriptBusy, setScriptBusy] = React.useState(false)
  const [scriptInstallRequest, setScriptInstallRequest] = React.useState(0)
  const [selecting, setSelecting] = React.useState(false)
  const [selected, setSelected] = React.useState(new Set<string>())
  const [batchConfirm, setBatchConfirm] = React.useState<{ action: PluginBatchAction; items: PluginBatchItem[] } | null>(null)
  const [missingOpen, setMissingOpen] = React.useState(false)
  const listRef = React.useRef<HTMLDivElement>(null)
  const seenOperations = React.useRef(new Set<string>())
  const previousSignature = React.useRef('')
  const mounted = React.useRef(true)
  const fileOperationLocked = React.useRef(false)
  const mutationLocked = React.useRef(false)
  const loading = React.useRef<Promise<void> | null>(null)

  const synchronize = React.useCallback(async (result: PluginMutation): Promise<void> => {
    const app = bridge().wbApp
    const [refreshed] = await Promise.all([app?.refresh?.(), app?.refreshPluginPages?.(result)])
    if (mounted.current) setSyncError(refreshed === true ? ''
      : t('插件状态已变更，但账号列表同步失败。请回到账号页点击「刷新」重试。'))
  }, [])

  const load = React.useCallback(async (): Promise<void> => {
    if (loading.current) return loading.current
    const request = (async () => {
      try {
        const next = await api<PluginsPayload & { maxPackBytes?: unknown }>('GET', '/api/plugins')
        if (!mounted.current) return
        const signature = JSON.stringify({
          packs: next.packs.map(pack => ({ id: pack.id, version: pack.version, enabled: pack.enabled, providers: pack.providers })),
          running: next.host?.running,
          scripts: next.scripts?.packages.map(pack => ({
            id: pack.id, version: pack.version, enabled: pack.enabled, running: pack.running, providers: pack.providers, error: pack.error,
          })),
        })
        const changed = Boolean(previousSignature.current && previousSignature.current !== signature)
        previousSignature.current = signature
        setMaxPackBytes(clampMaxPackBytes(next.maxPackBytes))
        setPayload(next)
        setError('')
        void bridge().wbApp?.refreshPluginPages?.(next)
        if (changed) await synchronize(next)
        for (const operation of next.operations || []) {
          if (operation.active) continue
          const key = `${operation.id}:${operation.updatedAt}`
          if (seenOperations.current.has(key)) continue
          seenOperations.current.add(key)
          if (Date.now() - operation.updatedAt > 30_000) continue
          if (operation.error) toast(operation.error, 'err')
          else if (operation.hostError) toast(operation.hostError, 'err')
          else if (operation.result?.installNotice) toast(t(operation.result.installNotice), 'ok')
          else if (operation.done) toast(t('插件包已安装到 v{version}', { version: operation.version }), 'ok')
        }
      } catch (cause) {
        if (mounted.current) setError(errorMessage(cause))
      }
    })()
    loading.current = request
    try { await request } finally { loading.current = null }
  }, [synchronize])

  React.useEffect(() => {
    mounted.current = true
    void load()
    const timer = window.setInterval(() => {
      const page = bridge().wbApp?.currentPage
      if (!document.hidden && (!page || page === 'plugins')) void load()
    }, 1500)
    return () => { mounted.current = false; window.clearInterval(timer) }
  }, [load])

  React.useEffect(() => {
    currentLoad = load
    return () => { if (currentLoad === load) currentLoad = null }
  }, [load])

  async function run(key: string, action: () => Promise<void>): Promise<void> {
    if (busy || scriptBusy || fileOperationLocked.current || mutationLocked.current) return
    mutationLocked.current = true
    setBusy(key)
    try { await action() }
    catch (error) { toast(error instanceof Error ? error.message : String(error), 'err') }
    finally { mutationLocked.current = false; if (mounted.current) setBusy('') }
  }

  async function check(): Promise<void> {
    await run('check', async () => {
      // 本体插件（发布包）与 Magpie 侧脚本插件各有各的检查链路，一起问，别只查一半。
      const [result] = await Promise.all([
        api<{ updates: PluginUpdate[]; error: boolean; reason?: string }>('GET', '/api/plugins/update'),
        api('GET', '/api/plugins/scripts/check').catch(() => undefined),
      ])
      await load()
      if (result.error) toast(result.reason || t('检查更新失败'), 'err')
    })
  }

  async function refreshStore(): Promise<void> {
    await run('refresh', async () => {
      const result = await api<ReleaseState>('GET', '/api/plugins/release')
      await load()
      if (result.error) toast(result.reason || t('检查更新失败'), 'err')
    })
  }

  async function accept(result: PluginMutation): Promise<void> {
    setPayload(previous => previous ? { ...previous, ...result } : previous)
    await synchronize(result)
    if (result.hostError) toast(result.hostError, 'err')
    else if (result.installNotice) toast(t(result.installNotice), 'ok')
    await load()
  }

  async function mutation(key: string, path: string, body: unknown): Promise<void> {
    await run(key, async () => { await accept(await api<PluginMutation>('POST', path, body)) })
  }

  async function start(id: string, update: boolean): Promise<void> {
    await run(`start:${id}`, async () => {
      const result = await api<{ operation: PluginOperation }>('POST', update
        ? '/api/plugins/update' : '/api/plugins/install-from-release', { id, target: payload?.target })
      setPayload(previous => previous ? {
        ...previous, operations: [...(previous.operations || []).filter(item => item.id !== id), result.operation],
      } : previous)
    })
  }

  const batch = usePluginBatch({ request: api, refresh: load, target: payload?.target || '', t,
    disabled: Boolean(busy) || scriptBusy || fileOperationLocked.current,
    synchronize: async () => {
      const next = await api<PluginsPayload>('GET', '/api/plugins')
      setPayload(next)
      await synchronize(next)
    },
    completed: items => setSelected(previous => { const next = new Set(previous); items.forEach(item => next.delete(item.key)); return next }),
  })
  const fileInstall = usePluginFileInstall({ request: api, t, maxPackBytes, previewAvailable: payload?.supportsPackPreview === true, disabled: Boolean(busy) || scriptBusy || batch.running,
    accept, refresh: load, remember: operation => {
      if (!operation.active) seenOperations.current.add(`${operation.id}:${operation.updatedAt}`)
      setPayload(previous => previous ? { ...previous, operations: [...(previous.operations || []).filter(item => item.id !== operation.id), operation] } : previous)
    },
  })
  fileOperationLocked.current = fileInstall.running
  mutationLocked.current = Boolean(busy) || batch.running

  const packs = payload?.packs || []
  const host = payload?.host
  const catalog = payload?.catalog
  const store = catalog?.plugins || []
  const detailsItem = detailsStore ? store.find(item => item.id === detailsStore.id) || detailsStore : null
  const detailsInstallable = Boolean(detailsItem?.version && !detailsItem.error
    && detailsItem.platforms.some(platform => platform.target === 'wasm32-unknown-unknown'))
  const installedIds = new Set(packs.map(pack => pack.id))
  const storePlugins = store.filter(item => !installedIds.has(item.id))
  const operations = payload?.operations || []
  const active = operations.some(operation => operation.active)
  const target = payload?.target || ''
  const locked = Boolean(busy) || scriptBusy || batch.running || fileInstall.running
  const settingsPack = packs.find(pack => pack.id === settingsPackId)
  const settingsOperation = operations.find(operation => operation.id === settingsPackId)

  const updates = payload?.updates || []
  const updateOf = (id: string) => updates.find(item => item.id === id)
  const updatable = (id: string) => updateOf(id)?.available === true
  const scripts = payload?.scripts
  const scriptItems = scripts?.packages || []
  const recommendations = scripts ? scriptRecommendations(scripts) : []
  const missingScripts = recommendations.filter(item => !isRecommendationInstalled(item, scriptItems))
  const keyword = search.trim().toLocaleLowerCase()
  const searchedPacks = packs.filter(pack => {
    const entry = store.find(item => item.id === pack.id)
    return !keyword || [pack.id, pluginName(pack.id, entry?.name || pack.name), pluginDescription(pack.id, entry?.description || pack.description),
      ...pack.providers.map(provider => `${provider.id} ${t(provider.label)}`)].join(' ').toLocaleLowerCase().includes(keyword)
  })
  const searchedStore = storePlugins.filter(item => !keyword
    || `${item.id} ${pluginName(item.id, item.name)} ${pluginDescription(item.id, item.description)}`.toLocaleLowerCase().includes(keyword))
  const searchedScripts = scriptItems.filter(pack => matchesScriptPlugin(pack, recommendations, search, t))
  const recommendedMatches = missingScripts.filter(item => matchesRecommendedPlugin(item, search, t))
  const filteredScripts = searchedScripts.filter(pack => matchesScriptPublisher(pack, scriptPublisher))
  const filteredRecommendations = recommendedMatches.filter(item => matchesScriptPublisher(item, scriptPublisher))
  const nativeItems: PluginFilterItem[] = [
    ...searchedPacks.map(pack => ({ installed: true, updatable: updatable(pack.id) })),
    ...searchedStore.map(() => ({ installed: false, updatable: false })),
  ]
  const magpieItems: PluginFilterItem[] = [
    ...filteredScripts.map(pack => ({ installed: true, updatable: scriptUpdate(scripts, pack.id) !== undefined })),
    ...filteredRecommendations.map(() => ({ installed: false, updatable: false })),
  ]
  const itemsFor = (chosen: SourceFilter): PluginFilterItem[] => chosen === 'agent2' ? nativeItems
    : chosen === 'magpie' ? magpieItems : [...nativeItems, ...magpieItems]
  const countMatching = (chosen: SourceFilter, next: PluginStatusFilter) => itemsFor(chosen).filter(item => matchesPluginStatus(item, next)).length
  const showNative = source !== 'magpie'
  const showScripts = source !== 'agent2'
  const installedRows = searchedPacks.filter(pack => matchesPluginStatus({ installed: true, updatable: updatable(pack.id) }, status))
  const storeRows = searchedStore.filter(() => matchesPluginStatus({ installed: false, updatable: false }, status))
  const scriptRows = filteredScripts.filter(pack => matchesPluginStatus({ installed: true, updatable: scriptUpdate(scripts, pack.id) !== undefined }, status))
  const shownCount = countMatching(source, status)
  const hasAny = packs.length + storePlugins.length + scriptItems.length + missingScripts.length > 0
  const filtersActive = source !== 'all' || status !== 'all' || scriptPublisher !== 'all' || keyword.length > 0
  const scriptGalleryVisible = Boolean(scripts && showScripts)

  function clearFilters(): void {
    setSource('all'); setStatus('all'); setScriptPublisher('all'); setSearch('')
  }
  const resolveIcon = (path: string) => `${gatewayBase.replace(/\/+$/, '')}${path}`
  const batchItems = pluginBatchItems(payload, t, resolveIcon)
  const shownKeys = [
    ...(showNative ? [...installedRows.map(pack => packSelectionKey(pack.id)), ...storeRows.map(item => packSelectionKey(item.id))] : []),
    ...(showScripts ? [...scriptRows.map(scriptSelectionKey), ...filteredRecommendations.filter(() => status === 'all' || status === 'missing').map(scriptSelectionKey)] : []),
  ]
  const pendingUpdates = applicablePluginItems(batchItems.filter(item => shownKeys.includes(item.key)), 'update')
  const missing = missingPluginItems(payload, batchItems)
  const missingAccounts = missing.required.reduce((total, item) => total + item.accounts, 0) + missing.unresolved.reduce((total, item) => total + item.count, 0)

  function changeSelection(keys: string[], checked: boolean): void {
    setSelected(previous => { const next = new Set(previous); keys.forEach(key => checked ? next.add(key) : next.delete(key)); return next })
  }
  const selectionFor = (key: string) => ({ checked: selected.has(key), disabled: locked || batchItems.find(item => item.key === key)?.active === true,
    onChange: (checked: boolean) => changeSelection([key], checked) })
  function selectGroup(kind: 'pack' | 'script'): void {
    const keys = batchItems.filter(item => item.kind === kind && shownKeys.includes(item.key) && !item.active).map(item => item.key)
    changeSelection(keys, !keys.every(key => selected.has(key)))
  }
  function confirmBatch(action: PluginBatchAction, items = batchItems.filter(item => selected.has(item.key))): void {
    const current = items.flatMap(item => { const row = item.required ? missing.required.find(row => row.key === item.key) || batchItems.find(row => row.key === item.key) : batchItems.find(row => row.key === item.key); return row ? [{ ...row, required: item.required }] : [] })
    const targets = applicablePluginItems(current, action)
    if (targets.length) setBatchConfirm({ action, items: targets })
  }

  React.useEffect(() => {
    if (!payload) return
    const valid = new Set(batchItems.map(item => item.key))
    setSelected(previous => {
      const next = new Set([...previous].filter(key => valid.has(key)))
      return next.size === previous.size ? previous : next
    })
  }, [payload])

  React.useEffect(() => {
    if (listRef.current) listRef.current.scrollTop = 0
  }, [source, status, scriptPublisher, search])

  React.useEffect(() => {
    if (settingsPackId && payload && !settingsPack) setSettingsPackId(null)
    if (detailsStore && payload?.packs.some(pack => pack.id === detailsStore.id)) setDetailsStore(null)
  }, [payload, settingsPack, settingsPackId, detailsStore])

  function openPackDetails(id: string, pane: PluginDetailsPane): void {
    setDetailsPane(pane); setSettingsPackId(id)
  }

  return <>
    <div className='plugins-page-layout'>
      <div className='plugins-fixed-area'>
        <PluginPageToolbar source={source} status={status} search={search} count={countMatching}
          selecting={selecting} locked={locked} checking={busy === 'check' || catalog?.checking === true}
          scriptsAvailable={Boolean(scripts)} scriptActive={scripts?.operation.active === true}
          onSource={setSource} onStatus={setStatus} onSearch={setSearch}
          onSelect={() => { setSelecting(value => !value); setSelected(new Set()) }} onFile={fileInstall.show} onMissing={() => setMissingOpen(true)}
          onRefresh={() => void refreshStore()} onCheck={() => void check()} onSettings={() => setSettingsOpen(true)}
          onScript={() => setScriptInstallRequest(value => value + 1)} onDiagnostics={() => setDiagnosticsOpen(true)} />
        {error || syncError ? <Alert variant='destructive' className='break-words'>{error || syncError}</Alert> : null}
        {missing.required.length || missing.unresolved.length ? <PluginMissingBanner count={missing.required.length + missing.unresolved.length}
          accounts={missingAccounts} onOpen={() => setMissingOpen(true)} t={t} /> : null}
        {selecting ? <PluginBatchToolbar items={batchItems} visibleKeys={shownKeys.filter(key => !batchItems.find(item => item.key === key)?.active)}
          selected={selected} disabled={locked} onSelection={changeSelection} onAction={confirmBatch}
          onExit={() => { setSelecting(false); setSelected(new Set()) }} t={t} /> : pendingUpdates.length ? <div className='plugins-update-summary'>
          <span>{t('有 {count} 个插件可更新', { count: pendingUpdates.length })}</span>
          <Button variant='link' size='xs' disabled={locked || active || scripts?.operation.active === true} onClick={() => confirmBatch('update', pendingUpdates)}>{t('全部更新')}</Button>
        </div> : null}
        {batch.task ? <PluginBatchProgress task={batch.task} t={t} onClose={batch.clear}
          onRetry={items => confirmBatch(batch.task!.action, items)} /> : null}
        {!fileInstall.open && fileInstall.results.length ? <div className='plugins-update-summary'><span>{fileInstall.running ? t('正在安装本地插件包…') : t('本地安装结果')}</span><Button variant='link' size='xs' onClick={() => fileInstall.setOpen(true)}>{t('查看详情')}</Button></div> : null}
      </div>
      <div ref={listRef} className='plugins-scroll-list'>
      {showNative && (installedRows.length > 0 || storeRows.length > 0) ? <section className='@container/plugin-gallery min-w-0 shrink-0' aria-label={t('Agent2API 社区')}>
        <PluginSectionHeading t={t} title={t('Agent2API 社区')} count={installedRows.length + storeRows.length}
          hint={t('独立插件，来自项目插件仓库')} actions={selecting ? <Button variant='link' size='xs' disabled={locked} onClick={() => selectGroup('pack')}>{t('全选本组')}</Button> : null} />
        <div className={PLUGIN_GRID_CLASS}>
          {installedRows.map(pack => {
            const operation = operations.find(item => item.id === pack.id)
            return <InstalledPluginCard key={pack.id} pack={pack} store={store.find(item => item.id === pack.id)}
              host={host || null} update={updateOf(pack.id)} operation={operation} disabled={locked || operation?.active === true}
              selection={selecting ? selectionFor(packSelectionKey(pack.id)) : undefined}
              onEnabled={enabled => void mutation(`enable:${pack.id}`, '/api/plugins/enable', { id: pack.id, enabled })}
              onSettings={() => openPackDetails(pack.id, 'settings')} onDetails={() => openPackDetails(pack.id, 'about')} />
          })}
          {storeRows.map(item => <StorePluginCard key={item.id} item={item} target={target}
            operation={operations.find(operation => operation.id === item.id)} disabled={locked}
            selection={selecting ? selectionFor(packSelectionKey(item.id)) : undefined}
            onInstall={() => void start(item.id, false)} onDetails={() => setDetailsStore(item)}
            onNotes={() => setNotes({ name: pluginName(item.id, item.name), version: item.version || '',
              notes: item.notes, page: item.page, prerelease: item.prerelease })} />)}
        </div>
      </section> : null}
      {!payload || (shownCount === 0 && !scripts?.operation.active && !scriptGalleryVisible) ? <Card className='shrink-0'><Empty>
        <EmptyTitle>{!payload || (!hasAny && catalog?.checking) ? t('正在读取插件列表…')
          : hasAny ? t('没有符合筛选条件的插件') : t('暂无可安装的插件')}</EmptyTitle>
        <EmptyDescription>{hasAny ? t('换一组筛选条件，或把来源和状态都切回「全部」。')
          : errorMessage(catalog?.reason) || t('可以点击「刷新」读取插件仓库，或从本地 zip 安装插件。')}</EmptyDescription>
        {payload && hasAny && filtersActive ? <Button variant='outline' size='sm' className='mt-2'
          onClick={clearFilters}>{t('清除筛选')}</Button> : null}
      </Empty></Card> : null}
      {scripts ? <div className='shrink-0'><ScriptPluginsPanel state={scripts} t={t} disabled={locked} query={search}
        resolveIcon={resolveIcon} onOpenExternal={openExternal}
        selection={selecting ? selectionFor : undefined} onSelectGroup={() => selectGroup('script')}
        galleryVisible={showScripts} showInstallAction={false} installRequest={scriptInstallRequest} onBusyChange={setScriptBusy}
        status={status} publisher={scriptPublisher} onPublisherChange={setScriptPublisher} onClearFilters={clearFilters}
        request={(path, body) => api('POST', path, body)} refresh={load} /></div> : null}
      <p className='m-0 shrink-0 text-[11.5px] leading-relaxed text-muted-foreground'>
        {t('插件独立安装与更新。卸载后账号数据仍会保留；脚本插件共用一个按需下载的运行时。')}
      </p>
      </div>
    </div>
    <PluginFileInstallDialog model={fileInstall} maxPackBytes={maxPackBytes} t={t} />
    {batchConfirm ? <PluginBatchConfirm key={`${batchConfirm.action}:${batchConfirm.items.map(item => item.key).join('|')}`}
      action={batchConfirm.action} items={batchConfirm.items} disabled={locked} t={t} onClose={() => setBatchConfirm(null)}
      onConfirm={() => { const { action, items } = batchConfirm; setBatchConfirm(null); void batch.execute(action, items) }} /> : null}
    {missingOpen ? <PluginMissingDialog required={missing.required} optional={missing.optional} unresolved={missing.unresolved}
      disabled={locked} t={t} onClose={() => setMissingOpen(false)} onInstall={items => { setMissingOpen(false); void batch.execute('install', items) }} /> : null}
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
        await load()
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
      primaryAction={<PluginGetButton t={t} disabled={locked || !detailsInstallable || operations.some(item => item.id === detailsItem.id && item.active)}
        onClick={() => { setDetailsStore(null); void start(detailsItem.id, false) }} />} /> : null}
    {settingsPack ? <PluginPackSettingsDialog key={settingsPack.id} pack={settingsPack} initialPane={detailsPane}
      store={store.find(item => item.id === settingsPack.id)} update={updateOf(settingsPack.id)} operation={settingsOperation}
      disabled={locked || settingsOperation?.active === true} onClose={() => setSettingsPackId(null)}
      onEnabled={enabled => void mutation(`enable:${settingsPack.id}`, '/api/plugins/enable', { id: settingsPack.id, enabled })}
      onProvider={(provider, enabled) => void mutation(`provider:${provider.id}`, '/api/plugins/provider', {
        id: settingsPack.id, provider: provider.id, enabled,
      })}
      onUpdate={() => void start(settingsPack.id, true)}
      onReload={async () => { await api('POST', '/api/plugins/reload', { id: settingsPack.id }); await load() }}
      onNotes={() => {
        const update = updateOf(settingsPack.id)
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
  interface Window { wbPluginsPage?: { load(): Promise<void> } }
}

window.wbPluginsPage = { async load() { await currentLoad?.() } }
