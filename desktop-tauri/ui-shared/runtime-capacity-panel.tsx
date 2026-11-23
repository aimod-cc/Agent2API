import * as React from 'react'
import { Button, Input, Label, Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@ui'
import type { CapacityApi, CapacitySettings, CapacitySnapshot } from './runtime-capacity'
import type { PluginTranslate } from './plugins-store'

const MIB = 1024 * 1024
const fields = (t: PluginTranslate): Array<{ key: keyof Omit<CapacitySettings, 'mode' | 'providerLimits'>; label: string; min: number; max: number }> => [
  { key: 'maxConversations', label: t('对话并发上限'), min: 1, max: 4096 },
  { key: 'maxInteractive', label: t('登录与续期并发'), min: 4, max: 256 },
  { key: 'maxBackground', label: t('后台任务并发'), min: 1, max: 256 },
  { key: 'backgroundPerProvider', label: t('单提供商后台并发'), min: 1, max: 64 },
  { key: 'maxImages', label: t('图片任务并发'), min: 1, max: 128 },
  { key: 'maxConnections', label: t('出站连接上限'), min: 64, max: 32768 },
  { key: 'memoryMib', label: t('运行内存预算（MiB）'), min: 0, max: 262144 },
  { key: 'queueRequests', label: t('排队请求上限'), min: 1, max: 16384 },
  { key: 'queueMemoryMib', label: t('排队内存上限（MiB）'), min: 8, max: 4096 },
  { key: 'queueWaitSeconds', label: t('最长排队时间（秒）'), min: 1, max: 120 },
]

export function RuntimeCapacityPanel({ api, t, identity = '', onSaved }: {
  api: CapacityApi
  t: PluginTranslate
  identity?: string
  onSaved?: () => void
}) {
  const [snapshot, setSnapshot] = React.useState<CapacitySnapshot | null>(null)
  const [draft, setDraft] = React.useState<CapacitySettings | null>(null)
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const [saved, setSaved] = React.useState(false)
  const [draftRevision, setDraftRevision] = React.useState('')
  const edited = React.useRef(false)
  const generation = React.useRef(0)
  const currentApi = React.useRef(api)
  currentApi.current = api

  React.useEffect(() => {
    const id = ++generation.current
    let active = true
    const load = async () => {
      try {
        const next = await currentApi.current<CapacitySnapshot>('GET', '/api/plugins/capacity')
        if (!active || generation.current !== id) return
        setSnapshot(next)
        setError('')
        if (!edited.current) { setDraft(next.settings); setDraftRevision(next.revision) }
      } catch (cause) {
        if (active) setError(cause instanceof Error ? cause.message : String(cause))
      }
    }
    setSnapshot(null)
    setDraft(null)
    edited.current = false
    setDraftRevision('')
    setError('')
    setBusy(false)
    setSaved(false)
    void load()
    const timer = window.setInterval(() => { if (!document.hidden) void load() }, 3000)
    return () => { active = false; generation.current++; window.clearInterval(timer) }
  }, [identity])

  async function save() {
    if (!draft || !snapshot || busy) return
    const id = generation.current
    setBusy(true)
    setError('')
    try {
      const next = await currentApi.current<CapacitySnapshot>('PATCH', '/api/plugins/capacity', { settings: draft, revision: draftRevision })
      if (generation.current !== id) return
      setSnapshot(next)
      setDraft(next.settings)
      setDraftRevision(next.revision)
      edited.current = false
      setSaved(true)
      onSaved?.()
    } catch (cause) {
      if (generation.current === id) setError(cause instanceof Error ? cause.message : String(cause))
    } finally { if (generation.current === id) setBusy(false) }
  }

  const modes = [{ value: 'auto', label: t('自动管理') }, { value: 'manual', label: t('手动设置') }] as const
  const changed = draft && snapshot && JSON.stringify(draft) !== JSON.stringify(snapshot.settings)
  const patch = (key: keyof CapacitySettings, value: unknown) => {
    edited.current = true
    setDraft(previous => previous ? { ...previous, [key]: value } : previous)
    setSaved(false)
  }
  const effective = snapshot?.effective
  const usage = snapshot?.usage
  return <div className='space-y-4'>
    <p className='text-[12px] leading-5 text-subtle'>{t('自动管理按运行内存预算调节容量；手动设置仍保留内存保护和上游账号限制。')}</p>
    <Select value={draft?.mode || 'auto'} disabled={!draft || busy} onValueChange={value => patch('mode', value)}>
      <SelectTrigger className='w-full' aria-label={t('运行容量')}><SelectValue>{modes.find(mode => mode.value === (draft?.mode || 'auto'))?.label}</SelectValue></SelectTrigger>
      <SelectContent>{modes.map(mode => <SelectItem key={mode.value} value={mode.value}>{mode.label}</SelectItem>)}</SelectContent>
    </Select>
    {effective && usage ? <div className='grid grid-cols-2 gap-x-4 gap-y-2 rounded-md border border-hairline p-3 text-[12px] tabular-nums'>
      <span>{t('正在处理')}</span><span className='text-right'>{usage.requests} / {effective.conversations}</span>
      <span>{t('排队中')}</span><span className='text-right'>{usage.queued}</span>
      <span>{t('后台任务')}</span><span className='text-right'>{usage.background} / {effective.background}</span>
      <span>{t('运行内存')}</span><span className='text-right'>{Math.ceil(usage.memoryBytes / MIB)} / {Math.floor(effective.memoryBytes / MIB)} MiB</span>
    </div> : <p className='text-[12px] text-subtle'>{t('读取中')}</p>}
    {effective?.memoryPressure || effective?.cpuPressure ? <p className='text-[12px] text-warning'>{t('当前资源紧张，已暂停启动新的后台和图片任务。')}</p> : null}
    {draft?.mode === 'manual' ? <div className='grid grid-cols-1 gap-3 sm:grid-cols-2'>
      {fields(t).map(field => <Label key={field.key} className='flex flex-col gap-1.5 text-[12px]'>
        {field.label}
        <Input type='number' min={field.min} max={field.max} step={1} value={draft[field.key]} disabled={busy}
          onChange={event => patch(field.key, Number(event.currentTarget.value))} aria-label={field.label} />
      </Label>)}
      <p className='text-[12px] text-subtle sm:col-span-2'>{t('运行内存填 0 表示自动；降低并发上限不会主动中断请求，降低内存预算可能影响后续分配。')}</p>
    </div> : null}
    {error || snapshot?.error ? <p role='alert' className='break-words text-[12px] text-danger'>{error || snapshot?.error}</p> : null}
    <div className='flex items-center gap-3'>
      <Button variant='outline' size='sm' disabled={!changed || busy} onClick={() => void save()}>{busy ? t('保存中') : t('保存')}</Button>
      {saved ? <span className='text-[12px] text-success'>{t('运行容量已保存')}</span> : null}
    </div>
  </div>
}
