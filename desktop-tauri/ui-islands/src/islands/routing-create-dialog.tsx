import * as React from 'react'
import { Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, Empty, Input, Label, RadioGroup, RadioGroupItem, SegmentedControl } from '@ui'
import { t } from '../i18n'
import type { AccountRecord } from './accounts-shared'
import { createRouteMembers, newRouteId, routeGroupFieldsError, routeModelKey, RoutingGroupForm, type RoutingGroupFields } from './routing-group-form'
import { RoutingModelPicker } from './routing-model-picker'
import { sortRoutingModels } from './routing-model-order'
import { RouteIcon } from './routing-ui'
import { errorMessage, type RouteGroup, type RouteModelChoice, type RoutingWorkspace } from './routing-types'

type RouteType = 'model' | 'group'

export function RoutingCreateDialog({ workspace, choices, accounts, lastUsed, reserved, groupModels, modelError, disabled, onCreateModel, onCreateGroup, onClose }: {
  workspace: RoutingWorkspace; choices: RouteModelChoice[]; accounts: AccountRecord[]; lastUsed?: Record<string, number>; reserved: string[]; groupModels: string[]
  modelError?: string; disabled: boolean; onCreateModel(model: string): Promise<boolean>; onCreateGroup(group: RouteGroup): boolean; onClose(): void
}) {
  const [type, setType] = React.useState<RouteType>('model')
  const [search, setSearch] = React.useState('')
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
  const query = search.trim().toLowerCase()
  const models = React.useMemo(() => availableModels.filter(model => model.toLowerCase().includes(query)), [availableModels, query])
  const memberChoices = React.useMemo(() => choices.filter(choice => !groupIds.has(choice.model.toLowerCase())), [choices, groupIds])
  const byKey = React.useMemo(() => new Map(memberChoices.map(choice => [routeModelKey(choice), choice])), [memberChoices])
  const picked = selectedMembers.flatMap(key => { const choice = byKey.get(key); return choice ? [choice] : [] })
  const selected = availableModels.includes(selectedModel)

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
    <DialogHeader><RouteIcon name='route' /><DialogTitle>{t('新增路由配置')}</DialogTitle></DialogHeader>
    <DialogBody>
      <SegmentedControl aria-label={t('路由类型')} value={type} disabled={locked} className='self-start' options={[
        { value: 'model', label: t('模型独立路由') }, { value: 'group', label: t('路由组') },
      ]} onValueChange={value => { setType(value); setError('') }} />
      {type === 'model' ? <>
        <p className='routing-config-note'>{t('从现有模型中创建独立配置，自动列出提供它的全部提供商；之后的修改只跟随这个模型。')}</p>
        <Input value={search} disabled={locked} placeholder={t('搜索模型')} aria-label={t('搜索模型')}
          onChange={event => setSearch(event.currentTarget.value)} />
        <div className='routing-create-models'>
          <Button variant='ghost' className='routing-model-all' disabled={locked} aria-pressed={!search.trim()} title={t('显示全部')}
            onClick={() => setSearch('')}>{t('全部')}</Button>
          <RadioGroup className='routing-model-picker routing-create-model-list' value={selectedModel} disabled={locked} aria-label={t('模型独立路由')}
            onValueChange={value => setSelectedModel(value)}>
            {models.map(model => <Label key={model} className='routing-single-model' data-checked={selectedModel === model ? '' : undefined}>
              <RadioGroupItem value={model} /><span title={model}>{model}</span>
            </Label>)}
            {!models.length ? <Empty>{search.trim() ? t('没有匹配的模型') : t('没有可添加的模型')}</Empty> : null}
          </RadioGroup>
        </div>
        <p className='routing-config-note'>{t('共 {n} 个可添加的模型', { n: availableModels.length })}{selected ? ` · ${selectedModel}` : ''}</p>
      </> : <>
        <RoutingGroupForm value={fields} onChange={setFields} disabled={locked} />
        <p className='routing-config-note'>{t('可以跨提供商、跨模型多选。同一提供商也可以加入不同模型。')}</p>
        <RoutingModelPicker choices={memberChoices} accounts={accounts} lastUsed={usageSnapshot} selected={selectedMembers} onChange={setSelectedMembers} disabled={locked} />
      </>}
      {error || type === 'model' && modelError ? <p className='routing-config-error' role='alert'>{error || modelError}</p> : null}
    </DialogBody>
    <DialogFooter><Button variant='outline' disabled={busy} onClick={onClose}>{t('取消')}</Button>
      <Button disabled={locked || (type === 'model' ? !selected : !picked.length || !fields.name.trim() || !fields.model.trim())} onClick={() => void create()}>
        {busy ? t('保存中…') : type === 'model' ? t('创建独立配置') : t('创建草稿')}
      </Button>
    </DialogFooter>
  </DialogContent></Dialog>
}
