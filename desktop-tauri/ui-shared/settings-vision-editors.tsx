/**
 * 图片兼容用到的两个选择器。
 *
 * 与旧版的区别（旧的 VisionTargetPicker：来源 + 搜索框 + 模型下拉 + 思考等级）：
 *   · 模型下拉并入了一层 —— 第一项是「沿用工具服务的视觉分析模型」，其余是具体模型，
 *     不再要求用户先理解「来源」这层概念；
 *   · 思考等级抽成独立的 VisionReasoningSelect，放在面板的「更多设置」里。
 *
 * 配置语义没变：选到具体模型 = 旧的「单独指定」+ 目标模型。
 */

import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue, cn } from '@ui'
import type { Translate } from './assistant-read-types'
import { visionTargetKey, type VisionModel } from './settings-vision-types'
import { SearchableSelect } from './searchable-select'

/**
 * 并入一层的视觉模型选择器。
 *
 * `includeShared` 时第一项是「沿用工具服务的视觉分析模型」（值固定为 `'shared'`）；
 * 选到具体模型时值是该模型的 `visionTargetKey`。`sharedUnset` 让第一项带上
 * 「（未配置）」，用户不用点开就知道工具服务那边还没配。
 */
export function VisionModelSelect({ models, value, onValueChange, t, includeShared = false, anyModel = false, sharedUnset = false, disabled, className, ariaLabel }: {
  models: VisionModel[]
  value: string
  onValueChange: (next: string) => void
  t: Translate
  includeShared?: boolean
  anyModel?: boolean
  sharedUnset?: boolean
  disabled?: boolean
  className?: string
  ariaLabel?: string
}) {
  const eligible = models.filter(model => anyModel || model.capabilities.supportsImages === true)
  const selected = eligible.find(model => visionTargetKey(model) === value)
  const label = value === 'shared'
    ? (sharedUnset ? t('沿用工具服务的视觉分析模型（未配置）') : t('沿用工具服务的视觉分析模型'))
    : selected ? `${selected.providerLabel} · ${selected.name || selected.model}`
      : value ? t('当前不可用') : t('请选择模型')

  return <SearchableSelect value={value} disabled={disabled} onValueChange={onValueChange}
    className={cn('w-[260px]', className)} aria-label={ariaLabel || t('视觉模型')}
    displayValue={label} searchPlaceholder={t('搜索模型')}
    options={[
      ...(includeShared ? [{ value: 'shared', label: t('沿用工具服务的视觉分析模型') }] : []),
      ...(value && value !== 'shared' && !selected ? [{ value, label, disabled: true }] : []),
      ...eligible.map(model => ({ value: visionTargetKey(model),
        label: `${model.providerLabel} · ${model.name || model.model}`, searchText: model.model,
      })),
    ]} />
}

/** 思考等级：只影响读图那一步，默认跟随模型自身配置 */
export function VisionReasoningSelect({ value, onValueChange, t, disabled, className }: {
  value: string
  onValueChange: (next: string) => void
  t: Translate
  disabled?: boolean
  className?: string
}) {
  const levels = [
    { value: '', label: t('跟随模型配置') },
    { value: 'minimal', label: 'Minimal' }, { value: 'low', label: 'Low' },
    { value: 'medium', label: 'Medium' }, { value: 'high', label: 'High' }, { value: 'xhigh', label: 'XHigh' },
  ]
  return <Select value={value} disabled={disabled} onValueChange={next => onValueChange(String(next ?? ''))}>
    <SelectTrigger className={cn('w-[180px]', className)} aria-label={t('思考等级')}>
      <SelectValue>{levels.find(level => level.value === value)?.label || value}</SelectValue>
    </SelectTrigger>
    <SelectContent>{levels.map(level => <SelectItem key={level.value} value={level.value}>{level.label}</SelectItem>)}</SelectContent>
  </Select>
}
