import * as React from 'react'
import {
  Badge, Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, Empty,
  InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput, Label, RadioGroup, RadioGroupItem,
  SegmentedControl, Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
} from '@ui'
import { t } from '../i18n'
import { providerOf } from './accounts-domain'
import type { AccountRecord } from './accounts-shared'
import { RoutingProviderMark } from './routing-canvas-controls'
import { createRouteMembers, newRouteId, routeGroupFieldsError, routeModelKey, RoutingGroupForm, type RoutingGroupFields } from './routing-group-form'
import { RoutingModelPicker, RoutingModelSummaryBar } from './routing-model-picker'
import { sortRoutingModels } from './routing-model-order'
import { RouteIcon } from './routing-ui'
import { errorMessage, type RouteGroup, type RouteModelChoice, type RoutingWorkspace } from './routing-types'

type RouteType = 'model' | 'group'
type StandaloneProvider = { id: string; label: string; accounts: number }

export function RoutingCreateDialog({ workspace, choices, accounts, lastUsed, reserved, groupModels, modelError, disabled, onCreateModel, onCreateGroup, onClose }: {
  workspace: RoutingWorkspace; choices: RouteModelChoice[]; accounts: AccountRecord[]; lastUsed?: Record<string, number>; reserved: string[]; groupModels: string[]
  modelError?: string; disabled: boolean; onCreateModel(model: string): Promise<boolean>; onCreateGroup(group: RouteGroup): boolean; onClose(): void
}) {
  const [type, setType] = React.useState<RouteType>('model')
  const [search, setSearch] = React.useState('')
  const [providerFilter, setProviderFilter] = React.useState('all')
  const [selectedModel, setSelectedModel] = React.useState('')
  const [selectedMembers, setSelectedMembers] = React.useState<string[]>([])
  const [fields, setFields] = React.useState<RoutingGroupFields>({ name: '', model: '', description: '' })
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  // 弹窗打开后保持排序快照，后台刷新不能移动正在选择的模型。
  const [usageSnapshot] = React.useState(() => lastUsed || workspace.modelLastUsed)
  const orderedModels = React.useMemo(() => sortRoutingModels(workspace.models, model => model, usageSnapshot), [workspace.models, usageSnapshot])
  const locked = disabled || busy
  const groupIds = React.useMemo(() => new Set(groupModels.map(model => model.toLowerCase())), [groupModels])
  const configured = React.useMemo(() => new Set(workspace.configuredModels.map(model => model.toLowerCase())), [workspace.configuredModels])
  const availableModels = React.useMemo(() => orderedModels.filter(model => !configured.has(model.toLowerCase()) && !groupIds.has(model.toLowerCase())), [orderedModels, configured, groupIds])

  const accountCountsByProvider = React.useMemo(() => {
    const map = new Map<string, Set<string>>()
    for (const account of accounts) {
      const id = providerOf(account)
      const set = map.get(id) || new Set<string>()
      set.add(account.id)
      map.set(id, set)
    }
    return new Map([...map.entries()].map(([id, set]) => [id, set.size]))
  }, [accounts])

  const modelProvidersMap = React.useMemo(() => {
    const map = new Map<string, StandaloneProvider[]>()
    for (const choice of choices) {
      const key = choice.model.toLowerCase()
      const list = map.get(key) || []
      if (!list.some(item => item.id === choice.provider)) {
        list.push({
          id: choice.provider,
          label: choice.providerLabel,
          accounts: accountCountsByProvider.get(choice.provider) ?? choice.accountCount ?? 0,
        })
        map.set(key, list)
      }
    }
    return map
  }, [choices, accountCountsByProvider])

  const standaloneProviders = React.useMemo(() => {
    const map = new Map<string, StandaloneProvider>()
    for (const model of availableModels) {
      for (const provider of modelProvidersMap.get(model.toLowerCase()) || []) {
        if (!map.has(provider.id)) map.set(provider.id, provider)
      }
    }
    return [...map.values()].sort((a, b) => Number(b.accounts > 0) - Number(a.accounts > 0) || a.label.localeCompare(b.label))
  }, [availableModels, modelProvidersMap])

  const recentStandalone = React.useMemo(() => {
    const used = usageSnapshot || {}
    const hits: string[] = []
    for (const model of availableModels) {
      const at = used[model] ?? used[model.toLowerCase()] ?? 0
      if (Number.isFinite(at) && at > 0) hits.push(model.toLowerCase())
      if (hits.length >= 3) break
    }
    return new Set(hits)
  }, [availableModels, usageSnapshot])

  const query = search.trim().toLowerCase()
  const models = React.useMemo(() => availableModels.filter(model => {
    const providers = modelProvidersMap.get(model.toLowerCase()) || []
    if (providerFilter !== 'all' && !providers.some(item => item.id === providerFilter)) return false
    if (!query) return true
    return model.toLowerCase().includes(query) || providers.some(item => item.label.toLowerCase().includes(query))
  }), [availableModels, modelProvidersMap, providerFilter, query])

  const providerFilterLabel = providerFilter === 'all'
    ? t('全部提供商')
    : standaloneProviders.find(item => item.id === providerFilter)?.label || t('全部提供商')

  const memberChoices = React.useMemo(() => choices.filter(choice => !groupIds.has(choice.model.toLowerCase())), [choices, groupIds])
  const byKey = React.useMemo(() => new Map(memberChoices.map(choice => [routeModelKey(choice), choice])), [memberChoices])
  const picked = selectedMembers.flatMap(key => { const choice = byKey.get(key); return choice ? [choice] : [] })
  const selected = availableModels.includes(selectedModel)
  const selectedProviders = selected ? (modelProvidersMap.get(selectedModel.toLowerCase()) || []) : []

  const create = async () => {
    if (locked) return
    setError('')
    if (type === 'group') {
      const message = routeGroupFieldsError(fields, reserved)
      if (message) { setError(message); return }
      if (!picked.length) { setError(t('请选择模型')); return }
      if (!onCreateGroup({ id: newRouteId(), name: fields.name.trim(), model: fields.model.trim().toLowerCase(), description: fields.description.trim(),
        strategy: 'order', sticky: 'auto', members: createRouteMembers(picked, accounts) })) {
        setError(t('路由数据正在更新，请稍后重试')); return
      }
      onClose()
      return
    }
    if (!selected) { setError(t('请选择模型')); return }
    setBusy(true)
    try { if (await onCreateModel(selectedModel)) onClose() }
    catch (failure) { setError(errorMessage(failure)) }
    finally { setBusy(false) }
  }

  return <Dialog open onOpenChange={(open, details) => {
    if (open) return
    if (busy) { details.cancel(); return }
    onClose()
  }}><DialogContent className='routing-create-dialog'>
    <DialogHeader>
      <span className='routing-create-badge'><RouteIcon name='route' size={15} /></span>
      <DialogTitle>{t('新增路由配置')}</DialogTitle>
      <SegmentedControl aria-label={t('路由类型')} value={type} disabled={locked} options={[
        { value: 'model', label: t('模型独立路由') }, { value: 'group', label: t('路由组') },
      ]} onValueChange={value => { setType(value); setError('') }} />
    </DialogHeader>
    <DialogBody>
      {type === 'model' ? <>
        <div className='routing-standalone-tools'>
          <div className='min-w-0 flex-1'>
            <InputGroup>
              <InputGroupInput value={search} disabled={locked} placeholder={t('搜索可添加的模型名称或提供商…')} aria-label={t('搜索模型')}
                onChange={event => setSearch(event.currentTarget.value)} />
              <InputGroupAddon align='inline-start'><RouteIcon name='search' size={13} /></InputGroupAddon>
              {search ? <InputGroupAddon align='inline-end'>
                <InputGroupButton size='icon-xs' aria-label={t('清空搜索')} disabled={locked} onClick={() => setSearch('')}>
                  <RouteIcon name='x' size={12} />
                </InputGroupButton>
              </InputGroupAddon> : null}
            </InputGroup>
          </div>
          {standaloneProviders.length > 1 ? <Select value={providerFilter} disabled={locked} onValueChange={value => setProviderFilter(value || 'all')}>
            <SelectTrigger className='routing-standalone-provider-select' aria-label={t('按提供商筛选')}>
              <SelectValue>{providerFilterLabel}</SelectValue>
            </SelectTrigger>
            <SelectContent>
              <SelectItem value='all'>{t('全部提供商')}</SelectItem>
              {standaloneProviders.map(item => <SelectItem key={item.id} value={item.id}>{item.label}</SelectItem>)}
            </SelectContent>
          </Select> : null}
        </div>

        <div className='routing-create-models'>
          <div className='routing-standalone-head'>
            <span>
              {t('模型名称')}
              <span className='routing-standalone-count'>
                {query || providerFilter !== 'all'
                  ? t('匹配 {matched} / {total}', { matched: models.length, total: availableModels.length })
                  : t('共 {n} 个 · 按最近使用排序', { n: availableModels.length })}
              </span>
            </span>
            <span>{t('关联提供商')}</span>
          </div>
          <RadioGroup className='routing-picker-scroll routing-create-model-list' value={selectedModel} disabled={locked} aria-label={t('模型独立路由')}
            onValueChange={value => { setSelectedModel(value); setError('') }}>
            {models.map(model => {
              const providers = modelProvidersMap.get(model.toLowerCase()) || []
              const shown = providers.slice(0, 4)
              const extra = providers.length - shown.length
              const totalAccounts = providers.reduce((sum, item) => sum + item.accounts, 0)
              const isRecent = recentStandalone.has(model.toLowerCase())
              const titleText = providers.length ? `${model} — ${providers.map(item => item.label).join('、')}` : model
              return <Label key={model} className='routing-picker-row routing-single-model' data-checked={selectedModel === model ? '' : undefined} title={titleText}>
                <RadioGroupItem value={model} />
                <div className='routing-single-model-main'>
                  <span className='routing-single-model-name'>{model}</span>
                  {isRecent ? <Badge variant='outline' shape='tag' className='shrink-0 font-normal text-muted-foreground'>{t('最近使用')}</Badge> : null}
                </div>
                {providers.length ? <div className='routing-single-model-meta'>
                  <div className='routing-logo-stack'>
                    {shown.map(item => <span key={item.id} className='routing-logo-stack-item'><RoutingProviderMark id={item.id} label={item.label} /></span>)}
                    {extra > 0 ? <span className='routing-logo-stack-item routing-logo-stack-more'>+{extra}</span> : null}
                  </div>
                  <span className='routing-single-model-counts'>{t('{providers} 家 · {accounts} 账号', { providers: providers.length, accounts: totalAccounts })}</span>
                </div> : null}
              </Label>
            })}
            {!models.length ? <div className='routing-picker-empty'>
              <Empty className='p-0'>{query || providerFilter !== 'all' ? t('没有匹配的模型') : t('没有可添加的模型')}</Empty>
              {(query || providerFilter !== 'all') ? <Button variant='outline' size='xs' disabled={locked} onClick={() => { setSearch(''); setProviderFilter('all') }}>{t('显示全部模型')}</Button> : null}
            </div> : null}
          </RadioGroup>
        </div>
      </> : <>
        <RoutingGroupForm value={fields} onChange={next => { setFields(next); setError('') }} disabled={locked} />
        <RoutingModelPicker choices={memberChoices} accounts={accounts} lastUsed={usageSnapshot} selected={selectedMembers}
          onChange={next => { setSelectedMembers(next); setError('') }} disabled={locked} />
      </>}
      {error || type === 'model' && modelError ? <p className='routing-config-error' role='alert'>{error || modelError}</p> : null}
    </DialogBody>
    <DialogFooter>
      {type === 'model' ? (selected ? <div className='routing-footer-picked'>
        <Badge variant='brand' shape='tag'>{t('已选')}</Badge>
        <span className='routing-footer-model-name' title={selectedModel}>{selectedModel}</span>
        {selectedProviders.length ? <span className='routing-footer-model-sub'>· {t('{n} 家提供商', { n: selectedProviders.length })}</span> : null}
      </div> : <span className='routing-footer-hint'>{t('创建后自动带入该模型的全部提供商，修改不影响全局路由')}</span>)
        : <RoutingModelSummaryBar picked={picked} disabled={locked}
          onRemove={key => setSelectedMembers(prev => prev.filter(item => item !== key))}
          onClear={() => setSelectedMembers([])} />}
      <Button variant='outline' disabled={busy} onClick={onClose}>{t('取消')}</Button>
      <Button disabled={locked || (type === 'model' ? !selected : !picked.length || !fields.name.trim() || !fields.model.trim())} onClick={() => void create()}>
        {busy ? t('保存中…') : type === 'model' ? t('创建独立配置') : t('创建草稿')}
      </Button>
    </DialogFooter>
  </DialogContent></Dialog>
}
