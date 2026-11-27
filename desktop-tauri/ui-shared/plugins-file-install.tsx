import * as React from 'react'
import { Alert, AlertDialog, AlertDialogBody, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle, Badge, Button, Input, Progress, SegmentedControl, Spinner } from '@ui'
import type { PluginMutation, PluginOperation, PluginsPayload } from './plugins'
import { PluginOperationUnconfirmed, waitPackOperation, type PluginRequest } from './plugins-batch-runner'
import type { PluginTranslate } from './plugins-store'
import { readFileAsDataUrl } from './file-read'
import { DIALOG_SEG_CLASS } from './dialog-segmented'
import { PluginTaskDialog, PluginTaskProgressBar, PluginTaskRemoveButton } from './plugins-task-dialog'

const errorMessage = (cause: unknown) => cause instanceof Error ? cause.message : String(cause)
const formatSize = (bytes: number) => bytes >= 1048576 ? `${(bytes / 1048576).toFixed(1)} MiB` : `${Math.max(1, Math.round(bytes / 1024))} KiB`

const MIB = 1024 * 1024
export const LEGACY_MAX_PACK_BYTES = 24 * MIB
const HARD_MAX_PACK_BYTES = 96 * MIB

export function clampMaxPackBytes(value: unknown): number {
  return typeof value === 'number' && Number.isSafeInteger(value) && value > 0 ? Math.min(value, HARD_MAX_PACK_BYTES) : LEGACY_MAX_PACK_BYTES
}

type Preview = { id: string; name: string; version: string; signed: boolean; sha256: string; previousVersion?: string | null; enabled: boolean; kind?: 'provider' | 'task_script' }
type TaskInstallResult = { kind: 'task_script'; taskPlugin: { id: string; name: string; version: string } }
type FileState = 'preparing' | 'ready' | 'installing' | 'installed' | 'warning' | 'unconfirmed' | 'failed' | 'invalid' | 'duplicate'
export type PluginFileResult = { key: string; file: File; state: FileState; preview?: Preview; error?: string; percent?: number | null }

async function readPack(file: File, maxPackBytes: number, t: PluginTranslate): Promise<string> {
  if (!file.size || file.size > maxPackBytes || !file.name.toLowerCase().endsWith('.zip')) throw new Error(t('请选择非空且不超过 {n} MiB 的 zip 插件包', { n: maxPackBytes / MIB }))
  const data = await readFileAsDataUrl(file, t('读取文件失败'))
  const encoded = data.slice(data.indexOf('base64,') + 7)
  if (encoded.length > Math.ceil(maxPackBytes / 3) * 4) throw new Error(t('插件包大小不能超过 {n} MiB', { n: maxPackBytes / MIB }))
  return encoded
}

type FileInstallSummary = {
  ready: number
  failed: number
  warnings: number
  succeeded: number
  done: number
  total: number
  skipped: number
  current?: PluginFileResult
  unconfirmed: boolean
}

function summarizeFileInstall(results: PluginFileResult[]): FileInstallSummary {
  return {
    ready: results.filter(item => item.state === 'ready').length,
    failed: results.filter(item => item.state === 'failed').length,
    warnings: results.filter(item => item.state === 'warning' || item.state === 'unconfirmed').length,
    succeeded: results.filter(item => item.state === 'installed' || item.state === 'warning').length,
    done: results.filter(item => ['installed', 'warning', 'failed', 'unconfirmed'].includes(item.state)).length,
    total: results.filter(item => !['invalid', 'duplicate'].includes(item.state)).length,
    skipped: results.filter(item => ['invalid', 'duplicate'].includes(item.state)).length,
    current: results.find(item => item.state === 'installing'),
    unconfirmed: results.some(item => item.state === 'unconfirmed'),
  }
}

