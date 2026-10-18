import { useEffect, useState } from 'react'
import { PageHead, IconButton, SectionHead, Empty, ErrorNote, copyWithToast } from '../components/ui'
import { ProviderLogo, GlyphLogo } from '../components/ProviderLogo'
import { Icon } from '../components/Icon'
import { useStore, store } from '../state/store'
import { fetchCheckinCenter, fetchRequests, type CheckinCenter, type RequestPage } from '../api/gateway'
import { formatCompact, formatInt, formatDuration, ellipsisMiddle } from '../lib/format'
import { apiBase, baseLabel } from '../lib/servers'
import { providerLabel } from '../lib/providers'
import { usePolling } from '../lib/usePolling'
import { t } from '../i18n'

/**
 * 首页。
 *
 * 回答三个问题：网关活着吗、今天花了多少、还有什么该做。
 * 这三件事在桌面端分散在报表 / 签到 / 设置三页 —— 手机上一屏给全，
 * 其余细节点进去看。
 */
export function HomePage() {
  const state = useStore()
  const session = state.session
  const accounts = session?.accounts?.accounts || []
  const enabled = accounts.filter(item => item.enabled).length
  const limited = accounts.filter(item => item.enabled && hasRateLimit(item)).length
  const [checkin, setCheckin] = useState<CheckinCenter | null>(null)
  const [requests, setRequests] = useState<RequestPage | null>(null)
  const [error, setError] = useState('')

  const load = async () => {
    try {
      const [center, page] = await Promise.all([fetchCheckinCenter(), fetchRequests({ limit: 3 })])
      setCheckin(center)
      setRequests(page)
      setError('')
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
    }
  }

  // 首页只轮询签到与最近请求：会话本身由 App 启动时拉一次，
  // 用户下拉刷新（下面的按钮）才重拉
  usePolling(load, 60_000)

  useEffect(() => {
    if (!session) void store.refresh()
  }, [session])

  const gateway = session?.health?.upstreamConfigured
  const daily = checkin?.daily
  const eligible = daily?.todayEligible || 0
  const done = daily?.todayDone || 0
  const percent = eligible ? Math.round((done / eligible) * 100) : 0
  const recent = requests?.entries || []

  return (
    <div className='m-page'>
      <PageHead
        title={t('网关运行中')}
        subtitle={t('{n} 个账号启用 · {m} 个限流中', { n: enabled, m: limited })}
        actions={<IconButton name='refresh' label={t('刷新')} onClick={() => void store.refresh(true)} />}
      />

      {state.sessionError ? <ErrorNote text={state.sessionError} /> : null}

      {/* 运行状态与今日读数 */}
      <div className='m-section' style={{ marginTop: 0 }}>
        <div
          className='m-card'
          style={{
            padding: '16px 16px 14px',
            borderRadius: 'var(--m-r-xl)',
            background: 'linear-gradient(150deg, oklch(0.60 0.21 257), oklch(0.50 0.21 257))',
            border: 0,
            color: '#fff',
            boxShadow: '0 16px 34px -18px oklch(0.50 0.21 257 / 90%)',
          }}
        >
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <span
              style={{
                display: 'inline-flex',
                alignItems: 'center',
                gap: 6,
                padding: '4px 9px 4px 8px',
                borderRadius: 999,
                background: 'rgba(255,255,255,.18)',
                fontSize: 12,
                fontWeight: 600,
              }}
            >
              <i
                style={{
                  width: 6,
                  height: 6,
                  borderRadius: '50%',
                  background: gateway ? '#6ef2a4' : '#ffd166',
                  boxShadow: gateway ? '0 0 0 3px rgba(110,242,164,.28)' : '0 0 0 3px rgba(255,209,102,.28)',
                }}
              />
              {gateway ? t('运行中') : t('未配置上游')}
            </span>
            <button
              type='button'
              onClick={() => void copyWithToast(apiBase() + '/v1', 'Base URL')}
              style={{
                marginLeft: 'auto',
                display: 'inline-flex',
                alignItems: 'center',
                gap: 5,
                padding: '4px 8px',
                borderRadius: 8,
                background: 'rgba(255,255,255,.12)',
                color: 'rgba(255,255,255,.9)',
                fontFamily: 'var(--m-mono)',
                fontSize: 11.5,
                border: 0,
              }}
            >
              {baseLabel(apiBase())}/v1
              <Icon name='copy' style={{ width: 13, height: 13 }} />
            </button>
          </div>

          <div style={{ display: 'flex', gap: 8, marginTop: 16 }}>
            <HeroMetric label={t('可用账号')} value={`${enabled}`} unit={`/ ${accounts.length}`} />
            <div style={{ width: 1, background: 'rgba(255,255,255,.22)', margin: '2px 4px' }} />
            <HeroMetric label={t('限流中')} value={`${limited}`} />
            <div style={{ width: 1, background: 'rgba(255,255,255,.22)', margin: '2px 4px' }} />
            <HeroMetric label={t('今日请求')} value={formatInt(requests?.total || 0)} />
          </div>

          <div
            style={{
              display: 'flex',
              gap: 14,
              marginTop: 13,
              paddingTop: 11,
              borderTop: '1px solid rgba(255,255,255,.18)',
              fontSize: 12,
              color: 'rgba(255,255,255,.82)',
            }}
          >
            <span>{t('今日签到 {done}/{total}', { done, total: eligible })}</span>
            <span>{t('最近请求 {n} 条', { n: requests?.matched || 0 })}</span>
          </div>
        </div>
      </div>

      {/* 签到卡 */}
      <div className='m-section'>
        <div className='m-card' style={{ display: 'flex', alignItems: 'center', gap: 13, padding: 15 }}>
          <Ring percent={percent} />
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ fontSize: 14.5, fontWeight: 600 }}>
              {t('今日签到 {done} / {total}', { done, total: eligible })}
            </div>
            <div style={{ fontSize: 12, color: 'var(--m-text-3)', marginTop: 3, whiteSpace: 'nowrap' }}>
              {eligible - done > 0
                ? t('还有 {n} 个待签', { n: eligible - done })
                : t('今天都签完了')}
            </div>
          </div>
          <button type='button' className='m-btn m-sm' onClick={() => store.push('checkin')}>
            <Icon name='checkin' />
            {t('签到')}
          </button>
        </div>
      </div>

      {/* 快捷入口 */}
      <div className='m-section'>
        <div className='m-card' style={{ display: 'grid', gridTemplateColumns: 'repeat(4, 1fr)', padding: '14px 8px 12px' }}>
          <QuickEntry icon='checkin' label={t('签到')} onClick={() => store.push('checkin')} />
          <QuickEntry icon='model' label={t('模型')} onClick={() => store.push('models')} />
          <QuickEntry icon='key' label={t('Key')} onClick={() => store.push('keys')} />
          <QuickEntry icon='doc' label={t('接入')} onClick={() => store.push('docs')} />
        </div>
      </div>

      {/* 限流告警：只留一条，按严重度取最需要处理的那条 */}
      {limited > 0 ? (
        <div className='m-section'>
          <button
            type='button'
            onClick={() => store.go('accounts')}
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 10,
              width: '100%',
              padding: '11px 13px',
              borderRadius: 'var(--m-r-md)',
              background: 'var(--m-warn-soft)',
              border: '1px solid color-mix(in srgb, var(--m-warn) 26%, transparent)',
              fontSize: 12.5,
              textAlign: 'left',
            }}
          >
            <Icon name='alert' style={{ width: 17, height: 17, color: 'var(--m-warn)', flex: 'none' }} />
            <span style={{ flex: 1 }}>
              {t('{n} 个账号正在限流，转发会自动跳过它们', { n: limited })}
            </span>
            <span style={{ color: 'var(--m-warn)', fontWeight: 600, fontSize: 12, flex: 'none' }}>{t('查看')}</span>
          </button>
        </div>
      ) : null}

      {/* 最近请求 */}
      <div className='m-section'>
        <SectionHead title={t('最近请求')} action={{ label: t('全部'), onClick: () => store.push('requests') }} />
        <div className='m-card'>
          {error ? (
            <Empty text={error} />
          ) : recent.length ? (
            recent.map((row, index) => (
              <button
                key={row.id || `${row.ts}-${index}`}
                type='button'
                className='m-row'
                onClick={() => store.push('requests')}
                style={{ minHeight: 46 }}
              >
                <StatusDot status={row.status} />
                <span
                  className='m-mono'
                  style={{ flex: 1, minWidth: 0, fontSize: 12.5, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}
                >
                  {ellipsisMiddle(row.upstreamModel || row.model || '—', 26)}
                </span>
                <span style={{ fontSize: 11, color: 'var(--m-text-3)', flex: 'none' }}>
                  {row.durationMs ? formatDuration(row.durationMs) : '—'}
                </span>
              </button>
            ))
          ) : (
            <Empty text={t('还没有请求记录')} />
          )}
        </div>
      </div>

      {/* 提供商概览 */}
      <div className='m-section'>
        <SectionHead title={t('账号分布')} action={{ label: t('管理'), onClick: () => store.go('accounts') }} />
        <div className='m-card'>
          {(session?.accounts?.providers || []).slice(0, 5).map(item => (
            <button
              key={item.id}
              type='button'
              className='m-row'
              onClick={() => store.go('accounts')}
            >
              <ProviderLogo id={item.id} label={item.label} />
              <div className='m-grow'>
                <div className='m-t1'>{providerLabel(item.id, item.label)}</div>
                <div className='m-t2'>{t('{n} 个账号', { n: item.count || 0 })}</div>
              </div>
              <span className='m-chev'>
                <Icon name='chev' />
              </span>
            </button>
          ))}
          {!(session?.accounts?.providers || []).length ? <Empty text={t('还没有添加账号')} /> : null}
        </div>
      </div>
    </div>
  )
}

