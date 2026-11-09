/**
 * 图片兼容的两块设置：辅助识图 / 切换视觉模型。
 *
 * 两块都是固定的一块设置，没有开关 —— 名单为空就等于没用这种方式。
 * 每块只回答两个问题：哪些模型走它、用哪只视觉模型；其余细节收进面板底部的「更多设置」。
 */

import * as React from 'react'
import { Button, MultiSelect } from '@ui'
import type { Translate } from './assistant-read-types'
import { Group, ListRow, Row, SwitchRow, Tip } from './settings-vision-rows'
import { VisionModelSelect } from './settings-vision-editors'
import {
  visionTargetKey, type VisionAssist, type VisionModel, type VisionModelRef, type VisionReroute, type VisionTarget,
} from './settings-vision-types'

/** 名单候选：看不了图（含能力未知）的模型才需要被接管，能看图的模型本来就原样放行 */
function listChoices(models: VisionModel[], t: Translate) {
  return models.filter(model => model.capabilities.supportsImages !== true)
    .map(model => ({ value: visionTargetKey(model), label: `${model.providerLabel} · ${model.name || model.model}` }))
}

export function visionModelLabel(models: VisionModel[], target: Pick<VisionTarget, 'provider' | 'model'>, t: Translate): string {
  const item = models.find(model => visionTargetKey(model) === visionTargetKey(target))
  return item ? `${item.providerLabel} · ${item.name || item.model}` : `${target.provider} · ${target.model}`
}

function WayHead({ title, tip }: { title: string; tip: string }) {
  return <div className='flex items-center gap-1.5 pb-2'>
    <b className='text-[13px] font-semibold text-foreground'>{title}</b>
    <Tip text={tip} label={title} />
  </div>
}

function ModelListRow({ way, label, tip, value, options, disabled, onChange, t }: {
  way: string
  label: string
  tip: string
  value: VisionModelRef[]
  options: { value: string; label: string }[]
  disabled: boolean
  onChange(next: VisionModelRef[]): void
  t: Translate
}) {
  const resolve = (key: string): VisionModelRef | null => {
    const parsed: unknown = JSON.parse(key)
    return Array.isArray(parsed) && parsed.length === 2 && parsed.every(item => typeof item === 'string')
      ? { provider: parsed[0] as string, model: parsed[1] as string }
      : null
  }
  return <Row first label={label} tip={tip} className='items-center max-[767px]:flex-col max-[767px]:items-stretch max-[767px]:gap-2' control={
    <MultiSelect className='w-[min(320px,100%)]' aria-label={t('{way}的适用模型', { way })}
      value={value.map(visionTargetKey)} options={options} disabled={disabled}
      placeholder={t('选择一个或多个模型')} searchPlaceholder={t('搜索提供商或模型')}
      onValueChange={keys => onChange(keys.map(resolve).filter((item): item is VisionModelRef => item !== null))} />
  } />
}

export function VisionAssistCard({ value, models, reasoning, disabled, onChange, t }: {
  value: VisionAssist
  models: VisionModel[]
  /** 面板「更多设置」里统一的思考等级：新选的目标跟着它走，不用再单独设一次 */
  reasoning: string
  disabled: boolean
  onChange(next: Partial<VisionAssist>): void
  t: Translate
}) {
  return <section className='flex min-w-0 flex-col' aria-label={t('辅助识图')}>
    <WayHead title={t('辅助识图')} tip={t('先把图片交给视觉模型读一遍，再让原模型结合读到的内容回答。')} />
    <Group>
      <ModelListRow way={t('辅助识图')} label={t('哪些模型用这种方式')}
        tip={t('可跨提供商多选；只处理这些模型看不了的图片。')}
        value={value.models} options={listChoices(models, t)} disabled={disabled} t={t}
        onChange={next => onChange({ models: next })} />
      <Row label={t('用哪只模型识图')} className='items-center max-[767px]:flex-col max-[767px]:items-stretch max-[767px]:gap-2' control={
        <VisionModelSelect models={models} t={t} includeShared disabled={disabled}
          className='w-[min(320px,100%)]' ariaLabel={t('用哪只模型识图')}
          value={value.source === 'shared' ? 'shared' : (value.target ? visionTargetKey(value.target) : '')}
          onValueChange={next => {
            if (next === 'shared') { onChange({ source: 'shared' }); return }
            const item = models.find(model => visionTargetKey(model) === next)
            if (item) onChange({ source: 'custom', target: { provider: item.provider, model: item.model, reasoning } })
          }} />
      } />
      <SwitchRow label={t('复用相同图片的识读结果')} disabled={disabled}
        tip={t('同一会话、同一问题、相同权限范围内复用；追问新细节会重新识读。')}
        checked={value.reuseResults} onCheckedChange={reuseResults => onChange({ reuseResults })} />
    </Group>
  </section>
}

