import { useEffect, useState } from 'react'
import {
  fetchModelPricing,
  saveModelPricing,
  type ModelPricingSettingsData,
  type RateWindow,
} from '../core/api-features'
import {
  refreshMobileFxRate,
  switchPricingCurrency,
  syncPricingDisplay,
  usePricingDisplay,
  type ModelPricingInfo,
} from '../core/pricing'
import { store, useStore } from '../core/store'
import {
  BottomSheet,
  CommonCard,
  ConnectedButtonGroup,
  SectionHeader,
  TonalChip,
} from '../ui/components'

type PricingTab = 'general' | 'model' | 'provider'
type ModelMode = 'default' | 'custom' | 'same_as'

const WEEKDAYS = ['日', '一', '二', '三', '四', '五', '六']

export function PricingSheet({
  initialProvider,
  initialModel,
  initialPricing,
  onClose,
}: {
  initialProvider?: string
  initialModel?: string
  initialPricing?: ModelPricingInfo | null
  onClose: () => void
}) {
  const { session } = useStore()
  const display = usePricingDisplay()
  const [tab, setTab] = useState<PricingTab>(initialModel ? 'model' : 'general')
  const [data, setData] = useState<ModelPricingSettingsData | null>(null)
  const [busy, setBusy] = useState(false)
  const [fxBusy, setFxBusy] = useState(false)

  // 模型覆盖表单
  const [targetKey, setTargetKey] = useState(() =>
    initialModel ? `${initialProvider || '*'}/${initialModel}` : '',
  )
  const [mode, setMode] = useState<ModelMode>(() =>
    initialPricing?.source === 'same_as' ? 'same_as' : 'custom',
  )
  const [inputPrice, setInputPrice] = useState('')
  const [outputPrice, setOutputPrice] = useState('')
  const [cacheReadPrice, setCacheReadPrice] = useState('')
  const [cacheWritePrice, setCacheWritePrice] = useState('')
  const [sameAs, setSameAs] = useState('')

  // 提供商倍率表单
  const providerList = (session?.accounts?.providers || [])
    .map(p => ({ id: p.id || '', label: p.label || p.id || '' }))
    .filter(p => p.id)
  const [providerId, setProviderId] = useState(
    () => initialProvider || providerList[0]?.id || 'workbuddy',
  )
  const [priceRate, setPriceRate] = useState('')
  const [windows, setWindows] = useState<RateWindow[]>([])

  const applyLoaded = (res: ModelPricingSettingsData) => {
    setData(res)
    syncPricingDisplay({
      currency: res.currency,
      exchangeRate: res.fx?.rate,
      fx: res.fx,
    })
    if (targetKey && res.modelPrices?.[targetKey]) {
      const p = res.modelPrices[targetKey]
      setMode('custom')
      setInputPrice(String(p.input))
      setOutputPrice(String(p.output))
      setCacheReadPrice(p.cacheRead ? String(p.cacheRead) : '')
      setCacheWritePrice(p.cacheWrite ? String(p.cacheWrite) : '')
    } else if (targetKey && res.modelSameAs?.[targetKey]) {
      setMode('same_as')
      setSameAs(res.modelSameAs[targetKey])
    } else if (initialPricing?.base) {
      setInputPrice(String(initialPricing.base.input))
      setOutputPrice(String(initialPricing.base.output))
      setCacheReadPrice(initialPricing.base.cacheRead ? String(initialPricing.base.cacheRead) : '')
      setCacheWritePrice(initialPricing.base.cacheWrite ? String(initialPricing.base.cacheWrite) : '')
    }
  }

  useEffect(() => {
    void fetchModelPricing()
      .then(applyLoaded)
      .catch(err => store.toast(err instanceof Error ? err.message : '加载计价设置失败', 'danger'))
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  useEffect(() => {
    if (!data) return
    const rule = data.providerPricing?.[providerId]
    setPriceRate(rule?.priceRate && rule.priceRate > 0 ? String(rule.priceRate) : '')
    setWindows(rule?.rateWindows ? rule.rateWindows.map(w => ({ ...w, days: [...(w.days || [])] })) : [])
  }, [data, providerId])

  const loadKeyDraft = (key: string) => {
    setTargetKey(key)
    const custom = data?.modelPrices?.[key]
    const ref = data?.modelSameAs?.[key]
    if (custom) {
      setMode('custom')
      setInputPrice(String(custom.input))
      setOutputPrice(String(custom.output))
      setCacheReadPrice(custom.cacheRead ? String(custom.cacheRead) : '')
      setCacheWritePrice(custom.cacheWrite ? String(custom.cacheWrite) : '')
      setTab('model')
    } else if (ref) {
      setMode('same_as')
      setSameAs(ref)
      setTab('model')
    }
  }

  const handleSaveModel = async () => {
    const key = targetKey.trim()
    if (!key || !key.includes('/')) {
      store.toast('目标格式应为 提供商/模型名（例如 */gpt-5 或 workbuddy/*）', 'danger')
      return
    }
    setBusy(true)
    try {
      const payload: Record<string, unknown> = { action: 'setModelPrice', key }
      if (mode === 'default') {
        payload.price = null
        payload.sameAs = null
      } else if (mode === 'same_as') {
        if (!sameAs.trim()) {
          store.toast('请输入要参考的模型名称', 'danger')
          return
        }
        payload.price = null
        payload.sameAs = sameAs.trim()
      } else {
        const parseNonNeg = (v: string) => {
          const t = v.trim()
          if (!t) return 0
          const n = Number(t)
          if (!Number.isFinite(n) || n < 0) throw new Error('单价必须是非负数字')
          return n
        }
        payload.price = {
          input: parseNonNeg(inputPrice),
          output: parseNonNeg(outputPrice),
          cacheRead: parseNonNeg(cacheReadPrice),
          cacheWrite: parseNonNeg(cacheWritePrice),
        }
        payload.sameAs = null
      }
      const next = await saveModelPricing(payload)
      applyLoaded(next)
      void store.refresh(true)
      store.toast('已更新模型单价设置', 'success')
    } catch (err) {
      store.toast(err instanceof Error ? err.message : '保存失败', 'danger')
    } finally {
      setBusy(false)
    }
  }

  const handleSaveProvider = async () => {
    if (!providerId.trim()) return
    setBusy(true)
    try {
      const rateNum = priceRate.trim() ? Number(priceRate.trim()) : null
      if (rateNum !== null && (!Number.isFinite(rateNum) || rateNum <= 0 || rateNum > 1000)) {
        throw new Error('整体倍率必须在 0 到 1000 之间')
      }
      const next = await saveModelPricing({
        action: 'setProviderPricing',
        provider: providerId.trim(),
        priceRate: rateNum,
        rateWindows: windows,
      })
      applyLoaded(next)
      void store.refresh(true)
      store.toast('已更新提供商倍率设置', 'success')
    } catch (err) {
      store.toast(err instanceof Error ? err.message : '保存失败', 'danger')
    } finally {
      setBusy(false)
    }
  }

  const handleRefreshFx = async () => {
    setFxBusy(true)
    try {
      await refreshMobileFxRate()
      const fresh = await fetchModelPricing().catch(() => null)
      if (fresh) applyLoaded(fresh)
      store.toast('汇率已刷新', 'success')
    } catch (err) {
      store.toast(err instanceof Error ? err.message : '刷新汇率失败', 'danger')
    } finally {
      setFxBusy(false)
    }
  }

  const overrides = Object.entries(data?.modelPrices || {})
  const sameAsList = Object.entries(data?.modelSameAs || {})

  return (
    <BottomSheet
      open
      title='模型计价与汇率'
      onClose={onClose}
      fixedHeader={
        <ConnectedButtonGroup
          value={tab}
          onChange={setTab}
          options={[
            { id: 'general', label: '货币与汇率' },
            { id: 'model', label: '模型单价' },
            { id: 'provider', label: '提供商倍率' },
          ]}
        />
      }
    >
      {tab === 'general' ? (
        <>
          <SectionHeader title='展示货币' />
          <CommonCard>
            <ConnectedButtonGroup
              value={display.currency}
              onChange={v => void switchPricingCurrency(v)}
              options={[
                { id: 'usd', label: '美元 USD ($)' },
                { id: 'cny', label: '人民币 CNY (¥)' },
              ]}
            />
            <div className='fc-inner-block' style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', gap: 10 }}>
              <div>
                <div style={{ fontSize: 13.5, fontWeight: 600 }}>1 USD ≈ ¥{display.exchangeRate.toFixed(4)} CNY</div>
                <div style={{ fontSize: 11.5, color: 'var(--fc-text-secondary)' }}>
                  {display.fx?.fallback ? '当前使用离线默认汇率 7.2' : '每 12 小时自动同步实时汇率'}
                </div>
              </div>
              <button
                type='button'
                className='fc-btn is-tonal is-sm is-pill'
                disabled={fxBusy}
                onClick={() => void handleRefreshFx()}
              >
                {fxBusy ? '刷新中…' : '刷新汇率'}
              </button>
            </div>
          </CommonCard>

          {(overrides.length > 0 || sameAsList.length > 0) ? (
            <>
              <SectionHeader title='已自定义的模型计价' />
              <CommonCard>
                {overrides.map(([key, price]) => (
                  <div
                    key={key}
                    className='fc-inner-block'
                    style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', gap: 8, cursor: 'pointer' }}
                    onClick={() => loadKeyDraft(key)}
                  >
                    <div style={{ minWidth: 0 }}>
                      <div className='fc-mono' style={{ fontSize: 12.5, fontWeight: 700 }}>{key}</div>
                      <div style={{ fontSize: 11.5, color: 'var(--fc-text-secondary)' }}>
                        输入 ${price.input}/M · 输出 ${price.output}/M
                      </div>
                    </div>
                    <TonalChip tone='primary'>自定义</TonalChip>
                  </div>
                ))}
                {sameAsList.map(([key, target]) => (
                  <div
                    key={key}
                    className='fc-inner-block'
                    style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', gap: 8, cursor: 'pointer' }}
                    onClick={() => loadKeyDraft(key)}
                  >
                    <div style={{ minWidth: 0 }}>
                      <div className='fc-mono' style={{ fontSize: 12.5, fontWeight: 700 }}>{key}</div>
                      <div style={{ fontSize: 11.5, color: 'var(--fc-text-secondary)' }}>参考 {target}</div>
                    </div>
                    <TonalChip tone='neutral'>参考</TonalChip>
                  </div>
                ))}
              </CommonCard>
            </>
          ) : null}
        </>
      ) : null}

      {tab === 'model' ? (
        <CommonCard>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
            <label style={{ fontSize: 12.5, fontWeight: 600 }}>
              规则范围（如 <span className='fc-mono'>provider/model</span> 或 <span className='fc-mono'>*/model</span>）
              <input
                className='fc-input fc-mono'
                value={targetKey}
                placeholder='workbuddy/claude-sonnet-4-5'
                onChange={e => setTargetKey(e.currentTarget.value)}
                style={{ marginTop: 6 }}
              />
            </label>
            <ConnectedButtonGroup
              value={mode}
              onChange={setMode}
              options={[
                { id: 'custom', label: '自定义单价' },
                { id: 'same_as', label: '参考模型' },
                { id: 'default', label: '清除规则' },
              ]}
            />
            {mode === 'custom' ? (
              <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 8 }}>
                <label style={{ fontSize: 12, fontWeight: 600 }}>
                  输入 ($/M)
                  <input className='fc-input' type='number' min='0' step='any' value={inputPrice} placeholder='0'
                    onChange={e => setInputPrice(e.currentTarget.value)} style={{ marginTop: 4 }} />
                </label>
                <label style={{ fontSize: 12, fontWeight: 600 }}>
                  输出 ($/M)
                  <input className='fc-input' type='number' min='0' step='any' value={outputPrice} placeholder='0'
                    onChange={e => setOutputPrice(e.currentTarget.value)} style={{ marginTop: 4 }} />
                </label>
                <label style={{ fontSize: 12, fontWeight: 600 }}>
                  缓存读 ($/M)
                  <input className='fc-input' type='number' min='0' step='any' value={cacheReadPrice} placeholder='0'
                    onChange={e => setCacheReadPrice(e.currentTarget.value)} style={{ marginTop: 4 }} />
                </label>
                <label style={{ fontSize: 12, fontWeight: 600 }}>
                  缓存写 ($/M)
                  <input className='fc-input' type='number' min='0' step='any' value={cacheWritePrice} placeholder='0'
                    onChange={e => setCacheWritePrice(e.currentTarget.value)} style={{ marginTop: 4 }} />
                </label>
              </div>
            ) : null}
            {mode === 'same_as' ? (
              <label style={{ fontSize: 12.5, fontWeight: 600 }}>
                参考的官方模型 ID
                <input
                  className='fc-input fc-mono'
                  value={sameAs}
                  placeholder='claude-sonnet-4-5'
                  onChange={e => setSameAs(e.currentTarget.value)}
                  style={{ marginTop: 6 }}
                />
              </label>
            ) : null}
            <button
              type='button'
              className='fc-btn is-primary'
              disabled={busy}
              onClick={() => void handleSaveModel()}
            >
              {busy ? '保存中…' : '保存模型计价'}
            </button>
          </div>
        </CommonCard>
      ) : null}

      {tab === 'provider' ? (
        <CommonCard>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
            <label style={{ fontSize: 12.5, fontWeight: 600 }}>
              提供商
              <select
                className='fc-input'
                value={providerId}
                onChange={e => setProviderId(e.currentTarget.value)}
                style={{ marginTop: 6 }}
              >
                {providerList.map(p => (
                  <option key={p.id} value={p.id}>{p.label} ({p.id})</option>
                ))}
              </select>
            </label>
            <label style={{ fontSize: 12.5, fontWeight: 600 }}>
              整体价格倍率（留空为 1× 原价）
              <input
                className='fc-input'
                type='number'
                min='0.001'
                max='1000'
                step='any'
                value={priceRate}
                placeholder='1.0'
                onChange={e => setPriceRate(e.currentTarget.value)}
                style={{ marginTop: 6 }}
              />
            </label>
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
              <span style={{ fontSize: 12.5, fontWeight: 600 }}>分时倍率时段</span>
              {windows.length < 8 ? (
                <button
                  type='button'
                  className='fc-btn is-tonal is-sm is-pill'
                  onClick={() => setWindows(prev => [...prev, { days: [], start: '00:00', end: '08:00', rate: 0.5 }])}
                >
                  ＋ 添加时段
                </button>
              ) : null}
            </div>
            {windows.map((w, idx) => (
              <div key={idx} className='fc-inner-block' style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', flexWrap: 'wrap', gap: 4 }}>
                  <div style={{ display: 'flex', gap: 4, flexWrap: 'wrap' }}>
                    {WEEKDAYS.map((label, dayIdx) => {
                      const active = (w.days || []).includes(dayIdx)
                      return (
                        <button
                          key={dayIdx}
                          type='button'
                          className={`fc-btn is-sm is-pill ${active ? 'is-primary' : ''}`}
                          onClick={() => setWindows(prev => prev.map((it, i) => {
                            if (i !== idx) return it
                            const cur = it.days || []
                            const nextDays = cur.includes(dayIdx) ? cur.filter(d => d !== dayIdx) : [...cur, dayIdx].sort()
                            return { ...it, days: nextDays }
                          }))}
                        >
                          {label}
                        </button>
                      )
                    })}
                  </div>
                  <button
                    type='button'
                    className='fc-btn is-sm is-danger is-pill'
                    onClick={() => setWindows(prev => prev.filter((_, i) => i !== idx))}
                  >
                    移除
                  </button>
                </div>
                <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr 1fr', gap: 6 }}>
                  <input className='fc-input fc-mono' value={w.start} placeholder='09:00'
                    onChange={e => setWindows(prev => prev.map((it, i) => i === idx ? { ...it, start: e.currentTarget.value } : it))} />
                  <input className='fc-input fc-mono' value={w.end} placeholder='18:00'
                    onChange={e => setWindows(prev => prev.map((it, i) => i === idx ? { ...it, end: e.currentTarget.value } : it))} />
                  <input className='fc-input fc-mono' type='number' min='0.001' max='1000' step='any' value={String(w.rate)}
                    onChange={e => setWindows(prev => prev.map((it, i) => i === idx ? { ...it, rate: Number(e.currentTarget.value) || 1 } : it))} />
                </div>
              </div>
            ))}
            <button
              type='button'
              className='fc-btn is-primary'
              disabled={busy}
              onClick={() => void handleSaveProvider()}
            >
              {busy ? '保存中…' : '保存提供商规则'}
            </button>
          </div>
        </CommonCard>
      ) : null}
    </BottomSheet>
  )
}