function HeroMetric({ label, value, unit }: { label: string; value: string; unit?: string }) {
  return (
    <div style={{ flex: 1, minWidth: 0 }}>
      <div style={{ fontSize: 11.5, color: 'rgba(255,255,255,.72)' }}>{label}</div>
      <div
        style={{
          fontSize: 24,
          fontWeight: 650,
          letterSpacing: '-0.03em',
          marginTop: 2,
          fontVariantNumeric: 'tabular-nums',
          whiteSpace: 'nowrap',
        }}
      >
        {value}
        {unit ? <span style={{ fontSize: 12, fontWeight: 500, opacity: 0.8, marginLeft: 3 }}>{unit}</span> : null}
      </div>
    </div>
  )
}

function Ring({ percent }: { percent: number }) {
  const size = 54
  const radius = (size - 8) / 2
  const circumference = 2 * Math.PI * radius
  const dash = (Math.min(100, Math.max(0, percent)) / 100) * circumference
  return (
    <div style={{ width: size, height: size, position: 'relative', flex: 'none' }}>
      <svg width={size} height={size} style={{ transform: 'rotate(-90deg)' }} aria-hidden='true'>
        <circle cx={size / 2} cy={size / 2} r={radius} fill='none' stroke='var(--m-surface-3)' strokeWidth={5} />
        <circle
          cx={size / 2}
          cy={size / 2}
          r={radius}
          fill='none'
          stroke='var(--m-primary)'
          strokeWidth={5}
          strokeLinecap='round'
          strokeDasharray={`${dash} ${circumference}`}
        />
      </svg>
      <span
        style={{
          position: 'absolute',
          inset: 0,
          display: 'grid',
          placeItems: 'center',
          fontSize: 12.5,
          fontWeight: 650,
          fontVariantNumeric: 'tabular-nums',
        }}
      >
        {percent}%
      </span>
    </div>
  )
}

