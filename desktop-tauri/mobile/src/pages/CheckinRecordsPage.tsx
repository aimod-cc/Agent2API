import { useEffect, useRef, useState } from 'react'
import { PageIntro, IconButton, Badge, Empty, ErrorNote, SkeletonList } from '../components/ui'
import { ProviderLogo } from '../components/ProviderLogo'
import { fetchCheckinRecords, type CheckinRecordRow } from '../api/gateway'
import { formatStamp } from '../lib/format'
import { providerLabel } from '../lib/providers'
import { usePullToRefresh, PullIndicator } from '../components/PullToRefresh'
import { t } from '../i18n'

/**
 * 签到记录（只读）。
 *
 * 签到中心只展示「今天签成了几个」，签到失败之后用户没法自查原因。这里把
 * 后端 `checkin_records` 的明细台账铺成卡片：结果、账号、提供商、来源、说明，
 * 支持按时间档位与结果档位收窄。
 *
 * 数据口径与桌面端「签到记录」页一致：`source` / `outcome` 的取值都照后端
 * 契约，不在前端另造词。这里不做轮询 —— 历史不会自己变，进页面拉一次，
 * 要看新的点右上角刷新。
 */

const PAGE_SIZE = 50

type RangeKey = 'today' | 'week' | 'month' | 'all'
type OutcomeKey = '' | 'success' | 'already' | 'pending' | 'failed'
type Tone = 'ok' | 'warn' | 'bad' | 'mute' | 'brand'

/**
 * 时间档位 → `start` 毫秒下界。
 *
 * 用 `new Date(y, m, d - back)` 构造本地自然日 0 点：跨月 / 跨年由 Date
 * 自己归一化，不手算时区偏移。`全部` 不传下界。
 */
function rangeStart(range: RangeKey): number | undefined {
  if (range === 'all') return undefined
  const now = new Date()
  const back = range === 'today' ? 0 : range === 'week' ? 6 : 29
  return new Date(now.getFullYear(), now.getMonth(), now.getDate() - back).getTime()
}

