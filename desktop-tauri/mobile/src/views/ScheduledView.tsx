import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  clearTaskJobRuns,
  createTaskJob,
  deleteTaskJob,
  fetchTaskJobRuns,
  fetchTaskJobs,
  fetchTaskPlugins,
  patchTaskJob,
  runTaskJobNow,
  stopTaskJob,
  triggerTaskJobAction,
  updateTaskJob,
  updateTaskPluginNotifyPermission,
} from '../core/api-task-plugins'
import { locale, t } from '../core/i18n'
import { serverIdentity } from '../core/servers'
import { store, useStore } from '../core/store'
import {
  BottomSheet,
  CommonCard,
  CommonScaffold,
  CommonSwitch,
  EmptyBlock,
  ListGroup,
  ListItem,
  RecordCard,
  SearchField,
  SectionHeader,
  SegmentedControl,
  TonalChip,
  TonalIconButton,
} from '../ui/components'
import { PickerSheet, PickerTrigger } from '../ui/pickers'
import { ScheduledTaskForm } from './ScheduledTaskForm'
import {
  defaultValuesForPlugin,
  TASK_CRON_PRESETS,
  TASK_TIMEZONES,
  type SandboxOutcome,
  type TaskActionDef,
  type TaskJob,
  type TaskJobRun,
  type TaskPlugin,
  type TaskProxyOption,
} from '../../../ui-shared/task-plugins-types'

function formatTimeShort(ts: number | null | undefined): string {
  if (!ts) return '—'
  const d = new Date(ts)
  if (Number.isNaN(d.getTime())) return '—'
  return d.toLocaleString(locale(), {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    hour12: false,
  })
}

type MobileJobDraft = {
  id: string | null
  pluginId: string
  name: string
  cronExpression: string
  timezone: string
  timeoutSeconds: number
  proxyId: string
  enabled: boolean
  values: Record<string, unknown>
  savedSecrets: Record<string, boolean>
  clearSecrets: string[]
  statusSummary: Record<string, string>
}

