import * as React from 'react'
import {
  Badge,
  Button,
  Field,
  FieldLabel,
  Input,
  SegmentedControl,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Sheet,
  SheetBody,
  SheetContent,
  SheetFooter,
  SheetHeader,
  SheetTitle,
  Switch,
  cn,
} from '@ui'
import { t } from '../../i18n'
import type { ManageModel } from '../models-custom-source'
import type { ModelPricingSettings } from '../model-pricing'
import { testBlockReason, type ModelTestTarget } from '../model-test-dialog'
import { CUSTOM_LEVEL, bindingReasoningLevel, levels as reasoningLevels, modelReasoning } from '../models-reasoning'
import {
  accept,
  addMappingError,
  bindingKeyOf,
  bindingsOf,
  enabledBindingConflicts,
  errorMessage,
  esc,
  getSnapshot,
  levelOf,
  modelRowOf,
  providerLabelOf,
  rowEnabled,
  rowKeyOf,
  runRowAction,
  shared,
  subscribe,
  toast,
  writeBinding,
  writeRemoveMapping,
  type Binding,
} from '../models-panel-state'
import { DrawerCapabilitiesTab, DrawerPricingTab } from './models-drawer-tabs'

export type ModelDrawerTab = 'mapping' | 'caps' | 'pricing'
export type ModelDrawerContext = {
  provider: string
  id: string
  tab: ModelDrawerTab
}

const DRAWER_TABS: readonly { value: ModelDrawerTab; label: string }[] = [
  { value: 'mapping', label: t('映射与等级') },
  { value: 'caps', label: t('能力与规格') },
  { value: 'pricing', label: t('计价与信息') },
]

function CopySmallIcon() {
  return (
    <svg width='13' height='13' viewBox='0 0 24 24' fill='none' stroke='currentColor' strokeWidth='1.8' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
      <rect x='9' y='9' width='13' height='13' rx='2' />
      <path d='M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1' />
    </svg>
  )
}

function PlaySmallIcon() {
  return (
    <svg width='12' height='12' viewBox='0 0 24 24' fill='none' stroke='currentColor' strokeWidth='2' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
      <polygon points='6 4 20 12 6 20 6 4' />
    </svg>
  )
}

