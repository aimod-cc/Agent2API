import * as React from 'react'
import {
  Alert,
  AlertDialog,
  AlertDialogBody,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  Badge,
  Button,
  Card,
  CardContent,
  CardFooter,
  CardHeader,
  CardTitle,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Empty,
  EmptyDescription,
  EmptyTitle,
  Input,
  InputGroup,
  InputGroupAddon,
  InputGroupInput,
  Label,
  SegmentedControl,
  Sheet,
  SheetBody,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
  Spinner,
  Switch,
  Textarea,
  useMediaQuery,
} from '@ui'
import { readFileAsDataUrl } from './file-read'
import { PLUGIN_GRID_CLASS, PluginSectionHeading } from './plugins-store'
import { PluginStreamFooter, usePluginStream } from './plugins-stream-footer'
import type { PluginStatusFilter } from './plugins-filters'
import {
  templateLoginCheckin,
  type TaskApiRequest,
  type TaskPlugin,
  type TaskPluginsPayload,
  type TaskPluginStoreItem,
  type TaskTranslate,
} from './task-plugins-types'
import { TaskPluginEditorDialog } from './task-plugin-editor'

function formatImportFileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1048576) return `${(bytes / 1024).toFixed(bytes >= 102400 ? 0 : 1)} KiB`
  return `${(bytes / 1048576).toFixed(1)} MiB`
}

