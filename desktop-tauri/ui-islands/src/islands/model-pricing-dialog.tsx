import * as React from 'react'
import {
  Badge, Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader,
  DialogTitle, Field, FieldLabel, Input, SegmentedControl, Select, SelectContent,
  SelectItem, SelectTrigger, SelectValue,
} from '@ui'
import { t } from '../i18n'
import {
  formatUnitPrice, pricingSourceLabel, updateCachedDisplay, usePricingDisplay,
  type ModelPrice, type ModelPriceTier, type ModelPricingInfo, type ModelPricingSettings,
} from './model-pricing'

export type ModelPricingContext = {
  provider: string
  providerLabel: string
  modelId: string
  pricing: ModelPricingInfo | null
  settings: ModelPricingSettings | null
}

type Mode = 'default' | 'custom' | 'same_as'

type DraftTier = {
  above: string
  input: string
  output: string
  cacheRead: string
  cacheWrite: string
  cacheWrite1h: string
}

function numToStr(value: number | undefined): string {
  return typeof value === 'number' && Number.isFinite(value) ? String(value) : ''
}

export function ModelPricingDialog({ context, onClose, onSaved }: {
  context: ModelPricingContext
  onClose: () => void
  onSaved: () => Promise<void> | void
}) {
  const display = usePricingDisplay()
  const exactKey = `${context.provider}/${context.modelId}`
  const providerWildKey = `${context.provider}/*`
  const modelWildKey = `*/${context.modelId}`

  const initialKey = React.useMemo(() => {
    const prices = context.settings?.modelPrices || {}
    const same = context.settings?.modelSameAs || {}
    if (prices[providerWildKey] && !prices[exactKey]) return providerWildKey
    if (prices[modelWildKey] && !prices[exactKey]) return modelWildKey
    if (same[modelWildKey] && !same[exactKey]) return modelWildKey
    return exactKey
  }, [context.settings, exactKey, providerWildKey, modelWildKey])

  const [targetKey, setTargetKey] = React.useState(initialKey)
  const [mode, setMode] = React.useState<Mode>(() => {
    const prices = context.settings?.modelPrices || {}
    const same = context.settings?.modelSameAs || {}
    if (prices[initialKey]) return 'custom'
    if (same[initialKey]) return 'same_as'
    if (context.pricing?.source === 'same_as') return 'same_as'
    return 'default'
  })

  const base = context.settings?.modelPrices?.[targetKey] || context.pricing?.base
  const [inputPrice, setInputPrice] = React.useState(() => numToStr(base?.input))
  const [outputPrice, setOutputPrice] = React.useState(() => numToStr(base?.output))
  const [cacheReadPrice, setCacheReadPrice] = React.useState(() => numToStr(base?.cacheRead))
  const [cacheWritePrice, setCacheWritePrice] = React.useState(() => numToStr(base?.cacheWrite))
  const [cacheWrite1hPrice, setCacheWrite1hPrice] = React.useState(() => numToStr(base?.cacheWrite1h))
  const [tiers, setTiers] = React.useState<DraftTier[]>(() =>
    (base?.tiers || []).map(item => ({
      above: String(item.above),
      input: String(item.input),
      output: String(item.output),
      cacheRead: numToStr(item.cacheRead),
      cacheWrite: numToStr(item.cacheWrite),
      cacheWrite1h: numToStr(item.cacheWrite1h),
    })),
  )
  const [sameAs, setSameAs] = React.useState(
    () => context.settings?.modelSameAs?.[targetKey] || context.pricing?.sameAs || '',
  )
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')

  React.useEffect(() => {
    const prices = context.settings?.modelPrices || {}
    const same = context.settings?.modelSameAs || {}
    const existingPrice = prices[targetKey]
    const existingSame = same[targetKey]
    if (existingPrice) {
      setMode('custom')
      setInputPrice(numToStr(existingPrice.input))
      setOutputPrice(numToStr(existingPrice.output))
      setCacheReadPrice(numToStr(existingPrice.cacheRead))
      setCacheWritePrice(numToStr(existingPrice.cacheWrite))
      setCacheWrite1hPrice(numToStr(existingPrice.cacheWrite1h))
      setTiers((existingPrice.tiers || []).map(item => ({
        above: String(item.above),
        input: String(item.input),
        output: String(item.output),
        cacheRead: numToStr(item.cacheRead),
        cacheWrite: numToStr(item.cacheWrite),
        cacheWrite1h: numToStr(item.cacheWrite1h),
      })))
    } else if (existingSame) {
      setMode('same_as')
      setSameAs(existingSame)
    } else {
      setMode('default')
      const fallback = context.pricing?.base
      setInputPrice(numToStr(fallback?.input))
      setOutputPrice(numToStr(fallback?.output))
      setCacheReadPrice(numToStr(fallback?.cacheRead))
      setCacheWritePrice(numToStr(fallback?.cacheWrite))
      setCacheWrite1hPrice(numToStr(fallback?.cacheWrite1h))
      setTiers((fallback?.tiers || []).map(item => ({
        above: String(item.above),
        input: String(item.input),
        output: String(item.output),
        cacheRead: numToStr(item.cacheRead),
        cacheWrite: numToStr(item.cacheWrite),
        cacheWrite1h: numToStr(item.cacheWrite1h),
      })))
    }
  }, [targetKey, context.settings, context.pricing])

  const scopeOptions = [
    { value: exactKey, label: t('仅当前提供商（{provider}）', { provider: context.providerLabel }) },
    { value: providerWildKey, label: t('当前提供商全部模型（{provider}/*）', { provider: context.provider }) },
    { value: modelWildKey, label: t('全部提供商该模型（*/{model}）', { model: context.modelId }) },
  ]
  const currentScopeLabel = scopeOptions.find(opt => opt.value === targetKey)?.label || scopeOptions[0].label

  const parseNum = (raw: string, label: string, fallback = 0): number => {
    const trimmed = raw.trim()
    if (!trimmed) return fallback
    const parsed = Number(trimmed)
    if (!Number.isFinite(parsed) || parsed < 0) {
      throw new Error(t('{label} 必须是非负数字', { label }))
    }
    return parsed
  }

  async function submit(): Promise<void> {
    const bridge = (window as unknown as {
      workbuddyDesktop?: { saveModelPricing?: (payload: Record<string, unknown>) => Promise<ModelPricingSettings> }
    }).workbuddyDesktop
    if (!bridge?.saveModelPricing) return
    setError('')
    try {
      const payload: Record<string, unknown> = { action: 'setModelPrice', key: targetKey }
      if (mode === 'default') {
        payload.price = null
        payload.sameAs = null
      } else if (mode === 'same_as') {
        const refModel = sameAs.trim()
        if (!refModel) {
          setError(t('请输入要参考的模型名称，例如 claude-sonnet-4-5'))
          return
        }
        payload.price = null
        payload.sameAs = refModel
      } else {
        const parsedTiers: ModelPriceTier[] = []
        for (const tier of tiers) {
          const above = Math.trunc(Number(tier.above.trim()))
          if (!Number.isFinite(above) || above <= 0) {
            throw new Error(t('阶梯阈值 token 数必须大于 0'))
          }
          const inP = parseNum(tier.input, t('阶梯输入单价'))
          const outP = parseNum(tier.output, t('阶梯输出单价'))
          const cr = parseNum(tier.cacheRead, t('阶梯缓存读取单价'))
          const cw = parseNum(tier.cacheWrite, t('阶梯缓存写入单价'))
          const cw1h = parseNum(tier.cacheWrite1h, t('阶梯 1h 缓存写入单价'))
          parsedTiers.push({
            above,
            input: inP,
            output: outP,
            ...(cr > 0 ? { cacheRead: cr } : {}),
            ...(cw > 0 ? { cacheWrite: cw } : {}),
            ...(cw1h > 0 ? { cacheWrite1h: cw1h } : {}),
          })
        }
        const price: ModelPrice = {
          input: parseNum(inputPrice, t('输入单价')),
          output: parseNum(outputPrice, t('输出单价')),
          cacheRead: parseNum(cacheReadPrice, t('缓存读取单价')),
          cacheWrite: parseNum(cacheWritePrice, t('缓存写入单价')),
          cacheWrite1h: parseNum(cacheWrite1hPrice, t('1小时缓存写入单价')),
          ...(parsedTiers.length > 0 ? { tiers: parsedTiers } : {}),
        }
        payload.price = price
        payload.sameAs = null
      }
      setBusy(true)
      const view = await bridge.saveModelPricing(payload)
      updateCachedDisplay({ currency: view.currency, exchangeRate: view.fx?.rate, fx: view.fx })
      await onSaved()
      onClose()
    } catch (err) {
      setError(err instanceof Error ? err.message : t('保存模型单价失败'))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Dialog open onOpenChange={open => { if (!open) onClose() }}>
      <DialogContent className='max-w-[560px]'>
        <DialogHeader>
          <DialogTitle>{t('模型计价设置 · {model}', { model: context.modelId })}</DialogTitle>
        </DialogHeader>
        <DialogBody className='flex flex-col gap-3'>
          <div className='flex flex-wrap items-center justify-between gap-2 rounded-md border border-border bg-[var(--surface-2)] px-3 py-2 text-xs'>
            <div className='flex items-center gap-2'>
              <Badge variant='outline' shape='tag'>
                {context.pricing?.priced ? pricingSourceLabel(context.pricing.source) : t('未定价')}
              </Badge>
              {context.pricing?.effective ? (
                <span>
                  {t('当前生效：输入 {input}/M · 输出 {output}/M', {
                    input: formatUnitPrice(context.pricing.effective.input, display.currency, display.exchangeRate),
                    output: formatUnitPrice(context.pricing.effective.output, display.currency, display.exchangeRate),
                  })}
                </span>
              ) : (
                <span className='text-[var(--text-3)]'>{t('尚未匹配到官网基准价')}</span>
              )}
            </div>
            <span className='text-[var(--text-3)]'>{t('单价填写均以美元($)/百万 Token 为准')}</span>
          </div>

          <Field>
            <FieldLabel>{t('生效范围')}</FieldLabel>
            <Select value={targetKey} onValueChange={v => v && setTargetKey(v)}>
              <SelectTrigger>
                <SelectValue>{currentScopeLabel}</SelectValue>
              </SelectTrigger>
              <SelectContent>
                {scopeOptions.map(opt => (
                  <SelectItem key={opt.value} value={opt.value}>{opt.label}</SelectItem>
                ))}
              </SelectContent>
            </Select>
          </Field>

          <Field>
            <FieldLabel>{t('计价方式')}</FieldLabel>
            <SegmentedControl
              value={mode}
              onValueChange={v => setMode(v as Mode)}
              aria-label={t('计价方式')}
              options={[
                { value: 'default', label: t('恢复默认 / 官网价') },
                { value: 'custom', label: t('自定义单价') },
                { value: 'same_as', label: t('参考同款模型') },
              ]}
            />
          </Field>

          {mode === 'custom' ? (
            <div className='flex flex-col gap-3'>
              <div className='grid grid-cols-2 gap-3 sm:grid-cols-3'>
                <Field>
                  <FieldLabel>{t('输入 ($/M)')}</FieldLabel>
                  <Input type='number' min='0' step='any' value={inputPrice} placeholder='0'
                    onChange={e => setInputPrice(e.currentTarget.value)} />
                </Field>
                <Field>
                  <FieldLabel>{t('输出 ($/M)')}</FieldLabel>
                  <Input type='number' min='0' step='any' value={outputPrice} placeholder='0'
                    onChange={e => setOutputPrice(e.currentTarget.value)} />
                </Field>
                <Field>
                  <FieldLabel>{t('缓存读取 ($/M)')}</FieldLabel>
                  <Input type='number' min='0' step='any' value={cacheReadPrice} placeholder='0'
                    onChange={e => setCacheReadPrice(e.currentTarget.value)} />
                </Field>
                <Field>
                  <FieldLabel>{t('缓存写入 5m ($/M)')}</FieldLabel>
                  <Input type='number' min='0' step='any' value={cacheWritePrice} placeholder='0'
                    onChange={e => setCacheWritePrice(e.currentTarget.value)} />
                </Field>
                <Field>
                  <FieldLabel>{t('缓存写入 1h ($/M)')}</FieldLabel>
                  <Input type='number' min='0' step='any' value={cacheWrite1hPrice} placeholder={t('Claude 自动 2×')}
                    onChange={e => setCacheWrite1hPrice(e.currentTarget.value)} />
                </Field>
              </div>
              <div className='flex items-center justify-between text-xs'>
                <span className='text-[var(--text-3)]'>{t('将单价全部填 0 可将该模型明确设为免费')}</span>
                <Button size='xs' variant='outline' type='button' onClick={() => setTiers(prev => [...prev, {
                  above: '200000', input: inputPrice || '0', output: outputPrice || '0',
                  cacheRead: cacheReadPrice || '0', cacheWrite: cacheWritePrice || '0', cacheWrite1h: '',
                }])}>{t('＋ 添加长上下文阶梯价')}</Button>
              </div>
              {tiers.length > 0 ? (
                <div className='flex flex-col gap-2 rounded-md border border-border p-2.5'>
                  {tiers.map((tier, idx) => (
                    <div key={idx} className='grid grid-cols-3 gap-2 items-end sm:grid-cols-6'>
                      <Field>
                        <FieldLabel className='text-[11px]'>{t('超出 Token')}</FieldLabel>
                        <Input type='number' min='1' value={tier.above}
                          onChange={e => setTiers(prev => prev.map((it, i) => i === idx ? { ...it, above: e.currentTarget.value } : it))} />
                      </Field>
                      <Field>
                        <FieldLabel className='text-[11px]'>{t('输入')}</FieldLabel>
                        <Input type='number' min='0' step='any' value={tier.input}
                          onChange={e => setTiers(prev => prev.map((it, i) => i === idx ? { ...it, input: e.currentTarget.value } : it))} />
                      </Field>
                      <Field>
                        <FieldLabel className='text-[11px]'>{t('输出')}</FieldLabel>
                        <Input type='number' min='0' step='any' value={tier.output}
                          onChange={e => setTiers(prev => prev.map((it, i) => i === idx ? { ...it, output: e.currentTarget.value } : it))} />
                      </Field>
                      <Field>
                        <FieldLabel className='text-[11px]'>{t('缓存读')}</FieldLabel>
                        <Input type='number' min='0' step='any' value={tier.cacheRead}
                          onChange={e => setTiers(prev => prev.map((it, i) => i === idx ? { ...it, cacheRead: e.currentTarget.value } : it))} />
                      </Field>
                      <Field>
                        <FieldLabel className='text-[11px]'>{t('缓存写')}</FieldLabel>
                        <Input type='number' min='0' step='any' value={tier.cacheWrite}
                          onChange={e => setTiers(prev => prev.map((it, i) => i === idx ? { ...it, cacheWrite: e.currentTarget.value } : it))} />
                      </Field>
                      <Button size='xs' variant='ghost' type='button'
                        onClick={() => setTiers(prev => prev.filter((_, i) => i !== idx))}>{t('移除')}</Button>
                    </div>
                  ))}
                </div>
              ) : null}
            </div>
          ) : null}

          {mode === 'same_as' ? (
            <Field>
              <FieldLabel>{t('参考的官方模型 ID')}</FieldLabel>
              <Input
                list='wb-pricing-reference-models'
                value={sameAs}
                placeholder={t('例如：claude-sonnet-4-5 或 gpt-5')}
                onChange={e => setSameAs(e.currentTarget.value)}
              />
              <datalist id='wb-pricing-reference-models'>
                {(context.settings?.referenceModels || []).map(item => (
                  <option key={item} value={item} />
                ))}
              </datalist>
            </Field>
          ) : null}

          {error ? <p className='text-xs text-[var(--danger)]'>{error}</p> : null}
        </DialogBody>
        <DialogFooter>
          <Button variant='outline' onClick={onClose} disabled={busy}>{t('取消')}</Button>
          <Button onClick={() => void submit()} disabled={busy}>
            {busy ? t('保存中…') : t('保存设置')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
