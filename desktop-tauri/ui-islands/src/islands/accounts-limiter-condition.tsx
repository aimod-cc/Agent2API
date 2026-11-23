import * as React from 'react'
import {
  Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle,
  Input, MetricCard, MultiSelect, Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
  Field, FieldGroup, FieldLabel,
} from '@ui'
import { t } from '../i18n'
import { numberText, type AccountRecord } from './accounts-shared'
import { providerOf } from './accounts-domain'
import type { LimiterCondition, LimiterMetric, LimiterWindow, QuotaSource } from './accounts-limiter-types'
import { metricLabel, modelsOf, sourceIdentity, sourceLabel, sourcesOf } from './accounts-limiter-model'
import { isCreditUnit, usageUnitLabel } from '../../../ui-shared/usage-readings'

const RESET_OPTIONS: Array<{ value: LimiterWindow['reset']; label: () => string }> = [
  { value: 'fixed', label: () => t('固定周期') },
  { value: 'rolling', label: () => t('滚动窗口') },
  { value: 'daily', label: () => t('自然日') },
  { value: 'weekly', label: () => t('自然周') },
  { value: 'monthly', label: () => t('自然月') },
  { value: 'total', label: () => t('生效至到期累计（不自动回满）') },
]

type Draft = {
  condition: LimiterCondition
  threshold: string
  unit: 'unit' | 'wan' | 'yi'
  period: string
  anchor: string
  end: string
}

export function blankCondition(): LimiterCondition {
  return { join: 'or', sourceId: 'local', models: [], metric: 'tokens', comparison: 'gte',
    threshold: 500_000, window: { reset: 'daily', utcOffsetMinutes: null } }
}

function dateInput(ms: number | undefined): string {
  if (!ms || !Number.isFinite(ms)) return ''
  const date = new Date(ms)
  return new Date(ms - date.getTimezoneOffset() * 60_000).toISOString().slice(0, 16)
}

function draftOf(condition: LimiterCondition): Draft {
  const unit = condition.metric === 'tokens' && condition.threshold % 100_000_000 === 0 ? 'yi'
    : condition.metric === 'tokens' && condition.threshold % 10_000 === 0 ? 'wan' : 'unit'
  return { condition: { ...structuredClone(condition), window: condition.window || condition.source?.window,
    sourceId: condition.sourceId || (condition.metric === 'tokens' || condition.metric === 'requests' ? 'local' : 'account') }, unit,
    threshold: String(condition.threshold / (unit === 'yi' ? 100_000_000 : unit === 'wan' ? 10_000 : 1)),
    period: String(((condition.window || condition.source?.window)?.seconds || 3600) / 3600),
    anchor: dateInput((condition.window || condition.source?.window)?.startsAt),
    end: dateInput((condition.window || condition.source?.window)?.endsAt),
  }
}

function metricsFor(source: QuotaSource | undefined): LimiterMetric[] {
  if (!source || source.id === 'local' || source.localOnly) return ['tokens', 'requests']
  if (source.id === 'account') return ['remaining']
  const result: LimiterMetric[] = []
  if (!source.unlimited) {
    if (source.remaining !== null && source.remaining !== undefined) result.push('remaining')
    if (source.used !== null && source.used !== undefined) result.push('used')
    if (source.remainingPercent !== null && source.remainingPercent !== undefined || (source.total || 0) > 0) {
      result.push('remaining_percent', 'used_percent')
    }
  }
  if (source.window) result.push('tokens', 'requests')
  return result.length ? result : ['tokens', 'requests']
}

