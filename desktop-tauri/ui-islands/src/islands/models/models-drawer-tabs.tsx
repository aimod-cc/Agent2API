import * as React from 'react'
import {
  Badge,
  Button,
  Field,
  FieldLabel,
  Input,
  Label,
  SegmentedControl,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  type SegmentedControlOption,
} from '@ui'
import { t } from '../../i18n'
import * as customSource from '../models-custom-source'
import type { ManageModel } from '../models-custom-source'
import {
  BOOLEAN_KEYS,
  CAPABILITY_KEYS,
  CAPABILITY_LABELS,
  TOKEN_KEYS,
  capabilitiesOf,
  capabilityState,
  capSourceOf,
  exactTokens,
  normalizeCapability,
  normalizeOverrides,
  patchFromDraft,
  reasoningLevelsOf,
  type CapabilityDraft,
  type CapabilityKey,
  type CapabilitySource,
} from '../model-capability'
import {
  formatUnitPrice,
  pricingSourceLabel,
  updateCachedDisplay,
  usePricingDisplay,
  type ModelPrice,
  type ModelPriceTier,
  type ModelPricingSettings,
} from '../model-pricing'
import {
  accept,
  errorMessage,
  esc,
  formatTime,
  providerLabelOf,
  rowKeyOf,
  runRowAction,
  shared,
  toast,
  visibilityOf,
  writeCapabilities,
  writeModelProtocol,
  writeRemoveModel,
} from '../models-panel-state'

/* ─── Tab 2：能力与规格 ───────────────────────────── */

const BOOLEAN_OPTIONS = (inheritLabel: string): SegmentedControlOption<string>[] => [
  { value: 'inherit', label: inheritLabel },
  { value: 'on', label: t('支持') },
  { value: 'off', label: t('不支持') },
]

function capHintText(
  key: CapabilityKey,
  value: number | boolean | null,
  overridden: boolean,
  custom: boolean,
  source: CapabilitySource | '' = '',
): string {
  const has = value !== null && value !== undefined
  const shown = TOKEN_KEYS.includes(key)
    ? exactTokens(value)
    : (capabilityState(value) === 'on' ? t('支持') : capabilityState(value) === 'off' ? t('不支持') : t('未声明'))
  if (overridden) return custom ? t('已填写：{value}', { value: shown }) : t('已覆盖：{value}', { value: shown })
  if (!has) return custom ? t('未填写') : t('上游未声明')
  if (source === 'gateway') return t('网关模拟支持')
  return source === 'modelsdev'
    ? t('models.dev 补齐：{value}', { value: shown })
    : t('继承上游：{value}', { value: shown })
}

function buildCapDraft(capabilities: ReturnType<typeof capabilitiesOf>, setKeys: CapabilityKey[]): CapabilityDraft {
  const draft = {} as CapabilityDraft
  for (const key of CAPABILITY_KEYS) {
    const set = setKeys.includes(key)
    const value = capabilities[key]
    if (TOKEN_KEYS.includes(key)) {
      draft[key] = set && typeof value === 'number' ? String(value) : ''
    } else {
      draft[key] = set ? (value === true ? 'on' : 'off') : 'inherit'
    }
  }
  return draft
}

