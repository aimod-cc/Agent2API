import { useCallback, useEffect, useRef, useState } from 'react'
import { Button, Sheet, SheetBody, SheetContent, SheetFooter, SheetHeader, SheetTitle , Card, Collapsible, CollapsibleTrigger, CollapsibleContent } from '@ui'
import { PageIntro, ErrorNote, Empty, SectionHead } from '../components/ui'
import { PullIndicator, usePullToRefresh } from '../components/PullToRefresh'
import { pluginOperationNotice } from '../../../ui-shared/plugins'
import {
  checkPluginUpdates, getPlugins, pluginErrorMessage, refreshPluginCatalog, setPluginEnabled, setPluginProvider,
  startPluginInstall, uninstallPlugin, type PackSummary, type PluginMutation, type PluginsPayload, type StorePlugin,
} from '../api/plugins'
import { usePolling } from '../lib/usePolling'
import { serverIdentity } from '../lib/servers'
import { store, useStore } from '../state/store'
import { PluginCard, StorePluginCard } from './plugins/PluginCards'
import { PluginSettingsSheet } from './plugins/PluginSettingsSheet'
import { PluginPackSettingsSheet } from './plugins/PluginPackSettingsSheet'
import { PluginDetailsDialog, type PluginDetailsPane } from '../../../ui-shared/plugins-details'
import { t } from '../i18n'
import { ScriptPluginsPanel } from '../../../ui-shared/plugins-scripts'
import { apiCall, resolveUrl } from '../api/client'
import { PluginStartupDialog, subscribePluginDiagnostics, takePluginDiagnostics } from '../../../ui-shared/plugin-startup-view'
import { refreshPluginStartup } from '../../../ui-shared/plugin-startup'
import { openExternal } from '../lib/community'
import '../styles/checkin.css'
import '../../../ui-shared/plugins-batch.css'
import { applicablePluginItems, missingPluginItems, packSelectionKey, pluginBatchItems, type PluginBatchAction, type PluginBatchItem } from '../../../ui-shared/plugins-batch-items'
import { PluginBatchConfirm, PluginBatchProgress, PluginBatchToolbar } from '../../../ui-shared/plugins-batch-view'
import { usePluginBatch } from '../../../ui-shared/plugins-batch-runner'
import { PluginMissingBanner, PluginMissingDialog } from '../../../ui-shared/plugins-missing'
import { usePluginRequest } from './plugins/usePluginRequest'
import { clampMaxPackBytes, PluginFileInstallDialog, PluginFileInstallProgress, usePluginFileInstall } from '../../../ui-shared/plugins-file-install'

