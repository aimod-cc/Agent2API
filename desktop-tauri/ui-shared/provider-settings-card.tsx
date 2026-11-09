import * as React from 'react'
import { Alert, Badge, Button, Card, Input, Label, Switch } from '@ui'
import { SearchableSelect } from './searchable-select'
import { documentText } from './provider-docs'
import {
  providerSettingsLabel, providerSettingsMembers,
  type ProviderSettingsRow, type ProviderSettingsTranslate, type ProviderSettingValue,
} from './provider-settings'

export function ProviderSettingsCard({ row, t, locale, saveValues, refresh, onDirtyChange }: {
  row: ProviderSettingsRow
  t: ProviderSettingsTranslate
  locale: string
  saveValues: (row: ProviderSettingsRow, values: Record<string, ProviderSettingValue>) => Promise<ProviderSettingsRow>
  refresh: () => Promise<void>
  onDirtyChange: (dirty: boolean) => void
}) {
  const [draft, setDraft] = React.useState<Record<string, ProviderSettingValue>>({})
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const [notice, setNotice] = React.useState<{ text: string; warning: boolean } | null>(null)
  const alive = React.useRef(true)
  const saving = React.useRef(false)
  const baseline = React.useRef<ProviderSettingsRow | null>(null)
  const id = React.useId()
  React.useEffect(() => { alive.current = true; return () => { alive.current = false } }, [])
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
