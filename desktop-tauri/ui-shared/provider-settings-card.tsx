import * as React from 'react'
import { Alert, Badge, Button, Card, Input, Label, Switch } from '@ui'
import { SearchableSelect } from './searchable-select'
import { documentText } from './provider-docs'
import {
  providerSettingsLabel, providerSettingsMembers,
  type ProviderSettingsRow, type ProviderSettingsTranslate, type ProviderSettingValue, type UpstreamSyncStatus,
} from './provider-settings'

function formatSyncTime(ms: number | undefined, locale: string): string {
  if (!ms || ms <= 0) return ''
  try {
    return new Intl.DateTimeFormat(locale || undefined, {
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
      hour12: false,
    }).format(new Date(ms))
  } catch {
    return new Date(ms).toLocaleString()
  }
}

export function ProviderSettingsCard({ row, t, locale, saveValues, syncUpstream, refresh, onDirtyChange }: {
  row: ProviderSettingsRow
  t: ProviderSettingsTranslate
  locale: string
  saveValues: (row: ProviderSettingsRow, values: Record<string, ProviderSettingValue>) => Promise<ProviderSettingsRow>
  syncUpstream?: (sourceId: string) => Promise<{ version?: string | null; status?: UpstreamSyncStatus }>
  refresh: () => Promise<void>
  onDirtyChange: (dirty: boolean) => void
}) {
  const [draft, setDraft] = React.useState<Record<string, ProviderSettingValue>>({})
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const [notice, setNotice] = React.useState<{ text: string; warning: boolean } | null>(null)
  const [syncBusy, setSyncBusy] = React.useState<Record<string, boolean>>({})
  const [syncError, setSyncError] = React.useState<Record<string, string>>({})
  const [syncNotice, setSyncNotice] = React.useState<Record<string, string>>({})
  const [cooldownEnds, setCooldownEnds] = React.useState<Record<string, number>>({})
  const [nowMs, setNowMs] = React.useState(() => Date.now())
  const alive = React.useRef(true)
  const saving = React.useRef(false)
  const baseline = React.useRef<ProviderSettingsRow | null>(null)
  const id = React.useId()
  React.useEffect(() => { alive.current = true; return () => { alive.current = false } }, [])
  React.useEffect(() => {
    const entries = Object.entries(row.upstreamSync || {})
    if (!entries.length) {
      setCooldownEnds(previous => Object.keys(previous).length ? {} : previous)
      return
    }
    const now = Date.now()
    const next: Record<string, number> = {}
    for (const [sourceId, status] of entries) {
      const secs = status?.cooldownRemainingSeconds ?? 0
      if (secs > 0) next[sourceId] = now + secs * 1000
    }
    setCooldownEnds(next)
  }, [row.upstreamSync])
  React.useEffect(() => {
    const now = Date.now()
    setNowMs(now)
    if (!Object.values(cooldownEnds).some(end => end > now)) return
    const timer = window.setInterval(() => {
      const tick = Date.now()
      setNowMs(tick)
      if (!Object.values(cooldownEnds).some(end => end > tick)) {
        window.clearInterval(timer)
      }
    }, 1000)
    return () => window.clearInterval(timer)
  }, [cooldownEnds])
  const view = baseline.current || row
  const dirty = Object.keys(draft).length > 0
  const changed = baseline.current !== null && (baseline.current.revision !== row.revision || baseline.current.provider !== row.provider)
  const locked = busy || changed || Boolean(row.error) || !row.revision
  const members = providerSettingsMembers(row)
  const label = providerSettingsLabel(row, locale, t)
  const customized = Object.values(row.values || {}).some(value => value !== null)

  function edit(key: string, value: ProviderSettingValue) {
    if (locked) return
    if (!baseline.current) baseline.current = row
    const next = { ...draft, [key]: value }
    if (value === (view.values?.[key] ?? null) && !view.invalid?.[key]) delete next[key]
    if (!Object.keys(next).length) baseline.current = null
    setDraft(next)
    setError('')
    setNotice(null)
    onDirtyChange(Object.keys(next).length > 0)
  }

  async function save() {
    if (locked || saving.current || !dirty) return
    saving.current = true
    setBusy(true); setError(''); setNotice(null)
    try {
      const result = await saveValues(view, draft)
      if (!alive.current) return
      baseline.current = null
      setDraft({})
      onDirtyChange(false)
      setNotice({ warning: result.saved === false, text: result.saved === false
        ? t('设置已生效，但写入磁盘失败（重启后会恢复原设置）') : t('提供商设置已保存') })
      await refresh()
    } catch (cause) {
      if (alive.current) setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      saving.current = false
      if (alive.current) setBusy(false)
    }
  }

  function reset() {
    baseline.current = null
    setDraft({}); setError(''); setNotice(null)
    onDirtyChange(false)
    void refresh()
  }

  async function triggerSync(sourceId: string, fallbackVersion: string) {
    if (!sourceId || !syncUpstream || syncBusy[sourceId]) return
    setSyncBusy(prev => ({ ...prev, [sourceId]: true }))
    setSyncError(prev => ({ ...prev, [sourceId]: '' }))
    setSyncNotice(prev => ({ ...prev, [sourceId]: '' }))
    try {
      const result = await syncUpstream(sourceId)
      if (!alive.current) return
      const secs = result?.status?.cooldownRemainingSeconds ?? 60
      if (secs > 0) {
        setCooldownEnds(prev => ({ ...prev, [sourceId]: Date.now() + secs * 1000 }))
      }
      const synced = result?.version?.trim() || result?.status?.version?.trim() || fallbackVersion
      setSyncNotice(prev => ({
        ...prev,
        [sourceId]: synced ? t('已同步最新版本：{version}', { version: synced }) : t('已完成同步检查'),
      }))
      await refresh()
    } catch (cause) {
      if (!alive.current) return
      const message = cause instanceof Error ? cause.message : String(cause)
      setSyncError(prev => ({ ...prev, [sourceId]: message }))
      const match = message.match(/(\d+)\s*秒/)
      const secs = match ? Number(match[1]) : 0
      if (secs > 0 && Number.isFinite(secs)) {
        setCooldownEnds(prev => ({ ...prev, [sourceId]: Date.now() + secs * 1000 }))
      }
      void refresh()
    } finally {
      if (alive.current) setSyncBusy(prev => ({ ...prev, [sourceId]: false }))
    }
  }

  return <Card className='min-w-0 overflow-hidden'>
    <form onSubmit={event => { event.preventDefault(); void save() }}>
      <div className='flex flex-wrap items-start justify-between gap-3 border-b border-hairline px-4 py-3.5'>
        <div className='min-w-0'>
          <div className='flex flex-wrap items-center gap-2'>
            <h3 className='text-sm font-semibold'>{label}</h3>
            <Badge variant='outline' shape='tag'>{dirty ? t('有未保存的修改') : customized ? t('已自定义') : t('使用默认值')}</Badge>
          </div>
          {row.schema ? <p className='mt-1 text-xs text-muted-foreground'>{documentText(row.schema.title, locale)}</p> : null}
        </div>
        <Button type='button' size='sm' variant='ghost' disabled={busy} onClick={reset}>
          {dirty ? t('放弃修改并刷新') : t('刷新')}
        </Button>
      </div>
      <div className='flex flex-col gap-5 p-4'>
        {members.length > 1 ? <p className='text-xs text-muted-foreground'>
          {t('共用设置：{providers}', { providers: members.map(provider => t(provider.label)).join(' / ') })}
        </p> : null}
        {row.schema?.description ? <p className='whitespace-pre-wrap text-xs leading-relaxed text-muted-foreground'>{documentText(row.schema.description, locale)}</p> : null}
        {row.error ? <Alert variant='destructive'>{row.error}</Alert> : null}
        {changed ? <Alert>{t('插件或设置已变化，请放弃修改并刷新后重新编辑。')}</Alert> : null}
        {view.schema?.fields.map(field => {
          const value = Object.prototype.hasOwnProperty.call(draft, field.key) ? draft[field.key] : view.values?.[field.key]
          const current = value ?? field.default
          const override = row.overrides?.[field.key]
          const fieldLabel = documentText(field.label, locale)
          const inputId = `${id}-${field.key}`
          const set = (value: ProviderSettingValue) => edit(field.key, value)
          if (field.type === 'upstream_version') {
            const sourceId = field.source_id || ''
            const sync = sourceId ? row.upstreamSync?.[sourceId] : undefined
            const defaultHours = sync?.defaultHours ?? 6
            const minHours = sync?.minHours ?? 1
            const maxHours = sync?.maxHours ?? 24
            const fallbackVersion = field.default == null ? '' : String(field.default)
            const syncedVersion = sync?.version?.trim() || ''
            const activeVersion = syncedVersion || fallbackVersion
            const busySync = Boolean(sourceId && (syncBusy[sourceId] || sync?.syncing))
            const remainingSecs = sourceId ? Math.max(0, Math.ceil(((cooldownEnds[sourceId] ?? 0) - nowMs) / 1000)) : 0
            const syncedText = formatSyncTime(sync?.syncedAt, locale)
            const nextRunText = formatSyncTime(sync?.nextRunAt, locale)
            const nextText = nextRunText
              ? (sync?.nextRunAt && sync.nextRunAt <= nowMs ? t('即将自动检查') : t('下次自动检查：{time}', { time: nextRunText }))
              : ''
            const statusLine = [syncedText ? t('上次同步：{time}', { time: syncedText }) : t('尚未同步'), nextText]
              .filter(Boolean).join(' · ')
            const blockingError = (sourceId && syncError[sourceId]) || (!syncedVersion && sync?.lastError ? sync.lastError : '')
            const backgroundWarning = !blockingError && syncedVersion && sync?.lastError ? sync.lastError : ''
            const syncNoticeText = sourceId ? syncNotice[sourceId] : ''
            return <div key={field.key} className='flex min-w-0 flex-col gap-2.5'>
              <p className='text-sm font-medium leading-none'>{fieldLabel}</p>
              <div className='flex flex-col gap-3 rounded-lg border border-hairline bg-muted/20 p-3'>
                <div className='flex flex-wrap items-center justify-between gap-2'>
                  <div className='min-w-0 flex flex-col gap-1'>
                    <div className='flex flex-wrap items-center gap-2'>
                      <span className='font-mono text-sm font-semibold'>{activeVersion || '—'}</span>
                      {!syncedVersion && fallbackVersion ? <Badge variant='outline' shape='tag'>{t('内置保底')}</Badge> : null}
                    </div>
                    <p className='text-xs text-muted-foreground'>{statusLine}</p>
                  </div>
                  <Button type='button' size='sm' variant='outline'
                    disabled={locked || busySync || !sourceId || !syncUpstream || remainingSecs > 0}
                    onClick={() => void triggerSync(sourceId, fallbackVersion)}>
                    {busySync ? t('同步中…')
                      : remainingSecs > 0 ? t('冷却中（{seconds} 秒）', { seconds: remainingSecs })
                        : t('立即同步')}
                  </Button>
                </div>
                <div className='flex flex-wrap items-center gap-2 border-t border-hairline pt-2.5'>
                  <Label htmlFor={inputId} className='text-xs text-muted-foreground'>{t('自动拉取间隔（小时）')}</Label>
                  <Input id={inputId} className='w-20' aria-label={t('自动拉取间隔（小时）')} disabled={locked}
                    type='number' autoComplete='off' min={minHours} max={maxHours} step={1}
                    value={value == null || Number.isNaN(value) ? '' : String(value)}
                    placeholder={String(defaultHours)}
                    onChange={event => {
                      const raw = event.currentTarget.value.trim()
                      set(raw === '' ? null : Number(raw))
                    }} />
                  <span className='text-xs text-muted-foreground'>
                    {t('小时（{min}–{max}，默认 {hours}）', { min: minHours, max: maxHours, hours: defaultHours })}
                  </span>
                  <Button type='button' size='sm' variant='outline' disabled={locked || value == null && !row.invalid?.[field.key]}
                    onClick={() => set(null)}>{t('恢复默认（{hours} 小时）', { hours: defaultHours })}</Button>
                </div>
              </div>
              {blockingError ? <Alert variant='destructive'>{blockingError}</Alert> : null}
              {backgroundWarning ? <Alert variant='warning'>
                {t('最近一次后台检查失败，继续沿用已缓存版本：{error}', { error: backgroundWarning })}
              </Alert> : null}
              {syncNoticeText ? <Alert>{syncNoticeText}</Alert> : null}
              {row.invalid?.[field.key] ? <Alert variant='destructive'>{t('已保存的值不符合当前插件规则，请修改或恢复默认值。')}</Alert> : null}
              {field.description ? <p className='whitespace-pre-wrap text-xs leading-relaxed text-muted-foreground'>{documentText(field.description, locale)}</p> : null}
            </div>
          }
          const effective = row.effective?.[field.key]
          const effectiveText = field.type === 'select'
            ? documentText(field.options.find(option => option.value === effective)?.label, locale) || String(effective ?? '—')
            : typeof effective === 'boolean' ? effective ? t('已开启') : t('已关闭') : String(effective ?? '—')
          return <div key={field.key} className='flex min-w-0 flex-col gap-2'>
            <Label htmlFor={inputId}>{fieldLabel}</Label>
            <div className='flex flex-wrap items-center gap-2'>
              {field.type === 'boolean' ? <Switch id={inputId} checked={current === true} disabled={locked} aria-label={fieldLabel}
                onCheckedChange={value => set(value === true)} />
                : field.type === 'select' ? <SearchableSelect id={inputId} value={typeof current === 'string' ? current : ''}
                  disabled={locked} onValueChange={value => set(value || null)} className='min-w-36 flex-1' aria-label={fieldLabel}
                  searchable={field.options.length > 8}
                  displayValue={documentText(field.options.find(option => option.value === current)?.label, locale) || t('请选择')}
                  options={field.options.map(option => ({ value: option.value, label: documentText(option.label, locale) }))} />
                  : <Input id={inputId} className='min-w-36 flex-1' aria-label={fieldLabel} disabled={locked}
                    type={field.type === 'number' ? 'number' : 'text'} autoComplete='off' spellCheck={false}
                    value={value == null ? '' : String(value)} placeholder={field.default == null ? '' : String(field.default)}
                    min={field.minimum ?? undefined} max={field.maximum ?? undefined}
                    maxLength={field.max_length ?? undefined} step={field.type === 'number' ? 'any' : undefined}
                    onChange={event => {
                      const value = event.currentTarget.value
                      set(field.type === 'number' ? value === '' ? null : Number(value) : value)
                    }} />}
              <Button type='button' size='sm' variant='outline' disabled={locked || value == null && !row.invalid?.[field.key]}
                onClick={() => set(null)}>{t('恢复插件默认值')}</Button>
            </div>
            <p className='break-words text-xs text-muted-foreground'>{t('当前生效值：{value}', { value: effectiveText })}</p>
            {row.invalid?.[field.key] ? <Alert variant='destructive'>{t('已保存的值不符合当前插件规则，请修改或恢复默认值。')}</Alert> : null}
            {override ? <Alert>{t('由 {source} 覆盖，手动设置暂不生效', { source: override.source })}</Alert> : null}
            {field.description ? <p className='whitespace-pre-wrap text-xs leading-relaxed text-muted-foreground'>{documentText(field.description, locale)}</p> : null}
          </div>
        })}
        {error ? <Alert variant='destructive'>{error}</Alert> : null}
        {notice ? <Alert variant={notice.warning ? 'warning' : 'default'}>{notice.text}</Alert> : null}
        {view.schema ? <div className='flex justify-end'>
          <Button type='submit' size='sm' disabled={locked || !dirty}>{busy ? t('保存中…') : t('保存')}</Button>
        </div> : null}
      </div>
    </form>
  </Card>
}
