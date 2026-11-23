import * as React from 'react'
import {
  Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader,
  DialogTitle, Field, FieldLabel, Input,
} from '@ui'
import { t } from '../i18n'
import {
  updateCachedDisplay, type ModelPricingSettings, type RateWindow,
} from './model-pricing'

const WEEKDAYS = [
  { value: 0, label: t('日') },
  { value: 1, label: t('一') },
  { value: 2, label: t('二') },
  { value: 3, label: t('三') },
  { value: 4, label: t('四') },
  { value: 5, label: t('五') },
  { value: 6, label: t('六') },
]

export type ProviderPricingContext = {
  provider: string
  providerLabel: string
  settings: ModelPricingSettings | null
}

type DraftWindow = {
  days: number[]
  start: string
  end: string
  rate: string
}

export function ProviderPricingDialog({ context, onClose, onSaved }: {
  context: ProviderPricingContext
  onClose: () => void
  onSaved: () => Promise<void> | void
}) {
  const existing = context.settings?.providerPricing?.[context.provider]
  const [priceRate, setPriceRate] = React.useState(
    () => (existing?.priceRate != null && existing.priceRate > 0 ? String(existing.priceRate) : ''),
  )
  const [windows, setWindows] = React.useState<DraftWindow[]>(() =>
    (existing?.rateWindows || []).map(w => ({
      days: Array.isArray(w.days) ? [...w.days] : [],
      start: w.start,
      end: w.end,
      rate: String(w.rate),
    })),
  )
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')

  function toggleDay(idx: number, day: number) {
    setWindows(prev => prev.map((w, i) => {
      if (i !== idx) return w
      const exists = w.days.includes(day)
      const nextDays = exists ? w.days.filter(d => d !== day) : [...w.days, day].sort((a, b) => a - b)
      return { ...w, days: nextDays }
    }))
  }

  async function submit(): Promise<void> {
    const bridge = (window as unknown as {
      workbuddyDesktop?: { saveModelPricing?: (payload: Record<string, unknown>) => Promise<ModelPricingSettings> }
    }).workbuddyDesktop
    if (!bridge?.saveModelPricing) return
    setError('')
    try {
      const trimmedRate = priceRate.trim()
      let parsedRate: number | null = null
      if (trimmedRate) {
        const val = Number(trimmedRate)
        if (!Number.isFinite(val) || val <= 0 || val > 1000) {
          throw new Error(t('整体倍率必须是大于 0 且不超过 1000 的数字'))
        }
        parsedRate = val
      }
      const parsedWindows: RateWindow[] = windows.map((w, idx) => {
        const rateVal = Number(w.rate.trim())
        if (!Number.isFinite(rateVal) || rateVal <= 0 || rateVal > 1000) {
          throw new Error(t('第 {n} 个时段的倍率必须在 0 到 1000 之间', { n: idx + 1 }))
        }
        return {
          days: w.days,
          start: w.start.trim(),
          end: w.end.trim(),
          rate: rateVal,
        }
      })
      setBusy(true)
      const view = await bridge.saveModelPricing({
        action: 'setProviderPricing',
        provider: context.provider,
        priceRate: parsedRate,
        rateWindows: parsedWindows,
      })
      updateCachedDisplay({ currency: view.currency, exchangeRate: view.fx?.rate, fx: view.fx })
      await onSaved()
      onClose()
    } catch (err) {
      setError(err instanceof Error ? err.message : t('保存提供商计价配置失败'))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Dialog open onOpenChange={open => { if (!open) onClose() }}>
      <DialogContent className='max-w-[540px]'>
        <DialogHeader>
          <DialogTitle>{t('提供商倍率与分时窗口 · {provider}', { provider: context.providerLabel })}</DialogTitle>
        </DialogHeader>
        <DialogBody className='flex flex-col gap-4'>
          <Field>
            <FieldLabel>{t('整体价格倍率（留空即 1× 原价）')}</FieldLabel>
            <Input
              type='number'
              min='0.001'
              max='1000'
              step='any'
              value={priceRate}
              placeholder='1.0'
              onChange={e => setPriceRate(e.currentTarget.value)}
            />
            <span className='mt-1 text-xs text-[var(--text-3)]'>
              {t('用于中转站折扣或加价（如 0.5 表示半价，2 表示双倍）；仅缩放官方参考价，不改变手动指定单价的模型。')}
            </span>
          </Field>

          <div className='flex flex-col gap-2'>
            <div className='flex items-center justify-between'>
              <span className='text-sm font-medium'>{t('分时倍率时段（最多 8 个）')}</span>
              {windows.length < 8 ? (
                <Button size='xs' variant='outline' type='button' onClick={() => setWindows(prev => [...prev, {
                  days: [], start: '00:00', end: '08:00', rate: '0.5',
                }])}>{t('＋ 添加时段')}</Button>
              ) : null}
            </div>
            {windows.length === 0 ? (
              <div className='rounded-md border border-dashed border-border p-3 text-center text-xs text-[var(--text-3)]'>
                {t('未设置分时时段；将全天按上方整体倍率计价。')}
              </div>
            ) : (
              <div className='flex flex-col gap-2.5'>
                {windows.map((w, idx) => (
                  <div key={idx} className='flex flex-col gap-2 rounded-md border border-border bg-[var(--surface-2)] p-2.5'>
                    <div className='flex flex-wrap items-center justify-between gap-2'>
                      <div className='flex items-center gap-1'>
                        {WEEKDAYS.map(day => {
                          const active = w.days.includes(day.value)
                          return (
                            <Button
                              key={day.value}
                              type='button'
                              size='xs'
                              variant={active ? 'default' : 'outline'}
                              onClick={() => toggleDay(idx, day.value)}
                            >
                              {day.label}
                            </Button>
                          )
                        })}
                        {w.days.length === 0 ? (
                          <span className='ml-1 text-[11px] text-[var(--text-3)]'>{t('每天')}</span>
                        ) : null}
                      </div>
                      <Button size='xs' variant='ghost' type='button'
                        onClick={() => setWindows(prev => prev.filter((_, i) => i !== idx))}>{t('删除')}</Button>
                    </div>
                    <div className='grid grid-cols-3 gap-2'>
                      <Field>
                        <FieldLabel className='text-[11px]'>{t('开始时间')}</FieldLabel>
                        <Input value={w.start} placeholder='09:00'
                          onChange={e => { const v = e.currentTarget.value; setWindows(prev => prev.map((it, i) => i === idx ? { ...it, start: v } : it)) }} />
                      </Field>
                      <Field>
                        <FieldLabel className='text-[11px]'>{t('结束时间')}</FieldLabel>
                        <Input value={w.end} placeholder='18:00'
                          onChange={e => { const v = e.currentTarget.value; setWindows(prev => prev.map((it, i) => i === idx ? { ...it, end: v } : it)) }} />
                      </Field>
                      <Field>
                        <FieldLabel className='text-[11px]'>{t('时段倍率')}</FieldLabel>
                        <Input type='number' min='0.001' max='1000' step='any' value={w.rate} placeholder='2.0'
                          onChange={e => { const v = e.currentTarget.value; setWindows(prev => prev.map((it, i) => i === idx ? { ...it, rate: v } : it)) }} />
                      </Field>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </div>

          {error ? <p className='text-xs text-[var(--danger)]'>{error}</p> : null}
        </DialogBody>
        <DialogFooter>
          <Button variant='outline' onClick={onClose} disabled={busy}>{t('取消')}</Button>
          <Button onClick={() => void submit()} disabled={busy}>
            {busy ? t('保存中…') : t('保存规则')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