export function VisionRerouteCard({ value, models, reasoning, maxTargets, disabled, onChange, t }: {
  value: VisionReroute
  models: VisionModel[]
  /** 面板「更多设置」里统一的思考等级：新选的目标跟着它走，不用再单独设一次 */
  reasoning: string
  maxTargets: number
  disabled: boolean
  onChange(next: Partial<VisionReroute>): void
  t: Translate
}) {
  const [candidate, setCandidate] = React.useState('')
  const [primary, ...backups] = value.targets
  const setTargets = (targets: VisionTarget[]) =>
    onChange({ targets, fallback: targets.length ? 'designated' : 'stop' })
  const move = (index: number, direction: number) => {
    const next = [...value.targets]
    const to = index + direction
    if (to < 0 || to >= next.length) return
    ;[next[index], next[to]] = [next[to], next[index]]
    setTargets(next)
  }
  const candidateModel = models.find(model => visionTargetKey(model) === candidate)
  const candidateUsed = Boolean(candidateModel && value.targets.some(item => visionTargetKey(item) === visionTargetKey(candidateModel!)))
  const canAdd = Boolean(candidateModel) && !candidateUsed && value.targets.length < maxTargets

  return <section className='flex min-w-0 flex-col' aria-label={t('切换视觉模型')}>
    <WayHead title={t('切换视觉模型')} tip={t('把整次请求交给视觉模型，由它直接回答，不再回到原模型。')} />
    <Group>
      <ModelListRow way={t('切换视觉模型')} label={t('哪些模型用这种方式')}
        tip={t('可跨提供商多选；这些模型的请求会整体改交视觉模型。')}
        value={value.models} options={listChoices(models, t)} disabled={disabled} t={t}
        onChange={next => onChange({ models: next })} />
      <Row label={t('接管回答的模型')} className='items-center max-[767px]:flex-col max-[767px]:items-stretch max-[767px]:gap-2' control={
        <div className='flex w-[min(320px,100%)] items-center gap-1'>
          <VisionModelSelect models={models} t={t} disabled={disabled} className='min-w-0 flex-1'
            ariaLabel={t('接管回答的模型')} value={primary ? visionTargetKey(primary) : ''}
            onValueChange={next => {
              const item = models.find(model => visionTargetKey(model) === next)
              setTargets(item ? [{ provider: item.provider, model: item.model, reasoning }, ...backups] : backups)
            }} />
          {primary ? <Button variant='ghost' size='xs' disabled={disabled}
            onClick={() => setTargets(backups)}>{t('清除')}</Button> : null}
        </div>
      } />
      <SwitchRow label={t('优先使用当前路由内的视觉模型')} disabled={disabled}
        tip={t('先尝试当前路由内的视觉模型，不可用时再按顺序尝试指定模型；指定模型可跨路由池。')}
        checked={value.preferCurrentRoute} onCheckedChange={preferCurrentRoute => onChange({ preferCurrentRoute })} />
      {backups.map((target, index) => <ListRow key={visionTargetKey(target)} index={index} title={visionModelLabel(models, target, t)}
        actions={<>
          <Button variant='ghost' size='xs' disabled={disabled || index === 0} onClick={() => move(index + 1, -1)}>{t('上移')}</Button>
          <Button variant='ghost' size='xs' disabled={disabled || index === backups.length - 1} onClick={() => move(index + 1, 1)}>{t('下移')}</Button>
          <Button variant='ghost' size='xs' disabled={disabled}
            onClick={() => setTargets(value.targets.filter((_, i) => i !== index + 1))}>{t('移除')}</Button>
        </>} />)}
      <Row label={t('备选模型')} tip={t('首选不可用时再依次尝试；留空则停止并说明原因。')} className='items-center max-[767px]:flex-col max-[767px]:items-stretch max-[767px]:gap-2' control={
        <div className='flex w-[min(320px,100%)] items-center gap-1'>
          <VisionModelSelect models={models} t={t} disabled={disabled || value.targets.length >= maxTargets}
            className='min-w-0 flex-1' ariaLabel={t('备选模型')} value={candidate} onValueChange={setCandidate} />
          <Button variant='outline' size='xs' disabled={disabled || !canAdd} onClick={() => {
            if (candidateModel) { setTargets([...value.targets, { provider: candidateModel.provider, model: candidateModel.model, reasoning }]); setCandidate('') }
          }}>{t('加入备选')}</Button>
        </div>
      } />
    </Group>
  </section>
}
