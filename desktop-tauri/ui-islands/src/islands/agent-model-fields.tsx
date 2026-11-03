import * as React from 'react'
import { Badge, Button, Card, CardContent, Collapsible, CollapsibleContent, CollapsibleTrigger, Empty, Field, FieldLabel, Input, InputGroup, InputGroupAddon, InputGroupInput } from '@ui'
import { t } from '../i18n'
import type { AgentClient, AgentModel, AgentProfile } from './agent-types'
import type { CatalogModel } from './agent-catalog'
import { AgentSelect, TextField } from './agent-fields'
import { blankModel } from './agent-model'

export function ModelsFields({ client, draft, update, catalog, disabled, native = false }: {
  client: AgentClient; draft: AgentProfile; update: (patch: Partial<AgentProfile>) => void; catalog: CatalogModel[]; disabled: boolean; native?: boolean
}) {
  const change = (index: number, patch: Partial<AgentModel>) => update({ models: draft.models.map((model, i) => i === index ? { ...model, ...patch } : model) })
  const imported = Boolean(draft.sourceId)
  const sourceIds = React.useRef(new Set(imported ? draft.models.map(model => model.id) : []))
  const [rowKeys, setRowKeys] = React.useState(() => draft.models.map(() => crypto.randomUUID()))
  const [expanded, setExpanded] = React.useState<string | null>(() => draft.models.length === 1 ? rowKeys[0] : null)
  const [query, setQuery] = React.useState('')
  const listRef = React.useRef<HTMLDivElement>(null)
  const addedKey = React.useRef<string | null>(null)
  React.useEffect(() => {
    if (!addedKey.current || addedKey.current !== expanded) return
    listRef.current?.querySelector<HTMLElement>(`[data-model-key="${expanded}"]`)?.scrollIntoView({ block: 'start' })
    addedKey.current = null
  }, [expanded, draft.models.length])
  const keyword = query.trim().toLowerCase()
  const rows = draft.models.map((model, index) => ({ model, index, key: rowKeys[index] }))
    .filter(({ model, key }) => !keyword || key === expanded || `${model.id} ${model.name}`.toLowerCase().includes(keyword))
  const add = () => {
    const key = crypto.randomUUID()
    addedKey.current = key
    setRowKeys(current => [...current.slice(0, draft.models.length), key]); setExpanded(key); setQuery('')
    update({ models: [...draft.models, blankModel()] })
  }
  const remove = (index: number) => {
    const models = draft.models.filter((_, i) => i !== index)
    if (expanded === rowKeys[index]) setExpanded(null)
    setRowKeys(current => current.filter((_, i) => i !== index))
    update({ models, ...(draft.model === draft.models[index].id ? { model: models[0]?.id || '', effort: '' } : {}) })
  }
  const visibleCapabilities = client.id === 'zcode' || client.id === 'kimi-code' ? ['supportsImages'] : client.id === 'zed' ? ['supportsTools', 'supportsImages'] : ['crush', 'dsh'].includes(client.id) ? ['supportsImages', 'supportsReasoning'] : ['supportsTools', 'supportsImages', 'supportsReasoning']
  const selected = draft.models.find(model => model.id === draft.model)
  const legacyKimi = client.id === 'kimi-code' && /(?:^|[/\\])\.kimi[/\\]config\.toml$/.test(client.resolvedPath)
  const context = { workbuddy: 'maxInputTokens', zcode: 'properties.contextWindow', 'kimi-code': 'max_context_size', zed: 'max_tokens', cline: 'contextWindow', crush: 'context_window', opencode: 'limit.context' }[client.id] || 'contextWindow'
  const output = { workbuddy: 'maxOutputTokens', zcode: 'optionSpecs.maxOutputTokens.max', 'kimi-code': 'max_output_size', zed: 'max_output_tokens', cline: 'maxTokens', crush: 'default_max_tokens', opencode: 'limit.output', dsh: 'maxTokens' }[client.id] || 'maxOutputTokens'
  return <section className='agent-models-fields flex min-w-0 shrink-0 flex-col gap-3'>
    <div className='flex flex-wrap items-center gap-2'><h3 className='text-sm font-semibold'>{t('模型列表')}</h3>
      <Badge variant='outline' shape='tag'>{draft.models.length}</Badge><span className='mr-auto' />
      <Button variant='outline' size='sm' disabled={disabled} onClick={add}>{t('添加模型')}</Button></div>
    {imported && !native && <p className='text-xs leading-relaxed text-subtle'>{t('导入模型及客户端未知字段会保留；本页删除方案不会删除客户端条目。')}</p>}
    {native && <p className='text-xs leading-relaxed text-subtle'>{t('移除模型会写入客户端配置；其它条目和未知字段保留，确认前可查看差异。')}</p>}
    <p className='text-xs leading-relaxed text-subtle'>{t('点击模型展开详细配置；搜索只筛选显示，不会移除模型。')}</p>
    {draft.models.length > 1 || query !== '' ? <InputGroup>
      <InputGroupInput type='search' aria-label={t('搜索模型')} placeholder={t('搜索模型 ID / 展示名…')} value={query} autoComplete='off'
        onChange={event => { setQuery(event.currentTarget.value); setExpanded(null) }} />
      <InputGroupAddon aria-hidden='true'>⌕</InputGroupAddon>
    </InputGroup> : null}
    <div ref={listRef} className='agent-model-list flex min-w-0 flex-col gap-2'>
    {rows.map(({ model, index, key }) => <Collapsible key={key} render={<Card />} className='agent-model-card shrink-0 border-border bg-surface-inset' data-model-key={key}
      open={expanded === key} onOpenChange={open => setExpanded(open ? key : null)}>
      <div className='flex min-w-0 items-center gap-2 pr-3'>
        <CollapsibleTrigger render={<Button variant='ghost' />} className='h-auto min-w-0 flex-1 shrink justify-start gap-3 whitespace-normal px-3 py-3 text-left text-foreground'>
          <svg viewBox='0 0 16 16' className={`size-3.5 text-subtle transition-transform ${expanded === key ? 'rotate-90' : ''}`} aria-hidden='true'>
            <path d='m6 3 5 5-5 5' stroke='currentColor' strokeWidth='1.5' strokeLinecap='round' strokeLinejoin='round' fill='none' />
          </svg>
          <span className='min-w-0 flex-1'>
            <span className='flex flex-wrap items-center gap-2'><span className='break-all text-sm font-semibold'>{model.name || model.id || t('模型 {n}', { n: index + 1 })}</span>
              {client.supportsDefault && model.id && model.id === draft.model && <Badge variant='brand' shape='tag'>{t('默认模型')}</Badge>}
            </span>
            {model.id && model.name && model.name !== model.id && <code className='mt-1 block break-all text-xs font-normal text-subtle'>{model.id}</code>}
            {(model.contextWindow != null || model.maxOutputTokens != null) && <span className='mt-1 flex flex-wrap gap-x-4 gap-y-1 text-[11px] font-normal text-subtle'>
              {model.contextWindow != null && <span>{t('上下文窗口')} · {model.contextWindow.toLocaleString()}</span>}
              {model.maxOutputTokens != null && <span>{t('最大输出 Token')} · {model.maxOutputTokens.toLocaleString()}</span>}
            </span>}
          </span>
        </CollapsibleTrigger>
        <Button variant='ghost' size='xs' className='text-destructive hover:text-destructive' title={t('移除模型：{name}', { name: model.name || model.id || index + 1 })}
          aria-label={t('移除模型：{name}', { name: model.name || model.id || index + 1 })}
          disabled={disabled || (!native && sourceIds.current.has(model.id)) || draft.models.length === 1} onClick={() => remove(index)}>{t('移除')}</Button>
      </div>
      <CollapsibleContent>
        <CardContent className='flex flex-col gap-3 border-t border-hairline p-3'>
          {draft.kind === 'gateway' && catalog.length > 0 && <Field><FieldLabel>{t('从网关目录选择')}</FieldLabel>
            <AgentSelect label={t('从网关目录选择')} value={model.id} disabled={disabled || sourceIds.current.has(model.id)} options={catalog.map(model => ({ value: model.id, label: model.label }))}
              onChange={id => {
                const selected = catalog.find(model => model.id === id)
                if (selected) {
                  const { label: _, provider: __, providers: ___, ...selectedModel } = selected
                  const next = { ...selectedModel, effort: '', reasoningLevels: client.supportsEffort ? selectedModel.reasoningLevels : [] }
                  update({ models: draft.models.map((current,i) => i === index ? next : current),
                    ...(client.supportsDefault && (!draft.model || draft.model === model.id) ? { model: id, effort: '' } : {}) })
                }
              }} /></Field>}
          <div className='grid grid-cols-1 gap-3 sm:grid-cols-2'>
            <TextField label={t('模型 ID')} fieldKey='id' value={model.id} onChange={id => {
              const models = draft.models.map((model,i) => i === index ? { ...model, id } : model)
              update({ models, ...(draft.model === model.id ? { model: id } : {}) })
            }} disabled={disabled || sourceIds.current.has(model.id)} />
            <TextField label={t('模型展示名')} fieldKey='name' value={model.name} onChange={name => change(index,{ name })} disabled={disabled} />
            <NumberField label={t('上下文窗口')} fieldKey={context} value={model.contextWindow} onChange={contextWindow => change(index,{contextWindow})} disabled={disabled} />
            {!legacyKimi && <NumberField label={t('最大输出 Token')} fieldKey={output} value={model.maxOutputTokens} onChange={maxOutputTokens => change(index,{maxOutputTokens})} disabled={disabled} />}
          </div>
          <div className='grid grid-cols-1 gap-3 sm:grid-cols-2'>
            {visibleCapabilities.map(key => <Field key={key}><FieldLabel>{capLabel(key)}</FieldLabel>
              <AgentSelect label={capLabel(key)} disabled={disabled} value={model[key as keyof AgentModel] === true ? 'on' : model[key as keyof AgentModel] === false ? 'off' : 'inherit'}
                options={[{value:'inherit',label:t('未声明 / 保留')},{value:'on',label:t('支持')},{value:'off',label:t('不支持')}]}
                onChange={value => change(index,{ [key]: value === 'inherit' ? null : value === 'on' })} /></Field>)}
          </div>
          {client.supportsEffort && !['cline', 'dsh'].includes(client.id) && (model.reasoningLevels.length > 0 || model.effort) && <Field><FieldLabel>{t('模型思考等级')}</FieldLabel>
            <AgentSelect label={t('模型思考等级')} value={model.effort} disabled={disabled} options={[{value:'',label:t('模型默认')},...[...new Set([...model.reasoningLevels,...(model.effort ? [model.effort] : [])])].map(value => ({value,label:value}))]} onChange={effort => change(index,{effort})} /></Field>}
        </CardContent>
      </CollapsibleContent>
    </Collapsible>)}
    {!rows.length && <Empty>{t(keyword ? '没有匹配的模型' : '请至少添加一个模型')}
      {keyword && <Button variant='ghost' size='sm' onClick={() => setQuery('')}>{t('清除搜索')}</Button>}
    </Empty>}
    </div>
    {client.supportsDefault && <Field><FieldLabel>{t('默认模型')}</FieldLabel>
      <AgentSelect label={t('默认模型')} value={draft.model} disabled={disabled} options={draft.models.filter(model => model.id).map(model => ({value:model.id,label:model.name || model.id}))} onChange={model => update({model, effort: ''})} />
      <p className='text-xs text-subtle'>{native ? t('确认写入后更新客户端默认模型；取消不会生效。') : client.configMode === 'single' ? t('保存方案不会切换客户端；应用后改变实际默认模型。') : t('保存方案不会切换客户端；选择“应用并设为默认”才改变实际默认模型。')}</p></Field>}
    {['cline','crush','dsh'].includes(client.id) && selected && (selected.reasoningLevels.length > 0 || draft.effort) && <Field><FieldLabel>{t('默认模型思考等级')}<code className='ml-auto text-[10px] text-subtle'>{client.id === 'cline' ? 'settings.reasoning.effort' : client.id === 'dsh' ? 'agent-default-model.reasoningEffort' : 'models.large.reasoning_effort'}</code></FieldLabel>
      <AgentSelect label={t('默认模型思考等级')} value={draft.effort} disabled={disabled} options={[{value:'',label:t('模型默认')},...[...new Set([...selected.reasoningLevels,...(draft.effort ? [draft.effort] : [])])].map(value => ({value,label:value}))]} onChange={effort => update({effort})} /></Field>}
  </section>
}

function capLabel(key: string): string {
  return { supportsTools: t('工具调用'), supportsImages: t('图片输入'), supportsReasoning: t('思考能力') }[key] || key
}
function NumberField({ label, fieldKey, value, onChange, disabled }: {
  label: string; fieldKey: string; value?: number | null; onChange: (value: number | null) => void; disabled: boolean
}) {
  const id = React.useId()
  return <Field><FieldLabel htmlFor={id}>{label}<code className='ml-auto text-[10px] text-subtle'>{fieldKey}</code></FieldLabel>
    <Input id={id} type='number' min={1} max={100_000_000} step={1} value={value ?? ''} placeholder={t('留空保留 / 未声明')} disabled={disabled}
      onChange={event => onChange(event.currentTarget.value === '' ? null : Number(event.currentTarget.value))} /></Field>
}