export function DrawerCapabilitiesTab({ model }: { model: ManageModel }) {
  const providerId = model.provider || ''
  const custom = customSource.isCustom(providerId)
  const providerRecord = custom ? customSource.record(providerId) : null
  const capabilities = capabilitiesOf(model)
  const setKeys = normalizeOverrides(model.capOverrides)
  const levelsInfo = reasoningLevelsOf(model)

  const [draft, setDraft] = React.useState<CapabilityDraft>(() => buildCapDraft(capabilities, setKeys))
  const [saving, setSaving] = React.useState(false)
  const [status, setStatus] = React.useState('')

  React.useEffect(() => {
    setDraft(buildCapDraft(capabilitiesOf(model), normalizeOverrides(model.capOverrides)))
    setStatus('')
  }, [model.provider, model.id, model.capabilities, model.capOverrides])

  const inheritLabel = custom ? t('自动') : t('继承')
  const clearLabel = custom ? t('恢复默认') : t('恢复继承')

  const defaultProtocol = customSource.normalizeCustomProtocol(providerRecord?.protocol)
    || customSource.normalizeCustomProtocol(model.providerProtocol)
    || 'chat_completions'
  const inferredProtocol = custom ? customSource.inferPresetModelProtocol(providerRecord, model.id) : undefined
  const explicitProtocol = customSource.normalizeCustomProtocol(model.protocol)
  const effectiveProtocol = explicitProtocol || inferredProtocol || defaultProtocol
  const protocolSelectValue = explicitProtocol || '__default__'
  const defaultProtocolLabel = inferredProtocol && inferredProtocol !== defaultProtocol
    ? t('自动匹配（{protocol}）', { protocol: customSource.customProtocolLabel(inferredProtocol) })
    : t('跟随提供商默认（{protocol}）', { protocol: customSource.customProtocolLabel(defaultProtocol) })
  const endpointUrl = custom
    ? customSource.previewCustomEndpointUrl(providerRecord?.baseUrl, effectiveProtocol, providerRecord?.urlSuffix)
    : ''

  async function saveCaps(): Promise<void> {
    if (saving) return
    for (const key of TOKEN_KEYS) {
      const raw = draft[key].trim()
      if (!raw) continue
      if (normalizeCapability(key, raw) === null) {
        setStatus(t('{label}要填 1 ~ 1 亿之间的整数（留空 = {inherit}）', {
          label: CAPABILITY_LABELS[key], inherit: inheritLabel,
        }))
        return
      }
    }
    setSaving(true)
    setStatus('')
    try {
      accept(await writeCapabilities(providerId, model.id, patchFromDraft(draft)))
      toast(t('✅ 已保存 {id} 的能力位（{provider}）', { id: model.id, provider: providerLabelOf(providerId) }))
    } catch (err) {
      setStatus(t('保存失败：{message}', { message: errorMessage(err) }))
    } finally {
      setSaving(false)
    }
  }

  return (
    <>
      {custom ? (
        <section className='mm-drawer-section'>
          <div className='mm-drawer-section-head'>
            <h3>{t('请求协议（自定义提供商）')}</h3>
          </div>
          <Select
            value={protocolSelectValue}
            onValueChange={next => {
              const val = String(next)
              const nextProto = val === '__default__' ? null : val
              void runRowAction(
                rowKeyOf(model),
                () => writeModelProtocol(providerId, model.id, nextProto),
                nextProto
                  ? t('已将 {model} 的请求协议设为 {protocol}', { model: model.id, protocol: customSource.customProtocolLabel(nextProto) })
                  : t('已将 {model} 恢复为默认协议', { model: model.id }),
              )
            }}
          >
            <SelectTrigger className='w-full'>
              <SelectValue>
                {explicitProtocol ? customSource.customProtocolLabel(explicitProtocol) : defaultProtocolLabel}
              </SelectValue>
            </SelectTrigger>
            <SelectContent>
              <SelectItem value='__default__'>{defaultProtocolLabel}</SelectItem>
              {customSource.CUSTOM_PROTOCOL_OPTIONS.map(option => (
                <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>
              ))}
            </SelectContent>
          </Select>
          {endpointUrl ? (
            <div className='break-all rounded-md border border-hairline bg-surface-inset px-2.5 py-1.5 font-mono text-[11px] text-muted-foreground'>
              {endpointUrl}
            </div>
          ) : null}
        </section>
      ) : null}

      <section className='mm-drawer-section'>
        <div className='mm-drawer-section-head'>
          <h3>{t('上下文与输出上限')}</h3>
        </div>
        <div className='grid grid-cols-2 gap-3'>
          {TOKEN_KEYS.map(key => {
            const current = capabilities[key]
            const overridden = setKeys.includes(key)
            const placeholder = overridden
              ? ''
              : typeof current === 'number'
                ? (capSourceOf(model, key) === 'modelsdev'
                  ? t('models.dev {value}', { value: exactTokens(current) })
                  : t('继承 {value}', { value: exactTokens(current) }))
                : (custom ? t('未填写') : t('上游未声明'))
            return (
              <div key={key} className='flex flex-col gap-1'>
                <div className='flex items-center justify-between gap-1'>
                  <Label htmlFor={`drawer-cap-${key}`} className='text-xs'>{CAPABILITY_LABELS[key]}</Label>
                  {overridden ? (
                    <Button variant='ghost' size='2xs' disabled={saving}
                      onClick={() => setDraft(prev => ({ ...prev, [key]: '' }))}>{clearLabel}</Button>
                  ) : null}
                </div>
                <Input
                  id={`drawer-cap-${key}`}
                  inputMode='numeric'
                  autoComplete='off'
                  spellCheck={false}
                  placeholder={placeholder}
                  disabled={saving}
                  value={draft[key]}
                  onChange={e => setDraft(prev => ({ ...prev, [key]: e.currentTarget.value }))}
                  onKeyDown={e => { if (e.key === 'Enter') { e.preventDefault(); void saveCaps() } }}
                />
                <span className='text-[11px] text-muted-foreground'>
                  {capHintText(key, current ?? null, overridden, custom, capSourceOf(model, key))}
                </span>
              </div>
            )
          })}
        </div>
      </section>

      <section className='mm-drawer-section'>
        <div className='mm-drawer-section-head'>
          <h3>{t('能力声明（通知下游客户端）')}</h3>
        </div>
        <div className='flex flex-col gap-3'>
          {BOOLEAN_KEYS.map(key => {
            const overridden = setKeys.includes(key)
            return (
              <div key={key} className='flex items-center justify-between gap-3'>
                <div className='min-w-0'>
                  <div className='text-[12.5px] font-medium text-foreground'>{CAPABILITY_LABELS[key]}</div>
                  <div className='text-[11px] text-muted-foreground'>
                    {capHintText(key, capabilities[key] ?? null, overridden, custom, capSourceOf(model, key))}
                  </div>
                </div>
                <SegmentedControl
                  value={draft[key]}
                  aria-label={CAPABILITY_LABELS[key]}
                  options={BOOLEAN_OPTIONS(inheritLabel)}
                  onValueChange={next => setDraft(prev => ({ ...prev, [key]: String(next) }))}
                />
              </div>
            )
          })}
        </div>
        {model.toolCallingMode ? (
          <p className='text-xs leading-relaxed text-subtle'>
            {model.toolCallingMode === 'emulated'
              ? t('此模型的工具调用由网关模拟，实际执行与权限确认由客户端负责。')
              : t('网页工具模拟已关闭；修改能力声明不会开启实际适配')}
          </p>
        ) : null}
        {levelsInfo ? (
          <div className='rounded-md border border-hairline bg-surface-inset px-3 py-2 text-xs text-subtle'>
            {levelsInfo.fixedLevel ? t('固定思考档位') : t('可用思考档位')}：
            <span className='font-mono text-primary-fg'>{levelsInfo.levels.join(' / ')}</span>
            {levelsInfo.defaultLevel ? t('（默认 {level}）', { level: levelsInfo.defaultLevel }) : ''}
          </div>
        ) : null}
        {status ? <div className='text-xs text-destructive'>{status}</div> : null}
        <div className='flex justify-end pt-1'>
          <Button size='sm' disabled={saving} onClick={() => void saveCaps()}>
            {saving ? t('保存中…') : t('保存能力设置')}
          </Button>
        </div>
      </section>
    </>
  )
}

