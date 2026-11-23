import { useEffect, useState } from 'react'
import {
  checkinSummary,
  deleteCheckinSchedule,
  fetchKeys,
  getUmid,
  installUmid,
  isCheckinTask,
  pluginTaskRunning,
  runCheckinSchedule,
  runPluginTaskNow,
  runTravel,
  saveAutoCheckin,
  saveCheckinSchedule,
  saveKeepalive,
  saveTravel,
  updatePluginTaskSettings,
  type CheckinCenter,
  type GatewayKey,
  type ScheduleEntry,
  type UmidState,
} from '../core/api'
import { formatDateTime, providerDisplayName } from '../core/format'
import { t } from '../core/i18n'
import { store } from '../core/store'
import {
  BottomSheet,
  CommonCard,
  CommonSwitch,
  ListGroup,
  ListItem,
  SectionHeader,
  TonalChip,
} from '../ui/components'
import { PickerSheet, PickerTrigger, type PickerOption } from '../ui/pickers'
import { checkinRangeText, defaultCheckinEnd } from '../../../ui-shared/checkin-window'

export function CheckinSchedulePanel({
  center,
  onReload,
}: {
  center: CheckinCenter
  onReload: () => Promise<void>
}) {
  const [busyKey, setBusyKey] = useState('')
  const [editingSchedule, setEditingSchedule] = useState<ScheduleEntry | 'new' | null>(null)
  const [modelsText, setModelsText] = useState(() => (center.keepalive?.models || []).join(', '))
  const [apiKeyId, setApiKeyId] = useState<string>(() => center.keepalive?.apiKeyId || '')
  const [gatewayKeys, setGatewayKeys] = useState<GatewayKey[]>([])
  const [keyPickerOpen, setKeyPickerOpen] = useState(false)
  const [travelEnabled, setTravelEnabled] = useState(() => Boolean(center.travel?.enabled))
  const [travelTimes, setTravelTimes] = useState(() => (center.travel?.times || []).join(', '))
  const [umid, setUmid] = useState<UmidState | null>(null)
  const [deleteTarget, setDeleteTarget] = useState<ScheduleEntry | null>(null)

  const hasUmid = center.adminPanels?.includes('qoder_umid') || center.daily.providers.some(g => g.id === 'qoder')

  useEffect(() => {
    setModelsText((center.keepalive?.models || []).join(', '))
    setApiKeyId(center.keepalive?.apiKeyId || '')
    setTravelEnabled(Boolean(center.travel?.enabled))
    setTravelTimes((center.travel?.times || []).join(', '))
  }, [center.keepalive?.apiKeyId, center.keepalive?.models, center.travel?.enabled, center.travel?.times])

  useEffect(() => {
    const ac = new AbortController()
    void fetchKeys(ac.signal)
      .then(res => setGatewayKeys(res.keys || []))
      .catch(() => {})
    if (hasUmid) {
      void getUmid()
        .then(setUmid)
        .catch(() => {})
    }
    return () => ac.abort()
  }, [hasUmid])

  const keyOptions: PickerOption[] = [
    { value: '', label: t('自动选择第一把可用 Key') },
    ...gatewayKeys.flatMap(k =>
      k.enabled !== false && k.id
        ? [
            {
              value: k.id,
              label: k.name || k.masked || k.id,
              subtitle: k.masked || k.key,
            },
          ]
        : [],
    ),
  ]

  const currentKeyLabel =
    keyOptions.find(o => o.value === apiKeyId)?.label || (apiKeyId ? apiKeyId : t('自动选择'))

  const runAction = async (key: string, fn: () => Promise<unknown>, okToast?: string) => {
    if (busyKey) return
    setBusyKey(key)
    try {
      await fn()
      if (okToast) store.toast(okToast, 'success')
      await onReload()
    } catch (err) {
      store.toast(err instanceof Error ? err.message : String(err), 'danger')
    } finally {
      setBusyKey('')
    }
  }

  const saveKeepaliveConfig = () => {
    const models = modelsText
      .split(/[\n,]+/)
      .map(s => s.trim())
      .filter(Boolean)
    void runAction('keepalive', () => saveKeepalive(models, apiKeyId || null), t('保活配置已保存'))
  }

  const saveTravelConfig = () => {
    const times = travelTimes
      .split(/[\n,]+/)
      .map(s => s.trim())
      .filter(Boolean)
    void runAction('travel-save', () => saveTravel({ enabled: travelEnabled, times }), t('猫猫旅行排程已保存'))
  }

  const checkinPluginTasks = (center.pluginTasks || []).filter(isCheckinTask)

  return (
    <>
      <CommonCard>
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 12 }}>
          <div>
            <div style={{ fontSize: 15, fontWeight: 700 }}>{t('定时自动签到')}</div>
            <div style={{ fontSize: 12, color: 'var(--fc-text-muted)', marginTop: 2 }}>
              {center.auto.running ? t('正在执行定时签到…') : t('按下方设定的时间窗口自动执行')}
            </div>
          </div>
          <CommonSwitch
            checked={center.auto.enabled}
            disabled={Boolean(busyKey)}
            onChange={next => void runAction('auto-master', () => saveAutoCheckin(next))}
          />
        </div>
      </CommonCard>

      <SectionHeader
        title={t('签到计划条目 ({n})', { n: center.auto.schedules.length })}
        trailing={
          <button
            type='button'
            className='fc-btn is-sm is-primary'
            onClick={() => setEditingSchedule('new')}
          >
            {t('新增计划')}
          </button>
        }
      />

      {center.auto.schedules.map(entry => {
        const range = checkinRangeText(entry.time, entry.timeEnd)
        const summary = entry.lastResult ? checkinSummary(entry.lastResult) : null
        return (
          <CommonCard key={entry.id}>
            <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 10 }}>
              <div style={{ minWidth: 0, flex: 1 }}>
                <div style={{ fontSize: 15, fontWeight: 700 }}>{entry.displayLabel || entry.label || range}</div>
                <div style={{ fontSize: 12.5, color: 'var(--fc-text-muted)', marginTop: 2 }}>
                  <span className='fc-mono'>{range}</span>
                  {entry.retryTime ? ` · ${t('补签')} ${entry.retryTime}` : ''}
                </div>
              </div>
              <CommonSwitch
                checked={entry.enabled}
                disabled={Boolean(busyKey)}
                onChange={next =>
                  void runAction(`toggle:${entry.id}`, () =>
                    saveCheckinSchedule({
                      id: entry.id,
                      label: entry.label,
                      enabled: next,
                      time: entry.time,
                      timeEnd: entry.timeEnd,
                      providers: entry.providers,
                    }),
                  )
                }
              />
            </div>

            {entry.providers?.length ? (
              <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6, marginTop: 8 }}>
                {entry.providers.slice(0, 5).map(p => (
                  <TonalChip key={p}>{providerDisplayName(p)}</TonalChip>
                ))}
                {entry.providers.length > 5 ? (
                  <TonalChip tone='primary'>
                    {t('等 {n} 个提供商', { n: entry.providers.length })}
                  </TonalChip>
                ) : null}
              </div>
            ) : null}

            {entry.nextRunAt || summary ? (
              <div style={{ fontSize: 12, color: 'var(--fc-text-muted)', marginTop: 8 }}>
                {entry.nextRunAt ? `${t('下次')}：${formatDateTime(entry.nextRunAt)}` : ''}
                {entry.nextRunAt && summary ? ' · ' : ''}
                {summary ? t('上次 成功 {s} / 跳过 {k} / 失败 {f}', { s: summary.succeeded, k: summary.skipped, f: summary.failed }) : ''}
              </div>
            ) : null}

            <div style={{ display: 'flex', gap: 8, marginTop: 10 }}>
              <button
                type='button'
                className='fc-btn is-sm is-tonal'
                disabled={Boolean(busyKey)}
                onClick={() =>
                  void runAction(`run:${entry.id}`, () => runCheckinSchedule(entry.id), t('定时签到已执行'))
                }
              >
                {busyKey === `run:${entry.id}` ? t('执行中…') : t('立即运行')}
              </button>
              <button type='button' className='fc-btn is-sm' onClick={() => setEditingSchedule(entry)}>
                {t('编辑')}
              </button>
              <button type='button' className='fc-btn is-sm is-danger' onClick={() => setDeleteTarget(entry)}>
                {t('删除')}
              </button>
            </div>
          </CommonCard>
        )
      })}

      <CommonCard>
        <div style={{ fontSize: 15, fontWeight: 700, marginBottom: 8 }}>{t('对话保活设置')}</div>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
          <div>
            <div style={{ fontSize: 12.5, color: 'var(--fc-text-muted)', marginBottom: 4 }}>{t('保活模型链（逗号分隔）')}</div>
            <input
              className='fc-input'
              value={modelsText}
              onChange={e => setModelsText(e.target.value)}
              placeholder={(center.keepalive?.defaultModels || []).join(', ') || 'deepseek-v4.1-flash-sg'}
            />
          </div>
          <div>
            <div style={{ fontSize: 12.5, color: 'var(--fc-text-muted)', marginBottom: 4 }}>{t('使用的 API Key')}</div>
            <PickerTrigger
              valueLabel={currentKeyLabel}
              onClick={() => setKeyPickerOpen(true)}
            />
          </div>
          {center.keepalive?.authError ? (
            <div style={{ fontSize: 12.5, color: 'var(--fc-danger)' }}>{center.keepalive.authError}</div>
          ) : null}
          <button
            type='button'
            className='fc-btn is-primary is-sm'
            disabled={busyKey === 'keepalive'}
            onClick={saveKeepaliveConfig}
          >
            {busyKey === 'keepalive' ? t('保存中…') : t('保存保活设置')}
          </button>
        </div>
      </CommonCard>

      {center.travel?.available !== false ? (
        <CommonCard>
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 10, marginBottom: 10 }}>
            <div>
              <div style={{ fontSize: 15, fontWeight: 700 }}>{t('猫猫旅行定时')}</div>
              <div style={{ fontSize: 12, color: 'var(--fc-text-muted)' }}>
                {center.travel.lastFiredTime ? t('最近执行：{t}', { t: center.travel.lastFiredTime }) : t('每日按设定时刻巡检一次')}
              </div>
            </div>
            <CommonSwitch checked={travelEnabled} onChange={setTravelEnabled} />
          </div>
          <input
            className='fc-input'
            value={travelTimes}
            onChange={e => setTravelTimes(e.target.value)}
            placeholder='08:30, 20:30'
          />
          <div style={{ display: 'flex', gap: 8, marginTop: 10 }}>
            <button
              type='button'
              className='fc-btn is-primary is-sm'
              disabled={Boolean(busyKey)}
              onClick={saveTravelConfig}
            >
              {t('保存')}
            </button>
            <button
              type='button'
              className='fc-btn is-tonal is-sm'
              disabled={Boolean(busyKey)}
              onClick={() => void runAction('travel-run', () => runTravel(), t('已执行一轮猫猫旅行'))}
            >
              {busyKey === 'travel-run' ? t('执行中…') : t('立即巡检')}
            </button>
          </div>
        </CommonCard>
      ) : null}

      {checkinPluginTasks.length ? (
        <>
          <SectionHeader title={t('插件定时任务')} />
          <ListGroup>
            {checkinPluginTasks.map(row => {
              const k = `${row.provider}:${row.task.id}`
              const running = pluginTaskRunning(row)
              return (
                <ListItem
                  key={k}
                  title={row.task.label}
                  subtitle={`${row.providerLabel || row.provider} · ${row.task.times.join(', ') || t('按周期运行')}`}
                  trailing={
                    <div style={{ display: 'inline-flex', alignItems: 'center', gap: 8 }}>
                      <button
                        type='button'
                        className='fc-btn is-sm is-tonal'
                        disabled={Boolean(busyKey) || running || Boolean(row.unavailable)}
                        onClick={() =>
                          void runAction(
                            `pt:${k}`,
                            () => runPluginTaskNow(row.provider, row.task.id),
                            t('已提交后台运行'),
                          )
                        }
                      >
                        {running ? t('运行中') : t('运行')}
                      </button>
                      <CommonSwitch
                        checked={row.task.enabled}
                        disabled={Boolean(busyKey) || Boolean(row.unavailable)}
                        onChange={next =>
                          void runAction(`ps:${k}`, () =>
                            updatePluginTaskSettings(row.provider, row.task.id, { enabled: next }),
                          )
                        }
                      />
                    </div>
                  }
                />
              )
            })}
          </ListGroup>
        </>
      ) : null}

      {hasUmid ? (
        <CommonCard>
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 10 }}>
            <div>
              <div style={{ fontSize: 15, fontWeight: 700 }}>{t('网关设备组件 (Qoder UMID)')}</div>
              <div style={{ fontSize: 12, color: 'var(--fc-text-muted)', marginTop: 2 }}>
                {umid?.available ? t('已就绪') : umid?.installing ? t('安装中…') : t('未安装')}
              </div>
            </div>
            {umid?.installSupported && !umid?.available ? (
              <button
                type='button'
                className='fc-btn is-sm is-primary'
                disabled={Boolean(busyKey) || umid?.installing}
                onClick={() =>
                  void runAction('umid', () => installUmid().then(setUmid), t('正在安装网关设备组件'))
                }
              >
                {umid?.installing ? t('安装中…') : t('立即安装')}
              </button>
            ) : null}
          </div>
        </CommonCard>
      ) : null}

      <PickerSheet
        open={keyPickerOpen}
        title={t('选择保活 API Key')}
        options={keyOptions}
        value={apiKeyId}
        onSelect={setApiKeyId}
        onClose={() => setKeyPickerOpen(false)}
      />

      <ScheduleEditorSheet
        open={Boolean(editingSchedule)}
        entry={editingSchedule === 'new' ? null : editingSchedule}
        providerOptions={center.auto.providerOptions}
        defaultRetryTime={center.auto.defaultRetryTime || '08:30'}
        onClose={() => setEditingSchedule(null)}
        onSaved={async () => {
          setEditingSchedule(null)
          await onReload()
        }}
      />

      <BottomSheet
        open={Boolean(deleteTarget)}
        title={t('删除定时计划')}
        onClose={() => setDeleteTarget(null)}
        footer={
          <div style={{ display: 'flex', gap: 10, width: '100%' }}>
            <button type='button' className='fc-btn' style={{ flex: 1 }} onClick={() => setDeleteTarget(null)}>
              {t('取消')}
            </button>
            <button
              type='button'
              className='fc-btn is-danger'
              style={{ flex: 1 }}
              disabled={Boolean(busyKey)}
              onClick={() => {
                if (!deleteTarget) return
                const id = deleteTarget.id
                setDeleteTarget(null)
                void runAction(`del:${id}`, () => deleteCheckinSchedule(id), t('已删除定时计划'))
              }}
            >
              {t('确认删除')}
            </button>
          </div>
        }
      >
        <div style={{ fontSize: 14, color: 'var(--fc-text-muted)' }}>
          {t('确定要删除定时签到计划「{name}」吗？', { name: deleteTarget?.displayLabel || deleteTarget?.label || '' })}
        </div>
      </BottomSheet>
    </>
  )
}

