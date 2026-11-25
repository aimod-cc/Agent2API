import { useEffect, useState } from 'react'
import {
  fetchHomeCheckinSummary,
  fetchStatsSummary,
  runCheckin,
  type CheckinCenterPayload,
  type StatsDimension,
  type StatsRange,
  type StatsSummary,
} from '../core/api'
import { copyText, formatCompact, formatNumber, formatPercent, providerDisplayName } from '../core/format'
import { t } from '../core/i18n'
import { formatCost, switchPricingCurrency, syncPricingDisplay, usePricingDisplay } from '../core/pricing'
import { baseLabel, serverIdentity } from '../core/servers'
import { store, useStore } from '../core/store'
import {
  CommonScaffold,
  ConnectedButtonGroup,
  DashboardCard,
  SegmentedControl,
  TonalChip,
  TonalIconButton,
} from '../ui/components'
import { GlyphIcon } from '../ui/icons'

function rangeOptions(): Array<{ id: StatsRange; label: string }> {
  return [
    { id: 'today', label: t('今天') },
    { id: 'yesterday', label: t('昨天') },
    { id: '7', label: t('7 天') },
    { id: '30', label: t('30 天') },
    { id: 'all', label: t('全部') },
  ]
}

type RankTab = 'models' | 'providers' | 'accounts'

const RANK_TABS: ReadonlyArray<{ id: RankTab; label: string }> = [
  { id: 'models', label: '按模型' },
  { id: 'providers', label: '按提供商' },
  { id: 'accounts', label: '按账号' },
]

