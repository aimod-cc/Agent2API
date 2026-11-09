import * as React from 'react'
import { mountOnPage } from '../island-lazy'
import { createRoot } from 'react-dom/client'
import {
  Alert, Button, Card, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader,
  DialogTitle, Empty, EmptyDescription, EmptyTitle, Input, InputGroup, InputGroupAddon,
  InputGroupInput, PageDescription, PageHeader, PageTitle, Progress, SegmentedControl,
} from '@ui'
import { InstalledPluginCard, StorePluginCard } from './plugins-cards'
import { PluginNotesDialog, type PluginNotes } from './plugins-notes'
import { PluginPackSettingsDialog } from './plugins-pack-settings'
import { PluginSettingsDialog } from './plugins-settings'
import { api, bridge, errorMessage, pluginDescription, pluginName, readBase64, toast } from './plugins-shared'
import type { PackSummary, PluginMutation, PluginOperation, PluginUpdate, PluginsPayload, ReleaseState, ScriptPackage } from './plugins-types'
import { t } from '../i18n'
import { ScriptPluginsPanel } from '../../../ui-shared/plugins-scripts'
import { matchesPluginStatus, matchesScriptPublisher, type PluginFilterItem, type PluginStatusFilter, type ScriptPublisherFilter } from '../../../ui-shared/plugins-filters'
import { isRecommendationInstalled, matchesRecommendedPlugin, matchesScriptPlugin, reinstallRequest, scriptPluginName, scriptRecommendations, scriptUpdate } from '../../../ui-shared/plugins-script-display'
import { PluginSectionHeading } from '../../../ui-shared/plugins-store'
import { PluginTasks } from '../../../ui-shared/plugin-tasks'
import { PluginDiagnostics, PluginPageTools } from './plugins-page-tools'
import { useGatewayBase } from './port-panel'
import { PluginStartupDialog, subscribePluginDiagnostics, takePluginDiagnostics } from '../../../ui-shared/plugin-startup-view'
import { refreshPluginStartup } from '../../../ui-shared/plugin-startup'
import { openExternal } from './update-shared'

const MIB = 1024 * 1024
const LEGACY_MAX_PACK_BYTES = 24 * MIB
const HARD_MAX_PACK_BYTES = 96 * MIB

type SourceFilter = 'all' | 'agent2' | 'magpie'

/** 一组带小标题的分段筛选。 */
function FilterGroup<T extends string>({ label, options, value, onChange }: {
  label: string
  options: ReadonlyArray<{ value: T; label: string; count: number }>
  value: T
  onChange: (value: T) => void
}) {
  return <div className='flex flex-wrap items-center gap-2.5'>
    <span className='text-[11.5px] text-muted-foreground'>{label}</span>
    <SegmentedControl options={options} value={value} onValueChange={onChange} aria-label={label} />
  </div>
}

function clampMaxPackBytes(value: unknown): number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value > 0
    ? Math.min(value, HARD_MAX_PACK_BYTES) : LEGACY_MAX_PACK_BYTES
}

function packSizeLimitMessage(maxPackBytes: number): string {
  return t('插件包大小不能超过 {n} MiB', { n: maxPackBytes / MIB })
}

async function readPackBase64(file: File, maxPackBytes: number): Promise<string> {
  if (file.size > maxPackBytes) throw new Error(packSizeLimitMessage(maxPackBytes))
  const packB64 = await readBase64(file)
  // FileReader 输出 ASCII base64；仅上传 packB64，JSON 开销远小于服务端预留的 1 MiB。
  if (packB64.length > Math.ceil(maxPackBytes / 3) * 4) {
    throw new Error(packSizeLimitMessage(maxPackBytes))
  }
  return packB64
}