export function usePluginFileInstall({ maxPackBytes, previewAvailable, disabled, accept, onTaskPluginInstalled, refresh, remember, request, t, scope = '' }: {
  request: PluginRequest; t: PluginTranslate; scope?: string
  maxPackBytes: number; previewAvailable: boolean; disabled: boolean; accept(result: PluginMutation): Promise<void>; onTaskPluginInstalled?(): Promise<void> | void; refresh(): Promise<void>; remember(operation: PluginOperation): void
}) {
  const [open, setOpen] = React.useState(false)
  const [results, setResults] = React.useState<PluginFileResult[]>([])
  const [phase, setPhase] = React.useState<'ready' | 'preparing' | 'running' | 'done'>('ready')
  const phaseRef = React.useRef<'ready' | 'preparing' | 'running' | 'done'>('ready')
  const rows = React.useRef<PluginFileResult[]>([])
  const lock = React.useRef(false)
  const sessionRef = React.useRef(0)
  const sequence = React.useRef(0)
  const alive = React.useRef(true)
  const scopeRef = React.useRef(scope)
  const setPhaseState = (next: 'ready' | 'preparing' | 'running' | 'done') => { phaseRef.current = next; setPhase(next) }
  React.useEffect(() => { alive.current = true; return () => { alive.current = false } }, [])
  React.useEffect(() => {
    if (scopeRef.current === scope) return
    scopeRef.current = scope
    if (phaseRef.current !== 'running') {
      sessionRef.current += 1
      lock.current = false
      rows.current = []
      setResults([])
      setPhaseState('ready')
      setOpen(false)
    }
  }, [scope])
  const publish = () => { if (alive.current && scopeRef.current === scope) setResults([...rows.current]) }
  const update = (key: string, patch: Partial<PluginFileResult>) => { rows.current = rows.current.map(item => item.key === key ? { ...item, ...patch } : item); publish() }

  const addFiles = async (files: File[]) => {
    if (disabled || lock.current || !files.length) return
    const session = ++sessionRef.current
    lock.current = true
    setPhaseState('preparing')
    setOpen(true)
    try {
      for (const file of files) {
        if (!alive.current || scopeRef.current !== scope || sessionRef.current !== session) return
        const key = `${Date.now()}:${sequence.current++}`
        const duplicate = rows.current.some(item => item.file.name === file.name && item.file.size === file.size && item.file.lastModified === file.lastModified)
        rows.current.push({ key, file, state: duplicate ? 'duplicate' : 'preparing', error: duplicate ? t('重复文件，已跳过') : undefined })
        publish()
        if (duplicate) continue
        try {
          if (!previewAvailable) throw new Error(t('当前网关不支持安装前预览，请更新网关后重试'))
          const packB64 = await readPack(file, maxPackBytes, t)
          if (!alive.current || scopeRef.current !== scope || sessionRef.current !== session) return
          const response = await request<{ preview: Preview }>('POST', '/api/plugins/install?preview=true', { packB64 }, { retry: 'never' })
          if (!alive.current || scopeRef.current !== scope || sessionRef.current !== session) return
          if (!response?.preview?.id) throw new Error(t('无法读取插件包信息，请更新网关后重试'))
          const same = rows.current.some(item => item.key !== key && (item.preview?.kind || 'provider') === (response.preview.kind || 'provider') && item.preview?.id === response.preview.id && item.preview?.version === response.preview.version && !['invalid', 'failed', 'duplicate'].includes(item.state))
          update(key, { preview: response.preview, state: same ? 'duplicate' : 'ready', error: same ? t('同一插件的相同版本，已跳过') : undefined })
        } catch (cause) {
          if (!alive.current || scopeRef.current !== scope || sessionRef.current !== session) return
          update(key, { state: 'invalid', error: errorMessage(cause) })
        }
      }
    } finally {
      if (sessionRef.current === session) {
        lock.current = false
        if (alive.current) {
          if (scopeRef.current !== scope) { rows.current = []; setResults([]); setOpen(false) }
          setPhaseState('ready')
        }
      }
    }
  }

  const start = async (retry = false) => {
    if (disabled || lock.current || rows.current.some(item => item.state === 'unconfirmed')) return
    const pending = rows.current.filter(item => item.state === (retry ? 'failed' : 'ready'))
    if (!pending.length) return
    const session = ++sessionRef.current
    lock.current = true
    setPhaseState('running')
    try {
      for (const item of pending) {
        if (!alive.current || scopeRef.current !== scope || sessionRef.current !== session) return
        update(item.key, { state: 'installing', error: undefined, percent: null })
        try {
          const packB64 = await readPack(item.file, maxPackBytes, t)
          if (!alive.current || scopeRef.current !== scope || sessionRef.current !== session) return
          let response: PluginMutation | { operation: PluginOperation } | TaskInstallResult
          if (!item.preview) throw new Error(t('无法读取插件包信息，请更新网关后重试'))
          const query = new URLSearchParams({ background: 'true', expected_id: item.preview.id, expected_version: item.preview.version, expected_sha256: item.preview.sha256 })
          try { response = await request('POST', `/api/plugins/install?${query}`, { packB64 }, { retry: 'never' }) }
          catch (cause) {
            if (/超时|timeout|timed out|network|fetch|连接|网络/i.test(errorMessage(cause))) throw new PluginOperationUnconfirmed(t('安装结果尚未确认，请查看插件状态后再重试'))
            throw cause
          }
          if ('taskPlugin' in response) {
            if (!alive.current || scopeRef.current !== scope || sessionRef.current !== session) return
            try { await onTaskPluginInstalled?.() } catch {}
            update(item.key, { state: 'installed', error: undefined, percent: 100 })
            continue
          }
          let result: PluginMutation
          if ('operation' in response) {
            if (!response.operation) throw new PluginOperationUnconfirmed(t('安装结果尚未确认，请查看插件状态后再重试'))
            const operation = await waitPackOperation(response.operation, request, t, operation => { remember(operation); update(item.key, { percent: operation.stage === 'downloading' ? operation.percent : null }) })
            if (operation.error) throw new Error(operation.error)
            if (!operation.done && !operation.hostError) throw new Error(t('插件安装未完成'))
            let state: PluginsPayload
            try { state = await request('GET', '/api/plugins') }
            catch { throw new PluginOperationUnconfirmed(t('插件已安装，但读取状态失败，请刷新后查看结果')) }
            result = { ...state, pack: operation.result?.pack, hostError: operation.hostError, installNotice: operation.result?.installNotice }
          } else result = response
          let notice = result.hostError || result.installNotice || result.pack?.unavailableReason || ''
          if (!alive.current || scopeRef.current !== scope || sessionRef.current !== session) return
          try { await accept(result) }
          catch (cause) { notice = t('插件已安装，但刷新界面失败：{error}', { error: errorMessage(cause) }) }
          update(item.key, { state: notice ? 'warning' : 'installed', error: notice || undefined, percent: 100 })
        } catch (cause) {
          const unconfirmed = cause instanceof PluginOperationUnconfirmed
          update(item.key, { state: unconfirmed ? 'unconfirmed' : 'failed', error: errorMessage(cause), percent: null })
          if (unconfirmed) break
        }
        try { await refresh() } catch {}
      }
    } finally {
      if (sessionRef.current === session) {
        lock.current = false
        if (alive.current) {
          if (scopeRef.current !== scope) { rows.current = []; setResults([]); setOpen(false); setPhaseState('ready') }
          else setPhaseState('done')
        }
      }
    }
  }
  const reset = () => {
    if (phaseRef.current === 'running') return
    sessionRef.current += 1
    lock.current = false
    rows.current = []
    publish()
    setPhaseState('ready')
  }
  const close = () => {
    // 安装进行中允许收起到后台；其余阶段关闭弹窗直接清理本轮记录，避免页面残留。
    if (phaseRef.current === 'running') { setOpen(false); return }
    reset()
    setOpen(false)
  }
  return { open, setOpen, close, results, phase,
    installing: phase === 'running',
    running: phase === 'running' || phase === 'preparing',
    backgroundVisible: !open && (phase === 'running' || phase === 'done') && results.length > 0,
    addFiles, start, reset,
    show: () => { if (disabled && phaseRef.current !== 'running') return; if (phaseRef.current === 'done') reset(); setOpen(true) },
    remove: (key: string) => { if (!lock.current) { rows.current = rows.current.filter(item => item.key !== key); publish() } } }
}

