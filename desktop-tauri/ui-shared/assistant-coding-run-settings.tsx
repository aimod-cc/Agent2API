import { useEffect, useRef, useState } from 'react'
import { Alert, Button, Input, Popover, PopoverContent, PopoverTitle, PopoverTrigger, cn } from '@ui'
import { CODING_BASE, codingPath, type CodingModel, type CodingSession, type CodingSettingsChange, type CodingTranslate, type CodingTransport } from './assistant-coding-types'
import { CodingIcon } from './assistant-coding-icons'

function effortLabel(value: string, t: CodingTranslate): string {
  switch (value) {
    case 'none': return t('不思考')
    case 'minimal': return t('极低')
    case 'low': return t('低')
    case 'medium': return t('中等')
    case 'high': return t('高')
    case 'xhigh': return t('极高')
    case 'max': return t('最高')
    case 'ultra': return 'Ultra'
    default: return value
  }
}

type Pane = 'root' | 'model' | 'effort' | 'speed' | 'custom'
const rowClass = 'h-auto min-h-9 w-full justify-start gap-3 whitespace-normal rounded-md px-3 py-2 text-left text-xs max-sm:min-h-11'
const optionClass = `${rowClass} font-normal`

export function CodingRunSettings({ session, transport, t, disabled, onChanged, onSavingChange }: {
  session: CodingSession; transport: CodingTransport; t: CodingTranslate; disabled: boolean
  onChanged: () => Promise<void>; onSavingChange: (saving: boolean) => void
}) {
  const [models, setModels] = useState<CodingModel[]>([])
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  const [custom, setCustom] = useState('')
  const [open, setOpen] = useState(false)
  const [pane, setPane] = useState<Pane>('root')
  const [keyword, setKeyword] = useState('')
  const gate = useRef(false)
  const mounted = useRef(true)
  useEffect(() => { mounted.current = true; return () => { mounted.current = false } }, [])
  useEffect(() => {
    const controller = new AbortController()
    setLoading(true)
    transport.json<{ models: CodingModel[] }>('GET', `${CODING_BASE}/models`, undefined, controller.signal)
      .then(data => { if (!controller.signal.aborted) { setModels(data.models); setError('') } })
      .catch(failure => { if (!controller.signal.aborted) setError(failure instanceof Error ? failure.message : String(failure)) })
      .finally(() => { if (!controller.signal.aborted) setLoading(false) })
    return () => controller.abort()
  }, [transport])
  const current = models.find(model => model.id === session.model)
  const visible = models.filter(model => (!model.hidden || model.id === session.model)
    && `${model.displayName} ${model.id}`.toLowerCase().includes(keyword.trim().toLowerCase()))
  const efforts = current?.reasoningEfforts || []
  const currentEffort = session.reasoningEffort || current?.defaultReasoningEffort
  const canChange = session.active && !disabled && !saving
  const fast = session.serviceTier === 'priority'
  const modelLabel = current?.displayName || session.model || t('默认模型')
  const reasonLabel = currentEffort ? effortLabel(currentEffort, t) : t('模型默认')
  const speedLabel = fast ? t('快速模式') : session.serviceTier && session.serviceTier !== 'default' ? session.serviceTier : t('标准模式')
  const paneLabel = pane === 'model' || pane === 'custom' ? t('模型') : pane === 'effort' ? t('思考等级') : pane === 'speed' ? t('速度') : t('运行设置')
  const apply = async (changes: CodingSettingsChange) => {
    if (gate.current || !canChange) return
    gate.current = true; setSaving(true); onSavingChange(true); setError(''); setNotice('')
    try {
      await transport.json('PUT', codingPath(session.id, '/settings'), { ...changes, expectedRevision: session.settingsRevision })
      if (mounted.current) {
        await onChanged()
        setNotice(t('Codex 已确认设置；用于后续请求，不会重启当前任务。'))
        setPane('root')
      }
    } catch (failure) {
      if (mounted.current) { setError(failure instanceof Error ? failure.message : String(failure)); await onChanged().catch(() => {}) }
    } finally {
      gate.current = false
      if (mounted.current) { setSaving(false); onSavingChange(false) }
    }
  }
  const pickModel = (value: string) => { if (value !== session.model) void apply({ model: value }); else setPane('root') }
  const pickEffort = (value: string) => { if (value !== currentEffort) void apply({ reasoningEffort: value }); else setPane('root') }
  const pickSpeed = (enabled: boolean) => {
    if (enabled !== fast || !enabled && session.serviceTier && session.serviceTier !== 'default') void apply({ fastMode: enabled })
    else setPane('root')
  }
  return <Popover open={open} onOpenChange={value => { setOpen(value); if (!value) { setPane('root'); setKeyword('') } }}>
    <PopoverTrigger render={<Button size='sm' variant='ghost' className='h-8 min-w-0 max-w-full shrink rounded-full px-2 font-normal max-sm:h-9'
      title={`${modelLabel} · ${reasonLabel} · ${speedLabel}`} aria-label={t('运行设置')} />}>
      <CodingIcon name={fast ? 'speed' : 'model'} className='size-3.5 text-muted-foreground' />
      <span className='min-w-0 truncate'>{modelLabel}</span>
      <span className='shrink-0 text-muted-foreground'>· {reasonLabel} · {speedLabel}</span>
      <CodingIcon name='chevron' className='size-3 text-muted-foreground' />
    </PopoverTrigger>
    <PopoverContent side='top' align='end' className='w-80 max-w-[calc(100vw-32px)] p-1.5'>
      <div className='flex min-h-8 items-center gap-1 px-1 pb-1'>
        {pane !== 'root' ? <Button size='icon-sm' variant='ghost' aria-label={t('返回')} onClick={() => setPane('root')}><CodingIcon name='back' /></Button> : null}
        <PopoverTitle className='px-2 text-xs font-medium'>{paneLabel}</PopoverTitle>
      </div>
      <div className='min-h-0 overflow-y-auto overscroll-contain'>
        {pane === 'root' ? <>
          <Button variant='ghost' className={rowClass} onClick={() => setPane('model')}><CodingIcon name='model' /><span>{t('模型')}</span><span className='ml-auto min-w-0 truncate text-muted-foreground'>{modelLabel}</span></Button>
          <Button variant='ghost' className={rowClass} onClick={() => setPane('effort')}><CodingIcon name='effort' /><span>{t('思考等级')}</span><span className='ml-auto text-muted-foreground'>{reasonLabel}</span></Button>
          <Button variant='ghost' className={rowClass} onClick={() => setPane('speed')}><CodingIcon name='speed' /><span>{t('速度')}</span><span className='ml-auto text-muted-foreground'>{speedLabel}</span></Button>
        </> : null}
        {pane === 'model' ? <>
          <div className='px-1 pb-2'><Input value={keyword} onChange={event => setKeyword(event.currentTarget.value)} placeholder={t('搜索模型…')} aria-label={t('搜索模型…')} /></div>
          {loading ? <p className='px-3 py-2 text-xs text-muted-foreground'>{t('正在读取可用模型…')}</p> : null}
          {!loading && !visible.length ? <p className='px-3 py-2 text-xs text-muted-foreground'>{t('没有匹配的模型')}</p> : null}
          {visible.map(model => <Button key={model.id} variant='ghost' disabled={!canChange || loading} aria-pressed={model.id === session.model}
            className={cn(optionClass, model.id === session.model && 'bg-primary-soft text-primary-fg')} onClick={() => pickModel(model.id)}>
            <span className='min-w-0 flex-1 break-words'>{model.displayName || model.id}</span>{model.id === session.model ? <CodingIcon name='check' /> : null}
          </Button>)}
          <Button variant='ghost' className={rowClass} disabled={!canChange} onClick={() => { setCustom(session.model || ''); setPane('custom') }}>{t('手动填写模型')}</Button>
        </> : null}
        {pane === 'effort' ? <>
          {efforts.length ? <>
            <Button variant='ghost' className={optionClass} disabled={!canChange || !current?.defaultReasoningEffort} onClick={() => pickEffort('default')}>{t('模型默认')}</Button>
            {efforts.map(item => <Button key={item.reasoningEffort} variant='ghost' disabled={!canChange} aria-pressed={item.reasoningEffort === currentEffort}
              className={cn(optionClass, item.reasoningEffort === currentEffort && 'bg-primary-soft text-primary-fg')} onClick={() => pickEffort(item.reasoningEffort)}>
              <span className='flex-1'>{effortLabel(item.reasoningEffort, t)}</span>{item.reasoningEffort === currentEffort ? <CodingIcon name='check' /> : null}
            </Button>)}
          </> : <p className='px-3 py-2 text-xs leading-relaxed text-muted-foreground'>{t('Codex 未提供当前模型的思考等级，不能在这里选择。')}</p>}
        </> : null}
        {pane === 'speed' ? <>
          <Button variant='ghost' disabled={!canChange} aria-pressed={!session.serviceTier || session.serviceTier === 'default'} className={cn(optionClass, (!session.serviceTier || session.serviceTier === 'default') && 'bg-primary-soft text-primary-fg')} onClick={() => pickSpeed(false)}>
            <span className='flex-1'>{t('标准模式')}</span>{!session.serviceTier || session.serviceTier === 'default' ? <CodingIcon name='check' /> : null}
          </Button>
          <Button variant='ghost' disabled={!canChange || !current?.supportsFast && !fast} aria-pressed={fast} className={cn(optionClass, fast && 'bg-primary-soft text-primary-fg')} onClick={() => pickSpeed(true)}>
            <span className='flex-1'>{t('快速模式')}</span>{fast ? <CodingIcon name='check' /> : null}
          </Button>
          <p className='px-3 py-2 text-xs leading-relaxed text-muted-foreground'>{t('快速模式可能消耗更多额度，仅在模型声明支持时可开启；实际速度由上游决定。')}</p>
        </> : null}
        {pane === 'custom' ? <div className='space-y-2 p-2'><Input value={custom} disabled={!canChange} onChange={event => setCustom(event.currentTarget.value)} placeholder={t('模型名称')} aria-label={t('模型名称')} />
          <Button className='w-full' disabled={!canChange || !custom.trim()} onClick={() => pickModel(custom.trim())}>{t('应用')}</Button></div> : null}
        {!session.active ? <p className='px-3 py-2 text-xs text-muted-foreground'>{t('恢复会话后可调整运行设置。')}</p> : null}
        {saving ? <p role='status' className='px-3 py-2 text-xs text-muted-foreground'>{t('等待 Codex 确认…')}</p> : null}
        {error ? <Alert variant='destructive' className='mt-2'>{error}</Alert> : null}
        {notice && !error ? <p role='status' className='px-3 py-2 text-xs leading-relaxed text-muted-foreground'>{notice}</p> : null}
      </div>
    </PopoverContent>
  </Popover>
}