function BindingRowCard({
  model,
  binding,
  candidates,
  allowsCustom,
  busy,
}: {
  model: ManageModel
  binding: Binding
  candidates: string[]
  allowsCustom: boolean
  busy: boolean
}) {
  const providerId = model.provider || ''
  const reasoningInfo = modelReasoning(model)
  const savedLevel = levelOf(binding.alias, model.id, providerId)
  const inPreset = !savedLevel || candidates.includes(savedLevel)
  const [selectVal, setSelectVal] = React.useState(inPreset ? (savedLevel || '__none__') : CUSTOM_LEVEL)
  const [customVal, setCustomVal] = React.useState(inPreset ? '' : savedLevel)

  React.useEffect(() => {
    const nextInPreset = !savedLevel || candidates.includes(savedLevel)
    setSelectVal(nextInPreset ? (savedLevel || '__none__') : CUSTOM_LEVEL)
    setCustomVal(nextInPreset ? '' : savedLevel)
  }, [savedLevel, binding.alias, model.id, providerId, candidates.join('\u0001')])

  const on = binding.enabled !== false
  const conflicts = on ? [] : enabledBindingConflicts(providerId, binding.alias, model.id)
  const effectiveLevel = reasoningInfo.bindingSupported === true
    ? bindingReasoningLevel(savedLevel, model)
    : ''

  async function saveLevel(nextLevel: string): Promise<void> {
    if (nextLevel && reasoningInfo.bindingSupported === true && !bindingReasoningLevel(nextLevel, model)) {
      toast(t('该等级无法注入，请选择上游可用档位或清除绑定。'), 'err')
      return
    }
    await runRowAction(
      bindingKeyOf(binding.alias, model.id, providerId),
      () => writeBinding(providerId, binding.alias, model.id, { reasoning: nextLevel }),
      t('✅ 已更新 {name} 的思考等级', { name: binding.alias }),
    )
  }

  async function confirmDelete(): Promise<void> {
    const ok = await shared().wbConfirm?.ask?.({
      title: t('删除映射'),
      html: t('确定删除映射「<strong>{alias} → {target}</strong>」？', { alias: esc(binding.alias), target: esc(model.id) }),
      okText: t('删除'),
      okClass: 'danger',
    })
    if (!ok) return
    void runRowAction(
      bindingKeyOf(binding.alias, model.id, providerId),
      () => writeRemoveMapping(providerId, binding.alias, model.id),
      t('映射已删除'),
    )
  }

  const displayLevel = selectVal === CUSTOM_LEVEL
    ? t('自定义等级…')
    : selectVal === '__none__'
      ? t('不覆盖（跟随客户端）')
      : selectVal

  return (
    <div className={cn('mm-bind-card', !on && 'is-off')}>
      <div className='flex items-center gap-2'>
        <Switch
          size='sm'
          checked={on}
          disabled={busy}
          title={on ? t('映射已启用，点击关闭') : conflicts.length
            ? t('启用后将自动关闭该提供商的其他同名绑定：{bindings}', { bindings: conflicts.map(b => `${b.alias} → ${b.target}`).join('、') })
            : t('映射已关闭，点击启用')}
          aria-label={t('{action}映射 {alias}', { action: on ? t('禁用') : t('启用'), alias: binding.alias })}
          onCheckedChange={next => {
            void runRowAction(
              bindingKeyOf(binding.alias, model.id, providerId),
              () => writeBinding(providerId, binding.alias, model.id, { enabled: next }),
              next ? (conflicts.length ? t('映射已启用，其他同名绑定已自动关闭') : t('映射已启用')) : t('映射已关闭'),
            )
          }}
        />
        <span
          className='min-w-0 flex-1 cursor-pointer truncate font-mono text-[12.5px] font-semibold text-foreground hover:underline'
          data-copy={binding.alias}
          title={t('点击复制：{name}', { name: binding.alias })}
        >
          {binding.alias}
        </span>
        {binding.isDefault ? (
          <Badge variant='outline' shape='tag'>{t('默认')}</Badge>
        ) : (
          <Button variant='ghost' size='xs' className='text-destructive' disabled={busy} onClick={() => void confirmDelete()}>
            {t('删除')}
          </Button>
        )}
      </div>

      <div className='mt-2 flex flex-wrap items-center gap-2'>
        <span className='shrink-0 text-[11.5px] text-muted-foreground'>{t('思考等级')}</span>
        {reasoningInfo.fixedLevel ? (
          <Badge variant='secondary' shape='tag' className='font-mono'>
            {t('固定思考档位')} {reasoningInfo.fixedLevel}
          </Badge>
        ) : reasoningInfo.bindingSupported === false ? (
          <span className='text-[11.5px] text-muted-foreground'>{t('当前通道未声明可用档位')}</span>
        ) : (
          <Select
            value={selectVal}
            disabled={busy}
            onValueChange={val => {
              const next = String(val)
              setSelectVal(next)
              if (next !== CUSTOM_LEVEL) {
                void saveLevel(next === '__none__' ? '' : next)
              }
            }}
          >
            <SelectTrigger className='h-7 w-[184px] text-xs'>
              <SelectValue>{displayLevel}</SelectValue>
            </SelectTrigger>
            <SelectContent>
              <SelectItem value='__none__'>{t('不覆盖（跟随客户端）')}</SelectItem>
              {candidates.map(lv => <SelectItem key={lv} value={lv}>{lv}</SelectItem>)}
              {allowsCustom || selectVal === CUSTOM_LEVEL ? (
                <SelectItem value={CUSTOM_LEVEL}>{t('自定义等级…')}</SelectItem>
              ) : null}
            </SelectContent>
          </Select>
        )}
      </div>

      {selectVal === CUSTOM_LEVEL ? (
        <div className='mt-2 flex items-center gap-2'>
          <Input
            maxLength={32}
            value={customVal}
            placeholder={t('自定义等级（例如 custom-high）')}
            disabled={busy}
            onChange={e => setCustomVal(e.currentTarget.value)}
            onKeyDown={e => { if (e.key === 'Enter') void saveLevel(customVal.trim()) }}
          />
          <Button size='sm' variant='outline' disabled={busy} onClick={() => void saveLevel(customVal.trim())}>
            {t('保存')}
          </Button>
        </div>
      ) : null}

      {effectiveLevel && savedLevel && effectiveLevel.toLowerCase() !== savedLevel.toLowerCase() ? (
        <p className='mt-1.5 text-[11px] text-subtle'>
          {t('绑定 {requested}，按当前通道将发送 {effective}。', { requested: savedLevel, effective: effectiveLevel })}
        </p>
      ) : null}
    </div>
  )
}