function ScheduleEditorSheet({
  open,
  entry,
  providerOptions,
  defaultRetryTime,
  onClose,
  onSaved,
}: {
  open: boolean
  entry: ScheduleEntry | null
  providerOptions: ReadonlyArray<{ id: string; label: string }>
  defaultRetryTime: string
  onClose: () => void
  onSaved: () => Promise<void>
}) {
  const [label, setLabel] = useState('')
  const [time, setTime] = useState('00:05')
  const [timeEnd, setTimeEnd] = useState(() => defaultCheckinEnd('00:05'))
  const [providers, setProviders] = useState<string[]>([])
  const [retryTime, setRetryTime] = useState(defaultRetryTime)
  const [retryGap, setRetryGap] = useState('30')
  const [retryLimit, setRetryLimit] = useState('2')
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    if (!open) return
    const nextTime = entry?.time || '00:05'
    setLabel(entry?.label || '')
    setTime(nextTime)
    setTimeEnd(entry?.timeEnd || defaultCheckinEnd(nextTime))
    setProviders(entry?.providers?.length ? entry.providers : providerOptions.map(p => p.id))
    setRetryTime(entry?.retryTime ?? defaultRetryTime)
    setRetryGap(String(entry?.retryGapMinutes ?? 30))
    setRetryLimit(String(entry?.retryLimit ?? 2))
  }, [defaultRetryTime, entry, open, providerOptions])

  const toggleProvider = (id: string) => {
    setProviders(prev => (prev.includes(id) ? prev.filter(x => x !== id) : [...prev, id]))
  }

  const save = async () => {
    if (saving || !providers.length) return
    setSaving(true)
    try {
      await saveCheckinSchedule({
        ...(entry ? { id: entry.id } : {}),
        label: label.trim(),
        enabled: entry ? entry.enabled : true,
        time: time.trim() || '00:05',
        timeEnd: timeEnd.trim() || defaultCheckinEnd(time.trim() || '00:05'),
        providers,
        retryTime: retryTime.trim(),
        retryGapMinutes: Number(retryGap) || 0,
        retryLimit: Number(retryLimit) || 0,
      })
      store.toast(t('定时计划已保存'), 'success')
      await onSaved()
    } catch (err) {
      store.toast(err instanceof Error ? err.message : String(err), 'danger')
    } finally {
      setSaving(false)
    }
  }

  return (
    <BottomSheet
      open={open}
      title={entry ? t('编辑定时计划') : t('新增定时计划')}
      onClose={onClose}
      footer={
        <div style={{ display: 'flex', gap: 10, width: '100%' }}>
          <button type='button' className='fc-btn' style={{ flex: 1 }} onClick={onClose}>
            {t('取消')}
          </button>
          <button
            type='button'
            className='fc-btn is-primary'
            style={{ flex: 1 }}
            disabled={saving || !providers.length}
            onClick={() => void save()}
          >
            {saving ? t('保存中…') : t('保存')}
          </button>
        </div>
      }
    >
      <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
        <div>
          <div style={{ fontSize: 12.5, color: 'var(--fc-text-muted)', marginBottom: 4 }}>{t('计划名称')}</div>
          <input className='fc-input' value={label} onChange={e => setLabel(e.target.value)} placeholder={t('例如：每日零点签到')} />
        </div>
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 10 }}>
          <div>
            <div style={{ fontSize: 12.5, color: 'var(--fc-text-muted)', marginBottom: 4 }}>{t('开始时间')}</div>
            <input className='fc-input fc-mono' value={time} onChange={e => setTime(e.target.value)} placeholder='00:05' />
          </div>
          <div>
            <div style={{ fontSize: 12.5, color: 'var(--fc-text-muted)', marginBottom: 4 }}>{t('结束时间')}</div>
            <input className='fc-input fc-mono' value={timeEnd} onChange={e => setTimeEnd(e.target.value)} placeholder='00:14' />
          </div>
        </div>
        <div>
          <div style={{ fontSize: 12.5, color: 'var(--fc-text-muted)', marginBottom: 6 }}>{t('包含的提供商')}</div>
          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6 }}>
            {providerOptions.map(p => {
              const active = providers.includes(p.id)
              return (
                <button
                  key={p.id}
                  type='button'
                  className={`fc-filter-pill ${active ? 'is-active' : ''}`}
                  onClick={() => toggleProvider(p.id)}
                >
                  {p.label}
                </button>
              )
            })}
          </div>
        </div>
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr 1fr', gap: 8 }}>
          <div>
            <div style={{ fontSize: 12, color: 'var(--fc-text-muted)', marginBottom: 4 }}>{t('补签时刻')}</div>
            <input className='fc-input fc-mono' value={retryTime} onChange={e => setRetryTime(e.target.value)} placeholder='08:30' />
          </div>
          <div>
            <div style={{ fontSize: 12, color: 'var(--fc-text-muted)', marginBottom: 4 }}>{t('间隔(分)')}</div>
            <input className='fc-input fc-mono' type='number' value={retryGap} onChange={e => setRetryGap(e.target.value)} />
          </div>
          <div>
            <div style={{ fontSize: 12, color: 'var(--fc-text-muted)', marginBottom: 4 }}>{t('次数上限')}</div>
            <input className='fc-input fc-mono' type='number' value={retryLimit} onChange={e => setRetryLimit(e.target.value)} />
          </div>
        </div>
      </div>
    </BottomSheet>
  )
}