export function PluginFileInstallProgress({ model, t }: { model: ReturnType<typeof usePluginFileInstall>; t: PluginTranslate }) {
  if (!model.backgroundVisible) return null
  const { done, total, succeeded, failed, warnings, current, unconfirmed } = summarizeFileInstall(model.results)
  const running = model.installing
  return (
    <PluginTaskProgressBar
      running={running}
      title={running ? t('正在安装本地插件包…') : unconfirmed ? t('本批已暂停，请先确认插件状态，再处理剩余文件。') : t('本地安装结果')}
      done={done}
      total={total}
      detail={
        current
          ? current.preview?.name || current.file.name
          : !running
            ? t('完成 {done} 个 · 失败 {failed} 个 · 需检查 {warnings} 个', { done: succeeded, failed, warnings })
            : undefined
      }
      progressLabel={t('插件安装进度')}
      onInspect={() => model.setOpen(true)}
      onClose={running ? undefined : model.reset}
      t={t}
    />
  )
}

type FileFilter = 'all' | 'ready' | 'installed' | 'failed'
const isPendingFile = (item: PluginFileResult) => item.state === 'ready' || item.state === 'installing'
const isInstalledFile = (item: PluginFileResult) => item.state === 'installed' || item.state === 'warning'
const isFailedFile = (item: PluginFileResult) => item.state === 'failed' || item.state === 'invalid' || item.state === 'unconfirmed'