function makeDraftFromPlugin(plugin: TaskPlugin): MobileJobDraft {
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

function makeDraftFromJob(job: TaskJob, plugin?: TaskPlugin): MobileJobDraft {
  return {
    id: job.id,
    pluginId: job.pluginId,
    name: job.name,
    cronExpression: job.cronExpression,
    timezone: job.timezone,
    timeoutSeconds: job.timeoutSeconds,
    proxyId: job.proxyId || '',
    enabled: job.enabled,
    values: { ...(plugin ? defaultValuesForPlugin(plugin) : {}), ...job.values },
    savedSecrets: { ...job.savedSecrets },
    clearSecrets: [],
    statusSummary: { ...job.statusSummary },
  }
}

export function ScheduledView() {
  const { server, revision, navTabs } = useStore()
  const [plugins, setPlugins] = useState<TaskPlugin[]>([])
  const [jobs, setJobs] = useState<TaskJob[]>([])
  const [proxies, setProxies] = useState<TaskProxyOption[]>([])
  const [loading, setLoading] = useState(true)
  const [keyword, setKeyword] = useState('')
  const [busyJobId, setBusyJobId] = useState('')

  // 详情 / 编辑抽屉
  const [draft, setDraft] = useState<MobileJobDraft | null>(null)
  const [sheetTab, setSheetTab] = useState<'form' | 'schedule' | 'runs'>('form')
  const [saving, setSaving] = useState(false)
  const [actionBusyId, setActionBusyId] = useState('')
  const [actionOutcome, setActionOutcome] = useState<SandboxOutcome | null>(null)
  const [runs, setRuns] = useState<TaskJobRun[]>([])
  const [runsLoading, setRunsLoading] = useState(false)
  const [confirmDelete, setConfirmDelete] = useState(false)

  // 选择器
  const [pickerKind, setPickerKind] = useState<'plugin' | 'cron' | 'timezone' | 'proxy' | null>(null)

  const loadData = useCallback(async (signal?: AbortSignal) => {
    const id = serverIdentity()
    if (!server?.base) return
    try {
      const [pluginsRes, jobsRes] = await Promise.all([
        fetchTaskPlugins(signal),
        fetchTaskJobs(signal),
      ])
      if (serverIdentity() !== id) return
      setPlugins(pluginsRes.plugins || [])
      setJobs(jobsRes.jobs || [])
      setProxies(jobsRes.proxies || [])
    } catch (err) {
      if (!signal?.aborted) {
        store.toast(err instanceof Error ? err.message : t('加载定时任务失败'), 'danger')
      }
    } finally {
      if (!signal?.aborted) setLoading(false)
    }
  }, [server?.base])

  useEffect(() => {
    const ac = new AbortController()
    void loadData(ac.signal)
    return () => ac.abort()
  }, [loadData, revision])

  const hasRunning = jobs.some(job => job.lastStatus === 'running')
  useEffect(() => {
    if (!hasRunning) return
    const timer = window.setInterval(() => {
      if (!document.hidden) void loadData()
    }, 2000)
    return () => window.clearInterval(timer)
  }, [hasRunning, loadData])

  const pluginMap = useMemo(() => new Map(plugins.map(item => [item.id, item])), [plugins])
  const enabledPlugins = useMemo(() => plugins.filter(item => item.enabled), [plugins])

  const filteredJobs = useMemo(() => {
    const q = keyword.trim().toLowerCase()
    if (!q) return jobs
    return jobs.filter(job => {
      const plugin = pluginMap.get(job.pluginId)
      return `${job.name} ${plugin?.name || job.pluginId} ${job.cronExpression}`.toLowerCase().includes(q)
    })
  }, [jobs, keyword, pluginMap])

  const loadRuns = async (jobId: string) => {
    setRunsLoading(true)
    try {
      const res = await fetchTaskJobRuns(jobId, 50)
      setRuns(res.runs || [])
    } catch (err) {
      store.toast(err instanceof Error ? err.message : t('加载记录失败'), 'danger')
      setRuns([])
    } finally {
      setRunsLoading(false)
    }
  }

  const openJob = (job: TaskJob) => {
    setActionOutcome(null)
    setConfirmDelete(false)
    setSheetTab('form')
    setDraft(makeDraftFromJob(job, pluginMap.get(job.pluginId)))
    void loadRuns(job.id)
  }

  const openCreate = () => {
    if (enabledPlugins.length === 0) {
      store.toast(t('请先在桌面端或网页端安装并启用定时任务插件'), 'info')
      return
    }
    setActionOutcome(null)
    setConfirmDelete(false)
    setSheetTab('form')
    setRuns([])
    setDraft(makeDraftFromPlugin(enabledPlugins[0]))
  }

  const persistDraft = async (current: MobileJobDraft): Promise<TaskJob> => {
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
      ? await updateTaskJob(current.id, payload)
      : await createTaskJob(payload)
    return res.job
  }

  const handleSave = async () => {
    if (!draft) return
    setSaving(true)
    try {
      await persistDraft(draft)
      store.toast(draft.id ? t('任务配置已保存') : t('已创建定时任务'), 'success')
      setDraft(null)
      await loadData()
    } catch (err) {
      store.toast(err instanceof Error ? err.message : t('保存失败'), 'danger')
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
        const created = await persistDraft(current)
        current = makeDraftFromJob(created, pluginMap.get(created.pluginId))
        setDraft(current)
        void loadData()
      }
      const res = await triggerTaskJobAction(current.id!, action.id, current.values)
      setActionOutcome(res.outcome)
      setDraft(prev => {
        if (!prev) return prev
        return {
          ...prev,
          values: res.outcome.patchForm ? { ...prev.values, ...res.outcome.patchForm } : prev.values,
          statusSummary: res.job?.statusSummary
            ?? (res.outcome.statusPatch ? { ...prev.statusSummary, ...res.outcome.statusPatch } : prev.statusSummary),
          savedSecrets: res.job?.savedSecrets ?? prev.savedSecrets,
        }
      })
      void loadData()
      void loadRuns(current.id!)
    } catch (err) {
      setActionOutcome({
        success: false,
        durationMs: 0,
        output: '',
        error: err instanceof Error ? err.message : t('操作执行失败'),
      })
    } finally {
      setActionBusyId('')
    }
  }

  const handleToggle = async (job: TaskJob, next: boolean) => {
    setBusyJobId(job.id)
    try {
      await patchTaskJob(job.id, { enabled: next })
      await loadData()
    } catch (err) {
      store.toast(err instanceof Error ? err.message : t('切换状态失败'), 'danger')
    } finally {
      setBusyJobId('')
    }
  }

  const handleRunOrStop = async (job: TaskJob) => {
    setBusyJobId(job.id)
    try {
      if (job.lastStatus === 'running') {
        await stopTaskJob(job.id)
        store.toast(t('已发出停止信号'), 'info')
      } else {
        const res = await runTaskJobNow(job.id)
        store.toast(
          res.outcome.success ? (res.outcome.message || t('任务执行成功')) : (res.outcome.error || t('任务执行失败')),
          res.outcome.success ? 'success' : 'danger',
        )
      }
      await loadData()
    } catch (err) {
      store.toast(err instanceof Error ? err.message : t('操作失败'), 'danger')
      await loadData()
    } finally {
      setBusyJobId('')
    }
  }

  const handleDeleteCurrent = async () => {
    if (!draft?.id) return
    setSaving(true)
    try {
      await deleteTaskJob(draft.id)
      store.toast(t('已删除定时任务'), 'success')
      setDraft(null)
      await loadData()
    } catch (err) {
      store.toast(err instanceof Error ? err.message : t('删除失败'), 'danger')
    } finally {
      setSaving(false)
    }
  }

  const handleToggleNotify = async (plugin: TaskPlugin, allowed: boolean) => {
    try {
      await updateTaskPluginNotifyPermission(plugin.id, allowed)
      await loadData()
      store.toast(allowed ? t('已允许该插件发送系统通知') : t('已禁止该插件发送系统通知'), 'success')
    } catch (err) {
      store.toast(err instanceof Error ? err.message : t('更新权限失败'), 'danger')
    }
  }

  const isSubPage = !navTabs.includes('scheduled')
  const draftPlugin = draft ? pluginMap.get(draft.pluginId) : undefined
  const selectedProxyLabel = draft?.proxyId
    ? (() => {
        const found = proxies.find(item => item.id === draft.proxyId)
        return found ? `${found.name} (${found.protocol})` : draft.proxyId
      })()
    : t('直连（不使用代理）')

  return (
    <CommonScaffold
      title={t('定时任务')}
      badge={<TonalChip tone='primary'>{jobs.length}</TonalChip>}
      onBack={isSubPage ? () => store.returnFromSubPage() : undefined}
      actions={
        <>
          <TonalIconButton icon='refresh' title={t('刷新')} onClick={() => void loadData()} />
          <TonalIconButton icon='plus' primary label={t('新建')} onClick={openCreate} />
        </>
      }
      stickyHeader={
        jobs.length > 3 ? (
          <SearchField value={keyword} onChange={setKeyword} placeholder={t('搜索任务名称或插件…')} />
        ) : undefined
      }
    >
      {loading ? (
        <CommonCard>
          <div style={{ textAlign: 'center', padding: '18px 0', fontSize: 13, color: 'var(--fc-text-secondary)' }}>
            {t('正在加载定时任务…')}
          </div>
        </CommonCard>
      ) : filteredJobs.length === 0 ? (
        <EmptyBlock
          icon='clock'
          title={jobs.length === 0 ? t('还没有定时任务') : t('没有匹配的任务')}
          description={
            jobs.length === 0
              ? (enabledPlugins.length > 0
                ? t('已安装 {count} 个定时任务插件，点击右上角「新建」即可开始。', { count: enabledPlugins.length })
                : t('先在桌面端或网页端的「定时任务 → 任务插件」安装或创建脚本插件，再回到这里创建与操作。'))
              : t('尝试换一个关键词搜索。')
          }
          action={
            jobs.length === 0 && enabledPlugins.length > 0 ? (
              <button type='button' className='fc-btn is-primary is-pill' onClick={openCreate}>
                {t('新建定时任务')}
              </button>
            ) : undefined
          }
        />
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
          {filteredJobs.map(job => {
            const plugin = pluginMap.get(job.pluginId)
            const pluginAvailable = Boolean(plugin && plugin.enabled)
            const running = job.lastStatus === 'running'
            const busy = busyJobId === job.id
            const summaryEntries = Object.entries(job.statusSummary || {})
            return (
              <RecordCard key={job.id} onClick={() => openJob(job)}>
                <div style={{ display: 'flex', alignItems: 'flex-start', justifyContent: 'space-between', gap: 10 }}>
                  <div style={{ minWidth: 0, flex: 1 }}>
                    <div style={{ display: 'flex', alignItems: 'center', gap: 6, flexWrap: 'wrap' }}>
                      <span style={{ fontSize: 15, fontWeight: 700, color: 'var(--fc-text)' }}>{job.name}</span>
                      <TonalChip tone='neutral'>{plugin?.name || job.pluginId}</TonalChip>
                      {!plugin ? (
                        <TonalChip tone='warning'>{t('插件未安装')}</TonalChip>
                      ) : !plugin.enabled ? (
                        <TonalChip tone='warning'>{t('插件已停用')}</TonalChip>
                      ) : null}
                    </div>
                    <div style={{ marginTop: 4, fontSize: 12, color: 'var(--fc-text-secondary)' }}>
                      <code style={{ fontFamily: 'monospace' }}>{job.cronExpression}</code>
                      {' · '}
                      {job.enabled && job.nextRunAt
                        ? t('下次：{time}', { time: formatTimeShort(job.nextRunAt) })
                        : t('未调度')}
                    </div>
                  </div>
                  <CommonSwitch
                    checked={job.enabled}
                    disabled={busy || (!job.enabled && !pluginAvailable)}
                    onChange={next => void handleToggle(job, next)}
                  />
                </div>

                {summaryEntries.length > 0 ? (
                  <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6, marginTop: 8 }}>
                    {summaryEntries.map(([key, val]) => {
                      const def = plugin?.statusFields.find(item => item.key === key)
                      return (
                        <TonalChip key={key} tone='primary'>
                          {def ? `${def.label}：${val}` : val}
                        </TonalChip>
                      )
                    })}
                  </div>
                ) : null}

                <div
                  style={{
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'space-between',
                    gap: 8,
                    marginTop: 10,
                    paddingTop: 8,
                    borderTop: '1px solid var(--fc-divider)',
                  }}
                >
                  <div style={{ display: 'flex', alignItems: 'center', gap: 6, minWidth: 0 }}>
                    {running ? (
                      <TonalChip tone='primary'>{t('运行中…')}</TonalChip>
                    ) : job.lastStatus === 'success' ? (
                      <TonalChip tone='success'>{t('上次成功')}</TonalChip>
                    ) : job.lastStatus === 'failed' ? (
                      <TonalChip tone='danger'>{t('上次失败')}</TonalChip>
                    ) : (
                      <span style={{ fontSize: 12, color: 'var(--fc-text-tertiary)' }}>{t('尚未运行')}</span>
                    )}
                    {job.lastRunAt ? (
                      <span style={{ fontSize: 11.5, color: 'var(--fc-text-tertiary)' }}>
                        {formatTimeShort(job.lastRunAt)}
                      </span>
                    ) : null}
                  </div>
                  <button
                    type='button'
                    className={`fc-btn is-sm is-pill ${running ? 'is-danger' : 'is-primary'}`}
                    disabled={busy || (!running && !pluginAvailable)}
                    onClick={e => {
                      e.stopPropagation()
                      void handleRunOrStop(job)
                    }}
                  >
                    {running ? t('停止') : t('立即运行')}
                  </button>
                </div>
              </RecordCard>
            )
          })}
        </div>
      )}

      {/* 任务配置与交互 BottomSheet */}
      <BottomSheet
        open={draft !== null}
        title={draft?.id ? draft.name : t('新建定时任务')}
        onClose={() => { if (!saving && !actionBusyId) setDraft(null) }}
        fixedHeader={
          draft ? (
            <SegmentedControl
              value={sheetTab}
              options={[
                { id: 'form', label: t('参数与操作') },
                { id: 'schedule', label: t('计划与权限') },
                ...(draft.id ? [{ id: 'runs' as const, label: t('运行记录') }] : []),
              ]}
              onChange={setSheetTab}
            />
          ) : undefined
        }
        footer={
          draft && sheetTab !== 'runs' ? (
            <div style={{ display: 'flex', gap: 8 }}>
              <button
                type='button'
                className='fc-btn'
                style={{ flex: 1 }}
                disabled={saving || Boolean(actionBusyId)}
                onClick={() => setDraft(null)}
              >
                {t('关闭')}
              </button>
              <button
                type='button'
                className='fc-btn is-primary'
                style={{ flex: 2 }}
                disabled={saving || Boolean(actionBusyId) || !draftPlugin}
                onClick={() => void handleSave()}
              >
                {saving ? t('保存中…') : t('保存配置')}
              </button>
            </div>
          ) : undefined
        }
      >
        {draft && sheetTab === 'form' ? (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
            {!draft.id ? (
              <CommonCard>
                <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
                  <div>
                    <div style={{ fontSize: 12.5, fontWeight: 600, marginBottom: 6 }}>{t('选择定时任务插件')}</div>
                    <PickerTrigger
                      valueLabel={draftPlugin?.name || draft.pluginId}
                      onClick={() => setPickerKind('plugin')}
                    />
                  </div>
                  <div>
                    <div style={{ fontSize: 12.5, fontWeight: 600, marginBottom: 6 }}>{t('任务名称')}</div>
                    <input
                      className='fc-input'
                      value={draft.name}
                      placeholder={t('例如：主号每日签到')}
                      onChange={e => setDraft(prev => prev ? { ...prev, name: e.target.value } : prev)}
                    />
                  </div>
                </div>
              </CommonCard>
            ) : null}

            {draftPlugin ? (
              <ScheduledTaskForm
                plugin={draftPlugin}
                values={draft.values}
                savedSecrets={draft.savedSecrets}
                statusSummary={draft.statusSummary}
                busyActionId={actionBusyId}
                outcome={actionOutcome}
                onChange={(key, value) =>
                  setDraft(prev => prev ? {
                    ...prev,
                    values: { ...prev.values, [key]: value },
                    clearSecrets: prev.clearSecrets.filter(item => item !== key),
                  } : prev)
                }
                onClearSecret={key =>
                  setDraft(prev => prev ? {
                    ...prev,
                    values: { ...prev.values, [key]: '' },
                    savedSecrets: { ...prev.savedSecrets, [key]: false },
                    clearSecrets: prev.clearSecrets.includes(key) ? prev.clearSecrets : [...prev.clearSecrets, key],
                  } : prev)
                }
                onTriggerAction={handleTriggerAction}
              />
            ) : (
              <CommonCard>
                <div style={{ fontSize: 13, color: 'var(--fc-danger)' }}>
                  {t('该任务关联的插件未安装，请重新安装后再配置参数。')}
                </div>
              </CommonCard>
            )}
          </div>
        ) : null}

        {draft && sheetTab === 'schedule' ? (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
            <CommonCard>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
                <div>
                  <div style={{ fontSize: 12.5, fontWeight: 600, marginBottom: 6 }}>{t('任务名称')}</div>
                  <input
                    className='fc-input'
                    value={draft.name}
                    onChange={e => setDraft(prev => prev ? { ...prev, name: e.target.value } : prev)}
                  />
                </div>
                <div>
                  <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 6 }}>
                    <span style={{ fontSize: 12.5, fontWeight: 600 }}>{t('定时规则（Cron）')}</span>
                    <button
                      type='button'
                      className='fc-btn is-sm'
                      onClick={() => setPickerKind('cron')}
                    >
                      {t('快捷预设')}
                    </button>
                  </div>
                  <input
                    className='fc-input'
                    style={{ fontFamily: 'monospace' }}
                    value={draft.cronExpression}
                    placeholder='0 9 * * *'
                    onChange={e => setDraft(prev => prev ? { ...prev, cronExpression: e.target.value } : prev)}
                  />
                </div>
                <div>
                  <div style={{ fontSize: 12.5, fontWeight: 600, marginBottom: 6 }}>{t('时区')}</div>
                  <PickerTrigger valueLabel={draft.timezone} onClick={() => setPickerKind('timezone')} />
                </div>
                <div>
                  <div style={{ fontSize: 12.5, fontWeight: 600, marginBottom: 6 }}>{t('超时上限（秒）')}</div>
                  <input
                    type='number'
                    className='fc-input'
                    min={1}
                    max={300}
                    value={String(draft.timeoutSeconds)}
                    onChange={e => setDraft(prev => prev ? { ...prev, timeoutSeconds: Number(e.target.value) || 60 } : prev)}
                  />
                </div>
                <div>
                  <div style={{ fontSize: 12.5, fontWeight: 600, marginBottom: 6 }}>{t('网络代理')}</div>
                  <PickerTrigger valueLabel={selectedProxyLabel} onClick={() => setPickerKind('proxy')} />
                </div>
              </div>
            </CommonCard>

            <ListGroup>
              <ListItem
                title={t('启用定时调度')}
                trailing={
                  <CommonSwitch
                    checked={draft.enabled}
                    onChange={next => setDraft(prev => prev ? { ...prev, enabled: next } : prev)}
                  />
                }
              />
              {draftPlugin?.declaresNotify ? (
                <ListItem
                  icon='bell'
                  title={t('允许发送系统通知')}
                  subtitle={t('通过设置里已启用的通知渠道推送结果')}
                  trailing={
                    <CommonSwitch
                      checked={draftPlugin.notifyAllowed}
                      onChange={next => void handleToggleNotify(draftPlugin, next)}
                    />
                  }
                />
              ) : null}
            </ListGroup>

            {draftPlugin ? (
              <CommonCard>
                <div style={{ fontSize: 12.5, fontWeight: 600, marginBottom: 6 }}>{t('允许访问的域名白名单')}</div>
                <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6 }}>
                  {draftPlugin.domains.length > 0 ? (
                    draftPlugin.domains.map(d => <TonalChip key={d}>{d}</TonalChip>)
                  ) : (
                    <span style={{ fontSize: 12, color: 'var(--fc-text-secondary)' }}>{t('仅离线计算，不联网')}</span>
                  )}
                </div>
              </CommonCard>
            ) : null}

            {draft.id ? (
              confirmDelete ? (
                <CommonCard>
                  <div style={{ fontSize: 13, fontWeight: 600, color: 'var(--fc-danger)' }}>
                    {t('确定删除任务「{name}」吗？加密参数与登录态缓存将一并清除。', { name: draft.name })}
                  </div>
                  <div style={{ display: 'flex', gap: 8, marginTop: 10 }}>
                    <button type='button' className='fc-btn' style={{ flex: 1 }} onClick={() => setConfirmDelete(false)}>
                      {t('取消')}
                    </button>
                    <button
                      type='button'
                      className='fc-btn is-danger'
                      style={{ flex: 1 }}
                      disabled={saving}
                      onClick={() => void handleDeleteCurrent()}
                    >
                      {t('确认删除')}
                    </button>
                  </div>
                </CommonCard>
              ) : (
                <button type='button' className='fc-btn is-danger' onClick={() => setConfirmDelete(true)}>
                  {t('删除这个任务')}
                </button>
              )
            ) : null}
          </div>
        ) : null}

        {draft && sheetTab === 'runs' && draft.id ? (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
              <SectionHeader title={t('最近运行记录')} />
              <div style={{ display: 'flex', gap: 8 }}>
                <button type='button' className='fc-btn is-sm' onClick={() => void loadRuns(draft.id!)}>
                  {t('刷新')}
                </button>
                <button
                  type='button'
                  className='fc-btn is-sm is-danger'
                  disabled={runs.length === 0}
                  onClick={async () => {
                    await clearTaskJobRuns(draft.id!)
                    setRuns([])
                    store.toast(t('已清空运行记录'), 'success')
                  }}
                >
                  {t('清空')}
                </button>
              </div>
            </div>
            {runsLoading ? (
              <CommonCard>
                <div style={{ textAlign: 'center', fontSize: 13, color: 'var(--fc-text-secondary)' }}>{t('加载中…')}</div>
              </CommonCard>
            ) : runs.length === 0 ? (
              <EmptyBlock icon='history' title={t('暂无运行记录')} />
            ) : (
              runs.map(run => (
                <CommonCard key={run.id}>
                  <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 6 }}>
                    <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                      <TonalChip tone={run.status === 'success' ? 'success' : run.status === 'failed' ? 'danger' : 'primary'}>
                        {run.status === 'success' ? t('成功') : run.status === 'failed' ? t('失败') : t('运行中')}
                      </TonalChip>
                      <TonalChip>
                        {run.triggerType === 'schedule' ? t('定时') : run.triggerType === 'manual' ? t('手动') : (run.triggerLabel || t('操作'))}
                      </TonalChip>
                    </div>
                    <span style={{ fontSize: 11.5, color: 'var(--fc-text-tertiary)' }}>
                      {formatTimeShort(run.startedAt)}
                      {run.durationMs !== null ? ` · ${run.durationMs}ms` : ''}
                    </span>
                  </div>
                  {run.error ? (
                    <div style={{ fontSize: 12, color: 'var(--fc-danger)', whiteSpace: 'pre-wrap' }}>{run.error}</div>
                  ) : null}
                  {run.output ? (
                    <pre
                      style={{
                        margin: 0,
                        padding: 8,
                        borderRadius: 8,
                        background: 'var(--fc-surface-muted, rgba(128,128,128,0.08))',
                        fontSize: 11.5,
                        whiteSpace: 'pre-wrap',
                        wordBreak: 'break-all',
                        maxHeight: 160,
                        overflow: 'auto',
                      }}
                    >
                      {run.output}
                    </pre>
                  ) : null}
                </CommonCard>
              ))
            )}
          </div>
        ) : null}
      </BottomSheet>

      {/* 公共选择器 */}
      <PickerSheet
        open={pickerKind === 'plugin'}
        title={t('选择定时任务插件')}
        value={draft?.pluginId || ''}
        options={enabledPlugins.map(item => ({ value: item.id, label: item.name, subtitle: item.description }))}
        onSelect={next => {
          const found = plugins.find(item => item.id === next)
          if (found) {
            setActionOutcome(null)
            setDraft(makeDraftFromPlugin(found))
          }
          setPickerKind(null)
        }}
        onClose={() => setPickerKind(null)}
      />
      <PickerSheet
        open={pickerKind === 'cron'}
        title={t('选择常用执行计划')}
        value={draft?.cronExpression || ''}
        options={TASK_CRON_PRESETS.map(p => ({ value: p.cron, label: t(p.label), subtitle: p.cron }))}
        onSelect={next => {
          setDraft(prev => prev ? { ...prev, cronExpression: next } : prev)
          setPickerKind(null)
        }}
        onClose={() => setPickerKind(null)}
      />
      <PickerSheet
        open={pickerKind === 'timezone'}
        title={t('选择时区')}
        value={draft?.timezone || 'Asia/Shanghai'}
        options={TASK_TIMEZONES.map(tz => ({ value: tz, label: tz }))}
        onSelect={next => {
          setDraft(prev => prev ? { ...prev, timezone: next } : prev)
          setPickerKind(null)
        }}
        onClose={() => setPickerKind(null)}
      />
      <PickerSheet
        open={pickerKind === 'proxy'}
        title={t('选择网络代理')}
        value={draft?.proxyId || '__direct__'}
        options={[
          { value: '__direct__', label: t('直连（不使用代理）') },
          ...proxies.map(p => ({ value: p.id, label: p.name, subtitle: `${p.protocol} · ${p.host}:${p.port}` })),
        ]}
        onSelect={next => {
          setDraft(prev => prev ? { ...prev, proxyId: next === '__direct__' ? '' : next } : prev)
          setPickerKind(null)
        }}
        onClose={() => setPickerKind(null)}
      />
    </CommonScaffold>
  )
}
