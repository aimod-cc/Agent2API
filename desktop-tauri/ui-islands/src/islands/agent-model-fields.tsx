import * as React from 'react'
import { Badge, Button, Card, CardContent, Collapsible, CollapsibleContent, CollapsibleTrigger, Empty, Field, FieldLabel, Input, InputGroup, InputGroupAddon, InputGroupInput } from '@ui'
import { t } from '../i18n'
import type { AgentClient, AgentModel, AgentProfile } from './agent-types'
import { catalogModel, type CatalogModel } from './agent-catalog'
import { AgentSelect, ClaudeModelFields, GatewayModelSelect, TextField } from './agent-fields'
import { blankModel, syncClaudeTiers } from './agent-model'
import { AgentModelPicker, AGENT_MODEL_LIMIT, SearchableModelSelect } from './agent-model-picker'
import { selectProfileModels } from './agent-quick-model'
import { modelFields, modelFieldNote, modelLevels } from './agent-model-config'

/**
 * 模型列表字段块。两种排布：
 *
 * · `stack`（默认，编辑方案弹窗用）：标题 → 候选选择器 → 已选模型卡片，一条纵向流；
 * · `split`（一键接入弹窗用）：标题 → **左右两栏** —— 左栏是从网关目录里挑（自己滚动），
 *   右栏是已选模型卡片。右边不跟着左边滚动，「选了什么、各自声明了什么」始终在视野里。
 *
 * 两种排布的卡片、候选列表、字段行为完全共用，只有摆放方式不同。
 */
