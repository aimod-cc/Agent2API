import * as React from 'react'
import {
  Badge,
  Button,
  Empty,
  Input,
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
  Popover,
  PopoverContent,
  PopoverTrigger,
  SegmentedControl,
  cn,
} from '@ui'
import { t } from '../i18n'
import { AgentIcon } from './agent-actions'
import type { CatalogModel } from './agent-catalog'
import { SplitModelsWorkspace } from './agent-profile-models'
import type { AgentClient, AgentProfile } from './agent-types'

type CatalogTab = 'group' | 'model'
export type ClaudeTierKey = 'opus' | 'sonnet' | 'haiku' | 'subagent'

function filterByTab(catalog: CatalogModel[], tab: CatalogTab, keyword: string): CatalogModel[] {
  const q = keyword.trim().toLowerCase()
  return catalog.filter(item => {
    const isGroup = Boolean(item.routingGroup)
    if (tab === 'group' ? !isGroup : isGroup) return false
    if (!q) return true
    return `${item.name} ${item.id} ${item.groupSummary || ''}`.toLowerCase().includes(q)
  })
}

export function detectClaudeDefaultTier(draft: AgentProfile): ClaudeTierKey {
  const current = draft.model.trim()
  if (current) {
    for (const key of ['sonnet', 'opus', 'haiku', 'subagent'] as const) {
      if (draft.claudeModels[key] === current) return key
    }
  }
  return 'sonnet'
}

/**
 * 纯两档切换（路由组 | 模型）的搜索下拉选择器，用于 Claude Code 四个坑位与批量套用。
 */
export function TargetModelCombobox({
  value,
  catalog,
  disabled,
  placeholder,
  allowEmpty = false,
  emptyLabel,
  onChange,
}: {
  value: string
  catalog: CatalogModel[]
  disabled?: boolean
  placeholder: string
  allowEmpty?: boolean
  emptyLabel?: string
  onChange: (nextId: string) => void
}) {
  const current = catalog.find(item => item.id === value)
  const groupCount = React.useMemo(() => catalog.filter(item => item.routingGroup).length, [catalog])
  const modelCount = catalog.length - groupCount
  const initialTab: CatalogTab = current
    ? (current.routingGroup ? 'group' : 'model')
    : (groupCount > 0 ? 'group' : 'model')

  const [open, setOpen] = React.useState(false)
  const [search, setSearch] = React.useState('')
  const [tab, setTab] = React.useState<CatalogTab>(initialTab)

  React.useEffect(() => {
    if (open) {
      setTab(current ? (current.routingGroup ? 'group' : 'model') : (groupCount > 0 ? 'group' : 'model'))
      setSearch('')
    }
  }, [open, current?.id, current?.routingGroup, groupCount])

  const matches = React.useMemo(() => filterByTab(catalog, tab, search), [catalog, tab, search])

  const pick = (id: string) => {
    onChange(id)
    setOpen(false)
    setSearch('')
  }

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger
        disabled={disabled}
        render={
          <Button
            type='button'
            variant='outline'
            className='h-9 w-full justify-between gap-2 bg-surface px-3 font-normal'
          />
        }
      >
        <span className='flex min-w-0 items-center gap-2'>
          {current ? (
            <>
              {current.routingGroup ? <Badge variant='brand' shape='tag' className='shrink-0'>{t('路由组')}</Badge> : null}
              <span className='truncate text-[13px] font-medium text-foreground'>{current.name}</span>
              {current.name !== current.id ? (
                <code className='hidden truncate text-[11px] text-muted-foreground sm:inline'>{current.id}</code>
              ) : null}
            </>
          ) : value ? (
            <span className='truncate font-mono text-[12.5px] text-foreground'>{value}</span>
          ) : (
            <span className='text-[12.5px] text-muted-foreground'>{placeholder}</span>
          )}
        </span>
        <span className={cn('agent-chevron shrink-0 text-subtle', open && 'is-open')}>
          <AgentIcon name='chevron' size={13} />
        </span>
      </PopoverTrigger>

      <PopoverContent align='start' className='agent-combo-popover'>
        <div className='flex items-center gap-2'>
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
          <InputGroup className='min-w-0 flex-1'>
            <InputGroupInput
              type='search'
              value={search}
              autoComplete='off'
              placeholder={tab === 'group' ? t('搜索路由组…') : t('搜索模型…')}
              onChange={e => setSearch(e.currentTarget.value)}
            />
            <InputGroupAddon aria-hidden='true'><AgentIcon name='search' size={12} /></InputGroupAddon>
            {search ? (
              <InputGroupAddon align='inline-end'>
                <InputGroupButton size='icon-xs' aria-label={t('清空')} onClick={() => setSearch('')}>
                  <AgentIcon name='x' size={11} />
                </InputGroupButton>
              </InputGroupAddon>
            ) : null}
          </InputGroup>
        </div>

        <div className='agent-combo-list'>
          {allowEmpty && value ? (
            <button type='button' className='agent-combo-item' onClick={() => pick('')}>
              <span className='text-xs text-muted-foreground'>{emptyLabel || t('不写入（留空）')}</span>
            </button>
          ) : null}

          {tab === 'group' ? (
            matches.length > 0 ? (
              matches.map(item => (
                <button
                  key={item.id}
                  type='button'
                  className={cn('agent-combo-item', item.id === value && 'is-selected')}
                  onClick={() => pick(item.id)}
                >
                  <div className='flex min-w-0 flex-1 flex-col gap-0.5 text-left'>
                    <div className='flex items-center gap-1.5'>
                      <span className='truncate text-[12.5px] font-medium text-foreground'>{item.name}</span>
                      {item.memberCount != null ? (
                        <span className='shrink-0 text-[11px] text-muted-foreground'>
                          · {t('{n} 个模型', { n: item.memberCount })}
                        </span>
                      ) : null}
                    </div>
                    {item.groupSummary ? (
                      <span className='truncate text-[11px] text-muted-foreground'>{item.groupSummary}</span>
                    ) : null}
                  </div>
                  <code className='hidden max-w-[34%] shrink-0 truncate text-[11px] text-muted-foreground sm:inline'>{item.id}</code>
                  {item.id === value ? <span className='shrink-0 text-primary-fg'><AgentIcon name='check' size={13} /></span> : null}
                </button>
              ))
            ) : (
              <Empty className='py-6'>{search ? t('没有匹配的路由组') : t('暂无可用路由组')}</Empty>
            )
          ) : matches.length > 0 ? (
            matches.map(item => (
              <button
                key={item.id}
                type='button'
                className={cn('agent-combo-item', item.id === value && 'is-selected')}
                onClick={() => pick(item.id)}
              >
                <span className='min-w-0 flex-1 truncate text-left text-[12.5px] text-foreground'>{item.name}</span>
                {item.name !== item.id ? (
                  <code className='max-w-[44%] shrink-0 truncate text-[11px] text-muted-foreground'>{item.id}</code>
                ) : null}
                {item.id === value ? <span className='shrink-0 text-primary-fg'><AgentIcon name='check' size={13} /></span> : null}
              </button>
            ))
          ) : (
            <Empty className='py-6'>{t('没有匹配的模型')}</Empty>
          )}
        </div>
      </PopoverContent>
    </Popover>
  )
}