export function PluginsPage() {
  const { server, route } = useStore()
  const [startupId, setStartupId] = useState<string | null>(route.params?.id || null)
  useEffect(() => {
    const open = () => { const id = takePluginDiagnostics(); if (id) setStartupId(id) }
    open()
    return subscribePluginDiagnostics(open)
  }, [])
  const [payload, setPayload] = useState<PluginsPayload | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState('')
  const [settings, setSettings] = useState(false)
  const [packSettings, setPackSettings] = useState<{ id: string; server: string; pane: PluginDetailsPane } | null>(null)
  const [storeDetails, setStoreDetails] = useState<{ item: StorePlugin; server: string } | null>(null)
  const [remove, setRemove] = useState<PackSummary | null>(null)
  const locked = useRef(false)
  const signature = useRef('')
  const seenOperations = useRef(new Set<string>())
  const [selecting, setSelecting] = useState(false)
  const [selected, setSelected] = useState(new Set<string>())
  const [batchConfirm, setBatchConfirm] = useState<{ action: PluginBatchAction; items: PluginBatchItem[] } | null>(null)
  const [missingOpen, setMissingOpen] = useState(false)
  const batchRunning = useRef(false)
  const fileRunning = useRef(false)
  const identity = serverIdentity()
  const request = usePluginRequest(identity)

  const load = useCallback(async () => {
    try {
      const data = await getPlugins()
      if (serverIdentity() !== identity) return
      const next = JSON.stringify({
        packs: data.packs.map(pack => ({ id: pack.id, version: pack.version, enabled: pack.enabled, providers: pack.providers })),
        host: data.host,
        scripts: data.scripts?.packages.map(pack => ({
          id: pack.id, version: pack.version, enabled: pack.enabled, running: pack.running, providers: pack.providers, error: pack.error,
        })),
      })
      if (signature.current !== next) {
        signature.current = next
        void store.refresh(true)
      }
      setPayload(data)
      setError('')
      for (const operation of data.operations || []) {
        if (operation.active) continue
        const key = `${operation.id}:${operation.updatedAt}`
        if (seenOperations.current.has(key)) continue
        seenOperations.current.add(key)
        if (Date.now() - operation.updatedAt > 30_000) continue
        const notice = pluginOperationNotice(data, operation, t)
        if (notice) store.toast(notice.kind === 'err' ? pluginErrorMessage(notice.message) : notice.message, notice.kind)
      }
    } catch (cause) {
      if (serverIdentity() === identity) setError(pluginErrorMessage(cause))
    }
  }, [identity])

  usePolling(load, payload?.operations?.some(operation => operation.active) || payload?.scripts?.operation.active
    || payload?.scripts?.catalog?.refreshing ? 1500 : 10_000)
  const pull = usePullToRefresh({ onRefresh: load })

  async function run<T>(key: string, task: () => Promise<T>): Promise<T | undefined> {
    if (locked.current || batchRunning.current || fileRunning.current) return undefined
    locked.current = true
    setBusy(key)
    try {
      const result = await task()
      if (serverIdentity() !== identity) return undefined
      await load()
      return result
    } catch (cause) {
      if (serverIdentity() === identity) store.toast(pluginErrorMessage(cause), 'err')
      return undefined
    } finally { locked.current = false; if (serverIdentity() === identity) setBusy('') }
  }

  async function mutation(key: string, task: () => Promise<PluginMutation>): Promise<boolean> {
    const result = await run(key, async () => {
      const result = await task()
      if (serverIdentity() !== identity) return result
      await store.refresh(true)
      if (result.hostError) store.toast(result.hostError, 'err')
      else if (result.installNotice) store.toast(t(result.installNotice))
      return result
    })
    return result !== undefined
  }

  const refresh = async () => {
    const result = await run('catalog', refreshPluginCatalog)
    if (result?.error) store.toast(pluginErrorMessage(result.reason) || t('读取插件仓库失败'), 'err')
  }
  const check = async () => {
    const result = await run('check', checkPluginUpdates)
    if (result?.error) store.toast(pluginErrorMessage(result.reason) || t('检查更新失败'), 'err')
  }
  const install = async (id: string, update: boolean) => {
    if (!payload?.target) return
    const result = await run(`install:${id}`, () => startPluginInstall(id, payload.target, update))
    if (result) store.toast(t('下载任务已交给网关，关闭手机不影响安装'))
  }

  const batch = usePluginBatch({ request, scope: identity, refresh: load, target: payload?.target || '', t, disabled: Boolean(busy) || fileRunning.current,
    synchronize: async () => { if (serverIdentity() === identity) await store.refresh(true) },
    completed: items => setSelected(previous => { const next = new Set(previous); items.forEach(item => next.delete(item.key)); return next }),
  })
  const maxPackBytes = clampMaxPackBytes(payload?.maxPackBytes)
  const fileInstall = usePluginFileInstall({ request, t, scope: identity, maxPackBytes, previewAvailable: payload?.supportsPackPreview === true,
    disabled: Boolean(busy) || batch.running, refresh: load,
    accept: async result => {
      if (serverIdentity() !== identity) return
      setPayload(previous => previous ? { ...previous, ...result } : previous)
      await store.refresh(true)
      await load()
    },
    remember: operation => {
      if (serverIdentity() !== identity) return
      if (!operation.active) seenOperations.current.add(`${operation.id}:${operation.updatedAt}`)
      setPayload(previous => previous ? { ...previous, operations: [...(previous.operations || []).filter(item => item.id !== operation.id), operation] } : previous)
    },
  })
  batchRunning.current = batch.running
  fileRunning.current = fileInstall.running
  const pageLocked = Boolean(busy) || batch.running || fileInstall.running
  const batchItems = pluginBatchItems(payload, t, resolveUrl)
  const missing = missingPluginItems(payload, batchItems)
  const changeSelection = (keys: string[], checked: boolean) => setSelected(previous => { const next = new Set(previous); keys.forEach(key => checked ? next.add(key) : next.delete(key)); return next })
  const selectionFor = (key: string) => ({ checked: selected.has(key), disabled: pageLocked || batchItems.find(item => item.key === key)?.active === true,
    onChange: (checked: boolean) => changeSelection([key], checked) })
  const confirmBatch = (action: PluginBatchAction, items = batchItems.filter(item => selected.has(item.key))) => {
    const current = items.flatMap(item => { const row = item.required ? missing.required.find(row => row.key === item.key) || batchItems.find(row => row.key === item.key) : batchItems.find(row => row.key === item.key); return row ? [{ ...row, required: item.required }] : [] })
    const targets = applicablePluginItems(current, action)
    if (targets.length) setBatchConfirm({ action, items: targets })
  }
  useEffect(() => { setSelected(new Set()); setSelecting(false); setBatchConfirm(null); setMissingOpen(false) }, [identity])

  const packs = payload?.packs || []
  const settingsPack = packSettings?.server === identity ? packs.find(pack => pack.id === packSettings.id) : undefined
  const catalog = payload?.catalog?.plugins || []
  // 当前通道拿不到可安装版本的插件不进列表，避免正式版用户看到满屏「暂未发布」。
  const uninstalledAll = catalog.filter(item => !packs.some(pack => pack.id === item.id))
  const unpublished = uninstalledAll.filter(item => !item.version)
  const unpublishedKeys = new Set(unpublished.map(item => packSelectionKey(item.id)))
  const unpublishedCount = unpublished.length
  const uninstalled = uninstalledAll.filter(item => Boolean(item.version))
  const detailsItem = storeDetails?.server === identity && !packs.some(pack => pack.id === storeDetails.item.id)
    ? catalog.find(item => item.id === storeDetails.item.id) || storeDetails.item : null
  const detailsInstallable = Boolean(detailsItem?.version && !detailsItem.error
    && detailsItem.platforms?.some(platform => platform.target === 'wasm32-unknown-unknown'))
  return (
    <div className='m-page' ref={pull.bindRef}>
      <PullIndicator pull={pull.pull} refreshing={pull.refreshing} />
      <PageIntro subtitle={t('管理所连接网关的插件，不在手机上安装提供商插件')} />
      <div className='m-section' style={{ marginTop: 0 }}>
        <Card className='m-card m-card-pad'>
          <div className='m-ck-title'>{server?.name || server?.base || t('当前网关')}</div>
          <p className='m-ck-note'>{t('安装、更新与启停均作用在这台网关上。插件平台由网关决定，与手机系统无关。')}</p>
          {payload ? <p className='m-ck-note m-mono'>{payload.target}</p> : null}
          <div className='m-ck-actions'>
            <Button size='sm' variant='outline' disabled={pageLocked} onClick={() => void refresh()}>{busy === 'catalog' ? t('读取中…') : t('刷新插件仓库')}</Button>
            <Button size='sm' variant='outline' disabled={pageLocked} onClick={() => void check()}>{busy === 'check' ? t('检查中…') : t('检查更新')}</Button>
            <Button size='sm' variant='outline' disabled={pageLocked} onClick={() => { setSelecting(value => !value); setSelected(new Set()) }}>{selecting ? t('退出批量管理') : t('批量管理')}</Button>
            <Button size='sm' disabled={pageLocked} onClick={fileInstall.show}>{t('本地安装')}</Button>
            <Button size='sm' variant='outline' disabled={pageLocked} onClick={() => setMissingOpen(true)}>{t('补全缺失插件')}</Button>
            <Button size='sm' variant='ghost' onClick={() => setSettings(true)}>{t('插件设置')}</Button>
          </div>
        </Card>
      </div>
      {selecting ? <div className='m-section'><PluginBatchToolbar items={batchItems} visibleKeys={batchItems.filter(item => !item.active && !unpublishedKeys.has(item.key)).map(item => item.key)} selected={selected}
        disabled={pageLocked} t={t} onSelection={changeSelection} onAction={confirmBatch} onExit={() => { setSelecting(false); setSelected(new Set()) }} /></div> : null}
      {missing.required.length || missing.unresolved.length ? <div className='m-section'><PluginMissingBanner count={missing.required.length + missing.unresolved.length}
        accounts={missing.required.reduce((sum, item) => sum + item.accounts, 0) + missing.unresolved.reduce((sum, item) => sum + item.count, 0)} onOpen={() => setMissingOpen(true)} t={t} /></div> : null}
      {batch.task ? <div className='m-section'><PluginBatchProgress task={batch.task} t={t} onClose={batch.clear} onRetry={items => confirmBatch(batch.task!.action, items)} /></div> : null}
      {fileInstall.backgroundVisible ? <div className='m-section'><PluginFileInstallProgress model={fileInstall} t={t} /></div> : null}
      {error ? <ErrorNote text={error} /> : null}
      {payload?.host?.lastError ? <ErrorNote text={payload.host.lastError} /> : null}
      {uninstalled.length || unpublishedCount > 0 ? <div className='m-section'><SectionHead title={t('可安装插件')} />
        {uninstalled.map(item => <StorePluginCard key={item.id} item={item} target={payload?.target || ''}
          operation={payload?.operations?.find(operation => operation.id === item.id)} busy={pageLocked} onInstall={() => void install(item.id, false)}
          selection={selecting ? selectionFor(packSelectionKey(item.id)) : undefined}
          onDetails={() => setStoreDetails({ item, server: identity })} />)}
        {!uninstalled.length ? <Card className='m-card'><Empty text={t('插件发布后会显示在这里。')} /></Card> : null}
      </div> : null}
      <div className='m-section'>
        <SectionHead title={t('已安装插件')} />
        {packs.map(pack => <PluginCard key={pack.id} pack={pack} busy={pageLocked} target={payload?.target || ''}
          selection={selecting ? selectionFor(packSelectionKey(pack.id)) : undefined}
          update={payload?.updates?.find(update => update.id === pack.id)}
          operation={payload?.operations?.find(operation => operation.id === pack.id)}
          onEnabled={enabled => void mutation(`enable:${pack.id}`, () => setPluginEnabled(pack.id, enabled))}
          onProvider={(provider, enabled) => void mutation(`provider:${provider}`, () => setPluginProvider(pack.id, provider, enabled))}
          onSettings={() => setPackSettings({ id: pack.id, server: identity, pane: 'settings' })}
          onDetails={() => setPackSettings({ id: pack.id, server: identity, pane: 'about' })}
          onUpdate={() => void install(pack.id, true)} onRemove={() => setRemove(pack)} />)}
        {!packs.length ? <Card className='m-card'><Empty text={payload ? t('这台网关还没有安装插件') : t('正在读取插件列表…')} /></Card> : null}
      </div>
      {payload?.scripts ? <div className='m-section'><Card className='m-card m-card-pad'>
        <ScriptPluginsPanel key={identity} state={payload.scripts} t={t} disabled={pageLocked} resolveIcon={resolveUrl} onOpenExternal={openExternal}
          selection={selecting ? selectionFor : undefined} onSelectGroup={() => { const keys = batchItems.filter(item => item.kind === 'script' && !item.active).map(item => item.key); changeSelection(keys, !keys.every(key => selected.has(key))) }}
          request={(path, body) => apiCall(path, { method: 'POST', body })} refresh={load} />
      </Card></div> : null}
      {payload?.catalog?.reason ? <div className='m-section'><p className='m-ck-note'>{pluginErrorMessage(payload.catalog.reason)}</p></div> : null}
      {payload ? <div className='m-section'><Collapsible className='m-card m-card-pad'>
        <CollapsibleTrigger className='m-ck-title'>{t('诊断信息')}</CollapsibleTrigger><CollapsibleContent keepMounted>
        <p className='m-ck-note'>{t('插件目录')}：<span className='m-mono'>{payload.pluginsDir}</span></p>
        <p className='m-ck-note'>{t('协议版本')}：{payload.protocol}</p>
      </CollapsibleContent></Collapsible></div> : null}
      {startupId ? <PluginStartupDialog id={startupId}
        name={packs.find(pack => pack.id === startupId)?.name || payload?.scripts?.packages.find(pack => pack.id === startupId)?.name || startupId}
        status={packs.find(pack => pack.id === startupId)?.startup || payload?.scripts?.packages.find(pack => pack.id === startupId)?.startup}
        script={startupId.startsWith('script-')} t={t} onClose={() => setStartupId(null)}
        retry={async () => {
          if (startupId.startsWith('script-')) await apiCall('/api/plugins/scripts/change', { method: 'POST', body: { id: startupId, action: 'reload' } })
          else await apiCall('/api/plugins/reload', { method: 'POST', body: { id: startupId } })
          refreshPluginStartup()
          await load()
        }} /> : null}
      <PluginFileInstallDialog model={fileInstall} maxPackBytes={maxPackBytes} t={t} />
      {batchConfirm ? <PluginBatchConfirm key={`${identity}:${batchConfirm.action}:${batchConfirm.items.map(item => item.key).join('|')}`}
        action={batchConfirm.action} items={batchConfirm.items} disabled={pageLocked} t={t} onClose={() => setBatchConfirm(null)}
        onConfirm={() => { const { action, items } = batchConfirm; setBatchConfirm(null); void batch.execute(action, items) }} /> : null}
      {missingOpen ? <PluginMissingDialog key={identity} required={missing.required} unresolved={missing.unresolved} disabled={pageLocked} t={t}
        onClose={() => setMissingOpen(false)} onInstall={items => { setMissingOpen(false); void batch.execute('install', items) }} /> : null}
      {settings ? <PluginSettingsSheet onClose={() => setSettings(false)} onChanged={load} /> : null}
      {detailsItem ? <PluginDetailsDialog name={t(detailsItem.name || detailsItem.id)} description={t(detailsItem.description)}
        version={detailsItem.version} sourceLabel={t('社区插件')} icon={detailsItem.icon} artwork='providers'
        t={t} onClose={() => setStoreDetails(null)}
        metadata={detailsItem.error ? <div className='text-sm text-warning'>{pluginErrorMessage(detailsItem.reason)}</div>
          : detailsItem.version && !detailsInstallable ? <p className='m-0 text-sm text-warning'>{t('当前网关平台暂无插件包')}</p> : null}
        primaryAction={<Button size='sm' disabled={Boolean(busy) || !detailsInstallable
          || payload?.operations?.some(item => item.id === detailsItem.id && item.active)}
          onClick={() => { setStoreDetails(null); void install(detailsItem.id, false) }}>{t('在网关下载并安装')}</Button>} /> : null}
      {settingsPack ? <PluginPackSettingsSheet key={`${identity}:${settingsPack.id}`} pack={settingsPack} initialPane={packSettings?.pane}
        disabled={Boolean(busy) || payload?.operations?.some(operation => operation.id === settingsPack.id && operation.active) === true}
        onClose={() => setPackSettings(null)}
        onReload={async () => { await apiCall('/api/plugins/reload', { method: 'POST', body: { id: settingsPack.id } }); await load() }}
        onEnabled={enabled => void mutation(`enable:${settingsPack.id}`, () => setPluginEnabled(settingsPack.id, enabled))}
        onProvider={(provider, enabled) => void mutation(`provider:${provider}`, () => setPluginProvider(settingsPack.id, provider, enabled))} /> : null}
      <Sheet open={Boolean(remove)} onOpenChange={(open, details) => {
        if (open) return
        if (busy) { details.cancel(); return }
        setRemove(null)
      }}>
        <SheetContent showCloseButton={!busy}>
          <SheetHeader><SheetTitle>{t('卸载插件包')}</SheetTitle></SheetHeader>
          <SheetBody><p className='m-ck-note'>{t('确定卸载「{name}」吗？对应提供商将停止转发，账号数据仍然保留，重新安装后恢复。', { name: remove?.name || '' })}</p></SheetBody>
          <SheetFooter>
            <Button variant='outline' disabled={Boolean(busy)} onClick={() => setRemove(null)}>{t('取消')}</Button>
            <Button variant='destructive' disabled={Boolean(busy)} onClick={async () => {
              if (remove && await mutation(`remove:${remove.id}`, () => uninstallPlugin(remove.id))) setRemove(null)
            }}>{busy ? t('卸载中…') : t('卸载')}</Button>
          </SheetFooter>
        </SheetContent>
      </Sheet>
    </div>
  )
}
