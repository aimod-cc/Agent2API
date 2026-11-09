import { useCallback, useEffect, useRef, useState } from 'react'
import { Button, Sheet, SheetBody, SheetContent, SheetFooter, SheetHeader, SheetTitle , Card, Collapsible, CollapsibleTrigger, CollapsibleContent } from '@ui'
import { PageIntro, ErrorNote, Empty, SectionHead } from '../components/ui'
import { PullIndicator, usePullToRefresh } from '../components/PullToRefresh'
import {
  checkPluginUpdates, getPlugins, pluginErrorMessage, refreshPluginCatalog, setPluginEnabled, setPluginProvider,
  startPluginInstall, uninstallPlugin, type PackSummary, type PluginMutation, type PluginsPayload,
} from '../api/plugins'
import { usePolling } from '../lib/usePolling'
import { serverIdentity } from '../lib/servers'
import { store, useStore } from '../state/store'
import { PluginCard, StorePluginCard } from './plugins/PluginCards'
import { PluginSettingsSheet } from './plugins/PluginSettingsSheet'
import { PluginPackSettingsSheet } from './plugins/PluginPackSettingsSheet'
import { t } from '../i18n'
import { GithubFailureText } from '../../../ui-shared/github-failure'
import { ScriptPluginsPanel } from '../../../ui-shared/plugins-scripts'
import { apiCall, resolveUrl } from '../api/client'
import { PluginTasks } from '../../../ui-shared/plugin-tasks'
import { PluginStartupDialog, subscribePluginDiagnostics, takePluginDiagnostics } from '../../../ui-shared/plugin-startup-view'
import { refreshPluginStartup } from '../../../ui-shared/plugin-startup'
import { openExternal } from '../lib/community'
import '../styles/checkin.css'

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
  const [packSettings, setPackSettings] = useState<{ id: string; server: string } | null>(null)
  const [remove, setRemove] = useState<PackSummary | null>(null)
  const locked = useRef(false)
  const signature = useRef('')
  const seenOperations = useRef(new Set<string>())
  const identity = serverIdentity()

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
        if (operation.error || operation.hostError) store.toast(pluginErrorMessage(operation.error || operation.hostError), 'err')
        else if (operation.done) store.toast(t('插件包已安装到 v{version}', { version: operation.version }))
      }
    } catch (cause) {
      if (serverIdentity() === identity) setError(pluginErrorMessage(cause))
    }
  }, [identity])

  usePolling(load, payload?.operations?.some(operation => operation.active) || payload?.scripts?.operation.active
    || payload?.scripts?.catalog?.refreshing ? 1500 : 10_000)
  const pull = usePullToRefresh({ onRefresh: load })

  async function run<T>(key: string, task: () => Promise<T>): Promise<T | undefined> {
    if (locked.current) return undefined
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

  const packs = payload?.packs || []
  const settingsPack = packSettings?.server === identity ? packs.find(pack => pack.id === packSettings.id) : undefined
  const catalog = payload?.catalog?.plugins || []
  const uninstalled = catalog.filter(item => !packs.some(pack => pack.id === item.id))
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
            <Button size='sm' variant='outline' disabled={Boolean(busy)} onClick={() => void refresh()}>{busy === 'catalog' ? t('读取中…') : t('刷新插件仓库')}</Button>
            <Button size='sm' variant='outline' disabled={Boolean(busy)} onClick={() => void check()}>{busy === 'check' ? t('检查中…') : t('检查更新')}</Button>
            <Button size='sm' variant='ghost' onClick={() => setSettings(true)}>{t('插件设置')}</Button>
          </div>
        </Card>
      </div>
      <div className='m-section'><PluginTasks key={identity} t={t}
        request={(method, path, body) => apiCall(path, { method, body })} /></div>
      {error ? <ErrorNote text={error} /> : null}
      {payload?.host?.lastError ? <ErrorNote text={payload.host.lastError} /> : null}
      {uninstalled.length ? <div className='m-section'><SectionHead title={t('可安装插件')} />
        {uninstalled.map(item => <StorePluginCard key={item.id} item={item} target={payload?.target || ''}
          operation={payload?.operations?.find(operation => operation.id === item.id)} busy={Boolean(busy)} onInstall={() => void install(item.id, false)} />)}
      </div> : null}
      <div className='m-section'>
        <SectionHead title={t('已安装插件')} />
        {packs.map(pack => <PluginCard key={pack.id} pack={pack} busy={Boolean(busy)} target={payload?.target || ''}
          update={payload?.updates?.find(update => update.id === pack.id)}
          operation={payload?.operations?.find(operation => operation.id === pack.id)}
          onEnabled={enabled => void mutation(`enable:${pack.id}`, () => setPluginEnabled(pack.id, enabled))}
          onProvider={(provider, enabled) => void mutation(`provider:${provider}`, () => setPluginProvider(pack.id, provider, enabled))}
          onSettings={() => setPackSettings({ id: pack.id, server: identity })}
          onUpdate={() => void install(pack.id, true)} onRemove={() => setRemove(pack)} />)}
        {!packs.length ? <Card className='m-card'><Empty text={payload ? t('这台网关还没有安装插件') : t('正在读取插件列表…')} /></Card> : null}
      </div>
      {payload?.scripts ? <div className='m-section'><Card className='m-card m-card-pad'>
        <ScriptPluginsPanel key={identity} state={payload.scripts} t={t} disabled={Boolean(busy)} resolveIcon={resolveUrl} onOpenExternal={openExternal}
          request={(path, body) => apiCall(path, { method: 'POST', body })} refresh={load} />
      </Card></div> : null}
      {payload?.catalog?.reason ? <div className='m-section'><p className='m-ck-note'><GithubFailureText value={payload.catalog} t={t} /></p></div> : null}
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
      {settings ? <PluginSettingsSheet onClose={() => setSettings(false)} onChanged={load} /> : null}
      {settingsPack ? <PluginPackSettingsSheet key={`${identity}:${settingsPack.id}`} pack={settingsPack}
        disabled={Boolean(busy) || payload?.operations?.some(operation => operation.id === settingsPack.id && operation.active) === true}
        onClose={() => setPackSettings(null)}
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
