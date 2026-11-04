import * as React from 'react'
import { mountOnPage } from '../island-lazy'
import { createRoot } from 'react-dom/client'
import {
  Badge, Button, Card, CardFooter, CardHeader, Collapsible, CollapsibleContent,
  CollapsibleTrigger, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader,
  DialogTitle, Input, Progress,
} from '@ui'
import { InstalledPluginCard, StorePluginCard } from './plugins-cards'
import { PluginNotesDialog, type PluginNotes } from './plugins-notes'
import { PluginPackSettingsDialog } from './plugins-pack-settings'
import { PluginSettingsDialog } from './plugins-settings'
import { api, bridge, errorMessage, pluginName, readBase64, toast } from './plugins-shared'
import type { PackSummary, PluginMutation, PluginOperation, PluginUpdate, PluginsPayload, ReleaseState } from './plugins-types'
import { t } from '../i18n'

const MIB = 1024 * 1024
const LEGACY_MAX_PACK_BYTES = 24 * MIB
const HARD_MAX_PACK_BYTES = 96 * MIB

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
  const [payload, setPayload] = React.useState<PluginsPayload | null>(null)
  const [maxPackBytes, setMaxPackBytes] = React.useState(LEGACY_MAX_PACK_BYTES)
  const [error, setError] = React.useState('')
  const [syncError, setSyncError] = React.useState('')
  const [busy, setBusy] = React.useState('')
  const [settingsOpen, setSettingsOpen] = React.useState(false)
  const [settingsPackId, setSettingsPackId] = React.useState<string | null>(null)
  const [confirmRemove, setConfirmRemove] = React.useState<PackSummary | null>(null)
  const [notes, setNotes] = React.useState<PluginNotes | null>(null)
  const fileRef = React.useRef<HTMLInputElement>(null)
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
    if (busy) return
    setBusy(key)
    try { await action() }
    catch (error) { toast(error instanceof Error ? error.message : String(error), 'err') }
    finally { if (mounted.current) setBusy('') }
  }

  async function check(): Promise<void> {
    await run('check', async () => {
      const result = await api<{ updates: PluginUpdate[]; error: boolean; reason?: string }>('GET', '/api/plugins/update')
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
  const locked = Boolean(busy)
  const settingsPack = packs.find(pack => pack.id === settingsPackId)
  const settingsOperation = operations.find(operation => operation.id === settingsPackId)

  React.useEffect(() => {
    if (settingsPackId && payload && !settingsPack) setSettingsPackId(null)
  }, [payload, settingsPack, settingsPackId])

  return <>
    <Card className='panel'>
      <CardHeader className='panel-head'>
        <div>
          <h2>{t('插件')}</h2>
          <p className='muted'>{t('按需安装插件，为 AI 路由网关扩展提供商能力。插件独立于主程序更新，下载完成后自动校验并安装。')}</p>
        </div>
        <div className='flex flex-wrap items-center justify-end gap-2'>
          <Badge variant={host?.running ? 'success' : 'outline'}>
            {host?.running ? t('运行中') : packs.length ? t('未运行') : t('未安装')}
          </Badge>
          <Button variant='outline' size='sm' disabled={locked} onClick={() => void refreshStore()}>
            {busy === 'refresh' ? t('刷新中…') : t('刷新')}
          </Button>
          <Button variant='outline' size='sm' disabled={locked} onClick={() => void check()}>
            {busy === 'check' || catalog?.checking ? t('检查中…') : t('检查更新')}
          </Button>
          <Button variant='outline' size='sm' onClick={() => setSettingsOpen(true)}>{t('插件设置')}</Button>
          <Button variant='outline' size='sm' disabled={locked || active} onClick={pickFile}>{t('从文件安装…')}</Button>
        </div>
      </CardHeader>
      <div className='p-4'>
        <Input ref={fileRef} type='file' accept='.zip,application/zip' className='hidden' onChange={event => void onFile(event)} />
        {error || syncError ? <p className='mb-3 text-[12px] text-destructive'>{error || syncError}</p> : null}
        {busy === 'file' ? <div className='mb-3 flex flex-col gap-2'>
          <span className='text-[12px] text-subtle'>{t('正在读取、校验并安装插件包…')}</span>
          <Progress value={null} />
        </div> : null}
        <div className='grid grid-cols-1 items-start gap-3.5 min-[900px]:grid-cols-2'>
          {storePlugins.map(item => <StorePluginCard key={item.id} item={item} target={target}
            operation={operations.find(operation => operation.id === item.id)} disabled={locked}
            onInstall={() => void start(item.id, false)}
            onNotes={() => setNotes({ name: pluginName(item.id, item.name), version: item.version || '',
              notes: item.notes, page: item.page, prerelease: item.prerelease })} />)}
          {packs.map(pack => {
            const update = payload?.updates?.find(item => item.id === pack.id)
            const operation = operations.find(item => item.id === pack.id)
            return <InstalledPluginCard key={pack.id} pack={pack} store={store.find(item => item.id === pack.id)}
              host={host || null} update={update} operation={operation} disabled={locked || operation?.active === true}
              onEnabled={enabled => void mutation(`enable:${pack.id}`, '/api/plugins/enable', { id: pack.id, enabled })}
              onSettings={() => setSettingsPackId(pack.id)}
              onRemove={() => setConfirmRemove(pack)} onUpdate={() => void start(pack.id, true)}
              onNotes={() => setNotes({ name: pluginName(pack.id, pack.name), version: update?.latest || '',
                notes: update?.notes, page: update?.page, prerelease: update?.prerelease })} />
          })}
        </div>
        {!storePlugins.length && !packs.length ? <div className='rounded-lg border border-border bg-surface px-4 py-5'>
          <p className='text-[13px] font-medium'>{!payload || catalog?.checking
            ? t('正在读取插件列表…') : t('暂无可安装的插件')}</p>
          <p className='mt-2 text-[12px] text-subtle'>{errorMessage(catalog?.reason) || t('可以点击「刷新」读取插件仓库，或从本地 zip 安装插件。')}</p>
        </div> : null}
        {payload ? <Collapsible className='mt-3 rounded-md border border-hairline bg-surface-2'>
          <CollapsibleTrigger render={<Button variant='ghost' />}
            className='group/diagnostics flex h-auto w-full justify-start gap-2 rounded-md px-3 py-3 text-[12px] text-subtle'>
            <svg viewBox='0 0 24 24' className='size-3.5 transition-transform duration-150 group-aria-expanded/diagnostics:rotate-90 motion-reduce:transition-none'
              fill='none' stroke='currentColor' strokeWidth='1.8' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
              <path d='m9 5 7 7-7 7' />
            </svg>
            {t('诊断信息')}
          </CollapsibleTrigger>
          <CollapsibleContent>
            <div className='flex flex-col gap-1 px-3 pb-3 text-[12px] text-subtle'>
              <span>{t('插件目录')} <code className='break-all'>{payload.pluginsDir}</code></span>
              <span>{t('当前平台')} <code>{payload.target}</code></span>
              <span>{t('协议版本')} {payload.protocol}</span>
              {catalog?.reason ? <span className='text-destructive'>{errorMessage(catalog.reason)}</span> : null}
              {host?.lastError ? <span className='text-destructive'>{host.lastError}</span> : null}
            </div>
          </CollapsibleContent>
        </Collapsible> : null}
      </div>
      <CardFooter className='panel-foot'>{t('插件包与主程序分开安装、分开升级。卸载插件后账号数据仍然保留，重新装回即可继续使用；仅该包提供的转发与添加账号需要插件，本体订阅不受影响。')}</CardFooter>
    </Card>
    <PluginSettingsDialog open={settingsOpen} onClose={() => setSettingsOpen(false)} onChanged={check} />
    {settingsPack ? <PluginPackSettingsDialog key={settingsPack.id} pack={settingsPack}
      name={store.find(item => item.id === settingsPack.id)?.name}
      disabled={locked || settingsOperation?.active === true} onClose={() => setSettingsPackId(null)}
      onEnabled={enabled => void mutation(`enable:${settingsPack.id}`, '/api/plugins/enable', { id: settingsPack.id, enabled })}
      onProvider={(provider, enabled) => void mutation(`provider:${provider.id}`, '/api/plugins/provider', {
        id: settingsPack.id, provider: provider.id, enabled,
      })} /> : null}
    <PluginNotesDialog value={notes} onClose={() => setNotes(null)} />
    <Dialog open={confirmRemove !== null} onOpenChange={next => { if (!next) setConfirmRemove(null) }}>
      <DialogContent>
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
