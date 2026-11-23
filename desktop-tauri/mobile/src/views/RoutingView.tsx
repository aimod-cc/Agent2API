import { useCallback, useEffect, useState } from 'react'
import {
  fetchRoutingConfig,
  fetchRoutingQueue,
  saveRoutingConfig,
  type RouteQueuePayload,
  type RouteSettings,
} from '../core/api'
import { formatRelativeTime, providerDisplayName } from '../core/format'
import { t } from '../core/i18n'
import { serverIdentity } from '../core/servers'
import { store, useStore } from '../core/store'
import {
  CommonCard,
  CommonScaffold,
  CommonSwitch,
  DashboardCard,
  EmptyBlock,
  ListGroup,
  ListItem,
  SectionHeader,
  SegmentedControl,
  TonalChip,
  TonalIconButton,
} from '../ui/components'
import { ProviderAvatar } from '../ui/icons'
import { PickerSheet, PickerTrigger, type PickerOption } from '../ui/pickers'
import { RoutingErrorsPanel } from './RoutingErrorsPanel'
import { RoutingStickySheet } from './RoutingStickySheet'

type RoutingTab = 'strategy' | 'queue' | 'errors'

const STRATEGY_OPTIONS: ReadonlyArray<PickerOption> = [
  { value: 'order', label: '顺序优先（按账号优先级）', subtitle: '优先使用排在前面的健康账号，最稳定' },
  { value: 'rotate', label: '全局轮询（所有账号轮转）', subtitle: '按请求依次轮流分发到不同账号' },
  { value: 'provider_rotate', label: '提供商内轮询（家内轮转）', subtitle: '先定提供商顺序，再在同家账号内轮转' },
]

const STICKY_OPTIONS: ReadonlyArray<PickerOption> = [
  { value: 'auto', label: '自动识别会话粘性', subtitle: '自动识别多轮对话并尽量固定同一账号' },
  { value: 'session', label: '整段会话固定账号', subtitle: '同一对话期间优先始终绑定同一个账号' },
  { value: 'turn', label: '单轮工具链内固定', subtitle: '同一轮工具调用内固定账号，下轮重新选' },
  { value: 'off', label: '关闭粘性（逐请求选路）', subtitle: '每次请求都重新按策略选号' },
]

