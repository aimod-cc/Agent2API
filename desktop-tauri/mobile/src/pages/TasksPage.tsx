import { useState } from 'react'
import { PageIntro, IconButton, Empty, ErrorNote, SkeletonList } from '../components/ui'
import { GlyphLogo } from '../components/ProviderLogo'
import { Icon } from '../components/Icon'
import { store } from '../state/store'
import {
  fetchScheduledTasks,
  patchScheduledTask,
  runScheduledTask,
  type ScheduledTask,
} from '../api/gateway'
import { formatRelative, formatStamp } from '../lib/format'
import { usePolling } from '../lib/usePolling'
import { t } from '../i18n'

/**
 * 定时任务。
 *
 * 手机上以「只读 + 开关」为主：改间隔要填数字，窄屏上既费事又容易点错，
 * 收进抽屉且给了上下限提示。后端任务额外给「立即执行」。
 *
 * 与桌面端一致：签到是每天定点型，不在这张表里（它有自己的页）。
 */
export function TasksPage() {
  const [tasks, setTasks] = useState<ScheduledTask[] | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState('')
  const [editing, setEditing] = useState<ScheduledTask | null>(null)
  const [draft, setDraft] = useState('')

  const load = async () => {
    try {
      const data = await fetchScheduledTasks()
      setTasks(data.tasks || [])
      setError('')
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
    }
  }

  usePolling(load, 30_000)

  const list = tasks || []
  const backend = list.filter(item => item.runner === 'backend')
  const frontend = list.filter(item => item.runner === 'frontend')
  const enabledCount = list.filter(item => item.enabled).length

  const toggle = async (task: ScheduledTask, next: boolean) => {
    if (!task.id) return
    setBusy(task.id)
    try {
      const data = await patchScheduledTask(task.id, { enabled: next })
      setTasks(data.tasks || [])
      store.toast(t('已{action}「{name}」', {
        action: next ? t('开启') : t('关闭'),
        name: task.label || task.id,
      }))
    } catch (failure) {
      store.toast(failure instanceof Error ? failure.message : String(failure), 'err')
    } finally {
      setBusy('')
    }
  }

  const run = async (task: ScheduledTask) => {
    if (!task.id) return
    setBusy(task.id)
    try {
      await runScheduledTask(task.id)
      store.toast(t('「{name}」已执行', { name: task.label || task.id }))
      await load()
    } catch (failure) {
      store.toast(failure instanceof Error ? failure.message : String(failure), 'err')
    } finally {
      setBusy('')
    }
  }

  const saveInterval = async () => {
    if (!editing?.id) return
    const value = Number(draft)
    if (!Number.isInteger(value) || value < (editing.min || 1) || value > (editing.max || 9999)) {
      store.toast(
        t('间隔必须是 {min}–{max} 的整数', { min: editing.min || 1, max: editing.max || 9999 }),
        'err',
      )
      return
    }
    setBusy(editing.id)
    try {
      const data = await patchScheduledTask(editing.id, { interval: value })
      setTasks(data.tasks || [])
      store.toast(t('已更新间隔'))
      setEditing(null)
    } catch (failure) {
      store.toast(failure instanceof Error ? failure.message : String(failure), 'err')
    } finally {
      setBusy('')
    }
  }

  return (
    <div className='m-page'>
      <PageIntro actions={<IconButton name='refresh' label={t('刷新')} onClick={() => void load()} />}
      />

      {error ? <ErrorNote text={error} /> : null}
      {!tasks ? <SkeletonList rows={4} height={64} /> : null}

      {tasks ? (
        <>
          <div className='m-section' style={{ marginTop: 0 }}>
            <div className='m-section-head'>
              <h2>{t('后台任务')}</h2>
            </div>
            <div className='m-card'>
              {backend.length ? (
                backend.map(task => (
                  <TaskRow
                    key={task.id}
                    task={task}
                    busy={busy === task.id}
                    onToggle={next => void toggle(task, next)}
                    onRun={() => void run(task)}
                    onEdit={() => {
                      setEditing(task)
                      setDraft(String(task.interval ?? ''))
                    }}
                  />
                ))
              ) : (
                <Empty text={t('没有后台任务')} />
              )}
            </div>
          </div>

          {frontend.length ? (
            <div className='m-section'>
              <div className='m-section-head'>
                <h2>{t('页面自动刷新')}</h2>
              </div>
              <div className='m-card'>
                {frontend.map(task => (
                  <TaskRow
                    key={task.id}
                    task={task}
                    busy={busy === task.id}
                    onToggle={next => void toggle(task, next)}
                  />
                ))}
              </div>
              <div style={{ marginTop: 8, fontSize: 11.5, color: 'var(--m-text-3)', padding: '0 2px' }}>
                {t('页面刷新由浏览器执行，只在页面打开时生效')}
              </div>
            </div>
          ) : null}
        </>
      ) : null}

      {/* 间隔编辑：用底部抽屉而不是居中弹窗 */}
      {editing ? (
        <div
          style={{
            position: 'fixed',
            inset: 0,
            zIndex: 50,
            display: 'flex',
            alignItems: 'flex-end',
            background: 'color-mix(in srgb, black 42%, transparent)',
          }}
          onClick={() => setEditing(null)}
        >
          <div
            onClick={event => event.stopPropagation()}
            style={{
              width: '100%',
              padding: '18px 18px calc(20px + var(--m-safe-b))',
              borderTopLeftRadius: 24,
              borderTopRightRadius: 24,
              background: 'var(--m-surface)',
              borderTop: '1px solid var(--m-line)',
            }}
          >
            <div style={{ fontSize: 16, fontWeight: 600 }}>{editing.label}</div>
            <div style={{ fontSize: 12, color: 'var(--m-text-3)', marginTop: 4 }}>
              {t('可填 {min}–{max} {unit}', {
                min: editing.min || 1,
                max: editing.max || 9999,
                unit: editing.unit === 'seconds' ? t('秒') : t('分钟'),
              })}
            </div>
            <div style={{ display: 'flex', gap: 8, marginTop: 14 }}>
              <input
                value={draft}
                onChange={event => setDraft(event.target.value.replace(/[^\d]/g, ''))}
                inputMode='numeric'
                style={{
                  flex: 1,
                  height: 46,
                  padding: '0 12px',
                  borderRadius: 12,
                  border: '1px solid var(--m-line-strong)',
                  background: 'var(--m-surface-2)',
                  color: 'var(--m-text)',
                  fontSize: 16,
                  fontFamily: 'var(--m-mono)',
                }}
              />
              <span style={{ display: 'grid', placeItems: 'center', fontSize: 13, color: 'var(--m-text-3)', width: 40 }}>
                {editing.unit === 'seconds' ? t('秒') : t('分')}
              </span>
            </div>
            <div style={{ display: 'flex', gap: 8, marginTop: 14 }}>
              <button type='button' className='m-btn m-ghost' style={{ flex: 1 }} onClick={() => setEditing(null)}>
                {t('取消')}
              </button>
              <button type='button' className='m-btn' style={{ flex: 1 }} onClick={() => void saveInterval()}>
                {t('保存')}
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </div>
  )
}

function TaskRow({
  task,
  busy,
  onToggle,
  onRun,
  onEdit,
}: {
  task: ScheduledTask
  busy: boolean
  onToggle: (next: boolean) => void
  onRun?: () => void
  onEdit?: () => void
}) {
  const intervalText =
    task.interval && task.unit
      ? t('每 {n} {unit}', { n: task.interval, unit: task.unit === 'seconds' ? t('秒') : t('分钟') })
      : '—'
  const next = task.nextRunAt ? formatRelative(task.nextRunAt) : ''
  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 11,
        padding: '13px 14px',
        borderBottom: '1px solid var(--m-line)',
      }}
    >
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ fontSize: 13.5, fontWeight: 550 }}>{task.label}</div>
        <div style={{ fontSize: 11.5, color: 'var(--m-text-3)', marginTop: 3 }}>
          {intervalText}
          {task.lastRunAt ? ` · ${formatStamp(task.lastRunAt)}` : ''}
          {next ? ` · ${next}` : ''}
        </div>
        {task.lastError ? (
          <div style={{ fontSize: 11, color: 'var(--m-danger)', marginTop: 3 }}>{task.lastError}</div>
        ) : task.lastResult ? (
          <div style={{ fontSize: 11, color: 'var(--m-text-3)', marginTop: 3 }}>{task.lastResult}</div>
        ) : null}
        {task.canRun && onRun ? (
          <div style={{ display: 'flex', gap: 6, marginTop: 8 }}>
            <button
              type='button'
              className='m-btn m-sm m-ghost'
              disabled={busy || task.running}
              onClick={onRun}
              style={{ height: 28, fontSize: 11.5, padding: '0 10px' }}
            >
              <Icon name='play' style={{ width: 12, height: 12 }} />
              {task.running ? t('执行中…') : t('立即执行')}
            </button>
            {onEdit ? (
              <button
                type='button'
                className='m-btn m-sm m-ghost'
                disabled={busy}
                onClick={onEdit}
                style={{ height: 28, fontSize: 11.5, padding: '0 10px' }}
              >
                <Icon name='edit' style={{ width: 12, height: 12 }} />
                {t('改间隔')}
              </button>
            ) : null}
          </div>
        ) : null}
      </div>
      <SwitchSmall checked={Boolean(task.enabled)} disabled={busy} label={task.label || ''} onChange={onToggle} />
    </div>
  )
}

function SwitchSmall({
  checked,
  onChange,
  disabled,
  label,
}: {
  checked: boolean
  onChange: (next: boolean) => void
  disabled?: boolean
  label: string
}) {
  return (
    <button
      type='button'
      role='switch'
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      style={{
        width: 40,
        height: 24,
        borderRadius: 999,
        border: checked ? '0' : '1px solid var(--m-line-strong)',
        background: checked ? 'var(--m-ok)' : 'var(--m-surface-3)',
        position: 'relative',
        transition: 'background .18s',
        padding: 0,
        flex: 'none',
        opacity: disabled ? 0.5 : 1,
      }}
    >
      <span
        style={{
          position: 'absolute',
          top: 2,
          left: checked ? 18 : 2,
          width: 20,
          height: 20,
          borderRadius: '50%',
          background: '#fff',
          boxShadow: '0 1px 3px rgba(0,0,0,.25)',
          transition: 'left .18s cubic-bezier(.3,.9,.4,1)',
        }}
      />
    </button>
  )
}
