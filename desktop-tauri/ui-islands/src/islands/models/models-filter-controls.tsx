import * as React from 'react'
import {
  Badge,
  Button,
  Field,
  FieldLabel,
  Popover,
  PopoverContent,
  PopoverHeader,
  PopoverTitle,
  PopoverTrigger,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@ui'
import { t } from '../../i18n'
import type { ManageModel } from '../models-custom-source'
import {
  accept,
  bindingsOf,
  errorMessage,
  same,
  toast,
  writeBinding,
} from '../models-panel-state'

export const VISIBILITY_FILTER_OPTIONS = [
  { value: 'visible', label: () => t('仅可见（默认）') },
  { value: 'all', label: () => t('全部模型') },
  { value: 'hidden', label: () => t('仅官方不展示') },
  { value: 'undeclared', label: () => t('未声明可见性') },
] as const

export const SOURCE_FILTER_OPTIONS = [
  { value: 'all', label: () => t('全部来源') },
  { value: 'remote', label: () => t('远程目录') },
  { value: 'builtin', label: () => t('内置静态表') },
  { value: 'manual', label: () => t('手动 / 自定义登记') },
] as const

export const SORT_FILTER_OPTIONS = [
  { value: 'default', label: () => t('默认（已启用优先）') },
  { value: 'rate-asc', label: () => t('倍率从低到高') },
  { value: 'rate-desc', label: () => t('倍率从高到低') },
  { value: 'price-asc', label: () => t('输入单价从低到高') },
  { value: 'price-desc', label: () => t('输入单价从高到低') },
] as const

function FilterIcon() {
  return (
    <svg width='13' height='13' viewBox='0 0 16 16' fill='none' stroke='currentColor' strokeWidth='1.6' strokeLinecap='round' aria-hidden='true'>
      <path d='M2.5 4h11M4.5 8h7M6.5 12h3' />
    </svg>
  )
}

export function ModelsFilterPopover({
  custom,
  visibility,
  source,
  sort,
  onVisibilityChange,
  onSourceChange,
  onSortChange,
  onReset,
}: {
  custom: boolean
  visibility: string
  source: string
  sort: string
  onVisibilityChange: (next: string) => void
  onSourceChange: (next: string) => void
  onSortChange: (next: string) => void
  onReset: () => void
}) {
  const activeCount =
    (!custom && visibility !== 'visible' ? 1 : 0)
    + (!custom && source !== 'all' ? 1 : 0)
    + (sort !== 'default' ? 1 : 0)

  const visibilityLabel = VISIBILITY_FILTER_OPTIONS.find(o => o.value === visibility)?.label() || t('仅可见（默认）')
  const sourceLabel = SOURCE_FILTER_OPTIONS.find(o => o.value === source)?.label() || t('全部来源')
  const sortLabel = SORT_FILTER_OPTIONS.find(o => o.value === sort)?.label() || t('默认（已启用优先）')
  const sortOptions = custom
    ? SORT_FILTER_OPTIONS.filter(o => o.value !== 'rate-asc' && o.value !== 'rate-desc')
    : SORT_FILTER_OPTIONS

  return (
    <Popover>
      <PopoverTrigger
        render={
          <Button variant='outline' size='sm' data-active={activeCount > 0 ? 'true' : undefined}>
            <FilterIcon />
            <span>{t('筛选与排序')}</span>
            {activeCount > 0 ? <Badge variant='brand' shape='tag' className='ml-0.5 px-1.5'>{activeCount}</Badge> : null}
          </Button>
        }
      />
      <PopoverContent align='start' className='w-[292px]'>
        <PopoverHeader>
          <PopoverTitle className='flex-1'>{t('筛选与排序')}</PopoverTitle>
          {activeCount > 0 ? (
            <Button variant='ghost' size='xs' onClick={onReset}>{t('恢复默认')}</Button>
          ) : null}
        </PopoverHeader>
        <div className='flex flex-col gap-3 p-3'>
          {!custom ? (
            <Field>
              <FieldLabel>{t('官方可见性')}</FieldLabel>
              <Select value={visibility} onValueChange={v => onVisibilityChange(String(v))}>
                <SelectTrigger className='w-full'>
                  <SelectValue>{visibilityLabel}</SelectValue>
                </SelectTrigger>
                <SelectContent>
                  {VISIBILITY_FILTER_OPTIONS.map(o => (
                    <SelectItem key={o.value} value={o.value}>{o.label()}</SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </Field>
          ) : null}
          {!custom ? (
            <Field>
              <FieldLabel>{t('模型来源')}</FieldLabel>
              <Select value={source} onValueChange={v => onSourceChange(String(v))}>
                <SelectTrigger className='w-full'>
                  <SelectValue>{sourceLabel}</SelectValue>
                </SelectTrigger>
                <SelectContent>
                  {SOURCE_FILTER_OPTIONS.map(o => (
                    <SelectItem key={o.value} value={o.value}>{o.label()}</SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </Field>
          ) : null}
          <Field>
            <FieldLabel>{t('列表排序')}</FieldLabel>
            <Select value={sort} onValueChange={v => onSortChange(String(v))}>
              <SelectTrigger className='w-full'>
                <SelectValue>{sortLabel}</SelectValue>
              </SelectTrigger>
              <SelectContent>
                {sortOptions.map(o => (
                  <SelectItem key={o.value} value={o.value}>{o.label()}</SelectItem>
                ))}
              </SelectContent>
            </Select>
          </Field>
        </div>
      </PopoverContent>
    </Popover>
  )
}

export function ModelsFilterChips({
  custom,
  visibility,
  source,
  sort,
  onClearVisibility,
  onClearSource,
  onClearSort,
}: {
  custom: boolean
  visibility: string
  source: string
  sort: string
  onClearVisibility: () => void
  onClearSource: () => void
  onClearSort: () => void
}) {
  const chips: Array<{ key: string; label: string; clear: () => void }> = []
  if (!custom && visibility !== 'visible') {
    const label = visibility === 'all' ? t('含不展示') : visibility === 'hidden' ? t('仅不展示') : t('未声明')
    chips.push({ key: 'vis', label: t('可见性：{value}', { value: label }), clear: onClearVisibility })
  }
  if (!custom && source !== 'all') {
    const label = source === 'remote' ? t('远程') : source === 'builtin' ? t('内置') : t('手动')
    chips.push({ key: 'src', label: t('来源：{value}', { value: label }), clear: onClearSource })
  }
  if (sort !== 'default') {
    const label = sort === 'rate-asc'
      ? t('倍率升序')
      : sort === 'rate-desc'
        ? t('倍率降序')
        : sort === 'price-asc'
          ? t('单价升序')
          : t('单价降序')
    chips.push({ key: 'sort', label: t('排序：{value}', { value: label }), clear: onClearSort })
  }
  if (!chips.length) return null
  return (
    <div className='mm-active-chips'>
      {chips.map(chip => (
        <span key={chip.key} className='mm-filter-chip'>
          <span>{chip.label}</span>
          <button type='button' aria-label={t('清除条件')} onClick={chip.clear}>×</button>
        </span>
      ))}
    </div>
  )
}

export function ModelsBatchBar({
  models,
  onOpenAdvanced,
  onClear,
}: {
  models: ManageModel[]
  onOpenAdvanced: () => void
  onClear: () => void
}) {
  const [busy, setBusy] = React.useState(false)
  if (!models.length) return null

  async function batchToggle(next: boolean): Promise<void> {
    if (busy) return
    if (next) {
      const seen = new Map<string, string>()
      for (const model of models) {
        for (const binding of bindingsOf(model)) {
          const key = JSON.stringify([(model.provider || '').toLowerCase(), binding.alias.toLowerCase()])
          const prev = seen.get(key)
          if (prev && !same(prev, model.id)) {
            toast(t('所选模型含同一提供商的同名绑定「{id}」，请分别启用，避免相互关闭', { id: binding.alias }), 'err')
            return
          }
          seen.set(key, model.id)
        }
      }
    }
    setBusy(true)
    let ok = 0
    let failed = 0
    let firstError = ''
    for (const model of models) {
      const provider = model.provider || ''
      try {
        for (const binding of bindingsOf(model)) {
          accept(await writeBinding(provider, binding.alias, model.id, { enabled: next }))
        }
        ok += 1
      } catch (error) {
        failed += 1
        if (!firstError) firstError = errorMessage(error)
      }
    }
    setBusy(false)
    const actionLabel = next ? t('启用') : t('禁用')
    if (failed === 0) {
      toast(t('✅ 批量{action}完成：共 {n} 个模型', { action: actionLabel, n: ok }))
      onClear()
    } else {
      toast(t('批量{action}完成：成功 {ok} 个，失败 {failed} 个（{error}）', {
        action: actionLabel, ok, failed, error: firstError,
      }), 'err')
    }
  }

  return (
    <div className='mm-batch-bar' role='region' aria-label={t('批量操作栏')}>
      <span className='mm-batch-count'>{t('已选 {n} 个模型', { n: models.length })}</span>
      <Button variant='outline' size='xs' disabled={busy} onClick={() => void batchToggle(true)}>
        {t('批量启用')}
      </Button>
      <Button variant='outline' size='xs' disabled={busy} onClick={() => void batchToggle(false)}>
        {t('批量禁用')}
      </Button>
      <Button id='btn-batch-models' variant='outline' size='xs' disabled={busy} onClick={onOpenAdvanced}>
        {t('更多操作…')}
      </Button>
      <Button variant='ghost' size='xs' className='ml-auto' disabled={busy} onClick={onClear}>
        {t('取消勾选')}
      </Button>
    </div>
  )
}