export function ModelsFields({ client, draft, update, catalog, disabled, native = false, quick = false, layout = 'stack' }: {
  client: AgentClient; draft: AgentProfile; update: (patch: Partial<AgentProfile>) => void; catalog: CatalogModel[]; disabled: boolean
  native?: boolean; quick?: boolean; layout?: 'stack' | 'split'
}) {
  const imported = Boolean(draft.sourceId)
  const sourceIds = React.useRef(new Set(native ? draft.models.map(model => model.id) : []))
  const [expanded, setExpanded] = React.useState<AgentModel | null>(null)
  const keys = React.useRef(new WeakMap<AgentModel, string>())
  const rowKey = (model: AgentModel) => {
    let key = keys.current.get(model)
    if (!key) { key = crypto.randomUUID(); keys.current.set(model, key) }
    return key
  }
  const change = (index: number, patch: Partial<AgentModel>) => update({ models: draft.models.map((model, i) => {
    if (i !== index) return model
    const next = { ...model, ...patch }
    keys.current.set(next, rowKey(model))
    return next
  }), ...((patch.effort !== undefined && ['claude-code', 'codex'].includes(client.id))
    || (patch.supportsReasoning === false && draft.models[index]?.id === draft.model) ? { effort: '' } : {}) })
  const [query, setQuery] = React.useState('')
  const [pickerOpen, setPickerOpen] = React.useState(quick || !draft.models.some(model => model.id))
  const split = layout === 'split'
  const isGateway = draft.kind === 'gateway'
  const keyword = query.trim().toLowerCase()
  const rows = draft.models.map((model, index) => ({ model, index }))
    .filter(({ model }) => !keyword || `${model.id} ${model.name}`.toLowerCase().includes(keyword))
  const replaceModels = (models: AgentModel[]) => {
    setExpanded(null)
    update(selectProfileModels(draft, models))
  }
  const add = () => {
    if (disabled || draft.models.length >= AGENT_MODEL_LIMIT) return
    const model = blankModel()
    setExpanded(model); setQuery('')
    update({ models: [...draft.models, model] })
  }
  const remove = (index: number) => replaceModels(draft.models.filter((_, i) => i !== index))
  const select = (ids: string[]) => {
    const existing = new Map(draft.models.map(model => [model.id, model]))
    const byId = new Map(catalog.map(model => [model.id, model]))
    const models = ids.flatMap(id => {
      const previous = existing.get(id)
      const selected = byId.get(id)
      return previous ? [previous] : selected ? [catalogModel(client, selected)] : []
    })
    replaceModels([...models, ...draft.models.filter(model => !model.id)])
  }
  const selectedCatalog = [...catalog, ...draft.models.filter(model => model.id && !catalog.some(item => item.id === model.id))
    .map(model => ({ ...model, label: model.name || model.id }))]
  const fields = modelFields(client, draft)
  const visibleCapabilities = fields.filter(key => key.startsWith('supports'))
  const fieldNote = modelFieldNote(client)
  const selected = draft.models.find(model => model.id === draft.model)
  const legacyKimi = client.id === 'kimi-code' && /(?:^|[/\\])\.kimi[/\\]config\.toml$/.test(client.resolvedPath)
  const context = { workbuddy: 'maxInputTokens', zcode: 'properties.contextWindow', 'kimi-code': 'max_context_size', zed: 'max_tokens', cline: 'contextWindow', crush: 'context_window', opencode: 'limit.context' }[client.id] || 'contextWindow'
  const output = { workbuddy: 'maxOutputTokens', zcode: 'optionSpecs.maxOutputTokens.max', 'kimi-code': 'max_output_size', zed: 'max_output_tokens', cline: 'maxTokens', crush: 'default_max_tokens', opencode: 'limit.output', dsh: 'maxTokens' }[client.id] || 'maxOutputTokens'
  const isExpanded = (model: AgentModel) => Boolean(expanded && rowKey(expanded) === rowKey(model))
  const readyCount = draft.models.filter(model => model.id).length

  const picker = <AgentModelPicker catalog={selectedCatalog} selected={draft.models.map(model => model.id).filter(Boolean)} onChange={select}
    disabled={disabled} locked={[]} limit={AGENT_MODEL_LIMIT - draft.models.filter(model => !model.id).length} />
  const queryField = draft.models.length > 3 || query !== '' ? <InputGroup>
    <InputGroupInput type='search' aria-label={t('搜索已选模型')} placeholder={t('搜索已选模型')} value={query} autoComplete='off' disabled={disabled}
      onChange={event => { setQuery(event.currentTarget.value); setExpanded(null) }} />
    <InputGroupAddon aria-hidden='true'>⌕</InputGroupAddon>
  </InputGroup> : null
  const notes = <>
    {imported && !native && <p className='text-xs leading-relaxed text-subtle'>{t('修改后点击「使用此配置」才会更新客户端。')}</p>}
    {native && <p className='text-xs leading-relaxed text-subtle'>{t('移除模型会写入客户端配置；其它条目和未知字段保留，确认前可查看差异。')}</p>}
  </>

  const card = (model: AgentModel, index: number) => {
    const open = isExpanded(model)
    const known = catalog.find(item => item.id === model.id)
    const levels = modelLevels(client, model.reasoningLevels, draft.protocol)
    const options = catalog.filter(item => item.id === model.id || !draft.models.some(current => current.id === item.id))
    const name = model.name || model.id || t('模型 {n}', { n: index + 1 })
    const edit = (patch: Partial<AgentModel>) => {
      const next = { ...model, ...patch }
      keys.current.set(next, rowKey(model)); setExpanded(next); change(index, patch)
    }
    const selectModel = (id: string) => {
      const picked = options.find(item => item.id === id)
      if (!picked) return
      const next = catalogModel(client, picked)
      keys.current.set(next, rowKey(model))
      setExpanded(next)
      update({ models: draft.models.map((current, i) => i === index ? next : current),
        ...(!draft.model || draft.model === model.id ? { model: id, effort: '' } : {}) })
    }
    return <Collapsible key={rowKey(model)} render={<Card />} className='agent-model-card shrink-0 rounded-[10px] border-0 bg-surface-inset shadow-none'
      open={open} onOpenChange={open => setExpanded(open ? model : null)}>
      <div className='flex min-w-0 items-center gap-1 pr-2.5'>
        <CollapsibleTrigger render={<Button variant='ghost' />}
          className='h-auto min-w-0 flex-1 shrink justify-start gap-3 whitespace-normal px-3.5 py-2.5 text-left text-foreground'>
          <span className='min-w-0 flex-1'>
            <span className='flex min-w-0 items-baseline gap-2' title={`${name}${model.id && model.name && model.name !== model.id ? ` · ${model.id}` : ''}`}>
              <span className='truncate text-[13px] font-semibold'>{name}</span>
              {model.id && model.name && model.name !== model.id && <code className='min-w-0 truncate text-[11px] font-normal text-subtle'>{model.id}</code>}
              {client.supportsDefault && model.id && model.id === draft.model && <Badge variant='brand' shape='tag' className='shrink-0'>{t('默认模型')}</Badge>}
            </span>
            <span className='mt-1.5 flex flex-wrap gap-x-4 gap-y-1 text-[11.5px] font-normal'>
              {numberAbility(t('上下文'), model.contextWindow)}
              {!legacyKimi && numberAbility(t('输出上限'), model.maxOutputTokens)}
              {['supportsTools', 'supportsImages', ...(['zcode', 'opencode', 'kimi-code'].includes(client.id) ? ['supportsVideo'] : []), 'supportsReasoning']
                .map(key => boolAbility(capLabel(key), model[key as keyof AgentModel]))}
            </span>
          </span>
          <svg viewBox='0 0 16 16' aria-hidden='true' className={`size-3.5 shrink-0 text-subtle transition-transform ${open ? 'rotate-180' : ''}`}>
            <path d='m4 6 4 4 4-4' stroke='currentColor' strokeWidth='1.5' strokeLinecap='round' strokeLinejoin='round' fill='none' />
          </svg>
        </CollapsibleTrigger>
        <Button variant='ghost' size='xs' className='shrink-0 text-subtle hover:text-destructive' title={t('移除模型：{name}', { name: model.name || model.id || index + 1 })}
          aria-label={t('移除模型：{name}', { name: model.name || model.id || index + 1 })}
          disabled={disabled} onClick={() => remove(index)}>{t('移除')}</Button>
      </div>
      <CollapsibleContent>
        <CardContent className='flex flex-col gap-3 border-t border-hairline p-3.5'>
          {isGateway && <div className='flex flex-wrap items-center gap-2'>
            <span className='mr-auto text-xs text-subtle'>{t('这里只调整此模型，不影响其它模型。')}</span>
            <Button variant='outline' size='xs' disabled={disabled || !known} onClick={() => {
              if (!known) return
              const next = catalogModel(client, known)
              keys.current.set(next, rowKey(model))
              setExpanded(next)
              change(index, next)
            }}>{t('重新填入目录能力')}</Button>
          </div>}
          {isGateway && catalog.length > 0 && <Field><FieldLabel>{t('从网关目录选择')}</FieldLabel>
            <GatewayModelSelect label={t('从网关目录选择')} value={model.id} disabled={disabled || sourceIds.current.has(model.id)} catalog={options} onChange={selectModel} />
          </Field>}
          <div className='grid grid-cols-1 gap-3 sm:grid-cols-2'>
            <TextField label={t('模型 ID')} fieldKey='id' value={model.id} onChange={id => {
              const next = { ...model, id }; keys.current.set(next, rowKey(model)); setExpanded(next)
              update({ models: draft.models.map((current, i) => i === index ? next : current), ...(draft.model === model.id ? { model: id } : {}) })
            }} disabled={disabled || sourceIds.current.has(model.id)} />
            {client.id !== 'zcode' && <TextField label={t('模型展示名')} fieldKey='name' value={model.name} onChange={name => edit({ name })} disabled={disabled} />}
            {fields.includes('contextWindow') && <NumberField label={t('上下文窗口')} fieldKey={client.id === 'codex' ? 'context_window' : context} value={model.contextWindow} onChange={contextWindow => edit({ contextWindow })} disabled={disabled} />}
            {fields.includes('maxOutputTokens') && <NumberField label={t('最大输出 Token')} fieldKey={output} value={model.maxOutputTokens} onChange={maxOutputTokens => edit({ maxOutputTokens })} disabled={disabled} />}
          </div>
          <div className='grid grid-cols-1 gap-3 sm:grid-cols-2'>
            {visibleCapabilities.map(key => <Field key={key}><FieldLabel>{capLabel(key)}</FieldLabel>
              <AgentSelect label={capLabel(key)} disabled={disabled} value={model[key as keyof AgentModel] === true ? 'on' : model[key as keyof AgentModel] === false && client.id !== 'kimi-code' ? 'off' : 'inherit'}
                options={[{ value: 'inherit', label: t('未声明 / 保留') }, { value: 'on', label: t('支持') }, ...(client.id === 'kimi-code' ? [] : [{ value: 'off', label: t('不支持') }])]}
                onChange={value => {
                  const patch: Partial<AgentModel> = { [key]: value === 'inherit' ? null : value === 'on' }
                  if (key === 'supportsReasoning' && value === 'off') patch.effort = ''
                  edit(patch)
                }} /></Field>)}
          </div>
          {client.id === 'zcode' && model.supportsReasoning === true && !model.reasoningLevels.length && <p className='text-xs text-subtle'>{t('模型未声明思考档位，保留客户端默认，不显示猜测选项。')}</p>}
          {client.id === 'zcode' && model.reasoningLevels.length > 0 && <p className='text-xs text-subtle'>{t('模型思考等级')} · {model.reasoningLevels.join(' / ')}</p>}
          {client.supportsEffort && model.supportsReasoning !== false && !['cline', 'dsh'].includes(client.id) && !(client.id === 'kimi-code' && legacyKimi) && (levels.length > 0 || model.effort) && <Field><FieldLabel>{t('模型思考等级')}</FieldLabel>
            <AgentSelect label={t('模型思考等级')} value={model.effort} disabled={disabled} options={[{ value: '', label: t('模型默认') }, ...[...new Set([...levels, ...(model.effort ? [model.effort] : [])])].map(value => ({ value, label: value }))]} onChange={effort => edit({ effort })} /></Field>}
        </CardContent>
      </CollapsibleContent>
    </Collapsible>
  }

  const list = <div className='agent-model-list flex min-w-0 flex-col gap-2'>
    {rows.map(({ model, index }) => card(model, index))}
    {!rows.length && <Empty>{t(keyword ? '没有匹配的模型' : '请至少添加一个模型')}
      {keyword && <Button variant='ghost' size='sm' onClick={() => setQuery('')}>{t('清除搜索')}</Button>}
    </Empty>}
  </div>

  return <section className='agent-models-fields flex min-w-0 shrink-0 flex-col gap-3'>
    <div className='flex flex-wrap items-center gap-2'><h3 className='text-sm font-semibold'>{t('模型列表')}</h3>
      <Badge variant='outline' shape='tag'>{readyCount}</Badge><span className='mr-auto' />
      {isGateway && !split && <Button variant='outline' size='sm' disabled={disabled} onClick={() => setPickerOpen(value => !value)}>{pickerOpen ? t('收起模型选择') : t('选择模型')}</Button>}
      <Button variant={isGateway ? 'ghost' : 'outline'} size='sm' disabled={disabled || draft.models.length >= AGENT_MODEL_LIMIT} onClick={add}>{isGateway ? t('手动添加模型') : t('添加模型')}</Button>
    </div>

    {split ? <>
      {isGateway && <p className='text-xs leading-relaxed text-subtle'>{t('可搜索并多选模型；上下文、输出上限和能力分别从网关目录自动填入。')}</p>}
      <div className='agent-models-split'>
        <div className='agent-models-col'>
          <div className='agent-models-col-head'>
            <h4>{t('可选模型')}</h4>
            <span className='agent-models-col-note'>{t('网关目录共 {n} 个', { n: selectedCatalog.length })}</span>
          </div>
          {isGateway ? picker : <Empty>{t('这个客户端不从网关目录选模型，用右侧的「添加模型」逐个填写。')}</Empty>}
        </div>
        <div className='agent-models-col'>
          <div className='agent-models-col-head'>
            <h4>{t('已选模型')}</h4>
            <Badge variant='outline' shape='tag'>{readyCount}</Badge>
            <span className='agent-models-col-note ml-auto'>{t('展开可调整能力；— 表示客户端未声明')}</span>
          </div>
          {queryField}
          {list}
        </div>
      </div>
      {notes}
    </> : <>
      {isGateway && <p className='text-xs leading-relaxed text-subtle'>{t('可搜索并多选模型；上下文、输出上限和能力分别从网关目录自动填入。')}</p>}
      {isGateway && pickerOpen && picker}
      {notes}
      <p className='text-xs leading-relaxed text-subtle'>{t('每个模型独立保存能力；展开可调整，未声明的数据不会猜测填写。')}</p>
      {queryField}
      {list}
    </>}

    {fieldNote && <p className='text-xs leading-relaxed text-subtle'>{fieldNote}</p>}
    {client.id === 'claude-code' && <ClaudeModelFields draft={draft} update={update} catalog={catalog} disabled={disabled} />}
    {client.supportsDefault && <Field><FieldLabel>{t('默认模型')}</FieldLabel>
      <SearchableModelSelect label={t('默认模型')} value={draft.model} disabled={disabled} options={draft.models.filter(model => model.id).map(model => ({ value: model.id, label: model.name || model.id }))} onChange={model => update({ model, effort: '', ...(client.id === 'claude-code' ? { claudeModels: syncClaudeTiers(draft.claudeModels, draft.model, model) } : {}) })} />
      <p className='text-xs text-subtle'>{quick ? t('接入后使用此默认模型，其它已选模型仍可在客户端切换。') : native ? t('确认写入后更新客户端默认模型；取消不会生效。') : t('使用此配置时设为默认，其他模型仍可在客户端选择。')}</p></Field>}
    {['cline', 'crush', 'dsh'].includes(client.id) && selected && selected.supportsReasoning !== false && (selected.reasoningLevels.length > 0 || draft.effort) && <Field><FieldLabel>{t('默认模型思考等级')}</FieldLabel>
      <AgentSelect label={t('默认模型思考等级')} value={draft.effort} disabled={disabled} options={[{ value: '', label: t('模型默认') }, ...[...new Set([...modelLevels(client, selected.reasoningLevels), ...(draft.effort ? [draft.effort] : [])])].map(value => ({ value, label: value }))]} onChange={effort => update({ effort })} /></Field>}
  </section>
}