function DrawerMappingTab({ model }: { model: ManageModel }) {
  const state = React.useSyncExternalStore(subscribe, getSnapshot)
  const providerId = model.provider || ''
  const bindings = bindingsOf(model)
  const reasoningInfo = modelReasoning(model)
  const candidates = reasoningLevels(state.data, model)
  const allowsCustom = reasoningInfo.bindingSupported === null

  const [newAlias, setNewAlias] = React.useState('')
  const [newLevel, setNewLevel] = React.useState('__none__')
  const [customLevel, setCustomLevel] = React.useState('')
  const [saving, setSaving] = React.useState(false)
  const [error, setError] = React.useState('')

  React.useEffect(() => {
    setNewAlias('')
    setNewLevel('__none__')
    setCustomLevel('')
    setError('')
  }, [model.provider, model.id])

  const levelToSave = newLevel === CUSTOM_LEVEL ? customLevel.trim() : (newLevel === '__none__' ? '' : newLevel)
  const duplicateError = newAlias.trim() ? addMappingError(providerId, newAlias.trim(), model.id) : ''

  async function addAlias(): Promise<void> {
    if (saving) return
    const wanted = newAlias.trim()
    if (!wanted) { setError(t('请填写对外映射名')); return }
    const dup = addMappingError(providerId, wanted, model.id)
    if (dup) { setError(dup); return }
    if (levelToSave && reasoningInfo.bindingSupported === true && !bindingReasoningLevel(levelToSave, model)) {
      setError(t('该等级无法注入，请选择上游可用档位或清除绑定。'))
      return
    }
    setSaving(true)
    setError('')
    try {
      accept(await writeBinding(providerId, wanted, model.id, { reasoning: levelToSave, create: true }))
      setNewAlias('')
      setNewLevel('__none__')
      setCustomLevel('')
      const label = providerLabelOf(providerId)
      const suffix = levelToSave ? t(' · 思考等级 {level}', { level: levelToSave }) : ''
      toast(t('✅ 已添加映射 {name} → {target}（{label}）{suffix}', { name: wanted, target: model.id, label, suffix }))
    } catch (err) {
      setError(t('保存失败：{message}', { message: errorMessage(err) }))
    } finally {
      setSaving(false)
    }
  }

  return (
    <>
      <section className='mm-drawer-section'>
        <div className='mm-drawer-section-head'>
          <h3>{t('当前对外绑定')}（{bindings.length}）</h3>
        </div>
        <div className='flex flex-col gap-2'>
          {bindings.map(binding => (
            <BindingRowCard
              key={`${binding.alias}\u0001${binding.isDefault ? 'd' : 'm'}`}
              model={model}
              binding={binding}
              candidates={candidates}
              allowsCustom={allowsCustom}
              busy={state.pending.has(bindingKeyOf(binding.alias, model.id, providerId))}
            />
          ))}
        </div>
      </section>

      <section className='mm-drawer-section'>
        <div className='mm-drawer-section-head'>
          <h3>{t('新增对外别名')}</h3>
        </div>
        <div className='flex flex-col gap-2.5 rounded-md border border-border bg-surface-2 p-3'>
          <Field>
            <FieldLabel htmlFor='drawer-new-alias'>{t('对外映射名（下游请求时用）')}</FieldLabel>
            <Input
              id='drawer-new-alias'
              maxLength={128}
              placeholder={t('例如 gpt-4o')}
              autoComplete='off'
              value={newAlias}
              disabled={saving}
              onChange={e => { setNewAlias(e.currentTarget.value); setError('') }}
              onKeyDown={e => { if (e.key === 'Enter') { e.preventDefault(); void addAlias() } }}
            />
          </Field>
          <div className='flex items-end gap-2'>
            <Field className='flex-1'>
              <FieldLabel>{t('思考等级')}</FieldLabel>
              <Select
                value={newLevel}
                disabled={saving || reasoningInfo.bindingSupported === false}
                onValueChange={v => setNewLevel(String(v))}
              >
                <SelectTrigger className='w-full'>
                  <SelectValue>
                    {newLevel === CUSTOM_LEVEL ? t('自定义等级…') : newLevel === '__none__' ? t('不覆盖') : newLevel}
                  </SelectValue>
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value='__none__'>{t('不覆盖')}</SelectItem>
                  {candidates.map(lv => <SelectItem key={lv} value={lv}>{lv}</SelectItem>)}
                  {allowsCustom ? <SelectItem value={CUSTOM_LEVEL}>{t('自定义等级…')}</SelectItem> : null}
                </SelectContent>
              </Select>
            </Field>
            <Button disabled={saving || !newAlias.trim() || Boolean(duplicateError)} onClick={() => void addAlias()}>
              {saving ? t('保存中…') : t('＋ 添加映射')}
            </Button>
          </div>
          {newLevel === CUSTOM_LEVEL ? (
            <Input
              maxLength={32}
              placeholder={t('自定义等级（例如 custom-high）')}
              value={customLevel}
              disabled={saving}
              onChange={e => setCustomLevel(e.currentTarget.value)}
            />
          ) : null}
          {newAlias.trim() ? (
            <div className='rounded bg-surface-inset px-2.5 py-1.5 font-mono text-[11.5px] text-subtle'>
              {t('下游请求 ')}<b className='text-primary-fg'>{newAlias.trim()}</b>{t(' → 转发 ')}{' '}
              <b className='text-primary-fg'>{model.id}</b>
              {levelToSave ? ` · ${levelToSave}` : ''}
            </div>
          ) : null}
          {duplicateError || error ? (
            <div className='text-xs text-destructive' role='status'>{duplicateError || error}</div>
          ) : null}
        </div>
      </section>
    </>
  )
}