export function LimiterConditionDialog({ account, provider, value, allowBalance = true, onSave, onClose }: {
  account?: AccountRecord
  provider?: string
  value: LimiterCondition
  allowBalance?: boolean
  onSave: (condition: LimiterCondition) => void
  onClose: () => void
}) {
  const [draft, setDraft] = React.useState(() => draftOf(value))
  const [error, setError] = React.useState('')
  const condition = draft.condition
  const providerId = account ? providerOf(account) : String(provider || '')
  const sources = sourcesOf(account, providerId).filter(source => (allowBalance || source.id === 'local')
    && (providerId !== 'codearts' || source.id === 'account' || isCreditUnit(source.unit || '')))
  if (condition.source && !sources.some(source => source.id === condition.sourceId)) sources.push(condition.source)
  const source = sources.find(source => source.id === condition.sourceId)
  const aggregate = sources.find(source => source.id === 'account' && isCreditUnit(source.unit || ''))
  const plans = [...new Map(sources.filter(source => source.planId).map(source => [source.planId!, source.planName || source.planId!])).entries()]
  const plan = source?.planId || ''
  const options = sources.filter(source => source.id === 'account' || (plan ? source.planId === plan : !source.planId))
  const local = condition.metric === 'tokens' || condition.metric === 'requests'
  const convertedThreshold = Number(draft.threshold) * (draft.unit === 'yi' ? 100_000_000 : draft.unit === 'wan' ? 10_000 : 1)
  const thresholdPreview = condition.metric === 'tokens' && draft.unit !== 'unit' && draft.threshold.trim()
    && Number.isSafeInteger(convertedThreshold) && convertedThreshold > 0 ? convertedThreshold.toLocaleString('en-US') : ''
  const aggregateView = aggregate?.unlimited ? t('不限') : aggregate?.remaining == null ? t('暂无读数')
    : `${numberText(aggregate.remaining)} ${t('积分')}`
  const modelOptions = source?.models?.length
    ? source.models.map(model => ({ value: model, label: model })) : modelsOf(account, providerId)
  for (const model of condition.models) if (!modelOptions.some(option => option.value === model)) modelOptions.push({ value: model, label: model })
  const metrics = [...new Set([...metricsFor(source), condition.metric])]
    .filter(metric => providerOf(account) !== 'codearts' || metric !== 'tokens' && metric !== 'requests')
  const update = (next: Partial<LimiterCondition>) => {
    setError('')
    setDraft(current => ({ ...current, condition: { ...current.condition, ...next } }))
  }
  const selectSource = (id: string) => {
    const source = sources.find(source => source.id === id)
    if (!source) return
    const metric = source.localOnly || source.localEstimate ? 'tokens' : metricsFor(source)[0]
    const threshold = metric === 'tokens' ? source.total || 500_000 : metric.endsWith('percent') ? 5 : 1
    setDraft(draftOf({ ...condition, sourceId: id, source: sourceIdentity(source),
      models: source.models?.length === 1 ? source.models.slice() : [], metric,
      comparison: metric.startsWith('remaining') ? 'lte' : 'gte', threshold,
      window: source.window || { reset: 'daily', utcOffsetMinutes: null },
    }))
    setError('')
  }
  const save = () => {
    const raw = Number(draft.threshold)
    const threshold = condition.metric === 'tokens'
      ? Math.round(raw * (draft.unit === 'yi' ? 100_000_000 : draft.unit === 'wan' ? 10_000 : 1)) : raw
    if (!draft.threshold.trim() || !Number.isFinite(threshold) || threshold < 0) {
      setError(t('限制阈值必须是非负有限数字')); return
    }
    if (local && (!Number.isSafeInteger(threshold) || threshold <= 0)) {
      setError(t('本地消耗上限必须是大于 0 的安全整数')); return
    }
    if (condition.metric.endsWith('percent') && threshold > 100) {
      setError(t('百分比阈值必须在 0~100 之间')); return
    }
    let window = condition.window
    if (local) {
      window = { ...window, reset: window?.reset || 'daily' }
      if (window.reset === 'fixed' || window.reset === 'rolling') {
        const seconds = Math.round(Number(draft.period) * 3600)
        if (!Number.isFinite(seconds) || seconds < 60 || seconds > 2_678_400) {
          setError(t('统计窗口必须是 1 分钟~31 天')); return
        }
        window.seconds = seconds
      }
      if (window.reset === 'total' || window.reset === 'fixed') {
        window.startsAt = draft.anchor ? new Date(draft.anchor).getTime() : 0
        window.endsAt = draft.end ? new Date(draft.end).getTime() : 0
        if (!Number.isFinite(window.startsAt) || !Number.isFinite(window.endsAt)
          || window.endsAt > 0 && window.endsAt <= window.startsAt) {
          setError(t('统计结束时间必须晚于开始时间')); return
        }
      }
    }
    onSave({ ...condition, threshold, window,
      source: source && source.id !== 'account' && source.id !== 'local' ? sourceIdentity(source) : null,
    })
  }

  return <Dialog open onOpenChange={next => { if (!next) onClose() }}>
    <DialogContent className='w-[min(680px,calc(100vw-48px))]' showCloseButton overlayForceRender>
      <DialogHeader><DialogTitle>{t('编辑限制条件')}</DialogTitle></DialogHeader>
      <DialogBody className='space-y-3'>
        {plans.length ? <Field orientation='horizontal' className='field-row'>
          <FieldLabel>{t('套餐')}</FieldLabel>
          <Select value={plan} onValueChange={id => {
            const next = sources.find(source => id ? source.planId === id : !source.planId)
            if (next) selectSource(next.id)
          }}>
            <SelectTrigger className='min-w-0 flex-1'><SelectValue>{plans.find(([id]) => id === plan)?.[1] || t('通用额度／本地保护')}</SelectValue></SelectTrigger>
            <SelectContent>
              <SelectItem value=''>{t('通用额度／本地保护')}</SelectItem>
              {plans.map(([id, label]) => <SelectItem key={id} value={id}>{label}</SelectItem>)}
            </SelectContent>
          </Select>
        </Field> : null}
        <Field orientation='horizontal' className='field-row'>
          <FieldLabel>{t('额度来源')}</FieldLabel>
          <Select value={condition.sourceId || 'account'} onValueChange={id => { if (id) selectSource(id) }}>
            <SelectTrigger className='min-w-0 flex-1'><SelectValue>{sourceLabel(source, condition.sourceId || 'account')}</SelectValue></SelectTrigger>
            <SelectContent>{options.map(source => <SelectItem key={source.id} value={source.id}>
              {sourceLabel(source)}
            </SelectItem>)}</SelectContent>
          </Select>
        </Field>
        {aggregate ? <MetricCard className='space-y-2'>
          <div className='flex flex-wrap items-center gap-2'>
            <span className='text-xs text-subtle'>{t('总积分')}</span>
            <span className='text-[13px] font-semibold tabular-nums'>{aggregateView}</span>
            <Button variant='outline' size='sm' className='ml-auto'
              disabled={source?.id === aggregate.id && condition.metric === 'remaining'}
              onClick={() => selectSource(aggregate.id)}>{t('按总积分设置限制')}</Button>
          </div>
          <p className='detail'>{t('总积分是当前可用积分的汇总，与余额列一致；不重复累加汇总行，不包含 Token、次数或其他单位的额度。')}</p>
        </MetricCard> : null}
        <Field orientation='horizontal' className='field-row'>
          <FieldLabel>{t('模型范围')}</FieldLabel>
          <MultiSelect value={condition.models} options={modelOptions}
            onValueChange={models => update({ models })} placeholder={source?.models?.length ? t('该额度中的全部模型') : t('全部模型')}
            className='min-w-0 flex-1' aria-label={t('模型范围')} />
        </Field>
        <Field orientation='horizontal' className='field-row'>
          <FieldLabel>{t('限制指标')}</FieldLabel>
          <Select value={condition.metric} onValueChange={metric => {
            if (!metric) return
            update({ metric: metric as LimiterMetric, comparison: metric.startsWith('remaining') ? 'lte' : 'gte' })
            setDraft(current => ({ ...current, threshold: '', unit: 'unit' }))
          }}>
            <SelectTrigger className='min-w-[190px]'><SelectValue>{metricLabel(condition.metric)}</SelectValue></SelectTrigger>
            <SelectContent>{metrics.map(metric => <SelectItem key={metric} value={metric}>{metricLabel(metric)}</SelectItem>)}</SelectContent>
          </Select>
        </Field>
        <FieldGroup orientation='horizontal' className='field-row'>
          <span className='text-[12.5px] text-subtle'>{t('触发条件')}</span>
          <Select value={condition.comparison} onValueChange={comparison => update({ comparison: comparison as LimiterCondition['comparison'] })}>
            <SelectTrigger className='w-[92px]' aria-label={t('触发条件')}><SelectValue>{condition.comparison === 'lt' ? '<' : condition.comparison === 'lte' ? '≤' : '≥'}</SelectValue></SelectTrigger>
            <SelectContent><SelectItem value='lt'>{'<'}</SelectItem><SelectItem value='lte'>≤</SelectItem><SelectItem value='gte'>≥</SelectItem></SelectContent>
          </Select>
          <Input type='number' min={0} className='max-w-[160px]' value={draft.threshold}
            onChange={event => setDraft({ ...draft, threshold: event.currentTarget.value })} placeholder={t('阈值')} aria-label={t('阈值')} />
          {condition.metric === 'tokens' ? <Select value={draft.unit} onValueChange={unit => setDraft({ ...draft, unit: unit as Draft['unit'] })}>
            <SelectTrigger className='w-[84px]' aria-label={t('Token 单位')}><SelectValue>{draft.unit === 'yi' ? t('亿') : draft.unit === 'wan' ? t('万') : t('个')}</SelectValue></SelectTrigger>
            <SelectContent><SelectItem value='yi'>{t('亿')}</SelectItem><SelectItem value='wan'>{t('万')}</SelectItem><SelectItem value='unit'>{t('个')}</SelectItem></SelectContent>
          </Select> : <span className='detail'>{condition.metric.endsWith('percent') ? '%' : condition.metric === 'requests' ? t('次') : usageUnitLabel(source?.unit || '', t)}</span>}
        </FieldGroup>
        {thresholdPreview ? <p className='detail'>{t('实际阈值：{value} Token', { value: thresholdPreview })}</p> : null}
        {local ? <>
          <Field orientation='horizontal' className='field-row'>
            <FieldLabel>{t('统计周期')}</FieldLabel>
            <Select value={condition.window?.reset || 'daily'} onValueChange={reset => update({ window: { ...condition.window, reset: reset as LimiterWindow['reset'] } })}>
              <SelectTrigger className='min-w-0 flex-1'><SelectValue>{RESET_OPTIONS.find(option => option.value === (condition.window?.reset || 'daily'))!.label()}</SelectValue></SelectTrigger>
              <SelectContent>{RESET_OPTIONS.map(option => <SelectItem key={option.value} value={option.value}>{option.label()}</SelectItem>)}</SelectContent>
            </Select>
          </Field>
          {condition.window?.reset === 'fixed' || condition.window?.reset === 'rolling' ? <Field orientation='horizontal' className='field-row'>
            <FieldLabel>{t('窗口长度')}</FieldLabel>
            <Input type='number' min={1 / 60} max={744} step='any' value={draft.period} className='max-w-[130px]'
              onChange={event => setDraft({ ...draft, period: event.currentTarget.value })} /><span className='detail'>{t('小时')}</span>
          </Field> : null}
          {condition.window?.reset === 'total' || condition.window?.reset === 'fixed' ? <>
            <Field orientation='horizontal' className='field-row'><FieldLabel>{t('累计起点')}</FieldLabel><Input type='datetime-local' value={draft.anchor}
              onChange={event => setDraft({ ...draft, anchor: event.currentTarget.value })} className='min-w-0 flex-1' /></Field>
            <Field orientation='horizontal' className='field-row'><FieldLabel>{t('到期时间')}</FieldLabel><Input type='datetime-local' value={draft.end}
              onChange={event => setDraft({ ...draft, end: event.currentTarget.value })} className='min-w-0 flex-1' /></Field>
          </> : null}
          {['daily', 'weekly', 'monthly'].includes(condition.window?.reset || 'daily') ? <Field orientation='horizontal' className='field-row'>
            <FieldLabel>{t('重置时区')}</FieldLabel>
            <Select value={condition.window?.utcOffsetMinutes === 480 ? 'beijing' : condition.window?.utcOffsetMinutes === 0 ? 'utc' : 'local'}
              onValueChange={zone => update({ window: { ...condition.window, reset: condition.window?.reset || 'daily', utcOffsetMinutes: zone === 'beijing' ? 480 : zone === 'utc' ? 0 : null } })}>
              <SelectTrigger className='min-w-[190px]'><SelectValue>{condition.window?.utcOffsetMinutes === 480 ? t('北京时间（UTC+8）') : condition.window?.utcOffsetMinutes === 0 ? 'UTC' : t('服务器本地时区')}</SelectValue></SelectTrigger>
              <SelectContent><SelectItem value='local'>{t('服务器本地时区')}</SelectItem><SelectItem value='beijing'>{t('北京时间（UTC+8）')}</SelectItem><SelectItem value='utc'>UTC</SelectItem></SelectContent>
            </Select>
          </Field> : null}
        </> : null}
        <p className='detail'>{local
          ? t('本地统计只包含经过本网关的请求，不等于上游账单；清理请求日志不会清零。套餐选择用于限定保护范围，不会强制上游扣指定套餐。')
          : t('按上游额度查询的读数判定；模型共用同一个额度池时，选择模型不会拆分该池余额。')}</p>
        {local && modelOptions.length > 0 ? <p className='detail'>{t('选中多个模型时，每个模型单独累计、单独比较阈值；不合并用量。')}</p> : null}
        {source?.localEstimate ? <p className='detail text-warning'>{t('上游没有提供此额度的分桶用量，当前使用本地估算保护；在其他客户端产生的消耗不会计入。')}</p> : null}
        {source?.channel && source.channel !== String(account?.quotaChannel || account?.zcodePlan || '') ? <p className='detail text-warning'>
          {t('此额度不属于账号当前使用的通道，切换到对应通道后才参与判定。')}
        </p> : null}
        {error ? <p className='detail text-destructive'>{error}</p> : null}
      </DialogBody>
      <DialogFooter><Button variant='outline' onClick={onClose}>{t('取消')}</Button><Button onClick={save}>{t('保存条件')}</Button></DialogFooter>
    </DialogContent>
  </Dialog>
}
