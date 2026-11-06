/**
 * 视觉模型选择器与「模型单独配置」编辑弹窗。
 *
 * 与旧版的区别（旧的 VisionTargetPicker：来源 + 搜索框 + 模型下拉 + 思考等级）：
 *   · 模型下拉并入了一层 —— 第一项是「沿用工具服务的视觉分析模型」，其余是具体模型，
 *     不再要求用户先理解「来源」这层概念；
 *   · 搜索框去掉（Base UI 的 Select 本来就能输入跳转），思考等级抽成独立的
 *     VisionReasoningSelect，放进面板的「更多设置」。
 *
 * 配置语义没变：选到具体模型 = 旧的「单独指定」+ 目标模型。
 */

import * as React from 'react'
import {
  Alert, Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle,
  Select, SelectContent, SelectItem, SelectTrigger, SelectValue, cn,
} from '@ui'
import type { Translate } from './assistant-read-types'
import { FooterNote, Group, Row } from './settings-vision-rows'
import { visionTargetKey, type VisionModel, type VisionOverride } from './settings-vision-types'

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

  return <Select value={value} disabled={disabled} onValueChange={next => onValueChange(String(next ?? ''))}>
    <SelectTrigger className={cn('w-[260px]', className)} aria-label={ariaLabel || t('视觉模型')}>
      <SelectValue>{label}</SelectValue>
    </SelectTrigger>
    <SelectContent>
      {includeShared ? <SelectItem value='shared'>{t('沿用工具服务的视觉分析模型')}</SelectItem> : null}
      {value && value !== 'shared' && !selected ? <SelectItem value={value} disabled>{label}</SelectItem> : null}
      {eligible.map(model => <SelectItem key={visionTargetKey(model)} value={visionTargetKey(model)}>
        {model.providerLabel} · {model.name || model.model}
      </SelectItem>)}
    </SelectContent>
  </Select>
}

/** 思考等级：只影响识读那一步，默认跟随模型自身配置 */
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

/** 「模型单独配置」的编辑弹窗：保存时挡住重复的提供商 + 模型 */
export function VisionOverrideEditor({ models, initial, onSave, onClose, t }: {
  models: VisionModel[]; initial?: VisionOverride; onSave(value: VisionOverride): string | void; onClose(): void; t: Translate
}) {
  const [value, setValue] = React.useState(initial ? visionTargetKey(initial) : '')
  const [mode, setMode] = React.useState<VisionOverride['mode']>(initial?.mode || 'assist')
  const [error, setError] = React.useState('')
  const selected = models.find(model => visionTargetKey(model) === value)
  const choices = [
    { value: 'assist', label: t('辅助识图 · 原模型回答') },
    { value: 'reroute', label: t('切换视觉模型 · 视觉模型回答') },
    { value: 'off', label: t('不做图片兼容') },
  ]

  return <Dialog open onOpenChange={open => { if (!open) onClose() }}><DialogContent>
    <DialogHeader><DialogTitle>{initial ? t('修改模型单独配置') : t('添加模型单独配置')}</DialogTitle></DialogHeader>
    <DialogBody className='gap-3'>
      <p className='m-0 text-[12.5px] leading-relaxed text-muted-foreground'>{t('只覆盖指定提供商的指定模型，其余跟随全局。')}</p>
      <Group>
        <Row first label={t('提供商与模型')} control={
          <VisionModelSelect models={models} value={value} onValueChange={setValue} t={t} anyModel />
        } />
        <Row label={t('处理方式')} description={t('沿用对应方式的视觉模型配置，不额外维护一份模型列表。')} control={
          <Select value={mode} onValueChange={next => setMode(next as VisionOverride['mode'])}>
            <SelectTrigger className='w-[260px]' aria-label={t('处理方式')}>
              <SelectValue>{choices.find(choice => choice.value === mode)?.label}</SelectValue>
            </SelectTrigger>
            <SelectContent>{choices.map(choice => <SelectItem key={choice.value} value={choice.value}>{choice.label}</SelectItem>)}</SelectContent>
          </Select>
        } />
      </Group>
      <FooterNote>{selected
        ? t('这条配置将对 {model} 生效。', { model: `${selected.providerLabel} · ${selected.name || selected.model}` })
        : t('选好模型后这条配置才会生效。')}</FooterNote>
      {error ? <Alert variant='destructive'>{error}</Alert> : null}
    </DialogBody>
    <DialogFooter>
      <Button variant='outline' onClick={onClose}>{t('取消')}</Button>
      <Button disabled={!selected} onClick={() => {
        if (selected) setError(onSave({ provider: selected.provider, model: selected.model, mode }) || '')
      }}>{initial ? t('保存修改') : t('添加配置')}</Button>
    </DialogFooter>
  </DialogContent></Dialog>
}