export function ModelConfigDrawer({
  context,
  shownModels,
  pricingSettings,
  onNavigate,
  onTabChange,
  onClose,
  onOpenTest,
  onSavedPricing,
}: {
  context: ModelDrawerContext
  shownModels: ManageModel[]
  pricingSettings: ModelPricingSettings | null
  onNavigate: (model: ManageModel) => void
  onTabChange: (tab: ModelDrawerTab) => void
  onClose: () => void
  onOpenTest: (target: ModelTestTarget) => void
  onSavedPricing: () => Promise<void> | void
}) {
  const state = React.useSyncExternalStore(subscribe, getSnapshot)
  const row = modelRowOf(context.provider, context.id)
  const index = shownModels.findIndex(m => (m.provider || '') === context.provider && m.id === context.id)
  const total = shownModels.length

  React.useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const tag = (event.target as HTMLElement | null)?.tagName
      if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return
      if (event.key === 'ArrowUp' && index > 0) {
        event.preventDefault()
        onNavigate(shownModels[index - 1])
      } else if (event.key === 'ArrowDown' && index >= 0 && index < total - 1) {
        event.preventDefault()
        onNavigate(shownModels[index + 1])
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [index, total, shownModels, onNavigate])

  if (!row) {
    return (
      <Sheet open onOpenChange={open => { if (!open) onClose() }}>
        <SheetContent side='right' className='w-[min(480px,92vw)]'>
          <SheetHeader><SheetTitle>{t('模型配置')}</SheetTitle></SheetHeader>
          <SheetBody>
            <p className='text-sm leading-relaxed text-subtle'>
              {t('这一行已不在当前清单里（模型被移除、或目录刷新后上游不再提供它）。')}
            </p>
          </SheetBody>
          <SheetFooter className='justify-end'>
            <Button variant='outline' onClick={onClose}>{t('关闭')}</Button>
          </SheetFooter>
        </SheetContent>
      </Sheet>
    )
  }

  const providerId = row.provider || ''
  const providerLabel = row.providerLabel || providerLabelOf(providerId)
  const enabled = rowEnabled(row)
  const busyRow = state.pending.has(rowKeyOf(row))
  const blocked = testBlockReason(providerId, row)

  async function toggleMaster(next: boolean): Promise<void> {
    if (!row) return
    const targetRow = row
    const list = bindingsOf(targetRow)
    await runRowAction(
      rowKeyOf(targetRow),
      async () => {
        let last: unknown = null
        for (const binding of list) {
          last = await writeBinding(providerId, binding.alias, targetRow.id, { enabled: next })
        }
        return last
      },
      next ? t('已启用 {id}', { id: targetRow.id }) : t('已禁用 {id}', { id: targetRow.id }),
    )
  }

  return (
    <Sheet open onOpenChange={open => { if (!open) onClose() }}>
      <SheetContent side='right' className='w-[min(480px,92vw)]'>
        <SheetHeader className='flex-col items-stretch gap-2.5 pr-12'>
          <div className='flex items-center justify-between gap-2'>
            <div className='flex min-w-0 items-center gap-2'>
              <Badge variant='outline' shape='tag'>{providerLabel}</Badge>
              {index >= 0 && total > 1 ? (
                <div className='flex items-center gap-0.5'>
                  <Button variant='ghost' size='icon-xs' disabled={index <= 0}
                    title={t('上一个模型（↑）')} onClick={() => onNavigate(shownModels[index - 1])}>↑</Button>
                  <Button variant='ghost' size='icon-xs' disabled={index >= total - 1}
                    title={t('下一个模型（↓）')} onClick={() => onNavigate(shownModels[index + 1])}>↓</Button>
                  <span className='font-mono text-[11px] text-muted-foreground tabular-nums'>{index + 1}/{total}</span>
                </div>
              ) : null}
            </div>
            <Button variant='outline' size='xs' disabled={Boolean(blocked)}
              title={blocked || t('以这个上游模型发一次最小请求，走真实转发链路')}
              onClick={() => onOpenTest({ provider: providerId, id: row.id })}>
              <PlaySmallIcon />
              {t('测试')}
            </Button>
          </div>

          <div className='flex min-w-0 items-center justify-between gap-2'>
            <div className='flex min-w-0 items-center gap-1.5'>
              <SheetTitle className='cursor-pointer truncate font-mono text-[14.5px] font-semibold text-foreground'
                data-copy={row.id} title={t('点击复制：{name}', { name: row.id })}>
                {row.id}
              </SheetTitle>
              <Button variant='ghost' size='icon-xs' data-copy={row.id} title={t('点击复制：{name}', { name: row.id })}>
                <CopySmallIcon />
              </Button>
            </div>
            <div className='flex shrink-0 items-center gap-2'>
              <span className='text-xs text-muted-foreground'>{enabled ? t('已启用') : t('已禁用')}</span>
              <Switch checked={enabled} disabled={busyRow} aria-label={t('切换模型启用状态')}
                onCheckedChange={next => void toggleMaster(next)} />
            </div>
          </div>

          {row.name && row.name !== row.id ? (
            <div className='-mt-1.5 truncate text-xs text-muted-foreground'>{row.name}</div>
          ) : null}

          <SegmentedControl<ModelDrawerTab>
            className='w-full [&_[data-slot=segmented-item]]:flex-1 [&_[data-slot=segmented-item]]:justify-center'
            value={context.tab}
            onValueChange={onTabChange}
            options={DRAWER_TABS}
            aria-label={t('配置分类')}
          />
        </SheetHeader>

        <SheetBody className='gap-4'>
          {context.tab === 'mapping' ? <DrawerMappingTab model={row} /> : null}
          {context.tab === 'caps' ? <DrawerCapabilitiesTab model={row} /> : null}
          {context.tab === 'pricing' ? (
            <DrawerPricingTab
              model={row}
              pricingSettings={pricingSettings}
              onSavedPricing={onSavedPricing}
              onRemoved={onClose}
            />
          ) : null}
        </SheetBody>

        <SheetFooter className='justify-between'>
          <span className='text-[11.5px] text-muted-foreground'>
            {total > 1 ? t('支持 ↑ ↓ 切换上下模型') : ''}
          </span>
          <Button variant='outline' size='sm' onClick={onClose}>{t('关闭')}</Button>
        </SheetFooter>
      </SheetContent>
    </Sheet>
  )
}