/* ─── Tab 3：计价与信息 ───────────────────────────── */

type PricingMode = 'default' | 'custom' | 'same_as'
type DraftTier = { above: string; input: string; output: string; cacheRead: string; cacheWrite: string; cacheWrite1h: string }
const numToStr = (value: number | undefined): string =>
  typeof value === 'number' && Number.isFinite(value) ? String(value) : ''

export function DrawerPricingTab({
  model,
  pricingSettings,
  onSavedPricing,
  onRemoved,
}: {
  model: ManageModel
  pricingSettings: ModelPricingSettings | null
  onSavedPricing: () => Promise<void> | void
  onRemoved: () => void
}) {
  const display = usePricingDisplay()
  const providerId = model.provider || ''
  const providerLabel = model.providerLabel || providerLabelOf(providerId)
  const exactKey = `${providerId}/${model.id}`
  const providerWildKey = `${providerId}/*`
  const modelWildKey = `*/${model.id}`

  const initialKey = React.useMemo(() => {
    const prices = pricingSettings?.modelPrices || {}
    const same = pricingSettings?.modelSameAs || {}
    if (prices[providerWildKey] && !prices[exactKey]) return providerWildKey
    if (prices[modelWildKey] && !prices[exactKey]) return modelWildKey
    if (same[modelWildKey] && !same[exactKey]) return modelWildKey
    return exactKey
  }, [pricingSettings, exactKey, providerWildKey, modelWildKey])

  const [targetKey, setTargetKey] = React.useState(initialKey)
  const [mode, setMode] = React.useState<PricingMode>('default')
  const [priceCurrency, setPriceCurrency] = React.useState<'usd' | 'cny'>('usd')
  const [inputPrice, setInputPrice] = React.useState('')
  const [outputPrice, setOutputPrice] = React.useState('')
  const [cacheReadPrice, setCacheReadPrice] = React.useState('')
  const [cacheWritePrice, setCacheWritePrice] = React.useState('')
  const [cacheWrite1hPrice, setCacheWrite1hPrice] = React.useState('')
  const [tiers, setTiers] = React.useState<DraftTier[]>([])
  const [sameAs, setSameAs] = React.useState('')
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')

  React.useEffect(() => {
    setTargetKey(initialKey)
  }, [initialKey])

  React.useEffect(() => {
    const prices = pricingSettings?.modelPrices || {}
    const same = pricingSettings?.modelSameAs || {}
    const existingPrice = prices[targetKey]
    const existingSame = same[targetKey]
    const fillFrom = (src: ModelPrice | null | undefined) => {
      setPriceCurrency(src ? (src.currency === 'cny' ? 'cny' : 'usd') : display.currency)
      setInputPrice(numToStr(src?.input))
      setOutputPrice(numToStr(src?.output))
      setCacheReadPrice(numToStr(src?.cacheRead))
      setCacheWritePrice(numToStr(src?.cacheWrite))
      setCacheWrite1hPrice(numToStr(src?.cacheWrite1h))
      setTiers((src?.tiers || []).map(item => ({
        above: String(item.above),
        input: String(item.input),
        output: String(item.output),
        cacheRead: numToStr(item.cacheRead),
        cacheWrite: numToStr(item.cacheWrite),
        cacheWrite1h: numToStr(item.cacheWrite1h),
      })))
    }
    if (existingPrice) {
      setMode('custom')
      fillFrom(existingPrice)
    } else if (existingSame) {
      setMode('same_as')
      setSameAs(existingSame)
    } else {
      setMode(model.pricing?.source === 'same_as' ? 'same_as' : 'default')
      setSameAs(model.pricing?.sameAs || '')
      fillFrom(model.pricing?.base)
    }
    setError('')
  }, [targetKey, model.provider, model.id, pricingSettings, model.pricing])

  const scopeOptions = [
    { value: exactKey, label: t('仅当前提供商（{provider}）', { provider: providerLabel }) },
    { value: providerWildKey, label: t('当前提供商全部模型（{provider}/*）', { provider: providerLabel }) },
    { value: modelWildKey, label: t('全部提供商该模型（*/{model}）', { model: model.id }) },
  ]
  const currentScopeLabel = scopeOptions.find(opt => opt.value === targetKey)?.label || scopeOptions[0].label

  const parseNum = (raw: string, label: string): number => {
    const trimmed = raw.trim()
    if (!trimmed) return 0
    const parsed = Number(trimmed)
    if (!Number.isFinite(parsed) || parsed < 0) throw new Error(t('{label} 必须是非负数字', { label }))
    return parsed
  }

  async function savePricing(): Promise<void> {
    const bridge = (window as unknown as {
      workbuddyDesktop?: { saveModelPricing?: (payload: Record<string, unknown>) => Promise<ModelPricingSettings> }
    }).workbuddyDesktop
    if (!bridge?.saveModelPricing || busy) return
    setError('')
    try {
      const payload: Record<string, unknown> = { action: 'setModelPrice', key: targetKey }
      if (mode === 'default') {
        payload.price = null
        payload.sameAs = null
      } else if (mode === 'same_as') {
        if (!sameAs.trim()) { setError(t('请输入要参考的模型名称，例如 claude-sonnet-4-5')); return }
        payload.price = null
        payload.sameAs = sameAs.trim()
      } else {
        const parsedTiers: ModelPriceTier[] = tiers.map(tier => {
          const above = Math.trunc(Number(tier.above.trim()))
          if (!Number.isFinite(above) || above <= 0) throw new Error(t('阶梯阈值 token 数必须大于 0'))
          const cr = parseNum(tier.cacheRead, t('阶梯缓存读取单价'))
          const cw = parseNum(tier.cacheWrite, t('阶梯缓存写入单价'))
          const cw1h = parseNum(tier.cacheWrite1h, t('阶梯 1h 缓存写入单价'))
          return {
            above,
            input: parseNum(tier.input, t('阶梯输入单价')),
            output: parseNum(tier.output, t('阶梯输出单价')),
            ...(cr > 0 ? { cacheRead: cr } : {}),
            ...(cw > 0 ? { cacheWrite: cw } : {}),
            ...(cw1h > 0 ? { cacheWrite1h: cw1h } : {}),
          }
        })
        payload.price = {
          input: parseNum(inputPrice, t('输入单价')),
          output: parseNum(outputPrice, t('输出单价')),
          cacheRead: parseNum(cacheReadPrice, t('缓存读取单价')),
          cacheWrite: parseNum(cacheWritePrice, t('缓存写入单价')),
          cacheWrite1h: parseNum(cacheWrite1hPrice, t('1小时缓存写入单价')),
          currency: priceCurrency,
          ...(parsedTiers.length > 0 ? { tiers: parsedTiers } : {}),
        } satisfies ModelPrice
        payload.sameAs = null
      }
      setBusy(true)
      const view = await bridge.saveModelPricing(payload)
      updateCachedDisplay({ currency: view.currency, exchangeRate: view.fx?.rate, fx: view.fx })
      await onSavedPricing()
      toast(t('✅ 已保存 {id} 的计价设置', { id: model.id }))
    } catch (err) {
      setError(err instanceof Error ? err.message : t('保存模型单价失败'))
    } finally {
      setBusy(false)
    }
  }

  async function confirmRemove(): Promise<void> {
    const ok = await shared().wbConfirm?.ask?.({
      title: t('移除自定义模型'),
      html: t('确定移除自定义模型「<strong>{id}</strong>」？这条登记会被<b>直接移除</b>，之后 <code>/v1/models</code> 不再广告它、请求它也会被拒。', { id: esc(model.id) }),
      okText: t('移除'),
      okClass: 'danger',
    })
    if (!ok) return
    await runRowAction(
      rowKeyOf(model),
      () => writeRemoveModel(providerId, model.id),
      customSource.isCustom(providerId) ? t('模型已移除') : t('自定义模型已移除'),
    )
    onRemoved()
  }

  const eff = model.pricing?.effective
  const srcCur = eff?.currency || model.pricing?.currency || 'usd'
  const vis = visibilityOf(model)
  const visText = vis === 'hidden'
    ? t('官方不展示（默认关闭）')
    : vis === 'visible'
      ? t('官方客户端可见')
      : model.visibility
        ? t('上游未声明')
        : t('未提供判据')
  const sourceText = model.source === 'manual'
    ? t('手动登记')
    : !model.source && customSource.isCustom(providerId)
      ? t('自定义提供商')
      : model.source === 'remote'
        ? t('上游远程目录')
        : model.source === 'builtin'
          ? t('内置静态清单')
          : '—'
  const refreshed = formatTime(Number(model.refreshedAt) || 0)
  const removable = model.source === 'manual' || customSource.isCustom(providerId)

  return (
    <>
      <section className='mm-drawer-section'>
        <div className='mm-drawer-section-head'>
          <h3>{t('单价与计价方式')}</h3>
          <Badge variant='outline' shape='tag'>
            {model.pricing?.priced && eff
              ? (eff.input === 0 && eff.output === 0
                ? t('免费')
                : `${formatUnitPrice(eff.input, display.currency, display.exchangeRate, srcCur)} / ${formatUnitPrice(eff.output, display.currency, display.exchangeRate, srcCur)}`)
              : t('未定价')}
          </Badge>
        </div>

        <div className='flex flex-col gap-3'>
          <Field>
            <FieldLabel>{t('生效范围')}</FieldLabel>
            <Select value={targetKey} onValueChange={v => v && setTargetKey(String(v))}>
              <SelectTrigger className='w-full'>
                <SelectValue>{currentScopeLabel}</SelectValue>
              </SelectTrigger>
              <SelectContent>
                {scopeOptions.map(opt => <SelectItem key={opt.value} value={opt.value}>{opt.label}</SelectItem>)}
              </SelectContent>
            </Select>
          </Field>

          <SegmentedControl<PricingMode>
            className='w-full [&_[data-slot=segmented-item]]:flex-1 [&_[data-slot=segmented-item]]:justify-center'
            value={mode}
            onValueChange={setMode}
            aria-label={t('计价方式')}
            options={[
              { value: 'default', label: t('恢复默认') },
              { value: 'custom', label: t('自定义单价') },
              { value: 'same_as', label: t('参考同款') },
            ]}
          />

          {mode === 'custom' ? (
            <div className='flex flex-col gap-2.5'>
              <SegmentedControl<'usd' | 'cny'>
                value={priceCurrency}
                onValueChange={v => setPriceCurrency(v)}
                aria-label={t('计价币种')}
                options={[
                  { value: 'usd', label: t('美元 ($)') },
                  { value: 'cny', label: t('人民币 (¥)') },
                ]}
              />
              <div className='grid grid-cols-2 gap-2.5'>
                <Field>
                  <FieldLabel>{priceCurrency === 'cny' ? t('输入 (¥/M)') : t('输入 ($/M)')}</FieldLabel>
                  <Input type='number' min='0' step='any' value={inputPrice} placeholder='0' onChange={e => setInputPrice(e.currentTarget.value)} />
                </Field>
                <Field>
                  <FieldLabel>{priceCurrency === 'cny' ? t('输出 (¥/M)') : t('输出 ($/M)')}</FieldLabel>
                  <Input type='number' min='0' step='any' value={outputPrice} placeholder='0' onChange={e => setOutputPrice(e.currentTarget.value)} />
                </Field>
                <Field>
                  <FieldLabel>{priceCurrency === 'cny' ? t('缓存读 (¥/M)') : t('缓存读 ($/M)')}</FieldLabel>
                  <Input type='number' min='0' step='any' value={cacheReadPrice} placeholder='0' onChange={e => setCacheReadPrice(e.currentTarget.value)} />
                </Field>
                <Field>
                  <FieldLabel>{priceCurrency === 'cny' ? t('缓存写 5m (¥/M)') : t('缓存写 5m ($/M)')}</FieldLabel>
                  <Input type='number' min='0' step='any' value={cacheWritePrice} placeholder='0' onChange={e => setCacheWritePrice(e.currentTarget.value)} />
                </Field>
                <Field className='col-span-2'>
                  <FieldLabel>{priceCurrency === 'cny' ? t('缓存写 1h (¥/M)') : t('缓存写 1h ($/M)')}</FieldLabel>
                  <Input type='number' min='0' step='any' value={cacheWrite1hPrice} placeholder={t('Claude 自动 2×')} onChange={e => setCacheWrite1hPrice(e.currentTarget.value)} />
                </Field>
              </div>
              <div className='flex items-center justify-between text-xs'>
                <span className='text-muted-foreground'>{t('全填 0 可明确设为免费')}</span>
                <Button size='xs' variant='outline' type='button' onClick={() => setTiers(prev => [...prev, {
                  above: '200000', input: inputPrice || '0', output: outputPrice || '0',
                  cacheRead: cacheReadPrice || '0', cacheWrite: cacheWritePrice || '0', cacheWrite1h: '',
                }])}>{t('＋ 阶梯价')}</Button>
              </div>
              {tiers.length > 0 ? (
                <div className='flex flex-col gap-2 rounded-md border border-border p-2'>
                  {tiers.map((tier, idx) => (
                    <div key={idx} className='grid grid-cols-3 items-end gap-1.5'>
                      <Field>
                        <FieldLabel className='text-[11px]'>{t('超出 Token')}</FieldLabel>
                        <Input type='number' min='1' value={tier.above}
                          onChange={e => { const v = e.currentTarget.value; setTiers(p => p.map((it, i) => i === idx ? { ...it, above: v } : it)) }} />
                      </Field>
                      <Field>
                        <FieldLabel className='text-[11px]'>{t('输入')}</FieldLabel>
                        <Input type='number' min='0' step='any' value={tier.input}
                          onChange={e => { const v = e.currentTarget.value; setTiers(p => p.map((it, i) => i === idx ? { ...it, input: v } : it)) }} />
                      </Field>
                      <div className='flex items-end gap-1'>
                        <Field className='flex-1'>
                          <FieldLabel className='text-[11px]'>{t('输出')}</FieldLabel>
                          <Input type='number' min='0' step='any' value={tier.output}
                            onChange={e => { const v = e.currentTarget.value; setTiers(p => p.map((it, i) => i === idx ? { ...it, output: v } : it)) }} />
                        </Field>
                        <Button size='icon-xs' variant='ghost' type='button' className='mb-1 text-destructive'
                          onClick={() => setTiers(p => p.filter((_, i) => i !== idx))}>×</Button>
                      </div>
                    </div>
                  ))}
                </div>
              ) : null}
            </div>
          ) : null}

          {mode === 'same_as' ? (
            <Field>
              <FieldLabel>{t('参考的官方模型 ID')}</FieldLabel>
              <Input list='wb-drawer-pricing-refs' value={sameAs}
                placeholder={t('例如：claude-sonnet-4-5 或 gpt-5')}
                onChange={e => setSameAs(e.currentTarget.value)} />
              <datalist id='wb-drawer-pricing-refs'>
                {(pricingSettings?.referenceModels || []).map(item => <option key={item} value={item} />)}
              </datalist>
            </Field>
          ) : null}

          {error ? <p className='text-xs text-destructive'>{error}</p> : null}
          <div className='flex justify-end'>
            <Button size='sm' disabled={busy} onClick={() => void savePricing()}>
              {busy ? t('保存中…') : t('保存计价设置')}
            </Button>
          </div>
        </div>
      </section>

      <section className='mm-drawer-section'>
        <div className='mm-drawer-section-head'>
          <h3>{t('模型元信息')}</h3>
        </div>
        <dl className='mm-meta-dl'>
          <div><dt>{t('计价来源')}</dt><dd>{model.pricing?.priced ? pricingSourceLabel(model.pricing.source) : t('未定价')}</dd></div>
          <div><dt>{t('官方可见性')}</dt><dd>{visText}</dd></div>
          <div><dt>{t('清单来源')}</dt><dd>{sourceText}</dd></div>
          <div><dt>{t('积分倍率')}</dt><dd className='font-mono'>{model.credits ? String(model.credits) : '—'}</dd></div>
          {refreshed ? <div><dt>{t('最近同步')}</dt><dd>{refreshed}</dd></div> : null}
          {model.upstreamStatus === 'undeployed' ? (
            <div><dt>{t('上游状态')}</dt><dd className='text-warning'>{t('上游未部署')}</dd></div>
          ) : null}
        </dl>
      </section>

      {removable ? (
        <section className='mm-drawer-section'>
          <div className='flex items-center justify-between gap-3 rounded-md border border-destructive-bd bg-destructive-soft px-3 py-2.5'>
            <div className='min-w-0'>
              <div className='text-xs font-semibold text-destructive'>{t('移除此模型登记')}</div>
              <div className='text-[11px] text-subtle'>{t('移除后不再出现在接口列表，请求也会被拒绝')}</div>
            </div>
            <Button variant='destructive' size='sm' onClick={() => void confirmRemove()}>{t('移除')}</Button>
          </div>
        </section>
      ) : null}
    </>
  )
}
