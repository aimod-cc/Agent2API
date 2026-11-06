import * as React from 'react'
import { Badge, Button, Checkbox, Empty, Input, InputGroup, InputGroupAddon, InputGroupInput, Popover, PopoverContent, PopoverTrigger } from '@ui'
import { t } from '../i18n'
import type { CatalogModel } from './agent-catalog'

export const AGENT_MODEL_LIMIT = 200

/**
 * 候选模型多选（紧凑列表）。
 *
 * ── 结构怎么排 ──────────────────────────────────────────────
 * 每项**一行**：勾选框 + 展示名（左，可截断）+ 模型 ID（右，等宽灰）；搜索与「已选计数 /
 * 全选 / 清空」压在同一行。列表自己滚动（`.agent-model-picker-list` 有 max-height）。
 *
 * ── 与上一版的差别（只在这些地方）────────────────────────────
 * 原来是「展示名一行 + 模型 ID 一行」的两行卡片：行高约 46px，一屏只看得到 6 项，
 * 搜索框、计数、两个批量动作还各占一行 —— 控件比内容更占地方。现在每项一行、
 * 一屏 15 项上下。行为一概没动：点整行切换、`limit` 上限、受控选择集、
 * 搜索只过滤不改变选择、`locked` 的项不可取消。
 *
 * 样式全在 `ui/css/page-agent.css` 的 `.agent-model-picker*` 里，这里只出结构。
 */
export function AgentModelPicker({ catalog, selected, onChange, disabled, locked = [], limit = AGENT_MODEL_LIMIT }: {
  catalog: CatalogModel[]; selected: string[]; onChange: (ids: string[]) => void; disabled?: boolean; locked?: string[]; limit?: number
}) {
  const [search, setSearch] = React.useState('')
  const picked = new Set(selected)
  const protectedIds = new Set(locked)
  const query = search.trim().toLowerCase()
  const matches = catalog.filter(model => `${model.id} ${model.name}`.toLowerCase().includes(query))
  const remaining = Math.max(0, limit - picked.size)
  const available = matches.filter(model => !picked.has(model.id) && !protectedIds.has(model.id))
  const selectVisible = () => onChange([...selected, ...available.slice(0, remaining).map(model => model.id)])
  const clear = () => onChange(selected.filter(id => protectedIds.has(id)))
  return <div className='agent-model-picker' role='group' aria-label={t('选择模型')}>
    <div className='agent-model-picker-bar'>
      <InputGroup className='agent-model-picker-search'>
        <InputGroupInput type='search' autoComplete='off' value={search} disabled={disabled} aria-label={t('搜索模型')}
          placeholder={t('搜索模型 ID / 展示名…')} onChange={event => setSearch(event.currentTarget.value)} />
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
          <span className='agent-model-picker-name'>{model.name || model.id}</span>
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
  const [open, setOpen] = React.useState(false)
  const [search, setSearch] = React.useState('')
  const text = options.find(option => option.value === value)?.label || value || t('请选择模型')
  const query = search.trim().toLowerCase()
  const matches = options.filter(option => `${option.value} ${option.label}`.toLowerCase().includes(query))
  return <Popover open={open && !disabled} onOpenChange={open => { setOpen(open); if (open) setSearch('') }}>
    <PopoverTrigger render={<Button variant='outline' disabled={disabled} />} className='w-full min-w-0 justify-between font-normal'
      aria-label={label} title={text}>
      <span className='min-w-0 truncate'>{text}</span><span aria-hidden='true'>⌄</span>
    </PopoverTrigger>
    <PopoverContent className='agent-model-select-popover' sideOffset={4}>
      <Input type='search' autoFocus autoComplete='off' disabled={disabled} value={search} placeholder={t('搜索模型')}
        aria-label={t('搜索模型')} onChange={event => setSearch(event.currentTarget.value)} />
      <div className='agent-model-select-options'>
        {matches.map(option => <Button key={option.value} variant='ghost' className='h-auto w-full justify-start whitespace-normal text-left font-normal'
          aria-pressed={value === option.value} disabled={disabled} onClick={() => { onChange(option.value); setOpen(false) }}>
          <span className='min-w-0 flex-1 break-all'>{option.label}</span>
          {value === option.value && <Badge variant='brand' shape='tag'>{t('已选择')}</Badge>}
        </Button>)}
        {!matches.length && <Empty>{t('没有匹配的模型')}</Empty>}
      </div>
    </PopoverContent>
  </Popover>
}
