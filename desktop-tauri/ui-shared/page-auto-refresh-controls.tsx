import * as React from 'react'
import { Button, Input, Label, Switch } from '@ui'
import type { PageRefreshPatch, PageRefreshSettings, RefreshTranslator } from './page-auto-refresh'

type Props = {
  settings: PageRefreshSettings
  onRefresh(): Promise<unknown>
  disabled?: boolean
  touch?: boolean
  t: RefreshTranslator
  notify(message: string, kind?: 'ok' | 'err'): void
}

export function PageAutoRefreshControls({ settings, onRefresh, disabled, touch, t, notify }: Props) {
  const [draft, setDraft] = React.useState<string | null>(null)
  const [refreshing, setRefreshing] = React.useState(false)
  const manualBusy = React.useRef(false)
  const { task } = settings
  const switchId = React.useId()
  const inputId = React.useId()
  const hintId = React.useId()
  const hint = task
    ? t('可填 {min}–{max} 秒，关闭后保留间隔', { min: task.min, max: task.max })
    : settings.error || t('正在加载…')

  async function save(patch: PageRefreshPatch) {
    try {
      await settings.save(patch)
    } catch (error) {
      const reason = error instanceof Error ? error.message : t('未知错误')
      notify(t('保存失败：{reason}', { reason }), 'err')
    }
  }

  function submitInterval(raw: string) {
    setDraft(null)
    if (!task) return
    const text = raw.trim()
    const value = Number(text)
    if (!/^\d+$/.test(text) || !Number.isInteger(value) || value < task.min || value > task.max) {
      notify(t('请输入 {min}–{max} 之间的整数秒数', { min: task.min, max: task.max }), 'err')
      return
    }
    if (value !== task.interval) void save({ interval: value })
  }

  async function refresh() {
    if (manualBusy.current || disabled) return
    manualBusy.current = true
    setRefreshing(true)
    try {
      await Promise.all([onRefresh(), settings.reload()])
    } catch (error) {
      const reason = error instanceof Error ? error.message : t('未知错误')
      notify(t('刷新失败：{reason}', { reason }), 'err')
    } finally {
      manualBusy.current = false
      setRefreshing(false)
    }
  }

  return <div className='page-auto-refresh' data-touch={touch ? 'true' : undefined} role='group' aria-label={t('页面自动刷新')}>
    <div className='page-auto-refresh-settings' aria-busy={settings.saving || settings.loading}>
      <Label htmlFor={switchId} className='page-auto-refresh-label'>{t('自动刷新')}</Label>
      <Switch id={switchId} size={touch ? 'touch' : 'default'} checked={task?.enabled === true}
        disabled={!task} aria-describedby={hintId} onCheckedChange={enabled => void save({ enabled })} />
      <Label htmlFor={inputId} className='page-auto-refresh-interval' data-disabled={!settings.enabled || undefined}>
        <span>{t('每')}</span>
        <Input id={inputId} type='number' inputMode='numeric' min={task?.min ?? 1} max={task?.max ?? 600} step={1}
          className='page-auto-refresh-input' aria-label={t('自动刷新间隔（秒）')} aria-describedby={hintId}
          disabled={!task || !settings.enabled} value={draft ?? (task ? String(task.interval) : '')} placeholder='—' title={hint}
          onChange={event => setDraft(event.currentTarget.value)}
          onBlur={event => { if (draft !== null) submitInterval(event.currentTarget.value) }}
          onKeyDown={event => { if (event.key === 'Enter') { event.preventDefault(); event.currentTarget.blur() } }} />
        <span>{t('秒')}</span>
      </Label>
    </div>
    <span id={hintId} className='sr-only'>{hint}</span>
    {settings.error ? <span className='page-auto-refresh-error' role='status' title={settings.error}>{t('设置未同步')}</span> : null}
    <Button variant='outline' size={touch ? 'touch-sm' : 'default'} disabled={disabled || refreshing}
      className='page-auto-refresh-button' aria-busy={refreshing} onClick={() => void refresh()}>
      <svg width='14' height='14' viewBox='0 0 24 24' fill='none' stroke='currentColor' strokeWidth='1.8'
        strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
        <path d='M20 4v5h-5M20 9a8 8 0 1 0 .1 6' />
      </svg>
      {refreshing ? t('刷新中…') : t('立即刷新')}
    </Button>
  </div>
}
