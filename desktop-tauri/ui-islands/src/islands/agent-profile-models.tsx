import * as React from 'react'
import {
  Badge,
  Button,
  Checkbox,
  Empty,
  Field,
  FieldLabel,
  Input,
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
  Label,
  RadioGroup,
  RadioGroupItem,
  SegmentedControl,
  cn,
} from '@ui'
import { t } from '../i18n'
import { newId } from '../../../ui-shared/ids'
import { AgentIcon } from './agent-actions'
import { catalogModel, type CatalogModel } from './agent-catalog'
import { AgentSelect } from './agent-fields'
import { blankModel } from './agent-model'
import { modelFieldNote, modelFields, modelLevels } from './agent-model-config'
import { AGENT_MODEL_LIMIT } from './agent-model-picker'
import { selectProfileModels } from './agent-quick-model'
import type { AgentClient, AgentModel, AgentProfile } from './agent-types'

type CatalogTab = 'group' | 'model'

function formatTokenCount(n?: number | null): string {
  if (n == null) return '—'
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n % 1_000_000 === 0 ? 0 : 1)}M`
  if (n >= 1000) return `${Math.round(n / 1000)}K`
  return String(n)
}

function capLabel(key: string): string {
  return {
    supportsTools: t('工具调用'),
    supportsImages: t('图片输入'),
    supportsVideo: t('视频输入'),
    supportsReasoning: t('思考能力'),
  }[key] || key
}

function filterByTab(catalog: CatalogModel[], tab: CatalogTab, keyword: string): CatalogModel[] {
  const q = keyword.trim().toLowerCase()
  return catalog.filter(item => {
    const isGroup = Boolean(item.routingGroup)
    if (tab === 'group' ? !isGroup : isGroup) return false
    if (!q) return true
    return `${item.name} ${item.id} ${item.groupSummary || ''}`.toLowerCase().includes(q)
  })
}

/**
 * 场景 B：多模型客户端（Codex / OpenCode / WorkBuddy 等）等高左右双栏工作台
 * 左栏纯「路由组 | 模型」两档切换，右栏固定高度展示已选模型与参数调整，互不挤压。
 */
export function SplitModelsWorkspace({
  client,
  draft,
  catalog,
  disabled,
  native,
  update,
}: {
  client: AgentClient
  draft: AgentProfile
  catalog: CatalogModel[]
  disabled: boolean
  native: boolean
  update: (patch: Partial<AgentProfile>) => void
}) {
  const isGateway = draft.kind === 'gateway'
  const sourceIds = React.useRef(new Set(native ? draft.models.map(m => m.id) : []))
  const keys = React.useRef(new WeakMap<AgentModel, string>())
  const rowKey = (model: AgentModel) => {
    let key = keys.current.get(model)
    if (!key) { key = newId(); keys.current.set(model, key) }
    return key
  }

  const groupCount = React.useMemo(() => catalog.filter(item => item.routingGroup).length, [catalog])
  const modelCount = catalog.length - groupCount
  const [tab, setTab] = React.useState<CatalogTab>(groupCount > 0 ? 'group' : 'model')
  const [search, setSearch] = React.useState('')
  const [expandedKey, setExpandedKey] = React.useState<string | null>(null)
  const [manualId, setManualId] = React.useState('')
  const [manualName, setManualName] = React.useState('')

  React.useEffect(() => {
    if (groupCount === 0 && tab === 'group') setTab('model')
  }, [groupCount, tab])

  const selectedIds = React.useMemo(() => new Set(draft.models.map(m => m.id).filter(Boolean)), [draft.models])
  const catalogById = React.useMemo(() => new Map(catalog.map(m => [m.id, m])), [catalog])
  const matches = React.useMemo(() => filterByTab(catalog, tab, search), [catalog, tab, search])
  const remaining = Math.max(0, AGENT_MODEL_LIMIT - draft.models.length)
  const fields = modelFields(client, draft)
  const visibleCapabilities = fields.filter(key => key.startsWith('supports'))
  const legacyKimi = client.id === 'kimi-code' && /(?:^|[/\\])\.kimi[/\\]config\.toml$/.test(client.resolvedPath)
  const fieldNote = modelFieldNote(client)

  const replaceModels = (models: AgentModel[]) => {
    update(selectProfileModels(draft, models))
  }

  const toggleItem = (item: CatalogModel, checked: boolean) => {
    if (checked) {
      if (selectedIds.has(item.id) || remaining <= 0) return
      replaceModels([...draft.models, catalogModel(client, item)])
    } else {
      replaceModels(draft.models.filter(m => m.id !== item.id))
    }
  }

  const selectAllVisible = () => {
    const additions = matches
      .filter(m => !selectedIds.has(m.id))
      .slice(0, remaining)
      .map(m => catalogModel(client, m))
    if (!additions.length) return
    replaceModels([...draft.models, ...additions])
  }

  const addManual = () => {
    if (disabled || draft.models.length >= AGENT_MODEL_LIMIT) return
    const cleanId = manualId.trim()
    if (cleanId && selectedIds.has(cleanId)) return
    const nextModel: AgentModel = { ...blankModel(), id: cleanId, name: manualName.trim() }
    const key = rowKey(nextModel)
    setExpandedKey(key)
    setManualId('')
    setManualName('')
    update({
      models: [...draft.models, nextModel],
      ...(!draft.model && cleanId ? { model: cleanId } : {}),
    })
  }

  const patchModel = (index: number, patch: Partial<AgentModel>) => {
    const target = draft.models[index]
    if (!target) return
    const key = rowKey(target)
    const next = { ...target, ...patch }
    keys.current.set(next, key)
    const nextModels = draft.models.map((item, i) => (i === index ? next : item))
    const resetEffort = (patch.effort !== undefined && ['claude-code', 'codex'].includes(client.id))
      || (patch.supportsReasoning === false && target.id === draft.model)
    update({
      models: nextModels,
      ...(patch.id !== undefined && draft.model === target.id ? { model: patch.id } : {}),
      ...(resetEffort ? { effort: '' } : {}),
    })
  }

  const removeModel = (index: number) => {
    replaceModels(draft.models.filter((_, i) => i !== index))
  }

  return (
    <div className='agent-split-wrap'>
      <div className='agent-split-board'>
        <div className='agent-split-col'>
          {isGateway ? (
            <>
              <div className='agent-split-col-head'>
                <SegmentedControl<CatalogTab>
                  value={tab}
                  aria-label={t('选择类型')}
                  options={[
                    { value: 'group', label: t('路由组'), count: groupCount },
                    { value: 'model', label: t('模型'), count: modelCount },
                  ]}
                  onValueChange={next => {
                    setTab(next)
                    setSearch('')
                  }}
                />
                <Button
                  type='button'
                  variant='link'
                  size='xs'
                  className='ml-auto'
                  disabled={disabled || remaining <= 0 || !matches.some(m => !selectedIds.has(m.id))}
                  onClick={selectAllVisible}
                >
                  {t('全选当前')}
                </Button>
              </div>

              <div className='agent-split-search'>
                <InputGroup>
                  <InputGroupInput
                    type='search'
                    value={search}
                    autoComplete='off'
                    disabled={disabled}
                    placeholder={tab === 'group' ? t('搜索路由组名称或 ID…') : t('搜索模型名称或 ID…')}
                    onChange={e => setSearch(e.currentTarget.value)}
                  />
                  <InputGroupAddon aria-hidden='true'><AgentIcon name='search' size={13} /></InputGroupAddon>
                  {search ? (
                    <InputGroupAddon align='inline-end'>
                      <InputGroupButton size='icon-xs' aria-label={t('清空')} onClick={() => setSearch('')}>
                        <AgentIcon name='x' size={11} />
                      </InputGroupButton>
                    </InputGroupAddon>
                  ) : null}
                </InputGroup>
              </div>

              <div className='agent-split-list'>
                {tab === 'group' ? (
                  matches.length > 0 ? (
                    matches.map(item => {
                      const checked = selectedIds.has(item.id)
                      return (
                        <label key={item.id} className='agent-picker-row' data-checked={checked ? '' : undefined} title={item.label}>
                          <Checkbox
                            checked={checked}
                            disabled={disabled || (!checked && remaining <= 0)}
                            onCheckedChange={v => toggleItem(item, Boolean(v))}
                          />
                          <div className='min-w-0 flex-1'>
                            <div className='flex items-center gap-1.5'>
                              <span className='truncate text-[12.5px] font-medium text-foreground'>{item.name}</span>
                              {item.memberCount != null ? (
                                <span className='shrink-0 text-[11px] text-muted-foreground'>
                                  · {t('{n} 个模型', { n: item.memberCount })}
                                </span>
                              ) : null}
                            </div>
                            {item.groupSummary ? (
                              <div className='truncate text-[11px] text-muted-foreground'>{item.groupSummary}</div>
                            ) : null}
                          </div>
                          <code className='hidden max-w-[30%] shrink-0 truncate text-[11px] text-muted-foreground sm:inline'>
                            {item.id}
                          </code>
                        </label>
                      )
                    })
                  ) : (
                    <Empty className='py-10'>{search ? t('没有匹配的路由组') : t('暂无可用路由组')}</Empty>
                  )
                ) : matches.length > 0 ? (
                  matches.map(item => {
                    const checked = selectedIds.has(item.id)
                    return (
                      <label key={item.id} className='agent-picker-row' data-checked={checked ? '' : undefined} title={item.label}>
                        <Checkbox
                          checked={checked}
                          disabled={disabled || (!checked && remaining <= 0)}
                          onCheckedChange={v => toggleItem(item, Boolean(v))}
                        />
                        <span className='min-w-0 flex-1 truncate text-[12.5px] font-medium text-foreground'>{item.name}</span>
                        {item.name !== item.id ? (
                          <code className='max-w-[42%] shrink-0 truncate text-[11px] text-muted-foreground'>{item.id}</code>
                        ) : null}
                      </label>
                    )
                  })
                ) : (
                  <Empty className='py-10'>{t('没有匹配的模型')}</Empty>
                )}
              </div>
            </>
          ) : (
            <div className='flex flex-1 flex-col gap-3 p-3.5'>
              <div className='text-[13px] font-semibold text-foreground'>{t('添加自定义模型')}</div>
              <Field className='gap-1.5'>
                <FieldLabel className='text-xs text-subtle'>{t('模型 ID')}</FieldLabel>
                <Input
                  value={manualId}
                  disabled={disabled}
                  autoComplete='off'
                  spellCheck={false}
                  placeholder={t('例如：gpt-5.3-codex')}
                  className='font-mono text-[12.5px]'
                  onChange={e => setManualId(e.currentTarget.value)}
                  onKeyDown={e => { if (e.key === 'Enter') { e.preventDefault(); addManual() } }}
                />
              </Field>
              {client.id !== 'zcode' ? (
                <Field className='gap-1.5'>
                  <FieldLabel className='text-xs text-subtle'>{t('展示名称（选填）')}</FieldLabel>
                  <Input
                    value={manualName}
                    disabled={disabled}
                    autoComplete='off'
                    placeholder={t('留空则直接使用模型 ID')}
                    onChange={e => setManualName(e.currentTarget.value)}
                    onKeyDown={e => { if (e.key === 'Enter') { e.preventDefault(); addManual() } }}
                  />
                </Field>
              ) : null}
              <Button type='button' size='sm' className='self-start' disabled={disabled || remaining <= 0} onClick={addManual}>
                <AgentIcon name='plus' size={13} />
                {t('添加到右侧列表')}
              </Button>
            </div>
          )}
        </div>

        <div className='agent-split-col'>
          <div className='agent-split-col-head'>
            <span className='text-[13px] font-semibold text-foreground'>{t('已选模型')}</span>
            <Badge variant='outline' shape='tag'>{draft.models.filter(m => m.id).length}</Badge>
            <div className='ml-auto flex items-center gap-1'>
              {isGateway ? (
                <Button type='button' variant='ghost' size='xs' disabled={disabled || remaining <= 0} onClick={addManual}>
                  <AgentIcon name='plus' size={12} />
                  {t('手动补充')}
                </Button>
              ) : null}
              {draft.models.length > 0 ? (
                <Button
                  type='button'
                  variant='ghost'
                  size='xs'
                  disabled={disabled}
                  className='text-subtle hover:text-destructive'
                  onClick={() => replaceModels([])}
                >
                  {t('清空')}
                </Button>
              ) : null}
            </div>
          </div>

          <div className='agent-split-list p-2.5'>
            {draft.models.length === 0 ? (
              <Empty className='my-auto py-12'>
                <span>{isGateway ? t('从左侧勾选路由组或模型') : t('在左侧输入模型 ID 并添加')}</span>
              </Empty>
            ) : (
              draft.models.map((item, index) => {
                const key = rowKey(item)
                const known = catalogById.get(item.id)
                const isGroup = Boolean(known?.routingGroup)
                const isDefault = client.supportsDefault && Boolean(item.id) && item.id === draft.model
                const expanded = expandedKey === key
                const levels = modelLevels(client, item.reasoningLevels, draft.protocol)

                return (
                  <div key={key} className={cn('agent-selected-card', isDefault && 'is-default')}>
                    <div className='agent-selected-card-head'>
                      <button
                        type='button'
                        className='agent-selected-card-toggle'
                        onClick={() => setExpandedKey(expanded ? null : key)}
                      >
                        <div className='flex items-center gap-1.5'>
                          {isGroup ? <Badge variant='brand' shape='tag'>{t('路由组')}</Badge> : null}
                          <span className='truncate text-[13px] font-semibold text-foreground'>
                            {item.name || item.id || t('模型 {n}', { n: index + 1 })}
                          </span>
                          {isDefault ? <Badge variant='success' shape='tag'>{t('默认')}</Badge> : null}
                        </div>
                        <div className='mt-1 flex flex-wrap gap-x-2.5 gap-y-0.5 text-[11.5px] text-muted-foreground'>
                          {isGroup && known?.memberCount != null ? (
                            <span>
                              {t('{n} 个成员模型', { n: known.memberCount })}
                              {item.contextWindow != null ? ` · ${t('上下文')} ${formatTokenCount(item.contextWindow)}` : ''}
                            </span>
                          ) : (
                            <>
                              <span>{t('上下文')} <strong className='font-medium text-foreground'>{formatTokenCount(item.contextWindow)}</strong></span>
                              {!legacyKimi ? <span>{t('输出')} <strong className='font-medium text-foreground'>{formatTokenCount(item.maxOutputTokens)}</strong></span> : null}
                              <span>{t('工具')} <strong className={item.supportsTools ? 'text-success' : 'text-subtle'}>{item.supportsTools ? '✓' : item.supportsTools === false ? '✕' : '—'}</strong></span>
                              <span>{t('图片')} <strong className={item.supportsImages ? 'text-success' : 'text-subtle'}>{item.supportsImages ? '✓' : item.supportsImages === false ? '✕' : '—'}</strong></span>
                              <span>{t('思考')} <strong className={item.supportsReasoning ? 'text-success' : 'text-subtle'}>{item.supportsReasoning ? '✓' : item.supportsReasoning === false ? '✕' : '—'}</strong></span>
                            </>
                          )}
                        </div>
                      </button>

                      <div className='flex shrink-0 items-center gap-1'>
                        {client.supportsDefault && item.id && !isDefault ? (
                          <Button type='button' variant='ghost' size='xs' disabled={disabled} onClick={() => update({ model: item.id, effort: '' })}>
                            {t('设为默认')}
                          </Button>
                        ) : null}
                        <Button
                          type='button'
                          variant='ghost'
                          size='icon-xs'
                          aria-label={t('展开参数')}
                          onClick={() => setExpandedKey(expanded ? null : key)}
                        >
                          <span className={cn('agent-chevron', expanded && 'is-open')}><AgentIcon name='chevron' size={13} /></span>
                        </Button>
                        <Button
                          type='button'
                          variant='ghost'
                          size='icon-xs'
                          disabled={disabled}
                          className='text-subtle hover:text-destructive'
                          aria-label={t('移除')}
                          onClick={() => removeModel(index)}
                        >
                          <AgentIcon name='trash' size={13} />
                        </Button>
                      </div>
                    </div>

                    {expanded ? (
                      <div className='grid grid-cols-1 gap-2.5 border-t border-hairline bg-surface p-3 sm:grid-cols-2'>
                        {isGateway && known ? (
                          <div className='flex items-center justify-end sm:col-span-2'>
                            <Button
                              type='button'
                              variant='outline'
                              size='xs'
                              disabled={disabled}
                              onClick={() => patchModel(index, catalogModel(client, known))}
                            >
                              {t('重新填入目录能力')}
                            </Button>
                          </div>
                        ) : null}
                        <Field className='gap-1'>
                          <FieldLabel className='text-[11px] text-subtle'>{t('模型 ID')}</FieldLabel>
                          <Input
                            value={item.id}
                            disabled={disabled || sourceIds.current.has(item.id)}
                            autoComplete='off'
                            spellCheck={false}
                            className='h-8 font-mono text-xs'
                            onChange={e => patchModel(index, { id: e.currentTarget.value })}
                          />
                        </Field>
                        {client.id !== 'zcode' ? (
                          <Field className='gap-1'>
                            <FieldLabel className='text-[11px] text-subtle'>{t('模型展示名')}</FieldLabel>
                            <Input
                              value={item.name}
                              disabled={disabled}
                              autoComplete='off'
                              className='h-8 text-xs'
                              onChange={e => patchModel(index, { name: e.currentTarget.value })}
                            />
                          </Field>
                        ) : null}
                        {fields.includes('contextWindow') ? (
                          <Field className='gap-1'>
                            <FieldLabel className='text-[11px] text-subtle'>{t('上下文窗口')}</FieldLabel>
                            <Input
                              type='number'
                              min={1}
                              max={100_000_000}
                              step={1}
                              disabled={disabled}
                              className='h-8 text-xs'
                              placeholder={t('留空保留 / 未声明')}
                              value={item.contextWindow ?? ''}
                              onChange={e => patchModel(index, { contextWindow: e.currentTarget.value === '' ? null : Number(e.currentTarget.value) })}
                            />
                          </Field>
                        ) : null}
                        {fields.includes('maxOutputTokens') ? (
                          <Field className='gap-1'>
                            <FieldLabel className='text-[11px] text-subtle'>{t('最大输出 Token')}</FieldLabel>
                            <Input
                              type='number'
                              min={1}
                              max={100_000_000}
                              step={1}
                              disabled={disabled}
                              className='h-8 text-xs'
                              placeholder={t('留空保留 / 未声明')}
                              value={item.maxOutputTokens ?? ''}
                              onChange={e => patchModel(index, { maxOutputTokens: e.currentTarget.value === '' ? null : Number(e.currentTarget.value) })}
                            />
                          </Field>
                        ) : null}
                        {visibleCapabilities.map(capKey => (
                          <Field key={capKey} className='gap-1'>
                            <FieldLabel className='text-[11px] text-subtle'>{capLabel(capKey)}</FieldLabel>
                            <AgentSelect
                              label={capLabel(capKey)}
                              disabled={disabled}
                              value={item[capKey as keyof AgentModel] === true ? 'on' : item[capKey as keyof AgentModel] === false && client.id !== 'kimi-code' ? 'off' : 'inherit'}
                              options={[
                                { value: 'inherit', label: t('未声明 / 保留') },
                                { value: 'on', label: t('支持') },
                                ...(client.id === 'kimi-code' ? [] : [{ value: 'off', label: t('不支持') }]),
                              ]}
                              onChange={value => {
                                const patch: Partial<AgentModel> = { [capKey]: value === 'inherit' ? null : value === 'on' }
                                if (capKey === 'supportsReasoning' && value === 'off') patch.effort = ''
                                patchModel(index, patch)
                              }}
                            />
                          </Field>
                        ))}
                        {client.supportsEffort && item.supportsReasoning !== false && !['cline', 'dsh'].includes(client.id) && !(client.id === 'kimi-code' && legacyKimi) && (levels.length > 0 || item.effort) ? (
                          <Field className='gap-1 sm:col-span-2'>
                            <FieldLabel className='text-[11px] text-subtle'>{t('模型思考等级')}</FieldLabel>
                            <AgentSelect
                              label={t('模型思考等级')}
                              value={item.effort}
                              disabled={disabled}
                              options={[
                                { value: '', label: t('模型默认') },
                                ...[...new Set([...levels, ...(item.effort ? [item.effort] : [])])].map(v => ({ value: v, label: v })),
                              ]}
                              onChange={effort => patchModel(index, { effort })}
                            />
                          </Field>
                        ) : null}
                      </div>
                    ) : null}
                  </div>
                )
              })
            )}
          </div>
        </div>
      </div>
      {fieldNote ? <p className='m-0 mt-2 text-[11.5px] text-muted-foreground'>{fieldNote}</p> : null}
    </div>
  )
}

/**
 * 场景 C：单模型客户端（如 Gemini CLI）
 */
export function SingleModelWorkspace({
  client,
  draft,
  catalog,
  disabled,
  update,
}: {
  client: AgentClient
  draft: AgentProfile
  catalog: CatalogModel[]
  disabled: boolean
  update: (patch: Partial<AgentProfile>) => void
}) {
  const isGateway = draft.kind === 'gateway' && catalog.length > 0
  const groupCount = React.useMemo(() => catalog.filter(item => item.routingGroup).length, [catalog])
  const modelCount = catalog.length - groupCount
  const current = catalog.find(m => m.id === draft.model)
  const [tab, setTab] = React.useState<CatalogTab>(
    current ? (current.routingGroup ? 'group' : 'model') : (groupCount > 0 ? 'group' : 'model')
  )
  const [search, setSearch] = React.useState('')

  React.useEffect(() => {
    if (groupCount === 0 && tab === 'group') setTab('model')
  }, [groupCount, tab])

  const matches = React.useMemo(() => filterByTab(catalog, tab, search), [catalog, tab, search])
  const allowed = client.id === 'claude-code' ? ['low', 'medium', 'high', 'xhigh', 'max', 'auto'] : ['none', 'minimal', 'low', 'medium', 'high', 'xhigh']
  const levels = current?.reasoningLevels.filter(level => allowed.includes(level)) || []

  if (!isGateway) {
    return (
      <div className='flex flex-1 flex-col justify-center rounded-md border border-border bg-surface-inset p-6'>
        <div className='mx-auto flex w-full max-w-[440px] flex-col gap-3'>
          <Field className='gap-1.5'>
            <FieldLabel>{t('模型 ID')}</FieldLabel>
            <Input
              value={draft.model}
              disabled={disabled}
              autoComplete='off'
              spellCheck={false}
              placeholder={client.id === 'gemini-cli' ? 'gemini-2.5-pro' : t('填写真实模型 ID')}
              className='font-mono text-[13px]'
              onChange={e => update({ model: e.currentTarget.value, effort: '' })}
            />
          </Field>
          {client.supportsEffort && (levels.length > 0 || draft.effort) ? (
            <Field className='gap-1.5'>
              <FieldLabel>{t('思考等级')}</FieldLabel>
              <AgentSelect
                label={t('思考等级')}
                value={draft.effort}
                disabled={disabled}
                options={[
                  { value: '', label: t('模型默认') },
                  ...[...new Set([...levels, ...(draft.effort ? [draft.effort] : [])])].map(v => ({ value: v, label: v })),
                ]}
                onChange={effort => update({ effort })}
              />
            </Field>
          ) : null}
        </div>
      </div>
    )
  }

  return (
    <div className='agent-single-board'>
      <div className='agent-split-col-head'>
        <SegmentedControl<CatalogTab>
          value={tab}
          aria-label={t('选择类型')}
          options={[
            { value: 'group', label: t('路由组'), count: groupCount },
            { value: 'model', label: t('模型'), count: modelCount },
          ]}
          onValueChange={next => {
            setTab(next)
            setSearch('')
          }}
        />
        <InputGroup className='ml-auto w-[260px]'>
          <InputGroupInput
            type='search'
            value={search}
            autoComplete='off'
            disabled={disabled}
            placeholder={tab === 'group' ? t('搜索路由组…') : t('搜索模型…')}
            onChange={e => setSearch(e.currentTarget.value)}
          />
          <InputGroupAddon aria-hidden='true'><AgentIcon name='search' size={13} /></InputGroupAddon>
        </InputGroup>
      </div>

      <RadioGroup
        value={draft.model}
        onValueChange={value => update({ model: value, effort: '' })}
        className='agent-split-list gap-1 p-2'
      >
        {tab === 'group' ? (
          matches.length > 0 ? (
            matches.map(item => (
              <Label key={item.id} className='agent-picker-row' data-checked={draft.model === item.id ? '' : undefined}>
                <RadioGroupItem value={item.id} disabled={disabled} />
                <div className='min-w-0 flex-1'>
                  <div className='flex items-center gap-1.5'>
                    <span className='truncate text-[13px] font-medium text-foreground'>{item.name}</span>
                    {item.memberCount != null ? (
                      <span className='text-[11px] text-muted-foreground'>· {t('{n} 个模型', { n: item.memberCount })}</span>
                    ) : null}
                  </div>
                  {item.groupSummary ? <div className='truncate text-[11.5px] text-muted-foreground'>{item.groupSummary}</div> : null}
                </div>
                <code className='shrink-0 text-[11px] text-muted-foreground'>{item.id}</code>
              </Label>
            ))
          ) : (
            <Empty className='py-10'>{search ? t('没有匹配的路由组') : t('暂无可用路由组')}</Empty>
          )
        ) : matches.length > 0 ? (
          matches.map(item => (
            <Label key={item.id} className='agent-picker-row' data-checked={draft.model === item.id ? '' : undefined}>
              <RadioGroupItem value={item.id} disabled={disabled} />
              <span className='min-w-0 flex-1 truncate text-[13px] font-medium text-foreground'>{item.name}</span>
              {item.name !== item.id ? <code className='shrink-0 text-[11px] text-muted-foreground'>{item.id}</code> : null}
            </Label>
          ))
        ) : (
          <Empty className='py-10'>{t('没有匹配的模型')}</Empty>
        )}
      </RadioGroup>
    </div>
  )
}
