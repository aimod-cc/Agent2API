import * as React from 'react'
import { Alert, Button, Checkbox, Label, Popover, PopoverContent, PopoverTitle, PopoverTrigger, Progress, SegmentedControl, Spinner } from '@ui'
import { PluginArtwork } from './plugin-artwork'
import { applicablePluginItems, type PluginBatchAction, type PluginBatchItem } from './plugins-batch-items'
import type { PluginBatchModel, PluginBatchResult } from './plugins-batch-runner'
import type { PluginTranslate } from './plugins-store'
import { DIALOG_SEG_CLASS } from './dialog-segmented'
import { PluginTaskDialog, PluginTaskProgressBar, PluginTaskRemoveButton, ScriptTrustConfirmDialog } from './plugins-task-dialog'

const formatSize = (bytes: number) => bytes >= 1048576 ? `${(bytes / 1048576).toFixed(1)} MiB` : `${Math.max(1, Math.round(bytes / 1024))} KiB`

function formatPluginVersion(value?: string | null): string {
  const trimmed = value?.trim() || ''
  if (!trimmed) return ''
  if (trimmed.startsWith('v')) return trimmed
  const isVersion = /^\d+\.\d+/.test(trimmed) && !/^[0-9a-f]{7,40}$/i.test(trimmed)
  return isVersion ? `v${trimmed}` : trimmed
}

function itemVersionDetail(action: PluginBatchAction, item: PluginBatchItem, t: PluginTranslate): string {
  const current = formatPluginVersion(item.version)
  const latest = formatPluginVersion(item.latest)
  if (action === 'update') {
    if (current && latest) return `${current} → ${latest}`
    if (latest) return latest
    if (current) return `${current} → ${t('最新版本')}`
    return t('最新版本')
  }
  if (action === 'install') return current || t('最新版本')
  return current
}

export function pluginBatchVerb(action: PluginBatchAction, t: PluginTranslate): string {
  if (action === 'install') return t('安装')
  if (action === 'update') return t('更新')
  if (action === 'uninstall') return t('卸载')
  return action === 'enable' ? t('启用') : t('停用')
}

export function PluginBatchToolbar({ items, visibleKeys, selected, disabled, onSelection, onAction, onExit, t }: {
  items: PluginBatchItem[]; visibleKeys: string[]; selected: Set<string>; disabled: boolean
  onSelection(keys: string[], checked: boolean): void; onAction(action: PluginBatchAction): void; onExit(): void; t: PluginTranslate
}) {
  const [open, setOpen] = React.useState(false)
  const visibleSet = React.useMemo(() => new Set(visibleKeys), [visibleKeys])
  const picked = items.filter(item => selected.has(item.key))
  const count = visibleKeys.filter(key => selected.has(key)).length
  const installed = picked.filter(item => item.installed).length
  const updates = applicablePluginItems(picked, 'update').length
  return <div className='plugins-selection-bar' role='region' aria-label={t('批量管理')}>
    <Label className='flex shrink-0 items-center gap-3 text-xs font-normal'>
      <Checkbox className='size-4' disabled={disabled || !visibleKeys.length} checked={count > 0 && count === visibleKeys.length}
        indeterminate={count > 0 && count < visibleKeys.length} aria-label={t('全选当前结果')}
        onCheckedChange={value => onSelection(visibleKeys, value === true)} />
      {t('已选 {count} 个插件', { count: picked.length })}
    </Label>
    <span className='plugins-selection-summary'>{t('{installed} 已安装 · {missing} 未安装', { installed, missing: picked.length - installed })}</span>
    <div className='plugins-selection-actions'>
      {(['install', 'update', 'uninstall'] as const).map(action => {
        const targets = applicablePluginItems(picked, action)
        if (!targets.length) return null
        const title = action === 'install' ? t('安装所选（{count}）', { count: targets.length })
          : action === 'update' ? t('批量更新（{count}）', { count: targets.length }) : t('批量卸载（{count}）', { count: targets.length })
        return <Button key={action} size='sm' disabled={disabled} variant={action === 'update' ? 'default' : 'outline'}
          className={action === 'uninstall' ? 'text-destructive hover:text-destructive' : undefined} onClick={() => onAction(action)}>{title}</Button>
      })}
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger render={<Button variant='outline' size='sm' disabled={disabled} />}>{t('更多')}</PopoverTrigger>
        <PopoverContent align='end' className='w-44 p-1.5'>
          <PopoverTitle className='sr-only'>{t('更多批量操作')}</PopoverTitle>
          {(['enable', 'disable'] as const).map(action => <Button key={action} variant='ghost' className='w-full justify-start' size='sm'
            disabled={!applicablePluginItems(picked, action).length} onClick={() => { setOpen(false); onAction(action) }}>
            {action === 'enable' ? t('批量启用') : t('批量停用')}
          </Button>)}
          <Button variant='ghost' size='sm' className='w-full justify-start' disabled={!updates && !items.some(item => visibleSet.has(item.key) && item.updatable)}
            onClick={() => { setOpen(false); onSelection([...selected], false); onSelection(items.filter(item => visibleSet.has(item.key) && item.updatable && !item.active).map(item => item.key), true) }}>{t('仅选可更新')}</Button>
          <Button variant='ghost' size='sm' className='w-full justify-start' onClick={() => { setOpen(false); onSelection([...selected], false) }}>{t('取消选择')}</Button>
        </PopoverContent>
      </Popover>
      <Button variant='ghost' size='sm' disabled={disabled} onClick={onExit}>{t('退出')}</Button>
    </div>
  </div>
}

