import { useEffect, useMemo, useState } from 'react'
import {
  fetchManageModels,
  refreshModelsCatalog,
  setModelEnabled,
  type ManageModel,
  type ManageView,
} from '../core/api'
import { copyText, providerDisplayName, syncProviderLabels } from '../core/format'
import { t } from '../core/i18n'
import { formatUnitPrice, pricingSourceLabel, syncPricingDisplay, usePricingDisplay } from '../core/pricing'
import { serverIdentity } from '../core/servers'
import { store, useStore } from '../core/store'
import {
  CommonScaffold,
  CommonSwitch,
  EmptyBlock,
  RecordCard,
  SearchField,
  TonalChip,
  TonalIconButton,
} from '../ui/components'
import { GlyphIcon, ProviderAvatar } from '../ui/icons'

export function ModelsView() {
  const { server, revision, navTabs } = useStore()
  const pricing = usePricingDisplay()
  const [manage, setManage] = useState<ManageView | null>(null)
  const [keyword, setKeyword] = useState('')
  const [refreshing, setRefreshing] = useState(false)

  const loadModels = async (signal?: AbortSignal) => {
    const id = serverIdentity()
    if (!server?.base) return
    try {
      const res = await fetchManageModels(signal)
      if (serverIdentity() !== id) return
      syncProviderLabels(res.providers)
      const extra = res as {
        pricingSettings?: {
          currency?: 'usd' | 'cny'
          fx?: { rate: number; updatedAt: number; stale: boolean; fallback: boolean }
        }
      }
      if (extra?.pricingSettings) {
        syncPricingDisplay({
          currency: extra.pricingSettings.currency,
          exchangeRate: extra.pricingSettings.fx?.rate,
          fx: extra.pricingSettings.fx,
        })
      }
      setManage(res)
    } catch {
      /* ignore */
    }
  }

  useEffect(() => {
    const ac = new AbortController()
    void loadModels(ac.signal)
    return () => ac.abort()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [revision, server?.base])

  const models = manage?.models || []

  const filteredModels = useMemo(() => {
    const q = keyword.trim().toLowerCase()
    if (!q) return models
    return models.filter(m => {
      const id = (m.id || m.name || '').toLowerCase()
      const prov = providerDisplayName(m.provider, m.providerLabel).toLowerCase()
      const aliases = (m.aliases || []).join(' ').toLowerCase()
      return id.includes(q) || prov.includes(q) || aliases.includes(q)
    })
  }, [models, keyword])

  const handleRefreshCatalog = async () => {
    if (refreshing) return
    setRefreshing(true)
    try {
      await refreshModelsCatalog()
      store.toast(t('模型目录已刷新'), 'success')
      await loadModels()
      void store.refresh(true)
    } catch (err) {
      store.toast(err instanceof Error ? err.message : t('刷新目录失败'), 'danger')
    } finally {
      setRefreshing(false)
    }
  }

  const handleToggleModel = async (m: ManageModel, next: boolean) => {
    const id = m.id || m.name || ''
    const provider = m.provider || ''
    if (!id) return
    setManage(prev =>
      prev
        ? {
            ...prev,
            models: (prev.models || []).map(item =>
              item.id === m.id && item.provider === m.provider ? { ...item, enabled: next } : item,
            ),
          }
        : prev,
    )
    try {
      const updated = await setModelEnabled({ id, provider, enabled: next })
      if (updated && typeof updated === 'object' && 'models' in updated) {
        setManage(updated as ManageView)
      }
      store.toast(next ? t('已启用模型 {id}', { id }) : t('已停用模型 {id}', { id }), 'success')
    } catch (err) {
      store.toast(err instanceof Error ? err.message : t('修改模型状态失败'), 'danger')
      void loadModels()
    }
  }

  const stickyHeader = (
    <>
      <SearchField
        value={keyword}
        onChange={setKeyword}
        placeholder={t('搜索模型 ID、别名或提供商…')}
      />
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          padding: '0 4px',
          fontSize: 12.5,
          color: 'var(--fc-text-secondary)',
        }}
      >
        <span>{t('共 {n} 个模型', { n: filteredModels.length })}</span>
        <button
          type='button'
          style={{
            border: 'none',
            background: 'transparent',
            color: 'var(--fc-primary)',
            fontWeight: 600,
            cursor: 'pointer',
            padding: 0,
          }}
          onClick={() => store.openSheet({ kind: 'model-mappings' })}
        >
          {t('别名映射 ({n})', { n: manage?.mappings?.length || 0 })} →
        </button>
      </div>
    </>
  )

  return (
    <CommonScaffold
      title={t('模型')}
      onBack={navTabs.includes('models') ? undefined : () => store.returnFromSubPage()}
      stickyHeader={stickyHeader}
      actions={
        <>
          <TonalIconButton
            icon='dollar'
            label={t('计价')}
            onClick={() => store.openSheet({ kind: 'pricing' })}
          />
          <TonalIconButton
            icon='route'
            label={t('映射')}
            onClick={() => store.openSheet({ kind: 'model-mappings' })}
          />
          <TonalIconButton
            icon='refresh'
            disabled={refreshing}
            onClick={handleRefreshCatalog}
            title={t('刷新模型目录')}
          />
        </>
      }
    >
      {filteredModels.length === 0 ? (
        <EmptyBlock
          icon='cube'
          title={models.length === 0 ? t('暂无可管理模型') : t('没有找到匹配的模型')}
          description={t('请确认已添加账号并点击右上角刷新目录')}
        />
      ) : (
        filteredModels.slice(0, 120).map((m, idx) => {
          const id = m.id || m.name || `model-${idx}`
          const enabled = m.enabled !== false
          const p = m.pricing
          const eff = p?.effective || p?.base
          const srcCurrency = eff?.currency || p?.currency || 'usd'
          const priceText =
            p?.priced && eff
              ? eff.input === 0 && eff.output === 0
                ? t('免费')
                : `${formatUnitPrice(eff.input, pricing.currency, pricing.exchangeRate, srcCurrency)} / ${formatUnitPrice(eff.output, pricing.currency, pricing.exchangeRate, srcCurrency)}`
              : t('未定价')
          const sourceText = p?.priced ? pricingSourceLabel(p.source) : null
          return (
            <RecordCard key={`${m.provider || 'p'}-${id}-${idx}`}>
              <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 10 }}>
                <div style={{ display: 'flex', alignItems: 'center', gap: 10, minWidth: 0, flex: 1 }}>
                  <ProviderAvatar provider={m.provider} label={m.providerLabel} size={34} />
                  <div style={{ minWidth: 0, flex: 1 }}>
                    <div
                      className='fc-mono'
                      style={{
                        fontSize: 13.5,
                        fontWeight: 700,
                        overflow: 'hidden',
                        textOverflow: 'ellipsis',
                        whiteSpace: 'nowrap',
                      }}
                    >
                      {id}
                    </div>
                    <div style={{ fontSize: 12, color: 'var(--fc-text-secondary)', marginTop: 2 }}>
                      {providerDisplayName(m.provider, m.providerLabel)}
                      {m.aliases?.length ? ` · ${t('别名')} ${m.aliases.join(', ')}` : ''}
                    </div>
                  </div>
                </div>
                <CommonSwitch checked={enabled} onChange={next => void handleToggleModel(m, next)} />
              </div>

              <div
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'space-between',
                  gap: 8,
                  flexWrap: 'wrap',
                }}
              >
                <div style={{ display: 'flex', alignItems: 'center', gap: 6, flexWrap: 'wrap' }}>
                  <button
                    type='button'
                    className='fc-btn is-sm is-pill'
                    style={{ background: p?.priced ? 'var(--fc-inset)' : 'transparent' }}
                    title={sourceText ? `${sourceText} · ${priceText}/M` : t('设置模型计价')}
                    onClick={() =>
                      store.openSheet({
                        kind: 'pricing',
                        provider: m.provider || '',
                        model: id,
                        pricing: p || null,
                      })
                    }
                  >
                    <GlyphIcon name='dollar' size={13} />
                    <span className='fc-mono'>{priceText}</span>
                    {p?.inPeak ? <span>{t('Peak')}</span> : null}
                  </button>
                  {m.supports_image_generation ? <TonalChip tone='primary'>{t('生图')}</TonalChip> : null}
                </div>
                <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                  <button
                    type='button'
                    className='fc-btn is-sm is-pill'
                    onClick={async () => {
                      const ok = await copyText(id)
                      store.toast(ok ? t('已复制模型 ID') : t('复制失败'), ok ? 'success' : 'danger')
                    }}
                  >
                    <GlyphIcon name='copy' size={13} />
                    <span>{t('复制')}</span>
                  </button>
                  <button
                    type='button'
                    className='fc-btn is-tonal is-sm is-pill'
                    onClick={() => store.openSheet({ kind: 'model-test', model: m })}
                  >
                    <GlyphIcon name='bolt' size={13} />
                    <span>{t('测试')}</span>
                  </button>
                </div>
              </div>
            </RecordCard>
          )
        })
      )}
    </CommonScaffold>
  )
}