type PendingStartAction = 'install' | 'retry'

export function PluginFileInstallDialog({ model, maxPackBytes, onOpenScheduledTasks, t }: { model: ReturnType<typeof usePluginFileInstall>; maxPackBytes: number; onOpenScheduledTasks?(): void; t: PluginTranslate }) {
  const input = React.useRef<HTMLInputElement>(null)
  const rowsRef = React.useRef<HTMLDivElement>(null)
  const cancelConfirmRef = React.useRef<HTMLButtonElement>(null)
  const [dragging, setDragging] = React.useState(false)
  const [confirmAction, setConfirmAction] = React.useState<PendingStartAction | null>(null)
  const [filter, setFilter] = React.useState<FileFilter>('all')
  const { ready, failed, succeeded, done, total, skipped, current, unconfirmed } = summarizeFileInstall(model.results)
  const installedTaskPlugins = model.results.some(item => item.state === 'installed' && item.preview?.kind === 'task_script')
  const pendingCount = model.results.filter(isPendingFile).length
  const installedCount = model.results.filter(isInstalledFile).length
  const failedCount = model.results.filter(isFailedFile).length
  React.useEffect(() => { if (!model.results.length) setFilter('all') }, [model.results.length])
  React.useEffect(() => { if (filter === 'failed' && failedCount === 0) setFilter('all') }, [filter, failedCount])
  React.useEffect(() => {
    if (!model.open || model.running || !model.results.length) setConfirmAction(null)
  }, [model.open, model.running, model.results.length])
  const visibleResults = filter === 'ready'
    ? model.results.filter(isPendingFile)
    : filter === 'installed'
      ? model.results.filter(isInstalledFile)
      : filter === 'failed'
        ? model.results.filter(isFailedFile)
        : model.results
  const requestStart = (action: PendingStartAction) => {
    const targetState: FileState = action === 'retry' ? 'failed' : 'ready'
    const hasUnsigned = model.results.some(item => item.state === targetState && item.preview?.signed === false)
    if (hasUnsigned) {
      setConfirmAction(action)
      return
    }
    void model.start(action === 'retry')
  }
  const confirmAndStart = () => {
    if (!confirmAction) return
    const retry = confirmAction === 'retry'
    setConfirmAction(null)
    void model.start(retry)
  }
  const labels: Record<FileState, string> = { preparing: t('正在校验包内容…'), ready: t('待安装'), installing: t('正在安装…'), installed: t('已安装'), warning: t('已安装，需检查'), unconfirmed: t('结果待确认'), failed: t('安装失败'), invalid: t('无法安装'), duplicate: t('已跳过') }
  const filterOptions: Array<{ value: FileFilter; label: string; count: number }> = [
    { value: 'all', label: t('全部'), count: model.results.length },
    { value: 'ready', label: t('待安装'), count: pendingCount },
    { value: 'installed', label: t('已安装'), count: installedCount },
    ...(failedCount > 0 ? [{ value: 'failed' as const, label: t('失败'), count: failedCount }] : []),
  ]
  return (
    <PluginTaskDialog
      open={model.open}
      confirmOpen={confirmAction !== null}
      onOpenChange={model.setOpen}
      onClose={model.close}
      hasRows={model.results.length > 0}
      title={model.phase === 'done' ? t('本地安装结果') : t('批量安装插件包')}
      subtitle={t('先确认文件清单，再逐个校验并安装')}
      rowsRef={rowsRef}
      top={
        model.phase === 'ready' || model.phase === 'preparing' ? (
          <>
            <Input
              type='file'
              ref={input}
              multiple
              accept='.zip,application/zip'
              className='hidden'
              onChange={event => {
                const files = Array.from(event.currentTarget.files || [])
                event.currentTarget.value = ''
                void model.addFiles(files)
              }}
            />
            <Button
              variant='outline'
              className={`plugins-file-drop ${dragging ? 'is-dragging' : ''}`}
              disabled={model.running}
              onClick={() => input.current?.click()}
              onDragOver={event => { event.preventDefault(); setDragging(true) }}
              onDragLeave={() => setDragging(false)}
              onDrop={event => {
                event.preventDefault()
                setDragging(false)
                void model.addFiles(Array.from(event.dataTransfer.files))
              }}
            >
              <svg viewBox='0 0 24 24' className='size-6 shrink-0 text-muted-foreground' fill='none' stroke='currentColor' strokeWidth='1.7' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'><path d='M12 16V4m0 0L7.5 8.5M12 4l4.5 4.5M4.5 15v4.5h15V15' /></svg>
              <span>
                <span className='block text-sm'>{t('拖入插件包，或选择文件')}</span>
                <span className='mt-1 block text-xs font-normal text-muted-foreground'>{t('支持多个 zip · 单个不超过 {n} MiB', { n: maxPackBytes / MIB })}</span>
              </span>
            </Button>
          </>
        ) : null
      }
      filterBar={
        model.results.length ? (
          <SegmentedControl<FileFilter>
            aria-label={t('按状态筛选')}
            className={DIALOG_SEG_CLASS}
            value={filter}
            onValueChange={next => {
              setFilter(next)
              if (rowsRef.current) rowsRef.current.scrollTop = 0
            }}
            options={filterOptions}
          />
        ) : null
      }
      listHeader={
        <>
          {t('已选 {count} 个文件', { count: model.results.length })}
          {skipped ? ` · ${t('跳过 {count} 个', { count: skipped })}` : ''}
        </>
      }
      listHeaderAction={
        !model.running && model.phase !== 'done' && model.results.length ? (
          <Button variant='link' size='xs' onClick={model.reset}>{t('清空列表')}</Button>
        ) : null
      }
      belowList={
        <>
          {unconfirmed ? <Alert variant='warning' className='shrink-0'>{t('本批已暂停，请先确认插件状态，再处理剩余文件。')}</Alert> : null}
          <p className='m-0 shrink-0 text-xs leading-5 text-muted-foreground'>{t('单项失败不会中断其余插件。账号和原有设置均保留。')}</p>
        </>
      }
      footerStatus={
        model.phase === 'running' ? (
          <>
            <p className='mb-2'>{current?.preview?.name || current?.file.name} · {done}/{total}</p>
            <Progress value={done} max={Math.max(1, total)} aria-label={t('插件安装进度')} />
          </>
        ) : model.phase === 'done'
          ? t('安装结束：{done} 个完成，{failed} 个失败', { done: succeeded, failed })
          : model.phase === 'preparing'
            ? t('正在校验包内容…')
            : t('待安装 {count} 个插件包', { count: ready })
      }
      footerActions={
        <>
          <Button variant='outline' onClick={model.close}>{model.installing ? t('收起') : model.phase === 'done' ? t('完成') : t('取消')}</Button>
          {model.phase === 'done' && installedTaskPlugins && onOpenScheduledTasks ? <Button variant='outline' onClick={onOpenScheduledTasks}>{t('前往定时任务')}</Button> : null}
          {!model.running && failed && !unconfirmed ? <Button variant='outline' onClick={() => requestStart('retry')}>{t('重试失败项')}</Button> : null}
          {!model.running && ready && !unconfirmed ? <Button onClick={() => requestStart('install')}>{t('开始安装（{count}）', { count: ready })}</Button> : null}
        </>
      }
      nestedDialog={
        <AlertDialog open={confirmAction !== null} onOpenChange={open => { if (!open) setConfirmAction(null) }}>
          <AlertDialogContent overlayForceRender className='text-foreground' initialFocus={cancelConfirmRef}>
            <AlertDialogHeader>
              <AlertDialogTitle>{t('确认安装未签名插件包')}</AlertDialogTitle>
            </AlertDialogHeader>
            <AlertDialogBody>
              <AlertDialogDescription className='m-0 text-[13px] leading-relaxed text-foreground'>
                {t('我信任本次所选的未签名插件包，并确认继续安装。')}
              </AlertDialogDescription>
            </AlertDialogBody>
            <AlertDialogFooter>
              <Button ref={cancelConfirmRef} variant='outline' onClick={() => setConfirmAction(null)}>{t('取消')}</Button>
              <Button onClick={confirmAndStart}>{t('继续安装')}</Button>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      }
    >
      {visibleResults.map(item => (
        <div key={item.key} className='plugins-file-row'>
          <svg viewBox='0 0 24 24' className='size-5 shrink-0 text-muted-foreground' fill='none' stroke='currentColor' strokeWidth='1.6' aria-hidden='true'><path d='M6 3h7l5 5v13H6V3ZM13 3v5h5' /></svg>
          <div className='min-w-0 flex-1'>
            <p className='m-0 flex flex-wrap items-center gap-2 text-sm'>
              {item.preview?.name || item.file.name}
              {item.preview?.kind === 'task_script' ? <Badge variant='secondary' shape='tag'>{t('定时任务')}</Badge> : null}
              {item.preview ? <Badge variant={item.preview.signed ? 'outline' : 'warning'} shape='tag'>{item.preview.signed ? t('已签名') : t('未签名')}</Badge> : null}
            </p>
            <p className='m-0 mt-1 break-all text-xs text-muted-foreground'>
              {item.preview && !item.preview.previousVersion ? `${t('版本')} ${item.preview.version} · ` : ''}{item.file.name}
            </p>
            {item.preview?.previousVersion ? <p className='m-0 mt-1 text-xs text-muted-foreground'>{t('已装版本 {current} · 文件版本 {next}', { current: item.preview.previousVersion, next: item.preview.version })}</p> : null}
            {item.preview?.enabled === false ? <p className='m-0 mt-1 text-xs text-muted-foreground'>{t('安装后保留原有停用状态')}</p> : null}
            {item.preview?.kind === 'task_script' && item.state === 'installed' ? <p className='m-0 mt-1 text-xs text-muted-foreground'>{t('已安装至「定时任务 → 任务插件」')}</p> : null}
            {item.error ? <p className={`m-0 mt-1 break-words text-xs ${item.state === 'failed' || item.state === 'invalid' ? 'text-destructive' : 'text-warning'}`}>{item.error}</p> : null}
          </div>
          <div className='plugins-file-state'>
            <span className={item.state === 'installed' ? 'text-success' : item.state === 'failed' || item.state === 'invalid' ? 'text-destructive' : item.state === 'unconfirmed' || item.state === 'warning' ? 'text-warning' : 'text-muted-foreground'}>
              {item.state === 'preparing' || item.state === 'installing' ? <Spinner className='inline-block size-3' /> : null} {labels[item.state]}
            </span>
            <span className='mt-1 block text-muted-foreground'>{formatSize(item.file.size)}</span>
          </div>
          {!model.running && model.phase !== 'done' ? (
            <PluginTaskRemoveButton label={t('移除文件 {name}', { name: item.file.name })} onClick={() => model.remove(item.key)} />
          ) : null}
        </div>
      ))}
      {!model.results.length ? (
        <p className='m-0 py-8 text-center text-xs text-muted-foreground'>{t('选择插件包后，文件清单会显示在这里')}</p>
      ) : !visibleResults.length ? (
        <p className='m-0 py-8 text-center text-xs text-muted-foreground'>{t('没有符合筛选条件的插件')}</p>
      ) : null}
    </PluginTaskDialog>
  )
}