type BatchFilter = 'all' | 'pending' | 'done' | 'failed'
type PendingBatchStart = 'start' | 'retry'

const isPendingResult = (row: PluginBatchResult) => row.state === 'waiting' || row.state === 'working'
const isDoneResult = (row: PluginBatchResult) => row.state === 'succeeded' || row.state === 'warning'
const isFailedResult = (row: PluginBatchResult) => row.state === 'failed' || row.state === 'unconfirmed'

function summarizeBatch(results: PluginBatchResult[]) {
  const waiting = results.filter(row => row.state === 'waiting').length
  const failed = results.filter(row => row.state === 'failed').length
  const warnings = results.filter(row => row.state === 'warning' || row.state === 'unconfirmed').length
  const succeeded = results.filter(isDoneResult).length
  const completed = results.filter(row => !isPendingResult(row)).length
  const current = results.find(row => row.state === 'working')
  const unconfirmed = results.some(row => row.state === 'unconfirmed')
  return { waiting, failed, warnings, succeeded, completed, total: results.length, current, unconfirmed }
}

function batchDialogTitle(action: PluginBatchAction, done: boolean, t: PluginTranslate): string {
  if (done) {
    if (action === 'update') return t('插件更新结果')
    if (action === 'install') return t('批量安装结果')
    return t('批量{action}结束', { action: pluginBatchVerb(action, t) })
  }
  if (action === 'update') return t('批量更新插件')
  if (action === 'install') return t('批量安装插件')
  if (action === 'uninstall') return t('批量卸载插件')
  if (action === 'enable') return t('批量启用插件')
  return t('批量停用插件')
}

function batchDialogSubtitle(action: PluginBatchAction, t: PluginTranslate): string {
  if (action === 'update') return t('先确认更新清单，再逐个下载并更新')
  if (action === 'install') return t('先确认安装清单，再逐个下载并安装')
  return t('先确认插件清单，再逐个执行操作')
}

function batchStatusLabel(action: PluginBatchAction, row: PluginBatchResult, t: PluginTranslate): string {
  if (row.state === 'waiting') {
    if (action === 'update') return t('待更新')
    if (action === 'install') return t('待安装')
    return t('待处理')
  }
  if (row.state === 'working') {
    if (row.message) return row.message
    if (action === 'update') return t('正在更新…')
    if (action === 'install') return t('正在安装…')
    return t('正在处理…')
  }
  if (row.state === 'succeeded') {
    if (action === 'update') return t('已更新')
    if (action === 'install') return t('已安装')
    return t('完成')
  }
  if (row.state === 'warning') return t('已完成，需检查')
  if (row.state === 'unconfirmed') return t('结果待确认')
  if (action === 'update') return t('更新失败')
  if (action === 'install') return t('安装失败')
  return t('操作失败')
}

