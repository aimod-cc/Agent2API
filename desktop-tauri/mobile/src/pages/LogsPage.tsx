import { useState } from 'react'
import { PageIntro, IconButton, Empty, ErrorNote, SkeletonList } from '../components/ui'
import { store } from '../state/store'
import { fetchLogs, type LogEntry } from '../api/gateway'
import { formatClock } from '../lib/format'
import { usePolling } from '../lib/usePolling'
import { t } from '../i18n'

/**
 * 运行日志。
 *
 * 与桌面端一致：这里只承载**不随请求量增长的**系统事件（网关的逐条请求在
 * 「请求日志」页）。手机上默认只看 warn 及以上 —— 全量在窄屏上刷不完，
 * 而真要看细节的人会去桌面端。
 *
 * 时间与级别排成一行、正文另起一行：桌面端的「时间 | 级别 | 正文」三列在
 * 390px 里会把正文压成一条竖线。
 */

type Level = '' | 'warn' | 'error'

export function LogsPage() {
  const [level, setLevel] = useState<Level>('warn')
  const [keyword, setKeyword] = useState('')
  const [entries, setEntries] = useState<LogEntry[] | null>(null)
  const [total, setTotal] = useState(0)
  const [error, setError] = useState('')

  const load = async () => {
    try {
      const page = await fetchLogs({ level: level || undefined, keyword: keyword.trim() || undefined, limit: 80 })
      setEntries(page.entries || [])
      setTotal(Number(page.total) || 0)
      setError('')
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
    }
  }

  usePolling(load, 30_000, [level, keyword])

  return (
    <div className='m-page'>
      <PageIntro actions={<IconButton name='refresh' label={t('刷新')} onClick={() => void load()} />}
      />

      <div className='m-section' style={{ marginTop: 0 }}>
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 8,
            height: 38,
            padding: '0 12px',
            borderRadius: 11,
            background: 'var(--m-surface)',
            border: '1px solid var(--m-line)',
            color: 'var(--m-text-3)',
          }}
        >
          <span style={{ fontSize: 13 }}>🔍</span>
          <input
            value={keyword}
            onChange={event => setKeyword(event.target.value)}
            placeholder={t('搜索关键字')}
            style={{
              flex: 1,
              minWidth: 0,
              border: 0,
              outline: 'none',
              background: 'transparent',
              color: 'var(--m-text)',
              fontSize: 16,
            }}
          />
        </div>
      </div>

      <div className='m-chips' style={{ padding: '10px 16px 12px' }}>
        {(
          [
            { value: 'warn' as Level, label: t('warn 及以上') },
            { value: 'error' as Level, label: t('仅 error') },
            { value: '' as Level, label: t('全部') },
          ]
        ).map(item => (
          <button
            key={item.value || 'all'}
            type='button'
            className='m-chip'
            data-on={level === item.value}
            onClick={() => setLevel(item.value)}
          >
            {item.label}
          </button>
        ))}
      </div>

      {error ? <ErrorNote text={error} /> : null}
      {!entries ? <SkeletonList rows={5} height={58} /> : null}

      {entries && !entries.length ? (
        <div className='m-section'>
          <div className='m-card'>
            <Empty text={t('没有符合条件的日志')} />
          </div>
        </div>
      ) : null}

      {entries && entries.length ? (
        <div className='m-section' style={{ marginTop: 0 }}>
          <div className='m-card'>
            {entries.map((entry, index) => (
              <div
                key={entry.id || index}
                style={{
                  display: 'grid',
                  gridTemplateColumns: 'auto auto 1fr',
                  alignItems: 'baseline',
                  gap: '4px 8px',
                  padding: '11px 14px',
                  borderBottom: index === entries.length - 1 ? 0 : '1px solid var(--m-line)',
                  fontSize: 11.8,
                }}
              >
                <span className='m-mono' style={{ color: 'var(--m-text-3)', fontSize: 10.8 }}>
                  {formatClock(entry.ts)}
                </span>
                <span
                  style={{
                    height: 16,
                    padding: '0 5px',
                    borderRadius: 5,
                    fontSize: 9.5,
                    fontWeight: 700,
                    letterSpacing: '.02em',
                    display: 'inline-flex',
                    alignItems: 'center',
                    background: levelBg(entry.level),
                    color: levelFg(entry.level),
                  }}
                >
                  {String(entry.level || 'info').toUpperCase()}
                </span>
                <span />
                <span style={{ gridColumn: '1 / -1', color: 'var(--m-text-2)', lineHeight: 1.55 }}>
                  {entry.category ? (
                    <span style={{ color: 'var(--m-text-3)' }}>[{entry.category}] </span>
                  ) : null}
                  {entry.message}
                </span>
              </div>
            ))}
          </div>
        </div>
      ) : null}

      <div className='m-section' style={{ paddingBottom: 8 }}>
        <button
          type='button'
          className='m-btn m-ghost'
          onClick={() => {
            void fetchLogs({ level: level || undefined, keyword: keyword.trim() || undefined, limit: 500 })
              .then(page => {
                const text = (page.entries || [])
                  .map(item => `${formatClock(item.ts)} [${item.level}] ${item.category || ''} ${item.message}`)
                  .join('\n')
                void navigator.clipboard.writeText(text).then(
                  () => store.toast(t('已复制 {n} 条日志', { n: (page.entries || []).length })),
                  () => store.toast(t('复制失败'), 'err'),
                )
              })
              .catch(() => store.toast(t('读取日志失败'), 'err'))
          }}
        >
          {t('复制当前筛选结果')}
        </button>
      </div>
    </div>
  )
}

function levelBg(level?: string): string {
  if (level === 'error') return 'var(--m-danger-soft)'
  if (level === 'warn') return 'var(--m-warn-soft)'
  if (level === 'debug') return 'var(--m-surface-3)'
  return 'var(--m-primary-soft)'
}

function levelFg(level?: string): string {
  if (level === 'error') return 'var(--m-danger)'
  if (level === 'warn') return 'var(--m-warn)'
  if (level === 'debug') return 'var(--m-text-3)'
  return 'var(--m-primary)'
}