export function DashboardView() {
  const { server, packaged, session, sessionError, loading, revision } = useStore()
  const pricing = usePricingDisplay()
  const [range, setRange] = useState<StatsRange>('today')
  const [rankTab, setRankTab] = useState<RankTab>('models')
  const [stats, setStats] = useState<StatsSummary | null>(null)
  const [checkin, setCheckin] = useState<CheckinCenterPayload | null>(null)
  const [checkinBusy, setCheckinBusy] = useState(false)

  useEffect(() => {
    const controller = new AbortController()
    const id = serverIdentity()
    if (!server?.base) return
    fetchStatsSummary(range, controller.signal)
      .then(data => {
        if (serverIdentity() === id) {
          setStats(data)
          if (data && (data.currency || data.exchangeRate || data.fx)) {
            syncPricingDisplay({
              currency: data.currency,
              exchangeRate: data.exchangeRate,
              fx: data.fx,
            })
          }
        }
      })
      .catch(() => {})
    return () => controller.abort()
  }, [range, revision, server?.base])

  useEffect(() => {
    const controller = new AbortController()
    const id = serverIdentity()
    if (!server?.base) return
    fetchHomeCheckinSummary(controller.signal)
      .then(data => {
        if (serverIdentity() === id) setCheckin(data)
      })
      .catch(() => {})
    return () => controller.abort()
  }, [revision, server?.base])

  const endpointBase = server?.base || (typeof location !== 'undefined' ? location.origin : '')
  const v1Url = endpointBase ? `${endpointBase.replace(/\/+$/, '')}/v1` : '/v1'

  const accountsList = session?.accounts?.accounts || []
  const summary = session?.accounts?.summary
  const totalAccounts = summary?.total ?? session?.accounts?.total ?? accountsList.length
  const enabledAccounts =
    summary?.enabled ?? accountsList.filter(a => a.enabled !== false).length
  const availableAccounts =
    summary?.available ??
    accountsList.filter(a => a.enabled !== false && !a.needsRelogin).length
  const needReloginCount =
    summary?.needsRelogin ?? accountsList.filter(a => a.needsRelogin).length

  const overview = stats?.overview
  const totalRequests = overview?.requests ?? 0
  const successfulRequests = overview?.successful ?? 0
  const successRate =
    totalRequests > 0 ? (successfulRequests / totalRequests) * 100 : 100
  const totalTokens = overview?.tokens ?? 0

  const todayDone = checkin?.daily?.todayDone ?? 0
  const todayEligible = checkin?.daily?.todayEligible ?? 0

  const handleQuickCheckin = async () => {
    if (checkinBusy) return
    setCheckinBusy(true)
    try {
      const res = await runCheckin()
      const ok = (res.succeeded ?? 0) + (res.alreadyDone ?? 0)
      store.toast(`签到完成：成功 ${ok} 个`, 'success')
      const fresh = await fetchHomeCheckinSummary().catch(() => null)
      if (fresh) setCheckin(fresh)
    } catch (err) {
      store.toast(err instanceof Error ? err.message : '签到失败', 'danger')
    } finally {
      setCheckinBusy(false)
    }
  }

  const rankItems: StatsDimension[] =
    rankTab === 'models'
      ? stats?.models || []
      : rankTab === 'providers'
        ? stats?.providers || []
        : stats?.accounts || []

  const topRankItems = rankItems.slice(0, 8)
  const maxRankTokens = Math.max(
    1,
    ...topRankItems.map(item => item.totalTokens ?? item.tokens ?? item.requests ?? 0),
  )

  return (
    <CommonScaffold
      title='总览'
      badge={
        server ? (
          <button
            type='button'
            className='fc-chip tone-primary'
            style={{
              border: 'none',
              cursor: packaged ? 'pointer' : 'default',
              height: 26,
              padding: '0 10px',
              borderRadius: 9999,
            }}
            onClick={() => packaged && store.openSheet({ kind: 'servers' })}
          >
            <GlyphIcon name='server' size={12} />
            <span>{server.name || baseLabel(server.base)}</span>
          </button>
        ) : null
      }
      actions={
        <>
          <TonalIconButton
            icon='calendar'
            label='签到'
            onClick={() => store.setTab('checkin')}
          />
          <TonalIconButton
            icon='refresh'
            disabled={loading}
            onClick={() => void store.refresh(true)}
            title='刷新'
          />
        </>
      }
    >
      {/* 网关连接异常恢复卡片 */}
      {sessionError ? (
        <DashboardCard
          icon='alert'
          title='无法连接到当前网关'
          subtitle={server?.base ? baseLabel(server.base) : '尚未配置网关地址'}
          trailing={<TonalChip tone='danger'>离线</TonalChip>}
        >
          <div style={{ fontSize: 13, color: 'var(--fc-text-secondary)', lineHeight: 1.5 }}>
            {sessionError}
          </div>
          <div style={{ display: 'flex', gap: 10 }}>
            {packaged ? (
              <button
                type='button'
                className='fc-btn is-primary'
                style={{ flex: 1 }}
                onClick={() => store.openSheet({ kind: 'servers' })}
              >
                <GlyphIcon name='server' size={16} />
                <span>切换或修改服务器</span>
              </button>
            ) : null}
            <button
              type='button'
              className='fc-btn'
              style={{ flex: packaged ? undefined : 1 }}
              onClick={() => void store.refresh(true)}
            >
              <GlyphIcon name='refresh' size={16} />
              <span>重试</span>
            </button>
          </div>
        </DashboardCard>
      ) : null}

      {/* 网关状态与接口地址 */}
      <DashboardCard
        icon='bolt'
        title={server?.name || '网关服务'}
        subtitle={
          session?.lastRequestModel
            ? `最近转发 · ${session.lastRequestModel}`
            : `${availableAccounts} 个可用账号 · ${(session?.models || []).length} 个可用模型`
        }
        trailing={
          <TonalChip tone={sessionError ? 'danger' : 'success'}>
            {sessionError ? '连接异常' : '运行中'}
          </TonalChip>
        }
      >
        <div
          className='fc-inner-block'
          style={{
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'space-between',
            gap: 8,
            padding: '10px 12px',
          }}
        >
          <div style={{ minWidth: 0, flex: 1 }}>
            <div style={{ fontSize: 11, color: 'var(--fc-text-tertiary)', fontWeight: 600 }}>
              OpenAI / Anthropic 兼容接入点
            </div>
            <div
              className='fc-mono'
              style={{
                fontSize: 12.2,
                fontWeight: 600,
                color: 'var(--fc-text)',
                overflow: 'hidden',
                textOverflow: 'ellipsis',
                whiteSpace: 'nowrap',
                marginTop: 2,
              }}
            >
              {v1Url}
            </div>
          </div>
          <button
            type='button'
            className='fc-btn is-tonal is-sm is-pill'
            style={{ padding: '0 10px', height: 30, flexShrink: 0 }}
            onClick={async () => {
              const ok = await copyText(v1Url)
              store.toast(ok ? '已复制接口地址' : '复制失败', ok ? 'success' : 'danger')
            }}
          >
            <GlyphIcon name='copy' size={13} />
            <span>复制</span>
          </button>
        </div>

        { todayEligible > 0 ? (
          <div
            className='fc-inner-block'
            style={{
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'space-between',
              gap: 10,
            }}
          >
            <div style={{ display: 'flex', alignItems: 'center', gap: 10, minWidth: 0 }}>
              <span
                style={{
                  width: 32,
                  height: 32,
                  borderRadius: 10,
                  background: 'var(--fc-inset)',
                  color: 'var(--fc-primary)',
                  display: 'inline-flex',
                  alignItems: 'center',
                  justifyContent: 'center',
                  flexShrink: 0,
                }}
              >
                <GlyphIcon name='calendar' size={17} />
              </span>
              <div style={{ minWidth: 0 }}>
                <div style={{ fontSize: 13.5, fontWeight: 600 }}>
                  今日签到 {todayDone} / {todayEligible}
                </div>
                <div style={{ fontSize: 11.5, color: 'var(--fc-text-secondary)' }}>
                  {todayDone >= todayEligible ? '今日全部已签' : '可领取每日免费额度'}
                </div>
              </div>
            </div>
            {todayDone < todayEligible ? (
              <button
                type='button'
                className='fc-btn is-primary is-sm is-pill'
                disabled={checkinBusy}
                onClick={handleQuickCheckin}
              >
                {checkinBusy ? '签到中…' : '一键签到'}
              </button>
            ) : (
              <button
                type='button'
                className='fc-btn is-sm is-pill'
                style={{ background: 'var(--fc-inset)' }}
                onClick={() => store.setTab('checkin')}
              >
                {t('详情')}
              </button>
            )}
          </div>
        ) : null}
      </DashboardCard>

      {/* 统计时间范围（FlClash ConnectedButtonGroup） */}
      <ConnectedButtonGroup
        options={rangeOptions()}
        value={range}
        onChange={setRange}
      />

      {/* 4 项核心指标卡片 */}
      <div className='fc-kpi-grid'>
        <div
          className='fc-kpi-card'
          onClick={() => store.openSheet({ kind: 'requests' })}
          style={{ cursor: 'pointer' }}
        >
          <div className='fc-kpi-top'>
            <span className='fc-kpi-label'>请求总数</span>
            <GlyphIcon name='activity' size={16} />
          </div>
          <div className='fc-kpi-value'>{formatCompact(totalRequests)}</div>
          <div className='fc-kpi-sub'>成功 {formatNumber(successfulRequests)} 次</div>
        </div>

        <div
          className='fc-kpi-card'
          onClick={() => store.openSheet({ kind: 'requests' })}
          style={{ cursor: 'pointer' }}
        >
          <div className='fc-kpi-top'>
            <span className='fc-kpi-label'>成功率</span>
            <TonalChip
              tone={
                totalRequests === 0
                  ? 'neutral'
                  : successRate >= 95
                    ? 'success'
                    : successRate >= 80
                      ? 'warning'
                      : 'danger'
              }
            >
              {totalRequests === 0 ? '无请求' : successRate >= 95 ? '稳定' : '留意'}
            </TonalChip>
          </div>
          <div className='fc-kpi-value'>
            {totalRequests === 0 ? '—' : formatPercent(successRate)}
          </div>
          <div className='fc-kpi-sub'>点此查看请求明细</div>
        </div>

        <div className='fc-kpi-card'>
          <div className='fc-kpi-top'>
            <span className='fc-kpi-label'>总 Token</span>
            <GlyphIcon name='spark' size={16} />
          </div>
          <div className='fc-kpi-value'>{formatCompact(totalTokens)}</div>
          <div className='fc-kpi-sub'>
            {overview?.topModel?.model ? `主力 ${overview.topModel.model}` : '含输入与输出'}
          </div>
        </div>

        <div
          className='fc-kpi-card'
          onClick={() => store.setTab('accounts')}
          style={{ cursor: 'pointer' }}
        >
          <div className='fc-kpi-top'>
            <span className='fc-kpi-label'>可用账号</span>
            <GlyphIcon name='accounts' size={16} />
          </div>
          <div className='fc-kpi-value'>
            {availableAccounts}
            <span style={{ fontSize: 15, fontWeight: 600, color: 'var(--fc-text-tertiary)' }}>
              {' '}
              / {totalAccounts}
            </span>
          </div>
          <div className='fc-kpi-sub'>
            {needReloginCount > 0
              ? `${needReloginCount} 个账号需重新登录`
              : `已启用 ${enabledAccounts} 个`}
          </div>
        </div>

        <div
          className='fc-kpi-card'
          onClick={() => store.openSheet({ kind: 'pricing' })}
          style={{ gridColumn: '1 / -1', cursor: 'pointer' }}
        >
          <div className='fc-kpi-top'>
            <span className='fc-kpi-label'>预估费用</span>
            <div
              onClick={e => e.stopPropagation()}
              style={{
                display: 'inline-flex',
                gap: 2,
                padding: 2,
                borderRadius: 999,
                background: 'var(--fc-inset)',
              }}
            >
              {(['usd', 'cny'] as const).map(code => {
                const active = pricing.currency === code
                return (
                  <button
                    key={code}
                    type='button'
                    onClick={() => void switchPricingCurrency(code)}
                    style={{
                      border: 'none',
                      borderRadius: 999,
                      padding: '2px 9px',
                      fontSize: 11.5,
                      fontWeight: 700,
                      cursor: 'pointer',
                      background: active ? 'var(--fc-primary)' : 'transparent',
                      color: active ? '#fff' : 'var(--fc-text-secondary)',
                    }}
                  >
                    {code === 'usd' ? '$' : '¥'}
                  </button>
                )
              })}
            </div>
          </div>
          <div
            style={{
              display: 'flex',
              alignItems: 'baseline',
              justifyContent: 'space-between',
              gap: 10,
            }}
          >
            <div className='fc-kpi-value'>
              {totalRequests === 0
                ? '—'
                : formatCost(overview?.costUsd ?? 0, pricing.currency, pricing.exchangeRate)}
            </div>
            <span style={{ fontSize: 12, color: 'var(--fc-primary)', fontWeight: 600 }}>
              计价设置 →
            </span>
          </div>
          <div className='fc-kpi-sub'>
            {(overview?.unpricedRequests ?? 0) > 0
              ? `另有 ${overview?.unpricedRequests} 条未匹配到定价`
              : pricing.currency === 'cny'
                ? `约合 $${(overview?.costUsd ?? 0).toFixed(2)} · 汇率 ${pricing.exchangeRate.toFixed(2)}`
                : '点此调整模型计价与汇率'}
          </div>
        </div>
      </div>

      {/* 用量排行卡片 */}
      <DashboardCard
        icon='cube'
        title='用量构成'
        subtitle='按 Token 消耗与请求次数排序'
      >
        <SegmentedControl
          inCard
          options={RANK_TABS}
          value={rankTab}
          onChange={setRankTab}
        />

        {topRankItems.length === 0 ? (
          <div
            className='fc-inner-block'
            style={{
              textAlign: 'center',
              padding: '22px 14px',
              color: 'var(--fc-text-secondary)',
              fontSize: 13,
            }}
          >
            当前时间范围内暂无调用记录
          </div>
        ) : (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
            {topRankItems.map((item, idx) => {
              const rawName = item.label || item.model || item.id || `项目 #${idx + 1}`
              const displayName =
                rankTab === 'providers' ? providerDisplayName(item.id, item.label) : rawName
              const tokens = item.totalTokens ?? item.tokens ?? 0
              const reqs = item.requests ?? 0
              const metric = tokens > 0 ? tokens : reqs
              const ratio = Math.max(4, Math.min(100, Math.round((metric / maxRankTokens) * 100)))

              return (
                <div
                  key={`${rawName}-${idx}`}
                  className='fc-inner-block'
                  style={{ display: 'flex', flexDirection: 'column', gap: 7 }}
                >
                  <div
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      justifyContent: 'space-between',
                      gap: 10,
                    }}
                  >
                    <span
                      style={{
                        fontSize: 13.5,
                        fontWeight: 600,
                        overflow: 'hidden',
                        textOverflow: 'ellipsis',
                        whiteSpace: 'nowrap',
                      }}
                    >
                      {displayName}
                    </span>
                    <span
                      className='fc-mono'
                      style={{
                        fontSize: 12.5,
                        fontWeight: 700,
                        color: 'var(--fc-text)',
                        flexShrink: 0,
                      }}
                    >
                      {item.costUsd !== undefined && item.costUsd !== null && item.costUsd > 0
                        ? `${formatCost(item.costUsd, pricing.currency, pricing.exchangeRate)} · `
                        : ''}
                      {formatCompact(tokens)} Token
                    </span>
                  </div>
                  <div
                    style={{
                      height: 6,
                      borderRadius: 999,
                      background: 'var(--fc-inset)',
                      overflow: 'hidden',
                    }}
                  >
                    <div
                      style={{
                        width: `${ratio}%`,
                        height: '100%',
                        borderRadius: 999,
                        background: 'var(--fc-primary)',
                        transition: 'width 260ms ease',
                      }}
                    />
                  </div>
                  <div
                    style={{
                      display: 'flex',
                      justifyContent: 'space-between',
                      fontSize: 11.5,
                      color: 'var(--fc-text-secondary)',
                    }}
                  >
                    <span>{formatNumber(reqs)} 次请求</span>
                    <span>
                      {item.percentage !== undefined
                        ? formatPercent(item.percentage)
                        : item.successRate !== undefined && item.successRate !== null
                          ? `成功率 ${formatPercent(item.successRate)}`
                          : ''}
                    </span>
                  </div>
                </div>
              )
            })}
          </div>
        )}
      </DashboardCard>
    </CommonScaffold>
  )
}