export function RoutingView() {
  const { server, revision, navTabs } = useStore()
  const [tab, setTab] = useState<RoutingTab>('strategy')
  const [routing, setRouting] = useState<RouteSettings | null>(null)
  const [queue, setQueue] = useState<RouteQueuePayload | null>(null)
  const [queueModel, setQueueModel] = useState('')
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [picker, setPicker] = useState<'strategy' | 'sticky' | null>(null)
  const [stickySheetOpen, setStickySheetOpen] = useState(false)
  const [errorsNonce, setErrorsNonce] = useState(0)

  const [breakerThreshold, setBreakerThreshold] = useState('3')
  const [breakerTimeout, setBreakerTimeout] = useState('60')
  const [breakerSuccess, setBreakerSuccess] = useState('1')

  const loadRouting = useCallback(async (signal?: AbortSignal) => {
    const id = serverIdentity()
    if (!server?.base) return
    setLoading(true)
    try {
      const [cfg, q] = await Promise.all([
        fetchRoutingConfig(signal),
        fetchRoutingQueue(queueModel.trim() || undefined, signal),
      ])
      if (serverIdentity() !== id) return
      setRouting(cfg)
      setQueue(q)
      if (cfg.routingBreakerThreshold !== undefined) setBreakerThreshold(String(cfg.routingBreakerThreshold))
      if (cfg.routingBreakerTimeoutSeconds !== undefined) setBreakerTimeout(String(cfg.routingBreakerTimeoutSeconds))
      if (cfg.routingBreakerSuccessThreshold !== undefined) setBreakerSuccess(String(cfg.routingBreakerSuccessThreshold))
    } catch (err) {
      if (!signal?.aborted) {
        store.toast(err instanceof Error ? err.message : t('加载路由配置失败'), 'danger')
      }
    } finally {
      if (!signal?.aborted) setLoading(false)
    }
  }, [queueModel, server?.base])

  useEffect(() => {
    const ac = new AbortController()
    void loadRouting(ac.signal)
    return () => ac.abort()
  }, [loadRouting, revision])

  const patchSetting = async (patch: Partial<RouteSettings>, okMsg = t('路由策略已保存')) => {
    if (saving) return
    setSaving(true)
    try {
      const saved = await saveRoutingConfig(patch)
      setRouting(saved)
      const freshQueue = await fetchRoutingQueue(queueModel.trim() || undefined).catch(() => null)
      if (freshQueue) setQueue(freshQueue)
      store.toast(okMsg, 'success')
    } catch (err) {
      store.toast(err instanceof Error ? err.message : t('保存策略失败'), 'danger')
    } finally {
      setSaving(false)
    }
  }

  const saveBreaker = () => {
    void patchSetting(
      {
        routingBreakerThreshold: Number(breakerThreshold) || 3,
        routingBreakerTimeoutSeconds: Number(breakerTimeout) || 60,
        routingBreakerSuccessThreshold: Number(breakerSuccess) || 1,
      },
      t('熔断器参数已保存'),
    )
  }

  const currentStrategy = routing?.routingStrategy || 'order'
  const currentSticky = routing?.routingSticky || 'auto'
  const strategyLabel =
    STRATEGY_OPTIONS.find(o => o.value === currentStrategy)?.label || currentStrategy
  const stickyLabel =
    STICKY_OPTIONS.find(o => o.value === currentSticky)?.label || currentSticky

  const stickyHeader = (
    <>
      <SegmentedControl<RoutingTab>
        value={tab}
        onChange={setTab}
        options={[
          { id: 'strategy', label: t('调度策略') },
          { id: 'queue', label: t('候选队列') },
          { id: 'errors', label: t('错误处理') },
        ]}
      />
      {tab === 'queue' ? (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
          <div style={{ display: 'flex', gap: 8 }}>
            <input
              type='text'
              className='fc-input fc-mono'
              style={{ height: 40, fontSize: 13 }}
              value={queueModel}
              onChange={e => setQueueModel(e.target.value)}
              placeholder={t('输入模型名预演候选队列（留空查全局）')}
            />
            <button
              type='button'
              className='fc-btn is-primary is-sm'
              style={{ height: 40, flex: 'none' }}
              onClick={() => void loadRouting()}
            >
              {t('预演')}
            </button>
          </div>
          {queue?.lastRequestModel && !queueModel ? (
            <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '0 4px' }}>
              <span style={{ fontSize: 12, color: 'var(--fc-text-secondary)' }}>
                {t('最近请求模型')}：<span className='fc-mono'>{queue.lastRequestModel}</span>
              </span>
              <button
                type='button'
                style={{
                  border: 'none',
                  background: 'transparent',
                  color: 'var(--fc-primary)',
                  fontSize: 12,
                  fontWeight: 600,
                  cursor: 'pointer',
                  padding: 0,
                }}
                onClick={() => setQueueModel(queue.lastRequestModel || '')}
              >
                {t('按此模型预演')}
              </button>
            </div>
          ) : null}
        </div>
      ) : null}
    </>
  )

  return (
    <CommonScaffold
      title={t('路由')}
      onBack={navTabs.includes('routing') ? undefined : () => store.returnFromSubPage()}
      stickyHeader={stickyHeader}
      actions={
        <>
          <TonalIconButton
            icon='sliders'
            label={t('参数')}
            onClick={() => store.openSheet({ kind: 'forwarding-config' })}
          />
          <TonalIconButton
            icon='refresh'
            disabled={loading}
            onClick={() => {
              if (tab === 'errors') setErrorsNonce(n => n + 1)
              else void loadRouting()
            }}
            title={t('刷新')}
          />
        </>
      }
    >
      {tab === 'strategy' ? (
        loading && !routing ? (
          <EmptyBlock icon='route' title={t('正在加载路由设置…')} />
        ) : (
          <>
            <CommonCard>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
                <div>
                  <div style={{ fontSize: 12.5, color: 'var(--fc-text-secondary)', fontWeight: 600, marginBottom: 6 }}>
                    {t('账号选路策略')}
                  </div>
                  <PickerTrigger
                    valueLabel={t(strategyLabel)}
                    disabled={saving}
                    onClick={() => setPicker('strategy')}
                    ariaLabel={t('账号选路策略')}
                  />
                </div>

                <div>
                  <div style={{ fontSize: 12.5, color: 'var(--fc-text-secondary)', fontWeight: 600, marginBottom: 6 }}>
                    {t('会话粘性（保持同一账号）')}
                  </div>
                  <PickerTrigger
                    valueLabel={t(stickyLabel)}
                    disabled={saving}
                    onClick={() => setPicker('sticky')}
                    ariaLabel={t('会话粘性')}
                  />
                </div>
              </div>
            </CommonCard>

            <ListGroup>
              <ListItem
                icon='history'
                title={t('查看活跃粘性会话')}
                subtitle={t('可检查或解除当前已绑定账号的会话')}
                chevron
                onClick={() => setStickySheetOpen(true)}
              />
            </ListGroup>

            <SectionHeader title={t('容错与保护')} />
            <ListGroup>
              <ListItem
                icon='shield'
                title={t('失败退避冷却')}
                subtitle={t('请求报错后按退避时长短暂跳过该账号')}
                trailing={
                  <CommonSwitch
                    checked={Boolean(routing?.routingFailoverBackoff)}
                    disabled={saving}
                    onChange={next => void patchSetting({ routingFailoverBackoff: next })}
                  />
                }
              />
              <ListItem
                icon='bolt'
                title={t('报错账号自动下沉')}
                subtitle={t('异常账号自动排到候选队尾，优先使用健康账号')}
                trailing={
                  <CommonSwitch
                    checked={Boolean(routing?.routingSink)}
                    disabled={saving}
                    onChange={next => void patchSetting({ routingSink: next })}
                  />
                }
              />
              <ListItem
                icon='alert'
                title={t('连续失败熔断保护')}
                subtitle={t('达到阈值后隔离保护，到期后半开探测恢复')}
                trailing={
                  <CommonSwitch
                    checked={Boolean(routing?.routingBreakerEnabled)}
                    disabled={saving}
                    onChange={next => void patchSetting({ routingBreakerEnabled: next })}
                  />
                }
              />
            </ListGroup>

            {routing?.routingBreakerEnabled ? (
              <CommonCard>
                <div style={{ fontSize: 14, fontWeight: 700, marginBottom: 10 }}>{t('熔断器阈值设置')}</div>
                <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr 1fr', gap: 8 }}>
                  <div>
                    <div style={{ fontSize: 11.5, color: 'var(--fc-text-secondary)', marginBottom: 4 }}>{t('连续失败(次)')}</div>
                    <input
                      className='fc-input fc-mono'
                      type='number'
                      value={breakerThreshold}
                      onChange={e => setBreakerThreshold(e.target.value)}
                    />
                  </div>
                  <div>
                    <div style={{ fontSize: 11.5, color: 'var(--fc-text-secondary)', marginBottom: 4 }}>{t('隔离冷却(秒)')}</div>
                    <input
                      className='fc-input fc-mono'
                      type='number'
                      value={breakerTimeout}
                      onChange={e => setBreakerTimeout(e.target.value)}
                    />
                  </div>
                  <div>
                    <div style={{ fontSize: 11.5, color: 'var(--fc-text-secondary)', marginBottom: 4 }}>{t('恢复成功(次)')}</div>
                    <input
                      className='fc-input fc-mono'
                      type='number'
                      value={breakerSuccess}
                      onChange={e => setBreakerSuccess(e.target.value)}
                    />
                  </div>
                </div>
                <button
                  type='button'
                  className='fc-btn is-primary is-sm'
                  style={{ width: '100%', marginTop: 10 }}
                  disabled={saving}
                  onClick={saveBreaker}
                >
                  {saving ? t('保存中…') : t('保存熔断器参数')}
                </button>
              </CommonCard>
            ) : null}
          </>
        )
      ) : tab === 'queue' ? (
        <>
          <DashboardCard
            icon='accounts'
            title={t('可用候选顺序')}
            subtitle={
              queue?.model
                ? t('按模型 {model} 预演', { model: queue.model })
                : t('当前下一次请求将按此顺序选号')
            }
            trailing={<TonalChip tone='primary'>{queue?.ordered?.length || 0}</TonalChip>}
          >
            {(queue?.ordered || []).length === 0 ? (
              <div
                className='fc-inner-block'
                style={{ fontSize: 13, color: 'var(--fc-text-secondary)', textAlign: 'center' }}
              >
                {t('当前暂无可用的就绪候选账号')}
              </div>
            ) : (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                {(queue?.ordered || []).map((entry, idx) => {
                  const isPicked = queue?.pickedId === entry.id || idx === 0
                  return (
                    <div
                      key={entry.id}
                      className='fc-inner-block'
                      style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 10 }}
                    >
                      <div style={{ display: 'flex', alignItems: 'center', gap: 10, minWidth: 0, flex: 1 }}>
                        <ProviderAvatar provider={entry.provider} label={entry.providerLabel} size={30} />
                        <div style={{ minWidth: 0, flex: 1 }}>
                          <div
                            style={{
                              fontSize: 13.5,
                              fontWeight: 700,
                              overflow: 'hidden',
                              textOverflow: 'ellipsis',
                              whiteSpace: 'nowrap',
                            }}
                          >
                            {entry.name || entry.id}
                          </div>
                          <div style={{ fontSize: 11.5, color: 'var(--fc-text-secondary)' }}>
                            {providerDisplayName(entry.provider, entry.providerLabel)}
                            {entry.reason ? ` · ${entry.reason}` : ''}
                          </div>
                        </div>
                      </div>
                      <TonalChip tone={isPicked ? 'primary' : 'neutral'}>
                        {isPicked ? t('首选') : `#${idx + 1}`}
                      </TonalChip>
                    </div>
                  )
                })}
              </div>
            )}
          </DashboardCard>

          {(queue?.blocked || []).length > 0 ? (
            <DashboardCard
              icon='alert'
              title={t('已跳过 / 暂不可用')}
              trailing={<TonalChip tone='warning'>{queue?.blocked?.length || 0}</TonalChip>}
            >
              <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                {(queue?.blocked || []).map(entry => (
                  <div
                    key={entry.id}
                    className='fc-inner-block'
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      justifyContent: 'space-between',
                      gap: 8,
                      fontSize: 12.5,
                    }}
                  >
                    <div style={{ minWidth: 0, flex: 1 }}>
                      <div style={{ fontWeight: 600 }}>{entry.name || entry.id}</div>
                      <div style={{ fontSize: 11.5, color: 'var(--fc-text-secondary)' }}>
                        {providerDisplayName(entry.provider, entry.providerLabel)}
                      </div>
                    </div>
                    <div style={{ fontSize: 12, color: 'var(--fc-warning)', textAlign: 'right' }}>
                      {entry.reason || t('冷却中')}
                      {entry.resetAt ? ` · ${formatRelativeTime(entry.resetAt)}` : ''}
                    </div>
                  </div>
                ))}
              </div>
            </DashboardCard>
          ) : null}
        </>
      ) : (
        <RoutingErrorsPanel refreshNonce={errorsNonce} />
      )}

      <PickerSheet
        open={picker === 'strategy'}
        title={t('选择账号选路策略')}
        options={STRATEGY_OPTIONS.map(o => ({ ...o, label: t(o.label), subtitle: o.subtitle ? t(o.subtitle) : undefined }))}
        value={currentStrategy}
        onSelect={next => void patchSetting({ routingStrategy: next })}
        onClose={() => setPicker(null)}
      />

      <PickerSheet
        open={picker === 'sticky'}
        title={t('选择会话粘性档位')}
        options={STICKY_OPTIONS.map(o => ({ ...o, label: t(o.label), subtitle: o.subtitle ? t(o.subtitle) : undefined }))}
        value={currentSticky}
        onSelect={next => void patchSetting({ routingSticky: next })}
        onClose={() => setPicker(null)}
      />

      <RoutingStickySheet
        open={stickySheetOpen}
        model={queueModel}
        onClose={() => setStickySheetOpen(false)}
        onChanged={() => void loadRouting()}
      />
    </CommonScaffold>
  )
}
