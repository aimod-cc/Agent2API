import * as React from 'react'
import {
  Alert, Badge, Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, DialogTrigger,
  Empty, Input, Label, RadioGroup, RadioGroupItem, Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
  Spinner, Switch, confirmDialog,
} from '@ui'
import { t } from '../i18n'
import { errorMessage, toast } from './models-panel-state'
import {
  compactRouteError, compactRoutesBridge, modelKey,
  type CompactModel, type CompactRoute, type CompactRoutesConfig, type CompactRoutesView,
} from './models-compact-state'

export function CompactRoutesDialog({ onClose }: { onClose(): void }) {
  const [view, setView] = React.useState<CompactRoutesView | null>(null)
  const [draft, setDraft] = React.useState<CompactRoutesConfig>({ enabled: false, rules: [] })
  const [loading, setLoading] = React.useState(true)
  const [saving, setSaving] = React.useState(false)
  const [error, setError] = React.useState('')
  const mounted = React.useRef(true)
  const sequence = React.useRef(0)
  const dirty = view !== null && JSON.stringify(draft) !== JSON.stringify(view.config)

  const load = React.useCallback(async () => {
    const current = ++sequence.current
    setLoading(true); setError('')
    try {
      const next = await compactRoutesBridge().getCompactRoutes()
      if (!next?.config || !Array.isArray(next.config.rules) || !Array.isArray(next.models)
        || !Array.isArray(next.clients) || !next.revision || !Number.isInteger(next.maxRules)) {
        throw new Error(t('压缩路由接口返回了不完整的数据'))
      }
      if (!mounted.current || current !== sequence.current) return
      setView(next); setDraft(structuredClone(next.config))
    } catch (failure) {
      if (mounted.current && current === sequence.current) setError(errorMessage(failure))
    } finally {
      if (mounted.current && current === sequence.current) setLoading(false)
    }
  }, [])

  React.useEffect(() => {
    mounted.current = true
    void load()
    return () => { mounted.current = false; sequence.current++ }
  }, [load])

  const close = async () => {
    if (saving) return
    if (dirty && !await confirmDialog(t('放弃尚未保存的压缩路由修改？'))) return
    onClose()
  }
  const reload = async () => {
    if (dirty && !await confirmDialog(t('重新读取会丢弃当前草稿，确定继续？'))) return
    void load()
  }
  const changeRule = (index: number, patch: Partial<CompactRoute>) => {
    setDraft(current => ({ ...current, rules: current.rules.map((rule, at) => at === index ? { ...rule, ...patch } : rule) }))
    setError('')
  }
  const save = async () => {
    if (!view || saving) return
    const invalid = compactRouteError(draft, view)
    if (invalid) { setError(invalid); return }
    setSaving(true); setError('')
    try {
      await compactRoutesBridge().saveCompactRoutes({ config: draft, revision: view.revision })
      toast(t('压缩路由已保存，下次压缩请求生效'), 'ok')
      onClose()
    } catch (failure) {
      if (mounted.current) setError(t('保存失败：{message}', { message: errorMessage(failure) }))
    } finally {
      if (mounted.current) setSaving(false)
    }
  }
  const disabled = loading || saving

  return <Dialog open onOpenChange={open => { if (!open) void close() }}>
    <DialogContent className='w-[min(920px,calc(100vw-32px))]'>
      <DialogHeader><DialogTitle>{t('压缩路由')}</DialogTitle><Badge variant='outline'>ZCode</Badge></DialogHeader>
      <DialogBody>
        <div className='flex flex-col gap-4'>
          <p className='text-sm text-muted-foreground'>{t('只在识别到 ZCode 上下文压缩请求时切换模型，普通对话仍使用原模型。')}</p>
          {loading ? <div className='flex items-center justify-center gap-2 py-8'><Spinner />{t('正在读取压缩路由…')}</div> : view ? <>
            <div className='flex items-center justify-between gap-4 rounded-lg border border-border p-3'>
              <div className='min-w-0'>
                <Label htmlFor='compact-routes-enabled'>{t('启用压缩路由')}</Label>
                <p className='mt-1 text-xs text-muted-foreground'>{t('关闭后保留规则，所有请求按原模型处理。')}</p>
              </div>
              <Switch id='compact-routes-enabled' checked={draft.enabled} disabled={disabled}
                onCheckedChange={enabled => setDraft(current => ({ ...current, enabled }))} />
            </div>
            {draft.rules.length ? <div className='flex flex-col gap-3'>
              {draft.rules.map((rule, index) => {
                const prefix = `compact-route-${index}`
                const usedSources = new Set(draft.rules.filter((other, at) => at !== index && other.client === rule.client).map(other => modelKey(other.source)))
                return <div key={index} className='rounded-lg border border-border p-3'>
                  <div className='mb-3 flex items-center justify-between gap-3'>
                    <span className='text-sm font-medium'>{t('规则 {n}', { n: index + 1 })}</span>
                    <div className='flex items-center gap-3'>
                      <label className='flex items-center gap-2 text-xs text-muted-foreground'>
                        {t('启用')}<Switch checked={rule.enabled} disabled={disabled} size='sm'
                          aria-label={t('启用规则 {n}', { n: index + 1 })} onCheckedChange={enabled => changeRule(index, { enabled })} />
                      </label>
                      <Button variant='ghost' size='sm' disabled={disabled}
                        aria-label={t('删除规则 {n}', { n: index + 1 })}
                        onClick={() => setDraft(current => ({ ...current, rules: current.rules.filter((_, at) => at !== index) }))}>{t('删除')}</Button>
                    </div>
                  </div>
                  <div className='grid grid-cols-1 items-start gap-3 sm:grid-cols-[120px_minmax(0,1fr)_auto_minmax(0,1fr)]'>
                    <div className='flex min-w-0 flex-col gap-1.5'>
                      <Label htmlFor={`${prefix}-client`}>{t('适用客户端')}</Label>
                      <Select value={rule.client} disabled={disabled} onValueChange={value => { if (value) changeRule(index, { client: String(value) }) }}>
                        <SelectTrigger id={`${prefix}-client`} className='w-full'><SelectValue>{view.clients.find(client => client.id === rule.client)?.label || rule.client}</SelectValue></SelectTrigger>
                        <SelectContent>{view.clients.map(client => <SelectItem key={client.id} value={client.id}>{client.label}</SelectItem>)}</SelectContent>
                      </Select>
                    </div>
                    <CompactModelSelect id={`${prefix}-source`} label={t('来源模型')} value={rule.source} models={view.models}
                      excluded={usedSources} disabled={disabled} onChange={source => changeRule(index, { source })} />
                    <span className='hidden pt-7 text-muted-foreground sm:block' aria-hidden='true'>→</span>
                    <CompactModelSelect id={`${prefix}-target`} label={t('压缩模型')} value={rule.target} models={view.models}
                      excluded={new Set([modelKey(rule.source)])} disabled={disabled} onChange={target => changeRule(index, { target })} />
                  </div>
                </div>
              })}
            </div> : <Empty>{t('尚未配置压缩规则，默认使用原模型。')}</Empty>}
            <Button variant='outline' className='self-start' disabled={disabled || draft.rules.length >= view.maxRules || !view.models.length}
              onClick={() => setDraft(current => ({ ...current, rules: [...current.rules, { client: view.clients[0]?.id || 'zcode', source: '', target: '', enabled: true }] }))}>
              {t('＋ 添加压缩规则')}
            </Button>
            {!view.models.length ? <Alert>{t('没有可选的对话模型，请先启用模型和对应账号。')}</Alert> : null}
            <div className='space-y-1 text-xs text-muted-foreground'>
              <p>{t('按客户端发送的模型 ID 匹配，支持模型别名和对话路由组；每次只映射一次。')}</p>
              <p>{t('规则不绑定提供商，压缩模型由其路由配置决定使用哪家提供商。')}</p>
              <p>{t('目标使用自身的思考设置或默认值，仍受网关 Key 权限限制；不可用时明确报错，不自动退回原模型。')}</p>
              <p>{t('仅支持已识别压缩格式的 ZCode；客户端修改压缩格式后可能需要更新适配。')}</p>
              <p>{t('如果 ZCode 已设置压缩路由，网关看到的是它改写后的模型。建议同一套规则只在一处配置。')}</p>
            </div>
          </> : null}
          {error ? <Alert variant='destructive' role='alert'>{error}</Alert> : null}
        </div>
      </DialogBody>
      <DialogFooter>
        <Button variant='ghost' disabled={disabled} onClick={() => void reload()}>{t('重新读取')}</Button>
        <Button variant='outline' disabled={saving} onClick={() => void close()}>{t('取消')}</Button>
        <Button disabled={disabled || !view || !dirty} onClick={() => void save()}>{saving ? t('保存中…') : t('保存')}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}