function PluginsPage(): React.ReactElement {
  const gatewayBase = useGatewayBase()
  const [payload, setPayload] = React.useState<PluginsPayload | null>(null)
  const [maxPackBytes, setMaxPackBytes] = React.useState(LEGACY_MAX_PACK_BYTES)
  const [error, setError] = React.useState('')
  const [syncError, setSyncError] = React.useState('')
  const [busy, setBusy] = React.useState('')
  const [settingsOpen, setSettingsOpen] = React.useState(false)
  const [settingsPackId, setSettingsPackId] = React.useState<string | null>(null)
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
  const [bulk, setBulk] = React.useState<{ index: number; total: number } | null>(null)
  const fileRef = React.useRef<HTMLInputElement>(null)
  const listRef = React.useRef<HTMLDivElement>(null)
  const seenOperations = React.useRef(new Set<string>())
  const previousSignature = React.useRef('')
  const mounted = React.useRef(true)
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
    if (busy || scriptBusy) return
    setBusy(key)
    try { await action() }
    catch (error) { toast(error instanceof Error ? error.message : String(error), 'err') }
    finally { if (mounted.current) setBusy('') }
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

  async function updateAll(ids: string[], scriptPacks: ScriptPackage[] = []): Promise<void> {
    if (busy || scriptBusy || (!ids.length && !scriptPacks.length)) return
    setBusy('updateAll')
    const total = ids.length + scriptPacks.length
    setBulk({ index: 0, total })
    const failures: string[] = []
    try {
      for (const [index, id] of ids.entries()) {
        if (!mounted.current) return
        setBulk({ index: index + 1, total })
        try {
          const result = await api<{ operation: PluginOperation }>('POST', '/api/plugins/update',
            { id, target: payload?.target })
          let operation = result.operation
          const remember = () => setPayload(previous => previous ? {
            ...previous, operations: [...(previous.operations || []).filter(item => item.id !== id), operation],
          } : previous)
          remember()
          // 安装接口只启动后台任务，必须等任务结束后再计为完成。
          while (operation.active && mounted.current) {
            await new Promise(resolve => window.setTimeout(resolve, 1000))
            if (!mounted.current) return
            const progress = await api<{ operations: PluginOperation[] }>('GET', '/api/plugins/progress')
            const next = progress.operations.find(item => item.id === id)
            if (!next) throw new Error(t('无法读取插件更新进度，请刷新后查看结果。'))
            operation = next
            remember()
          }
          if (!mounted.current) return
          if (!operation.done || operation.error || operation.hostError) {
            throw new Error(operation.error || operation.hostError || t('插件更新未完成'))
          }
        } catch (cause) {
          failures.push(`${pluginName(id, payload?.packs.find(pack => pack.id === id)?.name)}：${errorMessage(cause instanceof Error ? cause.message : cause)}`)
        }
        await load()
      }
      // ── Magpie 侧的脚本插件 ──────────────────────────────
      // 与本体插件不同：脚本宿主同一时间只允许一个任务（后端会拒第二个），所以
      // 这里一个个提交、等上一个结束再下一个；`trusted` 直接给真 —— 更新的是**同一
      // 来源、同一个包**（后端还会校验包 id 不许变），用户已经为它确认过一次信任。
      for (const [offset, pack] of scriptPacks.entries()) {
        if (!mounted.current) return
        setBulk({ index: ids.length + offset + 1, total })
        try {
          await api('POST', '/api/plugins/scripts/install', { ...reinstallRequest(pack), trusted: true })
          while (mounted.current) {
            await new Promise(resolve => window.setTimeout(resolve, 1000))
            if (!mounted.current) return
            const progress = await api<PluginsPayload>('GET', '/api/plugins')
            const operation = progress.scripts?.operation
            if (operation?.active === true) continue
            if (operation?.error) throw new Error(operation.error)
            break
          }
        } catch (cause) {
          failures.push(`${scriptPluginName(pack, recommendations, t)}：${errorMessage(cause instanceof Error ? cause.message : cause)}`)
        }
        await load()
      }
      const done = total - failures.length
      if (failures.length) {
        toast(t('已更新 {done} 个插件，{failed} 个失败：{error}', { done, failed: failures.length, error: failures.join('；') }), 'err')
      } else {
        toast(t('已更新 {count} 个插件', { count: done }), 'ok')
      }
    } finally {
      if (mounted.current) { setBusy(''); setBulk(null); await load() }
    }
  }

  function pickFile(): void {
    if (fileRef.current) { fileRef.current.value = ''; fileRef.current.click() }
  }

  async function onFile(event: React.ChangeEvent<HTMLInputElement>): Promise<void> {
    const file = event.currentTarget.files?.[0]
    if (!file) return
    if (file.size > maxPackBytes) { toast(packSizeLimitMessage(maxPackBytes), 'err'); return }
    await run('file', async () => {
      const packB64 = await readPackBase64(file, maxPackBytes)
      await accept(await api<PluginMutation>('POST', '/api/plugins/install', { packB64 }))
      toast(t('插件包已安装'), 'ok')
    })
  }

  const packs = payload?.packs || []
  const host = payload?.host
  const catalog = payload?.catalog
  const store = catalog?.plugins || []
  const installedIds = new Set(packs.map(pack => pack.id))
  const storePlugins = store.filter(item => !installedIds.has(item.id))
  const operations = payload?.operations || []
  const active = operations.some(operation => operation.active)
  const target = payload?.target || ''
  const locked = Boolean(busy) || scriptBusy
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
  const bulkIds = showNative ? installedRows.filter(pack => {
    const update = updateOf(pack.id)
    return update?.available && !update.error && !update.repairing && !operations.some(operation => operation.id === pack.id && operation.active)
  }).map(pack => pack.id) : []
  // Magpie 侧的更新同一批走：脚本插件一次只能有一个任务，所以分开逐个提交（见 updateAll）。
  const bulkScripts = showScripts && scripts?.operation.active !== true
    ? scriptRows.filter(pack => scriptUpdate(scripts, pack.id) !== undefined) : []
  const pendingUpdates = bulkIds.length + bulkScripts.length

  React.useEffect(() => {
    if (listRef.current) listRef.current.scrollTop = 0
  }, [source, status, scriptPublisher, search])

  React.useEffect(() => {
    if (settingsPackId && payload && !settingsPack) setSettingsPackId(null)
  }, [payload, settingsPack, settingsPackId])

  return <>
    <div className='mx-auto flex min-h-0 w-full max-w-[1180px] flex-1 flex-col gap-6 overflow-hidden text-foreground'>
      <div className='flex shrink-0 flex-col gap-6'>
      <PageHeader className='flex-wrap items-end gap-x-6 gap-y-3'>
        <div className='min-w-0 flex-1'>
          <PageTitle className='text-[26px] font-bold tracking-[-0.022em]'>{t('插件')}</PageTitle>
          <PageDescription className='mt-1.5 max-w-[52em] leading-relaxed'>
            {t('按需安装插件，为 AI 路由网关扩展提供商能力。插件独立于主程序更新，下载完成后自动校验并安装。')}
          </PageDescription>
        </div>
        <div className='ml-auto flex items-center gap-2'>
          <InputGroup className='w-[220px] rounded-pill border-hairline bg-surface-inset'>
            <InputGroupInput type='search' value={search} placeholder={t('搜索插件')} aria-label={t('搜索插件')}
              onChange={event => setSearch(event.currentTarget.value)} />
            <InputGroupAddon>
              <svg width='13' height='13' viewBox='0 0 16 16' fill='none' stroke='currentColor' strokeWidth='1.7'
                strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
                <circle cx='6.8' cy='6.8' r='4.6' /><path d='M10.3 10.3 14 14' />
              </svg>
            </InputGroupAddon>
          </InputGroup>
          <PluginPageTools disabled={locked} fileDisabled={locked || active} scriptDisabled={locked || scripts?.operation.active === true}
            scriptsAvailable={Boolean(scripts)} checking={busy === 'check' || catalog?.checking === true}
            onRefresh={() => void refreshStore()} onCheck={() => void check()} onSettings={() => setSettingsOpen(true)}
            onFile={pickFile} onScript={() => setScriptInstallRequest(value => value + 1)} onDiagnostics={() => setDiagnosticsOpen(true)} />
        </div>
      </PageHeader>
      <PluginTasks request={api} t={t} />
      <Input ref={fileRef} type='file' accept='.zip,application/zip' className='hidden' onChange={event => void onFile(event)} />
      {error || syncError ? <Alert variant='destructive' className='break-words'>{error || syncError}</Alert> : null}
      {busy === 'file' ? <div className='flex flex-col gap-2' aria-live='polite'>
        <span className='text-[12px] text-subtle'>{t('正在读取、校验并安装插件包…')}</span><Progress value={null} />
      </div> : null}
      <div className='flex flex-wrap items-center gap-x-7 gap-y-3'>
        <FilterGroup label={t('来源')} value={source} onChange={setSource} options={[
          { value: 'all', label: t('全部'), count: countMatching('all', status) },
          { value: 'agent2', label: t('Agent2API 社区'), count: countMatching('agent2', status) },
          { value: 'magpie', label: t('Magpie 插件'), count: countMatching('magpie', status) },
        ]} />
        <FilterGroup label={t('状态')} value={status} onChange={setStatus} options={[
          { value: 'all', label: t('全部'), count: countMatching(source, 'all') },
          { value: 'updatable', label: t('可更新'), count: countMatching(source, 'updatable') },
          { value: 'installed', label: t('已安装'), count: countMatching(source, 'installed') },
          { value: 'missing', label: t('未安装'), count: countMatching(source, 'missing') },
        ]} />
        {bulk ? <span className='ml-auto text-[12px] text-subtle' aria-live='polite'>
          {t('正在更新第 {index}／{total} 个…', { index: bulk.index, total: bulk.total })}
        </span> : pendingUpdates ? <div className='ml-auto flex flex-wrap items-center gap-2.5'>
          <span className='text-[12px] text-primary-fg'>{t('有 {count} 个插件可更新', { count: pendingUpdates })}</span>
          <Button size='sm' disabled={locked || active || scripts?.operation.active === true} onClick={() => void updateAll(bulkIds, bulkScripts)}>{t('全部更新')}</Button>
        </div> : null}
      </div>
      </div>
      <div ref={listRef} className='flex min-h-0 flex-1 flex-col gap-6 overflow-y-auto overscroll-contain pb-5 pr-2 [overflow-anchor:none] [scrollbar-gutter:stable]'>
      {showNative && (installedRows.length > 0 || storeRows.length > 0) ? <section className='shrink-0' aria-label={t('Agent2API 社区')}>
        <PluginSectionHeading t={t} title={t('Agent2API 社区')} count={installedRows.length + storeRows.length}
          hint={t('原生插件包，来自项目插件仓库')} />
        <div className='grid grid-cols-1 gap-3.5 min-[900px]:grid-cols-2'>
          {installedRows.map(pack => {
            const operation = operations.find(item => item.id === pack.id)
            return <InstalledPluginCard key={pack.id} pack={pack} store={store.find(item => item.id === pack.id)}
              host={host || null} update={updateOf(pack.id)} operation={operation} disabled={locked || operation?.active === true}
              onEnabled={enabled => void mutation(`enable:${pack.id}`, '/api/plugins/enable', { id: pack.id, enabled })}
              onSettings={() => setSettingsPackId(pack.id)} />
          })}
          {storeRows.map(item => <StorePluginCard key={item.id} item={item} target={target}
            operation={operations.find(operation => operation.id === item.id)} disabled={locked}
            onInstall={() => void start(item.id, false)}
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
      {scripts ? <div className='shrink-0'><ScriptPluginsPanel state={scripts} t={t} disabled={Boolean(busy)} query={search}
        resolveIcon={path => `${gatewayBase.replace(/\/+$/, '')}${path}`} onOpenExternal={openExternal}
        galleryVisible={showScripts} showInstallAction={false} installRequest={scriptInstallRequest} onBusyChange={setScriptBusy}
        status={status} publisher={scriptPublisher} onPublisherChange={setScriptPublisher} onClearFilters={clearFilters}
        request={(path, body) => api('POST', path, body)} refresh={load} /></div> : null}
      <p className='m-0 shrink-0 text-[11.5px] leading-relaxed text-muted-foreground'>
        {t('插件独立安装与更新。卸载后账号数据仍会保留；脚本插件共用一个按需下载的运行时。')}
      </p>
      </div>
    </div>
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
    {settingsPack ? <PluginPackSettingsDialog key={settingsPack.id} pack={settingsPack}
      store={store.find(item => item.id === settingsPack.id)} update={updateOf(settingsPack.id)} operation={settingsOperation}
      disabled={locked || settingsOperation?.active === true} onClose={() => setSettingsPackId(null)}
      onEnabled={enabled => void mutation(`enable:${settingsPack.id}`, '/api/plugins/enable', { id: settingsPack.id, enabled })}
      onProvider={(provider, enabled) => void mutation(`provider:${provider.id}`, '/api/plugins/provider', {
        id: settingsPack.id, provider: provider.id, enabled,
      })}
      onUpdate={() => void start(settingsPack.id, true)}
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