export function PluginBatchDialog({ model, disabled, t }: {
  model: PluginBatchModel
  disabled: boolean
  t: PluginTranslate
}) {
  const rowsRef = React.useRef<HTMLDivElement>(null)
  const [confirmAction, setConfirmAction] = React.useState<PendingBatchStart | null>(null)
  const [filter, setFilter] = React.useState<BatchFilter>('all')
  const task = model.task
  const results = task?.results ?? []
  const failedCount = results.filter(isFailedResult).length

  React.useEffect(() => {
    if (!model.open || !results.length) setFilter('all')
  }, [model.open, results.length])
  React.useEffect(() => {
    if (filter === 'failed' && failedCount === 0) setFilter('all')
  }, [filter, failedCount])
  React.useEffect(() => {
    if (!model.open || task?.running || !results.length) setConfirmAction(null)
  }, [model.open, task?.running, results.length])

  if (!task) return null

  const { waiting, failed, warnings, succeeded, completed, total, current, unconfirmed } = summarizeBatch(results)
  const action = task.action
  const verb = pluginBatchVerb(action, t)
  const destructive = action === 'uninstall' || action === 'disable'
  const accounts = results.reduce((sum, row) => sum + row.item.accounts, 0)
  const restoredAccounts = action === 'install'
    ? results.filter(row => row.state === 'succeeded' && row.item.required).reduce((sum, row) => sum + row.item.accounts, 0)
    : 0
  const pendingCount = results.filter(isPendingResult).length
  const doneCount = results.filter(isDoneResult).length
  const visibleResults = filter === 'pending'
    ? results.filter(isPendingResult)
    : filter === 'done'
      ? results.filter(isDoneResult)
      : filter === 'failed'
        ? results.filter(isFailedResult)
        : results

  const requestStart = (kind: PendingBatchStart) => {
    const targetState = kind === 'retry' ? 'failed' : 'waiting'
    const needsScriptConfirm = (action === 'install' || action === 'update')
      && results.some(row => row.state === targetState && row.item.kind === 'script')
    if (needsScriptConfirm) {
      setConfirmAction(kind)
      return
    }
    void model.start(kind === 'retry')
  }

  const confirmAndStart = () => {
    if (!confirmAction) return
    const retry = confirmAction === 'retry'
    setConfirmAction(null)
    void model.start(retry)
  }

  const pendingLabel = action === 'update' ? t('待更新') : action === 'install' ? t('待安装') : t('待处理')
  const doneLabel = action === 'update' ? t('已更新') : action === 'install' ? t('已安装') : t('已完成')
  const filterOptions: Array<{ value: BatchFilter; label: string; count: number }> = [
    { value: 'all', label: t('全部'), count: results.length },
    { value: 'pending', label: pendingLabel, count: pendingCount },
    { value: 'done', label: doneLabel, count: doneCount },
    ...(failedCount > 0 ? [{ value: 'failed' as const, label: t('失败'), count: failedCount }] : []),
  ]

  return (
    <PluginTaskDialog
      open={model.open}
      confirmOpen={confirmAction !== null}
      onOpenChange={model.setOpen}
      onClose={model.close}
      hasRows={results.length > 0}
      title={batchDialogTitle(action, task.phase === 'done', t)}
      subtitle={batchDialogSubtitle(action, t)}
      rowsRef={rowsRef}
      top={
        destructive ? (
          <Alert variant='warning' className='shrink-0'>
            {t('对应提供商将停止转发。账号、凭据和原有设置均保留，重新安装或启用并成功加载后恢复。')}
          </Alert>
        ) : null
      }
      filterBar={
        results.length ? (
          <SegmentedControl<BatchFilter>
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
      listHeader={t('已选 {count} 个插件', { count: results.length })}
      belowList={
        <>
          {unconfirmed ? (
            <Alert variant='warning' className='shrink-0'>{t('本批操作已暂停，请先刷新插件状态')}</Alert>
          ) : null}
          <p className='m-0 shrink-0 text-xs leading-5 text-muted-foreground'>
            {destructive && accounts
              ? t('保留 {count} 个账号的数据', { count: accounts })
              : t('单项失败不会中断其余插件。账号和原有设置均保留。')}
          </p>
        </>
      }
      footerStatus={
        task.running ? (
          <>
            <p className='mb-2'>{current?.item.name || ''} · {completed}/{total}</p>
            <Progress value={completed} max={Math.max(1, total)} aria-label={t('插件批量操作进度')} />
          </>
        ) : task.phase === 'done' ? (
          restoredAccounts && !failed
            ? t('{count} 个账号所需插件已就绪', { count: restoredAccounts })
            : t('完成 {done} 个 · 失败 {failed} 个 · 需检查 {warnings} 个', { done: succeeded, failed, warnings })
        ) : (
          t('已选 {count} 个插件', { count: waiting })
        )
      }
      footerActions={
        <>
          <Button variant='outline' onClick={model.close}>
            {task.running ? t('收起') : task.phase === 'done' ? t('完成') : t('取消')}
          </Button>
          {!task.running && failed > 0 && !unconfirmed ? (
            <Button variant='outline' disabled={disabled} onClick={() => requestStart('retry')}>
              {t('重试失败项')}
            </Button>
          ) : null}
          {task.phase === 'ready' && waiting > 0 && !unconfirmed ? (
            <Button variant={destructive ? 'destructive' : 'default'} disabled={disabled} onClick={() => requestStart('start')}>
              {t('开始{action}（{count}）', { action: verb, count: waiting })}
            </Button>
          ) : null}
        </>
      }
      nestedDialog={
        <ScriptTrustConfirmDialog
          open={confirmAction !== null}
          action={action === 'update' ? 'update' : 'install'}
          onCancel={() => setConfirmAction(null)}
          onConfirm={confirmAndStart}
          t={t}
        />
      }
    >
      {visibleResults.map(row => {
        const { item } = row
        const versionDetail = itemVersionDetail(action, item, t)
        const stateClass = row.state === 'succeeded'
          ? 'text-success'
          : row.state === 'failed'
            ? 'text-destructive'
            : row.state === 'warning' || row.state === 'unconfirmed'
              ? 'text-warning'
              : 'text-muted-foreground'
        const secondaryMetric = row.state === 'working' && typeof row.percent === 'number'
          ? `${Math.round(row.percent)}%`
          : item.size
            ? formatSize(item.size)
            : ''
        return (
          <div key={item.key} className='plugins-file-row'>
            <PluginArtwork src={item.icon} />
            <div className='min-w-0 flex-1'>
              <p className='m-0 text-sm'>{item.name}</p>
              <p className='m-0 mt-1 text-xs text-muted-foreground'>
                {item.sourceLabel}{versionDetail ? ` · ${versionDetail}` : ''}
              </p>
              {item.kind === 'script' && item.source ? (
                <p className='m-0 mt-1 break-all text-xs text-muted-foreground'>{item.source}</p>
              ) : null}
              {destructive && item.accounts ? (
                <p className='m-0 mt-1 text-xs text-warning'>{t('{count} 个账号受影响', { count: item.accounts })}</p>
              ) : null}
              {action === 'install' && item.required && item.accounts ? (
                <p className='m-0 mt-1 text-xs text-warning'>{t('{count} 个账号等待恢复', { count: item.accounts })}</p>
              ) : null}
              {row.message && (row.state === 'failed' || row.state === 'warning' || row.state === 'unconfirmed') ? (
                <p className={`m-0 mt-1 break-words text-xs ${row.state === 'failed' ? 'text-destructive' : 'text-warning'}`}>{row.message}</p>
              ) : null}
              {row.message && row.state === 'waiting' ? (
                <p className='m-0 mt-1 break-words text-xs text-muted-foreground'>{row.message}</p>
              ) : null}
            </div>
            <div className='plugins-file-state'>
              <span className={stateClass}>
                {row.state === 'working' ? <Spinner className='inline-block size-3' /> : null}{' '}
                {batchStatusLabel(action, row, t)}
              </span>
              {secondaryMetric ? <span className='mt-1 block tabular-nums text-muted-foreground'>{secondaryMetric}</span> : null}
            </div>
            {task.phase === 'ready' && !disabled ? (
              <PluginTaskRemoveButton
                label={t('移除插件 {name}', { name: item.name })}
                onClick={() => model.remove(item.key)}
              />
            ) : null}
          </div>
        )
      })}
      {!results.length ? (
        <p className='m-0 py-8 text-center text-xs text-muted-foreground'>{t('没有可处理的插件')}</p>
      ) : !visibleResults.length ? (
        <p className='m-0 py-8 text-center text-xs text-muted-foreground'>{t('没有符合筛选条件的插件')}</p>
      ) : null}
    </PluginTaskDialog>
  )
}

export function PluginBatchProgress({ model, t }: { model: PluginBatchModel; t: PluginTranslate }) {
  if (!model.backgroundVisible || !model.task) return null
  const task = model.task
  const { completed, total, succeeded, failed, warnings, current, unconfirmed } = summarizeBatch(task.results)
  const verb = pluginBatchVerb(task.action, t)
  const restoredAccounts = task.action === 'install'
    ? task.results.filter(row => row.state === 'succeeded' && row.item.required).reduce((sum, row) => sum + row.item.accounts, 0)
    : 0
  return (
    <PluginTaskProgressBar
      running={task.running}
      title={
        task.running
          ? t('正在批量{action}插件', { action: verb })
          : unconfirmed
            ? t('本批操作已暂停，请先刷新插件状态')
            : t('批量{action}结束', { action: verb })
      }
      done={completed}
      total={total}
      detail={
        current
          ? `${current.item.name}${current.message ? ` · ${current.message}` : ''}`
          : restoredAccounts && !failed
            ? t('{count} 个账号所需插件已就绪', { count: restoredAccounts })
            : t('完成 {done} 个 · 失败 {failed} 个 · 需检查 {warnings} 个', { done: succeeded, failed, warnings })
      }
      progressLabel={t('插件批量操作进度')}
      onInspect={() => model.setOpen(true)}
      onClose={task.running ? undefined : model.clear}
      t={t}
    />
  )
}