/**
 * 一项能力的读法：标签弱、值实。
 *
 * 灰度按组件库真实档位排（theme.css：--text-2 = text-subtle 是中间档，
 * --text-3 = text-muted-foreground 才是最淡）：标签最淡、值最实、支持用绿色。
 * 三种状态各有符号：`✓` 支持、`✕` 不支持、`—` 未声明 —— 不读字也看得出这张卡
 * 声明了多少（原来六项串成一行同色灰字，得逐段对齐才分得清）。
 */
function Ability({ label, value, tone }: { label: string; value: React.ReactNode; tone: 'ok' | 'value' | 'no' | 'none' }) {
  const valueClass = tone === 'ok' ? 'font-medium text-success'
    : tone === 'value' ? 'font-medium text-foreground'
      : tone === 'no' ? 'text-subtle' : 'text-muted-foreground'
  return <span className='whitespace-nowrap'><span className='text-muted-foreground'>{label} </span><span className={valueClass}>{value}</span></span>
}
function boolAbility(label: string, value: unknown) {
  if (value === true) return <Ability key={label} label={label} value='✓' tone='ok' />
  if (value === false) return <Ability key={label} label={label} value='✕' tone='no' />
  return <Ability key={label} label={label} value='—' tone='none' />
}
function numberAbility(label: string, value?: number | null) {
  if (value == null) return <Ability key={label} label={label} value='—' tone='none' />
  return <Ability key={label} label={label} value={value.toLocaleString()} tone='value' />
}

function capLabel(key: string): string {
  return { supportsTools: t('工具调用'), supportsImages: t('图片输入'), supportsVideo: t('视频输入'), supportsReasoning: t('思考能力') }[key] || key
}
function NumberField({ label, fieldKey, value, onChange, disabled }: {
  label: string; fieldKey: string; value?: number | null; onChange: (value: number | null) => void; disabled: boolean
}) {
  const id = React.useId()
  return <Field><FieldLabel htmlFor={id}>{label}<code className='ml-auto text-[10px] text-subtle'>{fieldKey}</code></FieldLabel>
    <Input id={id} type='number' min={1} max={100_000_000} step={1} value={value ?? ''} placeholder={t('留空保留 / 未声明')} disabled={disabled}
      onChange={event => onChange(event.currentTarget.value === '' ? null : Number(event.currentTarget.value))} /></Field>
}