export function CheckinRecordsPage() {
  const [range, setRange] = useState<RangeKey>('all')
  const [outcome, setOutcome] = useState<OutcomeKey>('')
  const [records, setRecords] = useState<CheckinRecordRow[]>([])
  const [total, setTotal] = useState(0)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  // 是否已经完成过一次请求：区分「首屏骨架」与「确实没有记录」
  const [initialized, setInitialized] = useState(false)
  // 请求序号：切档位很快时，只让最后一次请求的结果落地
  const seqRef = useRef(0)

  const load = async (reset: boolean) => {
    const seq = ++seqRef.current
    const offset = reset ? 0 : records.length
    if (reset) {
      // 换档位 = 回到第一页：先清空，避免旧档位的行混在新结果前面
      setRecords([])
      setTotal(0)
      setInitialized(false)
    }
    setLoading(true)
    setError('')
    try {
      const next = await fetchCheckinRecords({
        limit: PAGE_SIZE,
        offset,
        outcome: outcome || undefined,
        start: rangeStart(range),
      })
      if (seq !== seqRef.current) return
      const rows = Array.isArray(next?.records) ? next.records : []
      setRecords(prev => (reset ? rows : [...prev, ...rows]))
      setTotal(Number(next?.total) || 0)
      setInitialized(true)
    } catch (failure) {
      if (seq !== seqRef.current) return
      setError(failure instanceof Error ? failure.message : String(failure))
      setInitialized(true)
    } finally {
      if (seq === seqRef.current) setLoading(false)
    }
  }

  useEffect(() => {
    void load(true)
    // 只在档位变化时重拉；load 每次渲染都会重建，不能进依赖
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [range, outcome])

  // 下拉 = 回到第一页重拉当前筛选（与右上角刷新一致）
  const pull = usePullToRefresh({ onRefresh: () => load(true) })

  const ranges: Array<{ value: RangeKey; label: string }> = [
    { value: 'today', label: t('今天') },
    { value: 'week', label: t('7 天') },
    { value: 'month', label: t('30 天') },
    { value: 'all', label: t('全部') },
  ]
  const outcomes: Array<{ value: OutcomeKey; label: string }> = [
    { value: '', label: t('全部') },
    { value: 'success', label: t('成功领取') },
    { value: 'already', label: t('今日已领') },
    { value: 'pending', label: t('等待窗口') },
    { value: 'failed', label: t('失败') },
  ]
  // 只要动过任一档位就算「当前筛选」，空态文案据此区分
  const filtered = range !== 'all' || Boolean(outcome)

  return (
    <div className='m-page m-fixed' ref={pull.bindRef}>
      <PullIndicator pull={pull.pull} refreshing={pull.refreshing} />
      <div className='m-sticky'>
        <PageIntro
          subtitle={initialized ? t('共 {n} 条记录', { n: total }) : undefined}
          actions={<IconButton name='refresh' label={t('刷新')} onClick={() => void load(true)} />}
        />

        <div className='m-chips' style={{ padding: '0 16px 8px' }}>
          {ranges.map(item => (
            <button
              key={item.value}
              type='button'
              className='m-chip'
              data-on={range === item.value}
              onClick={() => setRange(item.value)}
            >
              {item.label}
            </button>
          ))}
        </div>

        <div className='m-chips' style={{ padding: '0 16px 12px' }}>
          {outcomes.map(item => (
            <button
              key={item.value || 'all'}
              type='button'
              className='m-chip'
              data-on={outcome === item.value}
              onClick={() => setOutcome(item.value)}
            >
              {item.label}
            </button>
          ))}
        </div>
      </div>

      <div className='m-body'>
        {error ? <ErrorNote text={error} /> : null}
        {!initialized && !error ? <SkeletonList rows={4} height={88} /> : null}

        {initialized && !records.length && !error ? (
          <div className='m-section'>
            <div className='m-card'>
              <Empty text={filtered ? t('当前筛选下没有记录') : t('还没有签到记录')} />
            </div>
          </div>
        ) : null}

        {records.length ? (
          <div className='m-section' style={{ marginTop: 12 }}>
            {records.map((row, index) => (
              <RecordCard key={row.rowId ?? `${row.ts}-${index}`} row={row} />
            ))}
          </div>
        ) : null}

        {initialized && records.length < total ? (
          <div className='m-section'>
            <button
              type='button'
              className='m-btn m-ghost'
              disabled={loading}
              onClick={() => void load(false)}
            >
              {loading ? t('加载中…') : t('加载更早的记录')}
            </button>
          </div>
        ) : null}
      </div>
    </div>
  )
}

/** 一条记录一张卡（只读，不做点击进详情） */
function RecordCard({ row }: { row: CheckinRecordRow }) {
  const view = outcomeView(row.outcome)
  const detail = row.error || row.message || ''
  return (
    <div
      style={{
        padding: '11px 13px',
        marginBottom: 9,
        borderRadius: 'var(--m-r-md)',
        background: 'var(--m-surface)',
        border: '1px solid var(--m-line)',
        boxShadow: 'var(--m-shadow-1)',
      }}
    >
      <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
        <Badge tone={view.tone}>{view.label}</Badge>
        <span
          style={{
            flex: 1,
            minWidth: 0,
            fontSize: 13,
            fontWeight: 550,
            overflow: 'hidden',
            textOverflow: 'ellipsis',
            whiteSpace: 'nowrap',
          }}
        >
          {row.accountName || row.accountId || '—'}
        </span>
        <span
          style={{
            fontSize: 11,
            color: 'var(--m-text-3)',
            flex: 'none',
            fontVariantNumeric: 'tabular-nums',
          }}
        >
          {formatStamp(row.ts)}
        </span>
      </div>

      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 6,
          marginTop: 8,
          fontSize: 11.5,
          color: 'var(--m-text-3)',
          minWidth: 0,
        }}
      >
        <ProviderLogo id={row.provider} />
        <span style={{ flex: 'none' }}>{providerLabel(row.provider)}</span>
        <span style={{ opacity: 0.45 }}>·</span>
        <span style={{ minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
          {sourceLabel(row.source)}
        </span>
      </div>

      {detail ? (
        <div
          style={{
            marginTop: 8,
            fontSize: 11.5,
            lineHeight: 1.5,
            wordBreak: 'break-word',
            color: row.error ? 'var(--m-danger)' : 'var(--m-text-3)',
          }}
        >
          {detail}
        </div>
      ) : null}
    </div>
  )
}

/** 结果 → 徽章文案与语义色（未知结果按中性灰处理） */
function outcomeView(outcome?: string): { label: string; tone: Tone } {
  switch (outcome) {
    case 'success':
      return { label: t('成功领取'), tone: 'ok' }
    case 'already':
      return { label: t('今日已领'), tone: 'brand' }
    case 'pending':
      return { label: t('等待窗口'), tone: 'warn' }
    case 'failed':
      return { label: t('失败'), tone: 'bad' }
    default:
      return { label: outcome || '—', tone: 'mute' }
  }
}

/** 触发来源（取值与后端 checkin_records 的 source 一致） */
function sourceLabel(source?: string): string {
  switch (source) {
    case 'schedule':
      return t('定时')
    case 'startup':
      return t('补签')
    case 'manual':
      return t('手动')
    case 'single':
      return t('单签')
    case 'activity':
      return t('活动')
    default:
      return source || '—'
  }
}
