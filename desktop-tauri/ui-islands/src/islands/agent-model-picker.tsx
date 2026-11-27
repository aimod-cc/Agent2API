import * as React from 'react'
import { Button, Checkbox, Empty, InputGroup, InputGroupAddon, InputGroupInput, SegmentedControl } from '@ui'
import { t } from '../i18n'
import { SearchableSelect } from '../../../ui-shared/searchable-select'
import type { CatalogModel } from './agent-catalog'

export const AGENT_MODEL_LIMIT = 200

type PickerTab = 'group' | 'model'

/**
 * 候选模型多选（供一键接入弹窗使用）。
 * 当目录存在路由组时提供「路由组 | 模型」两档切换，避免两类条目混排。
 */
export function AgentModelPicker({ catalog, selected, onChange, disabled, locked = [], limit = AGENT_MODEL_LIMIT }: {
  catalog: CatalogModel[]; selected: string[]; onChange: (ids: string[]) => void; disabled?: boolean; locked?: string[]; limit?: number
}) {
  const groupCount = React.useMemo(() => catalog.filter(model => model.routingGroup).length, [catalog])
  const modelCount = catalog.length - groupCount
  const [tab, setTab] = React.useState<PickerTab>(groupCount > 0 ? 'group' : 'model')
  const [search, setSearch] = React.useState('')

  React.useEffect(() => {
    if (groupCount === 0 && tab === 'group') setTab('model')
  }, [groupCount, tab])

  const picked = new Set(selected)
  const protectedIds = new Set(locked)
  const query = search.trim().toLowerCase()
  const matches = catalog.filter(model => {
    if (groupCount > 0 && (tab === 'group' ? !model.routingGroup : Boolean(model.routingGroup))) return false
    return `${model.id} ${model.name} ${model.groupSummary || ''}`.toLowerCase().includes(query)
  })
  const remaining = Math.max(0, limit - picked.size)
  const available = matches.filter(model => !picked.has(model.id) && !protectedIds.has(model.id))
  const selectVisible = () => onChange([...selected, ...available.slice(0, remaining).map(model => model.id)])
  const clear = () => onChange(selected.filter(id => protectedIds.has(id)))

  return <div className='agent-model-picker' role='group' aria-label={t('选择模型')}>
    {groupCount > 0 && (
      <div className='flex items-center gap-2'>
        <SegmentedControl<PickerTab>
          value={tab}
          aria-label={t('选择类型')}
          options={[
            { value: 'group', label: t('路由组'), count: groupCount },
            { value: 'model', label: t('模型'), count: modelCount },
          ]}
          onValueChange={next => { setTab(next); setSearch('') }}
        />
      </div>
    )}
    <div className='agent-model-picker-bar'>
      <InputGroup className='agent-model-picker-search'>
        <InputGroupInput type='search' autoComplete='off' value={search} disabled={disabled} aria-label={t('搜索模型')}
          placeholder={groupCount > 0 && tab === 'group' ? t('搜索路由组名称或 ID…') : t('搜索模型 ID / 展示名…')}
          onChange={event => setSearch(event.currentTarget.value)} />
        <InputGroupAddon aria-hidden='true'>⌕</InputGroupAddon>
      </InputGroup>
      <span className='agent-model-picker-count'>{t('已选择 {count} / {total} 个', { count: picked.size, total: catalog.length })}</span>
      <Button variant='link' size='xs' disabled={disabled || !remaining || !available.length} onClick={selectVisible}>{t('全选搜索结果')}</Button>
      <Button variant='link' size='xs' disabled={disabled || !selected.some(id => !protectedIds.has(id))} onClick={clear}>{t('清空选择')}</Button>
    </div>
    <div className='agent-model-picker-list'>
      {matches.map(model => {
        const checked = picked.has(model.id)
        const title = `${model.name || model.id}${model.name && model.name !== model.id ? ` · ${model.id}` : ''}`
        return <label key={model.id} className='agent-model-picker-row' data-checked={checked ? '' : undefined} title={title}>
          <Checkbox checked={checked} disabled={disabled || protectedIds.has(model.id) || !checked && !remaining}
            aria-label={model.label} onCheckedChange={on => onChange(on === true ? [...selected, model.id] : selected.filter(id => id !== model.id))} />
          <span className='agent-model-picker-name'>
            {model.name || model.id}
            {model.routingGroup && model.memberCount != null ? <span className='ml-1.5 text-[11px] text-subtle'>· {t('{n} 个模型', { n: model.memberCount })}</span> : null}
          </span>
          {model.name && model.name !== model.id && <code className='agent-model-picker-id'>{model.id}</code>}
        </label>
      })}
      {!matches.length && <Empty>{t(query ? '没有匹配的模型' : '没有可添加的模型')}</Empty>}
    </div>
    {!remaining && <p className='text-xs text-subtle'>{t('最多选择 {n} 个模型', { n: AGENT_MODEL_LIMIT })}</p>}
  </div>
}

export function SearchableModelSelect({ label, value, options, onChange, disabled }: {
  label: string; value: string; options: Array<{ value: string; label: string }>; onChange: (value: string) => void; disabled?: boolean
}) {
  const text = options.find(option => option.value === value)?.label || value || t('请选择模型')
  return <SearchableSelect value={value} options={options} onValueChange={onChange} disabled={disabled}
    className='w-full' aria-label={label} displayValue={text}
    searchPlaceholder={t('搜索模型')} emptyHint={t('没有匹配的模型')} />
}
