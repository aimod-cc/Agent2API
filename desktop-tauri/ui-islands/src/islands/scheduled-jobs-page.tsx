import * as React from 'react'
import { createRoot } from 'react-dom/client'
import {
  Alert,
  Badge,
  Button,
  Card,
  CardContent,
  Empty,
  EmptyDescription,
  EmptyTitle,
  InputGroup,
  InputGroupAddon,
  InputGroupInput,
  SegmentedControl,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Spinner,
  Switch,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@ui'
import { mountOnPage } from '../island-lazy'
import { t } from '../i18n'
import { api, bridge, errorMessage, toast } from './plugins-shared'
import { TaskPluginsSection } from '../../../ui-shared/task-plugins-section'
import type { PluginStatusFilter } from '../../../ui-shared/plugins-filters'
import {
  defaultValuesForPlugin,
  templateLoginCheckin,
  type SandboxOutcome,
  type TaskActionDef,
  type TaskJob,
  type TaskJobRun,
  type TaskJobsPayload,
  type TaskPlugin,
  type TaskPluginsPayload,
  type TaskProxyOption,
} from '../../../ui-shared/task-plugins-types'
import { TaskPluginEditorDialog } from '../../../ui-shared/task-plugin-editor'
import {
  formatTaskDateTime,
  formatTaskDuration,
  formatTaskProxyLabel,
  TaskJobConfigDialog,
  TaskJobDeleteDialog,
  TaskJobHistorySheet,
  type TaskJobDraft,
} from '../../../ui-shared/task-job-dialogs'

function createDraftFromPlugin(plugin: TaskPlugin): TaskJobDraft {
  return {
    id: null,
    pluginId: plugin.id,
    name: plugin.name,
    cronExpression: plugin.defaultCron || '0 9 * * *',
    timezone: plugin.defaultTimezone || 'Asia/Shanghai',
    timeoutSeconds: plugin.defaultTimeout || 60,
    proxyId: '',
    enabled: true,
    values: defaultValuesForPlugin(plugin),
    savedSecrets: {},
    clearSecrets: [],
    statusSummary: {},
  }
}

function createDraftFromJob(job: TaskJob, plugin?: TaskPlugin): TaskJobDraft {
  const base = plugin ? defaultValuesForPlugin(plugin) : {}
  return {
    id: job.id,
    pluginId: job.pluginId,
    name: job.name,
    cronExpression: job.cronExpression,
    timezone: job.timezone,
    timeoutSeconds: job.timeoutSeconds,
    proxyId: job.proxyId || '',
    enabled: job.enabled,
    values: { ...base, ...job.values },
    savedSecrets: { ...job.savedSecrets },
    clearSecrets: [],
    statusSummary: { ...job.statusSummary },
  }
}

function ScheduledJobsPage(): React.ReactElement {
  const [activeTab, setActiveTab] = React.useState<'jobs' | 'plugins'>('jobs')
  const [pluginStatus, setPluginStatus] = React.useState<PluginStatusFilter>('all')
  const [pluginQuery, setPluginQuery] = React.useState('')
  const [taskPayload, setTaskPayload] = React.useState<TaskPluginsPayload | null>(null)
  const [plugins, setPlugins] = React.useState<TaskPlugin[]>([])
  const [jobs, setJobs] = React.useState<TaskJob[]>([])
  const [proxies, setProxies] = React.useState<TaskProxyOption[]>([])
  const [loaded, setLoaded] = React.useState(false)
  const [error, setError] = React.useState('')
  const [search, setSearch] = React.useState('')
  const [pluginFilter, setPluginFilter] = React.useState('all')
  const [importRequest, setImportRequest] = React.useState(0)
  const [busyJobId, setBusyJobId] = React.useState('')

  const [draft, setDraft] = React.useState<TaskJobDraft | null>(null)
  const [saving, setSaving] = React.useState(false)
  const [actionBusyId, setActionBusyId] = React.useState('')
  const [actionOutcome, setActionOutcome] = React.useState<SandboxOutcome | null>(null)
  const [cronPreview, setCronPreview] = React.useState<{ ok: boolean; text: string }>({ ok: true, text: '' })

  const [historyJob, setHistoryJob] = React.useState<TaskJob | null>(null)
  const [runs, setRuns] = React.useState<TaskJobRun[]>([])
  const [runsLoading, setRunsLoading] = React.useState(false)

  const [deleteTarget, setDeleteTarget] = React.useState<TaskJob | null>(null)
  const [editorOpen, setEditorOpen] = React.useState(false)
  const [editorPlugin, setEditorPlugin] = React.useState<TaskPlugin | null>(null)
  const [editorIsNew, setEditorIsNew] = React.useState(true)

  const mounted = React.useRef(true)
  const pendingPluginOpen = React.useRef<string | null>(null)

  const load = React.useCallback(async () => {
    try {
      const [pluginsRes, jobsRes] = await Promise.all([
        api<TaskPluginsPayload>('GET', '/api/task-plugins'),
        api<TaskJobsPayload>('GET', '/api/task-jobs'),
      ])
      if (!mounted.current) return
      setTaskPayload(pluginsRes)
      setPlugins(pluginsRes.plugins || [])
      setJobs(jobsRes.jobs || [])
      setProxies(jobsRes.proxies || [])
      setError('')
      setLoaded(true)
      if (pendingPluginOpen.current) {
        const targetId = pendingPluginOpen.current
        pendingPluginOpen.current = null
        const target = (pluginsRes.plugins || []).find(item => item.id === targetId && item.enabled)
        if (target) {
          setActionOutcome(null)
          setDraft(createDraftFromPlugin(target))
        }
      }
    } catch (cause) {
      if (mounted.current) {
        setError(errorMessage(cause))
        setLoaded(true)
      }
    }
  }, [])

  React.useEffect(() => {
    mounted.current = true
    void load()
    return () => { mounted.current = false }
  }, [load])

  const hasRunning = jobs.some(job => job.lastStatus === 'running')
  React.useEffect(() => {
    if (!hasRunning) return
    const timer = window.setInterval(() => {
      if (!document.hidden && bridge().wbApp?.currentPage === 'scheduled') void load()
    }, 2000)
    return () => window.clearInterval(timer)
  }, [hasRunning, load])

  const openCreateForPlugin = React.useCallback((pluginId: string) => {
    setActiveTab('jobs')
    const found = plugins.find(item => item.id === pluginId && item.enabled)
    if (found) {
      setActionOutcome(null)
      setDraft(createDraftFromPlugin(found))
    } else {
      pendingPluginOpen.current = pluginId
      void load()
    }
  }, [plugins, load])

  React.useEffect(() => {
    currentLoad = load
    currentOpenForPlugin = openCreateForPlugin
    if (queuedOpenForPlugin) {
      const queued = queuedOpenForPlugin
      queuedOpenForPlugin = null
      openCreateForPlugin(queued)
    }
    return () => {
      if (currentLoad === load) currentLoad = null
      if (currentOpenForPlugin === openCreateForPlugin) currentOpenForPlugin = null
    }
  }, [load, openCreateForPlugin])

  React.useEffect(() => {
    if (!draft) return
    const expr = draft.cronExpression.trim()
    const tz = draft.timezone || 'Asia/Shanghai'
    if (!expr) {
      setCronPreview({ ok: false, text: t('请输入 5 段定时表达式') })
      return
    }
    const timer = window.setTimeout(async () => {
      try {
        const res = await api<{ valid: boolean; nextRunAt?: number | null; error?: string }>(
          'POST', '/api/task-jobs/cron-preview', { cronExpression: expr, timezone: tz },
        )
        if (res.valid) {
          setCronPreview({
            ok: true,
            text: res.nextRunAt ? t('下次执行：{time}', { time: formatTaskDateTime(res.nextRunAt) }) : t('表达式有效'),
          })
        } else {
          setCronPreview({ ok: false, text: res.error || t('定时表达式格式有误') })
        }
      } catch (cause) {
        setCronPreview({ ok: false, text: errorMessage(cause) })
      }
    }, 260)
    return () => window.clearTimeout(timer)
  }, [draft?.cronExpression, draft?.timezone])

  const pluginMap = React.useMemo(() => new Map(plugins.map(item => [item.id, item])), [plugins])
  const enabledPlugins = React.useMemo(() => plugins.filter(item => item.enabled), [plugins])
  const jobCounts = React.useMemo(() => {
    const map = new Map<string, number>()
    for (const job of jobs) map.set(job.pluginId, (map.get(job.pluginId) ?? 0) + 1)
    return map
  }, [jobs])
  const storeUninstalledCount = React.useMemo(
    () => (taskPayload?.store || []).filter(item => !item.installed).length,
    [taskPayload?.store],
  )
  const updatablePluginCount = React.useMemo(
    () => (taskPayload?.updates || []).length,
    [taskPayload?.updates],
  )

  const filteredJobs = React.useMemo(() => {
    const q = search.trim().toLowerCase()
    return jobs.filter(job => {
      if (pluginFilter !== 'all' && job.pluginId !== pluginFilter) return false
      if (!q) return true
      const plugin = pluginMap.get(job.pluginId)
      return `${job.name} ${plugin?.name || job.pluginId} ${job.cronExpression}`.toLowerCase().includes(q)
    })
  }, [jobs, pluginFilter, search, pluginMap])

  const openNewJob = () => {
    if (enabledPlugins.length === 0) {
      toast(t('请先在「任务插件」中安装或启用定时任务插件'), 'warn')
      setActiveTab('plugins')
      return
    }
    setActionOutcome(null)
    setDraft(createDraftFromPlugin(enabledPlugins[0]))
  }

  const persistDraft = async (current: TaskJobDraft): Promise<TaskJob> => {
    const payload = {
      pluginId: current.pluginId,
      name: current.name.trim() || pluginMap.get(current.pluginId)?.name || t('未命名任务'),
      cronExpression: current.cronExpression.trim(),
      timezone: current.timezone,
      timeoutSeconds: Number(current.timeoutSeconds) || 60,
      proxyId: current.proxyId,
      enabled: current.enabled,
      values: current.values,
      clearSecrets: current.clearSecrets,
    }
    const res = current.id
      ? await api<{ job: TaskJob }>('PUT', `/api/task-jobs/${encodeURIComponent(current.id)}`, payload)
      : await api<{ job: TaskJob }>('POST', '/api/task-jobs', payload)
    return res.job
  }

  const handleSaveDraft = async () => {
    if (!draft) return
    setSaving(true)
    try {
      await persistDraft(draft)
      toast(draft.id ? t('任务配置已保存') : t('已创建定时任务'), 'ok')
      setDraft(null)
      if (activeTab !== 'jobs') setActiveTab('jobs')
      await load()
    } catch (cause) {
      toast(errorMessage(cause), 'err')
    } finally {
      setSaving(false)
    }
  }

  const handleTriggerAction = async (action: TaskActionDef) => {
    if (!draft) return
    setActionBusyId(action.id)
    setActionOutcome(null)
    try {
      let current = draft
      if (!current.id) {
        const saved = await persistDraft(current)
        current = createDraftFromJob(saved, pluginMap.get(saved.pluginId))
        setDraft(current)
        void load()
      }
      const res = await api<{ outcome: SandboxOutcome; job?: TaskJob }>(
        'POST', `/api/task-jobs/${encodeURIComponent(current.id!)}/action`,
        { actionId: action.id, values: current.values, proxyId: current.proxyId },
      )
      setActionOutcome(res.outcome)
      setDraft(prev => prev ? {
        ...prev,
        values: res.outcome.patchForm ? { ...prev.values, ...res.outcome.patchForm } : prev.values,
        statusSummary: res.job?.statusSummary ?? (res.outcome.statusPatch ? { ...prev.statusSummary, ...res.outcome.statusPatch } : prev.statusSummary),
        savedSecrets: res.job?.savedSecrets ?? prev.savedSecrets,
      } : prev)
      await load()
    } catch (cause) {
      setActionOutcome({ success: false, durationMs: 0, output: '', error: errorMessage(cause) })
    } finally {
      setActionBusyId('')
    }
  }

  const runJobMutation = async (jobId: string, fn: () => Promise<void>) => {
    setBusyJobId(jobId)
    try { await fn() } catch (cause) { toast(errorMessage(cause), 'err') }
    finally { setBusyJobId('') }
  }

  const openHistory = async (job: TaskJob) => {
    setHistoryJob(job)
    setRunsLoading(true)
    try {
      const res = await api<{ runs: TaskJobRun[] }>('GET', `/api/task-jobs/${encodeURIComponent(job.id)}/runs?limit=50`)
      setRuns(res.runs || [])
    } catch (cause) {
      toast(errorMessage(cause), 'err')
      setRuns([])
    } finally {
      setRunsLoading(false)
    }
  }

  const openNewEditor = () => {
    setEditorPlugin(templateLoginCheckin())
    setEditorIsNew(true)
    setEditorOpen(true)
  }

  const draftPlugin = draft ? pluginMap.get(draft.pluginId) : undefined
  const selectedFilterLabel = pluginFilter === 'all' ? t('全部插件') : (pluginMap.get(pluginFilter)?.name || pluginFilter)

  return (
    <div className='flex flex-col gap-4'>
      <div className='flex flex-wrap items-center justify-between gap-2.5'>
        <div className='flex flex-wrap items-center gap-2'>
          <SegmentedControl<'jobs' | 'plugins'>
            aria-label={t('定时任务视图切换')}
            value={activeTab}
            onValueChange={setActiveTab}
            options={[
              { value: 'jobs', label: t('定时任务'), count: jobs.length > 0 ? jobs.length : undefined },
              { value: 'plugins', label: t('任务插件'), count: plugins.length > 0 ? plugins.length : undefined },
            ]}
          />
          {updatablePluginCount > 0 ? (
            <button
              type='button'
              className='cursor-pointer border-0 bg-transparent p-0'
              onClick={() => { setActiveTab('plugins'); setPluginStatus('updatable') }}
            >
              <Badge variant='warning' shape='tag' className='px-1.5 text-[10.5px]'>
                {t('{count} 可更新', { count: updatablePluginCount })}
              </Badge>
            </button>
          ) : null}
        </div>
        <div className='flex flex-wrap items-center gap-2'>
          {activeTab === 'jobs' ? (
            <>
              <InputGroup className='w-52 rounded-sm border-hairline bg-surface'>
                <InputGroupInput type='search' value={search} placeholder={t('搜索任务名称或插件')} aria-label={t('搜索任务名称或插件')}
                  onChange={e => setSearch(e.currentTarget.value)} />
                <InputGroupAddon><svg viewBox='0 0 16 16' className='size-3.5' fill='none' stroke='currentColor' strokeWidth='1.7' strokeLinecap='round' aria-hidden='true'><circle cx='6.8' cy='6.8' r='4.6' /><path d='M10.3 10.3 14 14' /></svg></InputGroupAddon>
              </InputGroup>
              {plugins.length > 0 ? (
                <Select value={pluginFilter} onValueChange={val => setPluginFilter(val ?? 'all')}>
                  <SelectTrigger className='h-[28px] w-40 px-2.5 text-xs'><SelectValue>{selectedFilterLabel}</SelectValue></SelectTrigger>
                  <SelectContent>
                    <SelectItem value='all'>{t('全部插件')}</SelectItem>
                    {plugins.map(item => <SelectItem key={item.id} value={item.id}>{item.name}</SelectItem>)}
                  </SelectContent>
                </Select>
              ) : null}
              <Button size='sm' onClick={openNewJob}>{t('新建任务')}</Button>
            </>
          ) : (
            <>
              <InputGroup className='w-52 rounded-sm border-hairline bg-surface'>
                <InputGroupInput type='search' value={pluginQuery} placeholder={t('搜索任务插件')} aria-label={t('搜索任务插件')}
                  onChange={e => setPluginQuery(e.currentTarget.value)} />
                <InputGroupAddon><svg viewBox='0 0 16 16' className='size-3.5' fill='none' stroke='currentColor' strokeWidth='1.7' strokeLinecap='round' aria-hidden='true'><circle cx='6.8' cy='6.8' r='4.6' /><path d='M10.3 10.3 14 14' /></svg></InputGroupAddon>
              </InputGroup>
              <Button variant='outline' size='sm' onClick={() => setImportRequest(v => v + 1)}>{t('导入脚本/包')}</Button>
              <Button size='sm' onClick={openNewEditor}>{t('编写脚本任务')}</Button>
            </>
          )}
        </div>
      </div>

      {activeTab === 'plugins' ? (
        <div className='flex flex-wrap items-center justify-between gap-2'>
          <SegmentedControl<PluginStatusFilter>
            aria-label={t('任务插件筛选')}
            value={pluginStatus}
            onValueChange={setPluginStatus}
            options={[
              { value: 'all', label: t('全部'), count: plugins.length + storeUninstalledCount },
              { value: 'installed', label: t('已安装'), count: plugins.length },
              { value: 'missing', label: t('官方商店'), count: storeUninstalledCount },
              { value: 'updatable', label: t('可更新'), count: updatablePluginCount },
            ]}
          />
        </div>
      ) : null}

      {error ? <Alert variant='destructive'>{error}</Alert> : null}

      {activeTab === 'jobs' ? (
        !loaded ? (
          <Card><CardContent className='flex items-center justify-center gap-2 py-12 text-xs text-muted-foreground'>
            <Spinner className='size-4' /><span>{t('正在加载定时任务…')}</span>
          </CardContent></Card>
        ) : filteredJobs.length === 0 ? (
          <Card><Empty>
            <EmptyTitle>{jobs.length === 0 ? t('还没有定时任务') : t('没有匹配的任务')}</EmptyTitle>
            <EmptyDescription>
              {jobs.length === 0
                ? (enabledPlugins.length > 0
                  ? t('已安装 {count} 个定时任务插件，点击「新建任务」即可添加自动执行计划。', { count: enabledPlugins.length })
                  : storeUninstalledCount > 0
                    ? t('官方商店有 {count} 个可用任务插件，可在「任务插件」中一键安装，或直接编写自己的脚本任务。', { count: storeUninstalledCount })
                    : t('可在「任务插件」中导入或安装任务插件，或点击「编写脚本任务」可视化制作自动化插件。'))
                : t('调整搜索词或插件筛选后再查看。')}
            </EmptyDescription>
            <div className='mt-3 flex flex-wrap items-center justify-center gap-2'>
              {jobs.length === 0 ? <>
                {enabledPlugins.length > 0
                  ? <Button size='sm' onClick={openNewJob}>{t('新建任务')}</Button>
                  : <Button size='sm' onClick={() => setActiveTab('plugins')}>{t('查看任务插件')}</Button>}
                <Button variant='outline' size='sm' onClick={openNewEditor}>{t('编写脚本任务')}</Button>
                {enabledPlugins.length === 0 ? (
                  <Button variant='outline' size='sm' onClick={() => setImportRequest(v => v + 1)}>{t('导入脚本/包')}</Button>
                ) : null}
              </> : <Button variant='outline' size='sm' onClick={() => { setSearch(''); setPluginFilter('all') }}>{t('清除筛选')}</Button>}
            </div>
          </Empty></Card>
        ) : (
          <Card className='overflow-hidden'><div className='overflow-x-auto'>
            <Table>
              <TableHeader><TableRow>
                <TableHead>{t('任务与状态')}</TableHead>
                <TableHead>{t('执行计划')}</TableHead>
                <TableHead>{t('网络代理')}</TableHead>
                <TableHead>{t('最近执行')}</TableHead>
                <TableHead className='w-16 text-center'>{t('启用')}</TableHead>
                <TableHead className='text-right'>{t('操作')}</TableHead>
              </TableRow></TableHeader>
              <TableBody>
                {filteredJobs.map(job => {
                  const plugin = pluginMap.get(job.pluginId)
                  const pluginAvailable = Boolean(plugin && plugin.enabled)
                  const running = job.lastStatus === 'running'
                  const busy = busyJobId === job.id
                  const summaryEntries = Object.entries(job.statusSummary || {})
                  return (
                    <TableRow key={job.id}>
                      <TableCell className='max-w-[280px]'>
                        <div className='flex flex-col gap-1'>
                          <div className='flex items-center gap-1.5'>
                            <span className='font-medium text-foreground'>{job.name}</span>
                            <Badge variant='outline' className='text-[10.5px]'>{plugin?.name || job.pluginId}</Badge>
                            {!plugin ? <Badge variant='warning' className='text-[10.5px]'>{t('插件未安装')}</Badge>
                              : !plugin.enabled ? <Badge variant='secondary' className='text-[10.5px]'>{t('插件已停用')}</Badge> : null}
                          </div>
                          {summaryEntries.length > 0 ? <div className='flex flex-wrap gap-1'>
                            {summaryEntries.map(([key, val]) => {
                              const def = plugin?.statusFields.find(item => item.key === key)
                              return <Badge key={key} variant='secondary' className='text-[10.5px] font-normal'>{def ? `${def.label}：${val}` : val}</Badge>
                            })}
                          </div> : null}
                        </div>
                      </TableCell>
                      <TableCell>
                        <div className='flex flex-col gap-0.5 text-xs'>
                          <code className='font-mono text-[12px] text-foreground'>{job.cronExpression}</code>
                          <span className='text-[11px] text-muted-foreground'>
                            {job.enabled && job.nextRunAt ? t('下次：{time}', { time: formatTaskDateTime(job.nextRunAt) }) : t('未调度')}
                          </span>
                        </div>
                      </TableCell>
                      <TableCell className='w-44'>
                        <Select value={job.proxyId || '__direct__'} disabled={busy}
                          onValueChange={val => void runJobMutation(job.id, async () => {
                            await api('PATCH', `/api/task-jobs/${encodeURIComponent(job.id)}`, { proxyId: !val || val === '__direct__' ? '' : val })
                            await load()
                          })}>
                          <SelectTrigger className='h-8 text-xs'><SelectValue>{formatTaskProxyLabel(job.proxyId, proxies, t)}</SelectValue></SelectTrigger>
                          <SelectContent>
                            <SelectItem value='__direct__'>{t('直连（不使用代理）')}</SelectItem>
                            {proxies.map(item => <SelectItem key={item.id} value={item.id}>{item.name} ({item.protocol})</SelectItem>)}
                          </SelectContent>
                        </Select>
                      </TableCell>
                      <TableCell>
                        <div className='flex flex-col gap-0.5 text-xs'>
                          <div className='flex items-center gap-1.5'>
                            {running ? <Badge variant='brand' className='gap-1 text-[10.5px]'><Spinner className='size-3' />{t('运行中')}</Badge>
                              : job.lastStatus === 'success' ? <Badge variant='success' className='text-[10.5px]'>{t('成功')}</Badge>
                                : job.lastStatus === 'failed' ? <Badge variant='destructive' className='text-[10.5px]' title={job.lastError || ''}>{t('失败')}</Badge>
                                  : <span className='text-muted-foreground'>{t('尚未运行')}</span>}
                            {job.lastDurationMs !== null && !running ? <span className='text-[11px] text-muted-foreground'>{formatTaskDuration(job.lastDurationMs)}</span> : null}
                          </div>
                          {job.lastRunAt ? <span className='text-[11px] text-muted-foreground'>{formatTaskDateTime(job.lastRunAt)}</span> : null}
                          {job.lastStatus === 'failed' && job.lastError ? <span className='max-w-[200px] truncate text-[11px] text-destructive' title={job.lastError}>{job.lastError}</span> : null}
                        </div>
                      </TableCell>
                      <TableCell className='text-center'>
                        <Switch checked={job.enabled} disabled={busy || (!job.enabled && !pluginAvailable)} aria-label={t('启用定时任务')}
                          onCheckedChange={checked => void runJobMutation(job.id, async () => {
                            await api('PATCH', `/api/task-jobs/${encodeURIComponent(job.id)}`, { enabled: Boolean(checked) })
                            await load()
                          })} />
                      </TableCell>
                      <TableCell className='text-right'>
                        <div className='flex flex-wrap items-center justify-end gap-1.5'>
                          <Button size='xs' variant={running ? 'destructive' : 'outline'} disabled={busy || (!running && !pluginAvailable)}
                            onClick={() => void runJobMutation(job.id, async () => {
                              if (running) {
                                await api('POST', `/api/task-jobs/${encodeURIComponent(job.id)}/stop`, {})
                                toast(t('已发出停止信号'), 'ok')
                              } else {
                                const res = await api<{ outcome: SandboxOutcome }>('POST', `/api/task-jobs/${encodeURIComponent(job.id)}/run`, {})
                                toast(res.outcome.success ? (res.outcome.message || t('任务执行成功')) : (res.outcome.error || t('任务执行失败')), res.outcome.success ? 'ok' : 'err')
                              }
                              await load()
                            })}>
                            {running ? t('停止') : t('运行')}
                          </Button>
                          <Button size='xs' variant='outline' disabled={!plugin} onClick={() => { setActionOutcome(null); setDraft(createDraftFromJob(job, plugin)) }}>{t('配置')}</Button>
                          <Button size='xs' variant='ghost' onClick={() => void openHistory(job)}>{t('记录')}</Button>
                          <Button size='xs' variant='ghost' disabled={busy || !pluginAvailable}
                            onClick={() => void runJobMutation(job.id, async () => {
                              await api('POST', `/api/task-jobs/${encodeURIComponent(job.id)}/duplicate`, {})
                              toast(t('已复制任务配置（密钥与登录态未复制，请重新配置）'), 'ok')
                              await load()
                            })}>
                            {t('复制')}
                          </Button>
                          <Button size='xs' variant='ghost' className='text-destructive' disabled={busy} onClick={() => setDeleteTarget(job)}>{t('删除')}</Button>
                        </div>
                      </TableCell>
                    </TableRow>
                  )
                })}
              </TableBody>
            </Table>
          </div></Card>
        )
      ) : null}

      <TaskJobConfigDialog
        draft={draft}
        draftPlugin={draftPlugin}
        enabledPlugins={enabledPlugins}
        proxies={proxies}
        cronPreview={cronPreview}
        saving={saving}
        actionBusyId={actionBusyId}
        actionOutcome={actionOutcome}
        t={t}
        onChangeDraft={updater => setDraft(prev => (prev ? updater(prev) : prev))}
        onSelectPlugin={pluginId => {
          const next = plugins.find(item => item.id === pluginId)
          if (next) { setActionOutcome(null); setDraft(createDraftFromPlugin(next)) }
        }}
        onEditPlugin={plugin => { setEditorPlugin(plugin); setEditorIsNew(false); setEditorOpen(true) }}
        onTogglePluginNotify={async (plugin, notifyAllowed) => {
          try {
            await api('PATCH', `/api/task-plugins/${encodeURIComponent(plugin.id)}`, { notifyAllowed })
            await load()
            toast(notifyAllowed ? t('已允许该插件发送系统通知') : t('已禁止该插件发送系统通知'), 'ok')
          } catch (cause) { toast(errorMessage(cause), 'err') }
        }}
        onTriggerAction={handleTriggerAction}
        onSave={() => void handleSaveDraft()}
        onClose={() => setDraft(null)}
      />

      <TaskJobHistorySheet
        job={historyJob}
        runs={runs}
        loading={runsLoading}
        t={t}
        onClose={() => setHistoryJob(null)}
        onRefresh={() => historyJob && void openHistory(historyJob)}
        onClear={async () => {
          if (!historyJob) return
          try {
            await api('DELETE', `/api/task-jobs/${encodeURIComponent(historyJob.id)}/runs`)
            setRuns([])
            toast(t('已清空运行记录'), 'ok')
          } catch (cause) { toast(errorMessage(cause), 'err') }
        }}
      />

      <TaskJobDeleteDialog
        target={deleteTarget}
        t={t}
        onClose={() => setDeleteTarget(null)}
        onConfirm={() => {
          if (!deleteTarget) return
          const target = deleteTarget
          setDeleteTarget(null)
          void runJobMutation(target.id, async () => {
            await api('DELETE', `/api/task-jobs/${encodeURIComponent(target.id)}`)
            toast(t('已删除任务「{name}」', { name: target.name }), 'ok')
            await load()
          })
        }}
      />

      <TaskPluginEditorDialog
        open={editorOpen}
        initialPlugin={editorPlugin}
        isNew={editorIsNew}
        autoCreateDefaultJob={editorIsNew}
        request={api}
        t={t}
        onClose={() => setEditorOpen(false)}
        onSaved={async (saved, createJob) => {
          setEditorOpen(false)
          await load()
          if (createJob) { setActionOutcome(null); setDraft(createDraftFromPlugin(saved)) }
          toast(editorIsNew ? t('插件已创建') : t('插件已更新'), 'ok')
        }}
      />
      <TaskPluginsSection
        payload={taskPayload}
        status={pluginStatus}
        query={pluginQuery}
        request={api}
        refresh={load}
        jobCounts={jobCounts}
        embedded
        hideContent={activeTab !== 'plugins'}
        importRequest={importRequest}
        t={t}
        notify={(msg, kind) => toast(msg, kind === 'err' ? 'err' : 'ok')}
        onOpenScheduledPage={pluginId => {
          if (pluginId) openCreateForPlugin(pluginId)
        }}
      />
    </div>
  )
}

const PAGE_SELECTOR = '.page[data-page="scheduled"]'
let mountedFlag = false
let currentLoad: (() => Promise<void>) | null = null
let currentOpenForPlugin: ((pluginId: string) => void) | null = null
let queuedOpenForPlugin: string | null = null

function mount(): void {
  if (mountedFlag) return
  const section = document.querySelector<HTMLElement>(PAGE_SELECTOR)
  if (!section) return
  mountedFlag = true
  section.replaceChildren()
  createRoot(section).render(<ScheduledJobsPage />)
}

mountOnPage('scheduled', PAGE_SELECTOR, mount)

window.wbScheduledPage = {
  async load() { await currentLoad?.() },
  openCreateForPlugin(pluginId: string) {
    if (currentOpenForPlugin) currentOpenForPlugin(pluginId)
    else queuedOpenForPlugin = pluginId
  },
}