function QuickEntry({
  icon,
  label,
  onClick,
}: {
  icon: Parameters<typeof GlyphLogo>[0]['name']
  label: string
  onClick: () => void
}) {
  return (
    <button
      type='button'
      onClick={onClick}
      style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        gap: 7,
        padding: '6px 2px',
        background: 'none',
        border: 0,
        fontSize: 11.5,
        color: 'var(--m-text-2)',
      }}
    >
      <span
        style={{
          width: 42,
          height: 42,
          borderRadius: 13,
          display: 'grid',
          placeItems: 'center',
          background: 'var(--m-surface-2)',
          border: '1px solid var(--m-line)',
          color: 'var(--m-primary)',
        }}
      >
        <Icon name={icon} style={{ width: 21, height: 21 }} />
      </span>
      {label}
    </button>
  )
}

/** 状态圆点：成功绿、4xx/5xx 红、在途黄 */
export function StatusDot({ status }: { status?: number }) {
  const tone = !status ? 'warn' : status >= 200 && status < 400 ? 'ok' : 'bad'
  const color = tone === 'ok' ? 'var(--m-ok)' : tone === 'warn' ? 'var(--m-warn)' : 'var(--m-danger)'
  return <i style={{ width: 7, height: 7, borderRadius: '50%', background: color, flex: 'none' }} />
}

function hasRateLimit(account: { rateLimits?: Record<string, unknown> }): boolean {
  return Boolean(account.rateLimits && Object.keys(account.rateLimits).length)
}