export function TaskPluginsSection({
  payload,
  status,
  query,
  request,
  refresh,
  onOpenScheduledPage,
  jobCounts,
  embedded = false,
  hideContent = false,
  drawer,
  scrollRef,
  importRequest = 0,
  createRequest = 0,
  t,
  notify,
}: {
  payload: TaskPluginsPayload | null
  status?: PluginStatusFilter
  query?: string
  request: TaskApiRequest
  refresh: () => Promise<void>
  onOpenScheduledPage?: (pluginId?: string) => void
  jobCounts?: ReadonlyMap<string, number>
  embedded?: boolean
  hideContent?: boolean
  drawer?: {
    open: boolean
    onOpenChange: (open: boolean) => void
  }
  scrollRef?: React.RefObject<HTMLElement | null>
  importRequest?: number
  createRequest?: number
  t: TaskTranslate
  notify?: (message: string, kind?: 'ok' | 'err') => void
}) {
  const mobile = useMediaQuery('(max-width: 767px)')
  const [localStatus, setLocalStatus] = React.useState<PluginStatusFilter>('all')
  const [localQuery, setLocalQuery] = React.useState('')
  const effectiveStatus = status ?? localStatus
  const effectiveQuery = query ?? localQuery

  const [busyId, setBusyId] = React.useState('')
  const [inspectId, setInspectId] = React.useState<string | null>(null)
  const [removeTarget, setRemoveTarget] = React.useState<TaskPlugin | null>(null)
  const [editor, setEditor] = React.useState<{ plugin: TaskPlugin | null; isNew: boolean } | null>(null)
  const [importOpen, setImportOpen] = React.useState(false)
  const [importText, setImportText] = React.useState('')
  const [importFile, setImportFile] = React.useState<{ name: string; size: number } | null>(null)
  const [importDragging, setImportDragging] = React.useState(false)
  const [importBusy, setImportBusy] = React.useState(false)
  const [importError, setImportError] = React.useState('')
  const [exportData, setExportData] = React.useState<{ filename: string; content: string } | null>(null)
  const [copied, setCopied] = React.useState(false)
  const sectionRef = React.useRef<HTMLElement>(null)
  const drawerBodyRef = React.useRef<HTMLDivElement>(null)
  const importFileInputRef = React.useRef<HTMLInputElement>(null)
  const lastImportRequest = React.useRef(importRequest)
  const lastCreateRequest = React.useRef(createRequest)

  const openEditor = React.useCallback((next: { plugin: TaskPlugin | null; isNew: boolean }) => {
    drawer?.onOpenChange(false)
    setEditor(next)
  }, [drawer])

  const openImport = React.useCallback(() => {
    drawer?.onOpenChange(false)
    setImportError('')
    setImportDragging(false)
    setImportOpen(true)
  }, [drawer])

  React.useEffect(() => {
    if (importRequest > lastImportRequest.current) {
      lastImportRequest.current = importRequest
      openImport()
    }
  }, [importRequest, openImport])

  React.useEffect(() => {
    if (createRequest > lastCreateRequest.current) {
      lastCreateRequest.current = createRequest
      openEditor({ plugin: templateLoginCheckin(), isNew: true })
    }
  }, [createRequest, openEditor])

  const installed = payload?.plugins ?? []
  const storeItems = payload?.store ?? []
  const storeUninstalledCount = storeItems.filter(item => !item.installed).length
  const updates = payload?.updates ?? []
  const q = effectiveQuery.trim().toLowerCase()

  const matchText = (name: string, desc: string, id: string) =>
    !q || `${name} ${desc} ${id}`.toLowerCase().includes(q)

  const displayDescription = (description?: string | null): string => {
    const trimmed = description?.trim()
    return trimmed ? t(trimmed) : t('定时任务脚本插件')
  }

  const visibleInstalled = installed.filter(p => {
    if (!matchText(p.name, p.description, p.id)) return false
    if (effectiveStatus === 'missing') return false
    if (effectiveStatus === 'updatable') return updates.some(u => u.id === p.id)
    return true
  })

  const visibleStore = storeItems.filter(item => {
    if (item.installed) return false
    if (!matchText(item.name, item.description, item.id)) return false
    if (effectiveStatus === 'installed' || effectiveStatus === 'updatable') return false
    return true
  })

  const inspecting = inspectId ? installed.find(p => p.id === inspectId) ?? null : null
  const totalCount = visibleInstalled.length + visibleStore.length
  const taskRows = [
    ...visibleInstalled.map(plugin => ({ kind: 'installed' as const, plugin })),
    ...visibleStore.map(item => ({ kind: 'store' as const, item })),
  ]
  const stream = usePluginStream(totalCount, JSON.stringify([effectiveStatus, q]))
  const visibleRows = stream.slice(taskRows)

  const patchPlugin = async (id: string, patch: { enabled?: boolean; notifyAllowed?: boolean }) => {
    setBusyId(id)
    try {
      await request('PATCH', `/api/task-plugins/${encodeURIComponent(id)}`, patch)
      await refresh()
      if (patch.notifyAllowed !== undefined) {
        notify?.(patch.notifyAllowed ? t('已开启系统通知权限') : t('已关闭系统通知权限'), 'ok')
      }
    } catch (cause) {
      notify?.(cause instanceof Error ? cause.message : String(cause), 'err')
    } finally {
      setBusyId('')
    }
  }

  const installFromStore = async (item: TaskPluginStoreItem | string) => {
    const id = typeof item === 'string' ? item : item.id
    setBusyId(id)
    try {
      await request('POST', '/api/task-plugins/install-store', { id })
      await refresh()
      notify?.(t('定时任务插件已安装'), 'ok')
    } catch (cause) {
      notify?.(cause instanceof Error ? cause.message : String(cause), 'err')
    } finally {
      setBusyId('')
    }
  }

  const uninstall = async (plugin: TaskPlugin) => {
    setBusyId(plugin.id)
    try {
      await request('DELETE', `/api/task-plugins/${encodeURIComponent(plugin.id)}`)
      await refresh()
      notify?.(t('已卸载插件，相关任务配置已保留'), 'ok')
    } catch (cause) {
      notify?.(cause instanceof Error ? cause.message : String(cause), 'err')
    } finally {
      setBusyId('')
      setRemoveTarget(null)
      setInspectId(null)
    }
  }

  const openExport = async (plugin: TaskPlugin) => {
    setBusyId(plugin.id)
    try {
      const data = await request<{ filename: string; content: string }>(
        'GET',
        `/api/task-plugins/${encodeURIComponent(plugin.id)}/export`
      )
      setCopied(false)
      setExportData(data)
    } catch (cause) {
      notify?.(cause instanceof Error ? cause.message : String(cause), 'err')
    } finally {
      setBusyId('')
    }
  }

  const handleImportSubmit = async () => {
    if (!importText.trim()) return
    setImportBusy(true)
    setImportError('')
    try {
      await request('POST', '/api/task-plugins/import', { rawScript: importText })
      setImportOpen(false)
      setImportText('')
      setImportFile(null)
      await refresh()
      notify?.(t('定时任务插件已导入'), 'ok')
    } catch (cause) {
      setImportError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setImportBusy(false)
    }
  }

  const handleFileUpload = async (file: File | null) => {
    if (!file) return
    setImportError('')
    if (!/\.(js|mjs|txt|zip)$/i.test(file.name)) {
      setImportError(t('仅支持 .js、.mjs、.txt 脚本文件或 .zip 插件包'))
      return
    }
    if (file.size === 0) {
      setImportError(t('文件为空'))
      return
    }
    setImportBusy(true)
    try {
      if (file.name.toLowerCase().endsWith('.zip')) {
        const dataUrl = await readFileAsDataUrl(file, t('读取文件失败'))
        const zipBase64 = dataUrl.slice(dataUrl.indexOf('base64,') + 7)
        await request('POST', '/api/task-plugins/import', { zipBase64 })
        setImportOpen(false)
        setImportFile(null)
        setImportText('')
        await refresh()
        notify?.(t('定时任务插件包已导入'), 'ok')
      } else {
        const text = await file.text()
        setImportFile({ name: file.name, size: file.size })
        setImportText(text)
      }
    } catch (cause) {
      setImportError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setImportBusy(false)
    }
  }

  const galleryContent = (
    <section ref={sectionRef} className='@container/plugin-gallery min-w-0 shrink-0' aria-label={t('定时任务插件')}>
      {!embedded && !drawer ? (
        <PluginSectionHeading
          t={t}
          title={t('定时任务插件')}
          count={totalCount}
          hint={t('沙箱轻脚本插件，按声明自动渲染表单与操作按钮')}
          actions={
            <>
              <Button variant='outline' size='sm' onClick={openImport}>
                {t('导入脚本/包')}
              </Button>
              <Button size='sm' onClick={() => openEditor({ plugin: templateLoginCheckin(), isNew: true })}>
                {t('新建定时任务插件')}
              </Button>
            </>
          }
        />
      ) : null}

      {totalCount === 0 ? (
        <Card className={embedded || drawer ? '' : 'mt-2'}>
          <Empty>
            <EmptyTitle>{installed.length + storeItems.length === 0 ? t('暂无定时任务插件') : t('没有符合筛选条件的任务插件')}</EmptyTitle>
            <EmptyDescription>
              {installed.length + storeItems.length === 0
                ? t('可以在线新建自定义定时任务脚本，或导入单文件 / ZIP 插件包。')
                : t('切换筛选条件或调整搜索词后再查看。')}
            </EmptyDescription>
            {installed.length + storeItems.length === 0 ? (
              <div className='mt-3 flex flex-wrap items-center justify-center gap-2'>
                <Button size='sm' onClick={() => openEditor({ plugin: templateLoginCheckin(), isNew: true })}>
                  {t('新建定时任务插件')}
                </Button>
                <Button variant='outline' size='sm' onClick={openImport}>
                  {t('导入脚本/包')}
                </Button>
              </div>
            ) : null}
          </Empty>
        </Card>
      ) : (
        <>
          <div className={embedded || drawer ? PLUGIN_GRID_CLASS : `${PLUGIN_GRID_CLASS} mt-2`}>
            {visibleRows.map(row => {
              if (row.kind === 'installed') {
                const plugin = row.plugin
                const update = updates.find(u => u.id === plugin.id)
                const busy = busyId === plugin.id
                const usedJobs = jobCounts?.get(plugin.id) ?? 0
                return (
                  <Card key={plugin.id} className='flex flex-col'>
                    <CardHeader className='items-start border-b-0 pb-2'>
                      <div className='flex size-9 shrink-0 items-center justify-center rounded-lg bg-primary-soft text-xs font-semibold text-primary-fg'>
                        {plugin.icon && plugin.icon.startsWith('data:image/') ? (
                          <img src={plugin.icon} alt='' className='size-7 rounded object-contain' />
                        ) : (
                          plugin.name.slice(0, 1)
                        )}
                      </div>
                      <div className='min-w-0 flex-1'>
                        <div className='flex flex-wrap items-center gap-1.5'>
                          <CardTitle className='truncate'>{plugin.name}</CardTitle>
                          <span className='text-[11px] text-muted-foreground'>v{plugin.version}</span>
                        </div>
                        <div className='mt-1 flex flex-wrap gap-1'>
                          {plugin.signed ? (
                            <Badge variant='brand' shape='tag'>{t('官方签名')}</Badge>
                          ) : plugin.source === 'local' ? (
                            <Badge variant='outline' shape='tag'>{t('本地自建')}</Badge>
                          ) : (
                            <Badge variant='warning' shape='tag'>{t('第三方未签名')}</Badge>
                          )}
                          {update ? (
                            <Badge variant='warning' shape='tag'>
                              {t('可更新 v{v}', { v: update.latest })}
                            </Badge>
                          ) : null}
                          {usedJobs > 0 ? (
                            <Badge variant='secondary' shape='tag' className='font-normal'>
                              {t('{count} 个任务', { count: usedJobs })}
                            </Badge>
                          ) : null}
                        </div>
                      </div>
                    </CardHeader>
                    <CardContent className='flex flex-1 flex-col gap-2 pt-0'>
                      <p className='m-0 line-clamp-2 text-xs leading-relaxed text-subtle'>
                        {displayDescription(plugin.description)}
                      </p>
                      <div className='mt-auto flex flex-wrap gap-1.5'>
                        <Badge variant='outline' shape='tag'>
                          {t('{n} 个域名白名单', { n: plugin.domains.length })}
                        </Badge>
                        {plugin.declaresNotify ? (
                          <Badge variant={plugin.notifyAllowed ? 'success' : 'warning'} shape='tag'>
                            {plugin.notifyAllowed ? t('系统通知（开启）') : t('系统通知（已关）')}
                          </Badge>
                        ) : null}
                      </div>
                    </CardContent>
                    <CardFooter className='justify-between bg-surface-2/60'>
                      <div className='flex flex-wrap items-center gap-1.5'>
                        {update ? (
                          <Button size='sm' disabled={busy} onClick={() => void installFromStore(plugin.id)}>
                            {busy ? <Spinner /> : null}{t('更新')}
                          </Button>
                        ) : null}
                        {plugin.source !== 'official' ? (
                          <Button size='sm' variant='outline' disabled={busy} onClick={() => openEditor({ plugin, isNew: false })}>
                            {t('编辑')}
                          </Button>
                        ) : null}
                        <Button size='sm' variant='outline' disabled={busy} onClick={() => setInspectId(plugin.id)}>
                          {t('权限与设置')}
                        </Button>
                        {plugin.enabled && onOpenScheduledPage ? (
                          <Button size='sm' variant='ghost' onClick={() => {
                            drawer?.onOpenChange(false)
                            onOpenScheduledPage(plugin.id)
                          }}>
                            {t('建任务')}
                          </Button>
                        ) : null}
                      </div>
                      <Switch
                        checked={plugin.enabled}
                        disabled={busy}
                        aria-label={`${t('启用')} ${plugin.name}`}
                        onCheckedChange={enabled => void patchPlugin(plugin.id, { enabled })}
                      />
                    </CardFooter>
                  </Card>
                )
              }
              const item = row.item
              const busy = busyId === item.id
              return (
                <Card key={`store:${item.id}`} className='flex flex-col'>
                  <CardHeader className='items-start border-b-0 pb-2'>
                    <div className='flex size-9 shrink-0 items-center justify-center rounded-lg bg-info-soft text-xs font-semibold text-info'>
                      {item.icon ? <img src={item.icon} alt='' className='size-7 rounded object-contain' /> : item.name.slice(0, 1)}
                    </div>
                    <div className='min-w-0 flex-1'>
                      <div className='flex items-center gap-1.5'>
                        <CardTitle className='truncate'>{item.name}</CardTitle>
                        {item.version ? <span className='text-[11px] text-muted-foreground'>v{item.version}</span> : null}
                      </div>
                      <div className='mt-1 flex gap-1'>
                        <Badge variant='brand' shape='tag'>{t('官方商店')}</Badge>
                      </div>
                    </div>
                  </CardHeader>
                  <CardContent className='flex-1 pt-0'>
                    <p className='m-0 line-clamp-2 text-xs leading-relaxed text-subtle'>{displayDescription(item.description)}</p>
                  </CardContent>
                  <CardFooter className='justify-end bg-surface-2/60'>
                    <Button size='sm' disabled={busy || !item.asset} onClick={() => void installFromStore(item)}>
                      {busy ? <Spinner /> : null}
                      {item.asset ? t('安装') : t('暂无发布包')}
                    </Button>
                  </CardFooter>
                </Card>
              )
            })}
          </div>
          <PluginStreamFooter stream={stream} scrollRef={drawer ? drawerBodyRef : scrollRef} t={t} />
        </>
      )}
    </section>
  )

  return (
    <>
      {drawer ? (
        <Sheet open={drawer.open} onOpenChange={drawer.onOpenChange}>
          <SheetContent
            side={mobile ? 'bottom' : 'right'}
            showHandle={mobile}
            className={mobile ? 'text-foreground' : 'w-[min(760px,94vw)] text-foreground'}
          >
            <SheetHeader className='shrink-0'>
              <div className='flex min-w-0 flex-1 flex-wrap items-center justify-between gap-2 pr-8'>
                <SheetTitle className='m-0'>{t('插件管理')}</SheetTitle>
                <div className='flex flex-wrap items-center gap-2'>
                  <Button variant='outline' size='sm' onClick={openImport}>{t('导入脚本/包')}</Button>
                  <Button size='sm' onClick={() => openEditor({ plugin: templateLoginCheckin(), isNew: true })}>
                    {t('新建定时任务插件')}
                  </Button>
                </div>
              </div>
              <SheetDescription className='sr-only'>{t('管理定时任务插件与官方商店')}</SheetDescription>
            </SheetHeader>
            <div className='flex shrink-0 flex-wrap items-center justify-between gap-2 border-b border-hairline px-5 py-3'>
              <SegmentedControl<PluginStatusFilter>
                aria-label={t('任务插件筛选')}
                value={effectiveStatus}
                onValueChange={setLocalStatus}
                options={[
                  { value: 'all', label: t('全部'), count: installed.length + storeUninstalledCount },
                  { value: 'installed', label: t('已安装'), count: installed.length },
                  { value: 'missing', label: t('官方商店'), count: storeUninstalledCount },
                  { value: 'updatable', label: t('可更新'), count: updates.length },
                ]}
              />
              <InputGroup className='w-48 rounded-sm border-hairline bg-surface'>
                <InputGroupInput
                  type='search'
                  value={effectiveQuery}
                  placeholder={t('搜索任务插件')}
                  aria-label={t('搜索任务插件')}
                  onChange={e => setLocalQuery(e.currentTarget.value)}
                />
                <InputGroupAddon>
                  <svg viewBox='0 0 16 16' className='size-3.5' fill='none' stroke='currentColor' strokeWidth='1.7' strokeLinecap='round' aria-hidden='true'>
                    <circle cx='6.8' cy='6.8' r='4.6' />
                    <path d='M10.3 10.3 14 14' />
                  </svg>
                </InputGroupAddon>
              </InputGroup>
            </div>
            <SheetBody ref={drawerBodyRef} className='min-h-0 flex-1 gap-4 [scrollbar-gutter:stable]'>
              {galleryContent}
            </SheetBody>
            <SheetFooter className='shrink-0'>
              <Button variant='outline' size='sm' onClick={() => drawer.onOpenChange(false)}>{t('完成')}</Button>
            </SheetFooter>
          </SheetContent>
        </Sheet>
      ) : !hideContent ? galleryContent : null}

      {/* 权限与详情弹窗 */}
      <Dialog open={Boolean(inspecting)} onOpenChange={open => { if (!open) setInspectId(null) }}>
        <DialogContent className='w-[min(560px,calc(100vw-32px))] text-foreground'>
          <DialogHeader>
            <DialogTitle>{inspecting?.name} · {t('权限与设置')}</DialogTitle>
          </DialogHeader>
          {inspecting ? (
            <DialogBody>
              {inspecting.description?.trim() ? <p className='m-0 text-xs leading-relaxed text-subtle'>{displayDescription(inspecting.description)}</p> : null}
              <div className='flex flex-col gap-2 rounded-md border border-hairline bg-surface-2 p-3.5'>
                <div className='text-xs font-semibold'>{t('允许访问的网址白名单')}</div>
                <div className='flex flex-wrap gap-1.5'>
                  {inspecting.domains.length ? inspecting.domains.map(d => (
                    <Badge key={d} variant='outline' shape='tag'>{d}</Badge>
                  )) : <span className='text-xs text-muted-foreground'>{t('未声明域名（仅本地纯计算）')}</span>}
                </div>
              </div>
              {inspecting.declaresNotify ? (
                <div className='flex items-center justify-between gap-3 rounded-md border border-hairline bg-surface-2 p-3.5'>
                  <div className='min-w-0'>
                    <Label className='text-[13px] font-medium'>{t('允许调用系统通知')}</Label>
                    <p className='m-0 mt-0.5 text-[11.5px] text-muted-foreground'>
                      {t('该插件声明了通知权限，默认开启；关闭后不会发送任何系统通知。')}
                    </p>
                  </div>
                  <Switch
                    checked={inspecting.notifyAllowed}
                    disabled={busyId === inspecting.id}
                    onCheckedChange={allowed => void patchPlugin(inspecting.id, { notifyAllowed: allowed })}
                  />
                </div>
              ) : null}
              <div className='rounded-md border border-hairline bg-surface-inset p-3 text-xs text-muted-foreground'>
                {t('界面声明：{fields} 个输入项 · {actions} 个按钮 · {status} 个状态指标', {
                  fields: inspecting.fields.length,
                  actions: inspecting.actions.length,
                  status: inspecting.statusFields.length,
                })}
              </div>
            </DialogBody>
          ) : null}
          <DialogFooter>
            {inspecting ? (
              <>
                <Button variant='outline' className='mr-auto' onClick={() => void openExport(inspecting)}>
                  {t('导出分享文件')}
                </Button>
                {inspecting.source !== 'official' ? (
                  <Button variant='outline' onClick={() => { setInspectId(null); openEditor({ plugin: inspecting, isNew: false }) }}>
                    {t('编辑脚本')}
                  </Button>
                ) : null}
                <Button variant='destructive' onClick={() => setRemoveTarget(inspecting)}>
                  {t('卸载')}
                </Button>
              </>
            ) : null}
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* 卸载确认 */}
      <AlertDialog open={Boolean(removeTarget)} onOpenChange={open => { if (!open) setRemoveTarget(null) }}>
        <AlertDialogContent overlayForceRender className='text-foreground'>
          <AlertDialogHeader>
            <AlertDialogTitle>{t('卸载定时任务插件')}</AlertDialogTitle>
          </AlertDialogHeader>
          <AlertDialogBody>
            <AlertDialogDescription className='m-0 text-[13px] leading-relaxed text-foreground'>
              {t('确认卸载「{name}」吗？名下定时任务将暂停，填写的配置与历史记录会保留，重装后恢复。', { name: removeTarget?.name || '' })}
            </AlertDialogDescription>
          </AlertDialogBody>
          <AlertDialogFooter>
            <Button variant='outline' onClick={() => setRemoveTarget(null)}>{t('取消')}</Button>
            <Button variant='destructive' onClick={() => { if (removeTarget) void uninstall(removeTarget) }}>
              {t('确认卸载')}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      {/* 导入弹窗 */}
      <Dialog open={importOpen} onOpenChange={next => {
        if (!next && importBusy) return
        setImportOpen(next); if (!next) setImportDragging(false)
      }}>
        <DialogContent className='w-[min(640px,calc(100vw-32px))] text-foreground'>
          <DialogHeader><DialogTitle>{t('导入定时任务插件')}</DialogTitle></DialogHeader>
          <DialogBody>
            <Alert className='shrink-0'>{t('支持上传 .js 单文件或官方/社区 .zip 插件包，也可直接粘贴单文件脚本内容。')}</Alert>
            <Input ref={importFileInputRef} type='file' accept='.js,.mjs,.txt,.zip' className='hidden' disabled={importBusy}
              onChange={e => { const file = e.currentTarget.files?.[0] ?? null; e.currentTarget.value = ''; void handleFileUpload(file) }} />
            <div
              onDragOver={e => { e.preventDefault(); if (!importBusy) setImportDragging(true) }}
              onDragLeave={e => { if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setImportDragging(false) }}
              onDrop={e => { e.preventDefault(); setImportDragging(false); if (!importBusy) void handleFileUpload(e.dataTransfer.files?.[0] ?? null) }}
              className={`flex shrink-0 items-center gap-2 rounded-md border border-dashed px-3 py-2.5 transition-colors ${
                importDragging
                  ? 'border-primary bg-primary-soft'
                  : 'border-control-border bg-surface-2 hover:border-control-border-hover'
              } ${importBusy ? 'opacity-60' : ''}`}
            >
              <button
                type='button'
                disabled={importBusy}
                onClick={() => importFileInputRef.current?.click()}
                className='flex min-w-0 flex-1 items-center gap-3 rounded-sm border-0 bg-transparent p-0 text-left outline-none focus-visible:shadow-focus disabled:cursor-not-allowed'
              >
                <span className='flex size-8 shrink-0 items-center justify-center rounded-sm border border-hairline bg-surface text-muted-foreground'>
                  {importBusy ? (
                    <Spinner className='size-3.5' />
                  ) : (
                    <svg viewBox='0 0 24 24' className='size-4' fill='none' stroke='currentColor' strokeWidth='1.7' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
                      <path d='M12 16V4m0 0L7.5 8.5M12 4l4.5 4.5M4.5 15v4.5h15V15' />
                    </svg>
                  )}
                </span>
                <span className='min-w-0 flex-1'>
                  <span className='block truncate text-[12.5px] font-medium text-foreground' title={importFile?.name}>
                    {importFile ? importFile.name : t('点击选择文件，或拖放到此处')}
                  </span>
                  <span className='mt-0.5 block truncate text-[11.5px] text-muted-foreground'>
                    {importFile ? formatImportFileSize(importFile.size) : '.js / .mjs / .txt / .zip'}
                  </span>
                </span>
              </button>
              <Button
                type='button'
                variant='outline'
                size='sm'
                disabled={importBusy}
                onClick={() => importFileInputRef.current?.click()}
              >
                {importFile ? t('重新选择') : t('选择文件')}
              </Button>
              {importFile ? (
                <Button
                  type='button'
                  variant='ghost'
                  size='icon-sm'
                  disabled={importBusy}
                  aria-label={t('清空')}
                  title={t('清空')}
                  onClick={() => {
                    setImportFile(null)
                    setImportText('')
                    setImportError('')
                  }}
                >
                  <svg viewBox='0 0 16 16' className='size-3' fill='none' stroke='currentColor' strokeWidth='1.7' strokeLinecap='round' aria-hidden='true'>
                    <path d='m4 4 8 8M12 4l-8 8' />
                  </svg>
                </Button>
              ) : null}
            </div>
            <Textarea
              rows={12}
              disabled={importBusy}
              className='font-mono text-xs'
              value={importText}
              placeholder='/* @agent2api-task-plugin ... */'
              onChange={e => {
                const value = e.currentTarget.value
                setImportText(value)
                if (!value) setImportFile(null)
              }}
            />
            {importError ? <Alert variant='destructive' className='shrink-0 whitespace-pre-wrap break-words'>{importError}</Alert> : null}
          </DialogBody>
          <DialogFooter>
            <Button variant='outline' disabled={importBusy} onClick={() => setImportOpen(false)}>{t('取消')}</Button>
            <Button disabled={importBusy || !importText.trim()} onClick={() => void handleImportSubmit()}>
              {importBusy ? <Spinner /> : null}{t('解析并导入')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* 导出弹窗 */}
      <Dialog open={Boolean(exportData)} onOpenChange={open => { if (!open) setExportData(null) }}>
        <DialogContent className='w-[min(640px,calc(100vw-32px))] text-foreground'>
          <DialogHeader><DialogTitle>{t('导出分享')} · {exportData?.filename}</DialogTitle></DialogHeader>
          <DialogBody>
            <Alert variant='success'>
              {t('导出内容仅包含界面声明与脚本代码，不包含账号密码、表单参数和登录凭证。')}
            </Alert>
            <Textarea rows={14} readOnly className='font-mono text-xs' value={exportData?.content || ''} />
          </DialogBody>
          <DialogFooter>
            <Button variant='outline' onClick={() => setExportData(null)}>{t('关闭')}</Button>
            <Button
              variant='outline'
              onClick={() => {
                if (!exportData) return
                const blob = new Blob([exportData.content], { type: 'text/javascript;charset=utf-8' })
                const url = URL.createObjectURL(blob)
                const a = document.createElement('a')
                a.href = url
                a.download = exportData.filename
                a.click()
                URL.revokeObjectURL(url)
              }}
            >
              {t('下载文件')}
            </Button>
            <Button
              onClick={() => {
                if (exportData) {
                  void navigator.clipboard?.writeText(exportData.content)
                  setCopied(true)
                }
              }}
            >
              {copied ? t('已复制') : t('复制内容')}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      {/* 双向编辑器 */}
      <TaskPluginEditorDialog
        open={Boolean(editor)}
        initialPlugin={editor?.plugin ?? null}
        isNew={editor?.isNew ?? true}
        autoCreateDefaultJob={editor?.isNew === true}
        request={request}
        t={t}
        onClose={() => setEditor(null)}
        onSaved={async (plugin, createJob) => {
          setEditor(null)
          if (createJob && !embedded && !drawer) {
            try {
              await request('POST', '/api/task-jobs', {
                pluginId: plugin.id,
                name: plugin.name,
                cronExpression: plugin.defaultCron,
                timezone: plugin.defaultTimezone,
                timeoutSeconds: plugin.defaultTimeout,
                proxyId: '',
                enabled: false,
                values: {},
              })
            } catch { /* ignore */ }
          }
          await refresh()
          notify?.(t('定时任务插件已保存'), 'ok')
          if (createJob && onOpenScheduledPage) onOpenScheduledPage(plugin.id)
        }}
      />
    </>
  )
}