/**
 * 场景 A：Claude Code 专属四档坑位映射工作台
 */
export function ClaudeMappingWorkspace({
  client,
  draft,
  catalog,
  disabled,
  native,
  defaultTier,
  onDefaultTierChange,
  update,
}: {
  client: AgentClient
  draft: AgentProfile
  catalog: CatalogModel[]
  disabled: boolean
  native: boolean
  defaultTier: ClaudeTierKey
  onDefaultTierChange: (tier: ClaudeTierKey) => void
  update: (patch: Partial<AgentProfile>) => void
}) {
  const [subTab, setSubTab] = React.useState<'slots' | 'menu'>('slots')
  const [quickFillId, setQuickFillId] = React.useState('')
  const isGateway = draft.kind === 'gateway' && catalog.length > 0

  const slots: Array<{ key: ClaudeTierKey; name: string; badge: string; desc: string; tone: string }> = [
    { key: 'opus', name: 'Opus', badge: t('深度推理'), desc: t('架构设计、复杂排障与高难度编码'), tone: 'is-opus' },
    { key: 'sonnet', name: 'Sonnet', badge: t('主力开发'), desc: t('日常对话、代码编写与绝大多数任务'), tone: 'is-sonnet' },
    { key: 'haiku', name: 'Haiku', badge: t('轻量快速'), desc: t('代码补全、摘要与低延迟辅助请求'), tone: 'is-haiku' },
    { key: 'subagent', name: t('子 Agent'), badge: t('子任务'), desc: t('主代理拆分出的并行调研与子任务'), tone: 'is-sub' },
  ]

  const setTierValue = (key: ClaudeTierKey, val: string) => {
    const nextTiers = { ...draft.claudeModels, [key]: val }
    const nextModel = defaultTier === key
      ? (val || nextTiers.sonnet || nextTiers.opus || nextTiers.haiku || nextTiers.subagent || '')
      : (draft.model || val)
    update({ claudeModels: nextTiers, model: nextModel, effort: '' })
  }

  const setDefaultSlot = (key: ClaudeTierKey) => {
    onDefaultTierChange(key)
    const target = draft.claudeModels[key]
    if (target) update({ model: target, effort: '' })
  }

  const applyAllTiers = (targetId: string) => {
    setQuickFillId(targetId)
    if (!targetId) return
    update({
      claudeModels: { opus: targetId, sonnet: targetId, haiku: targetId, subagent: targetId },
      model: targetId,
      effort: '',
    })
  }

  const readyMenuCount = draft.models.filter(m => m.id).length

  return (
    <div className='agent-claude-workspace'>
      <div className='agent-ws-head'>
        <SegmentedControl<'slots' | 'menu'>
          value={subTab}
          aria-label={t('Claude Code 配置视图')}
          options={[
            { value: 'slots', label: t('四档模型映射'), count: 4 },
            { value: 'menu', label: t('/model 菜单（可选）'), count: readyMenuCount },
          ]}
          onValueChange={setSubTab}
        />

        {subTab === 'slots' && isGateway ? (
          <div className='ml-auto flex items-center gap-2'>
            <span className='hidden text-xs text-muted-foreground sm:inline'>{t('一键四档统一指向')}</span>
            <div className='w-[228px]'>
              <TargetModelCombobox
                value={quickFillId}
                catalog={catalog}
                disabled={disabled}
                placeholder={t('批量套用到全部 4 档…')}
                onChange={applyAllTiers}
              />
            </div>
          </div>
        ) : null}
      </div>

      {subTab === 'slots' ? (
        <div className='agent-claude-slots'>
          {slots.map(slot => {
            const val = draft.claudeModels[slot.key]
            const isDefault = defaultTier === slot.key
            return (
              <div key={slot.key} className={cn('agent-slot-row', isDefault && 'is-default')}>
                <div className='agent-slot-source'>
                  <span className={cn('agent-slot-pill', slot.tone)}>{slot.name}</span>
                  <div className='min-w-0'>
                    <div className='flex items-center gap-1.5'>
                      <span className='text-[13px] font-semibold text-foreground'>{slot.badge}</span>
                      {isDefault ? <Badge variant='brand' shape='tag'>{t('默认启动')}</Badge> : null}
                    </div>
                    <p className='m-0 mt-0.5 truncate text-[11.5px] text-muted-foreground'>{slot.desc}</p>
                  </div>
                </div>

                <span className='agent-slot-arrow' aria-hidden='true'>
                  <svg width='20' height='14' viewBox='0 0 20 14' fill='none'>
                    <path d='M1 7h16m0 0-4.5-4.5M17 7l-4.5 4.5' stroke='currentColor' strokeWidth='1.6' strokeLinecap='round' strokeLinejoin='round' />
                  </svg>
                </span>

                <div className='agent-slot-target'>
                  {isGateway ? (
                    <TargetModelCombobox
                      value={val}
                      catalog={catalog}
                      disabled={disabled}
                      allowEmpty
                      emptyLabel={t('不写入')}
                      placeholder={t('选择要映射的路由组或模型')}
                      onChange={next => setTierValue(slot.key, next)}
                    />
                  ) : (
                    <Input
                      value={val}
                      disabled={disabled}
                      autoComplete='off'
                      spellCheck={false}
                      placeholder={t('填写 {name} 对应的模型 ID', { name: slot.name })}
                      className='font-mono text-[12.5px]'
                      onChange={e => setTierValue(slot.key, e.currentTarget.value)}
                    />
                  )}
                </div>

                <div className='agent-slot-action'>
                  {isDefault ? (
                    <span className='inline-flex items-center gap-1 text-[11.5px] font-medium text-primary-fg'>
                      <AgentIcon name='check' size={12} />
                      {t('默认')}
                    </span>
                  ) : (
                    <Button
                      type='button'
                      variant='ghost'
                      size='xs'
                      disabled={disabled}
                      className='text-subtle'
                      onClick={() => setDefaultSlot(slot.key)}
                    >
                      {t('设为默认')}
                    </Button>
                  )}
                </div>
              </div>
            )
          })}
        </div>
      ) : (
        <SplitModelsWorkspace
          client={client}
          draft={draft}
          catalog={catalog}
          disabled={disabled}
          native={native}
          update={update}
        />
      )}
    </div>
  )
}