function CompactModelSelect({ id, label, value, models, excluded, disabled, onChange }: {
  id: string; label: string; value: string; models: CompactModel[]; excluded: Set<string>; disabled: boolean; onChange(value: string): void
}) {
  const [open, setOpen] = React.useState(false)
  const [search, setSearch] = React.useState('')
  const [picked, setPicked] = React.useState('')
  const searchRef = React.useRef<HTMLInputElement>(null)
  const ids = [...new Map(models.map(model => [modelKey(model.id), model.id])).values()].filter(id => modelKey(id))
  const selected = ids.find(id => modelKey(id) === modelKey(value))
  const available = ids.filter(id => !excluded.has(modelKey(id)) || modelKey(id) === modelKey(value))
  const query = modelKey(search)
  const filtered = available.filter(id => modelKey(id).includes(query))
  const canConfirm = !disabled && available.includes(picked)
  const display = selected || (value ? `${value} · ${t('不可用')}` : t('请选择模型'))
  const changeOpen = (next: boolean) => {
    if (next) {
      setSearch('')
      setPicked(selected || '')
    }
    setOpen(next)
  }
  const confirm = () => {
    if (!canConfirm) return
    onChange(picked)
    setOpen(false)
  }

  return <div className='flex min-w-0 flex-col gap-1.5'>
    <Label htmlFor={id}>{label}</Label>
    <Dialog open={open} onOpenChange={changeOpen}>
      <DialogTrigger disabled={disabled} render={<Button id={id} variant='outline' className='w-full min-w-0 justify-between' title={display} />}>
        <span className='min-w-0 truncate'>{display}</span>
        <svg viewBox='0 0 16 16' className='size-3.5 text-muted-foreground' fill='none' aria-hidden='true'>
          <path d='m4 6 4 4 4-4' stroke='currentColor' strokeWidth='1.5' strokeLinecap='round' strokeLinejoin='round' />
        </svg>
      </DialogTrigger>
      <DialogContent overlayForceRender className='routing-create-dialog' initialFocus={searchRef}>
        <DialogHeader className='shrink-0'><DialogTitle>{label}</DialogTitle></DialogHeader>
        <DialogBody className='min-h-0 overflow-hidden'>
          <Input ref={searchRef} type='search' className='shrink-0' value={search} disabled={disabled}
            placeholder={t('搜索模型 ID…')} aria-label={t('搜索模型 ID…')} autoComplete='off'
            onChange={event => setSearch(event.currentTarget.value)} />
          <RadioGroup className='routing-model-picker min-h-0' value={picked} disabled={disabled} aria-label={label}
            onValueChange={setPicked}>
            {filtered.map(model => <Label key={model} className='routing-single-model shrink-0' data-checked={picked === model ? '' : undefined}>
              <RadioGroupItem value={model} /><span title={model}>{model}</span>
            </Label>)}
            {!filtered.length ? <Empty>{t('没有匹配的模型')}</Empty> : null}
          </RadioGroup>
          <p className='shrink-0 text-xs text-muted-foreground' role='status'>{t('{n} 个模型', { n: filtered.length })}</p>
          <p className='shrink-0 text-xs text-muted-foreground'>{t('规则不绑定提供商，压缩模型由其路由配置决定使用哪家提供商。')}</p>
        </DialogBody>
        <DialogFooter className='shrink-0'>
          <span className='mr-auto min-w-0 truncate text-xs text-muted-foreground' title={picked}>{picked}</span>
          <Button variant='outline' onClick={() => setOpen(false)}>{t('取消')}</Button>
          <Button disabled={!canConfirm} onClick={confirm}>{t('确认选择')}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </div>
}
