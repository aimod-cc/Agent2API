import { useState } from 'react'
import { PageIntro, IconButton, Empty, ErrorNote, SkeletonList } from '../components/ui'
import { ProviderLogo } from '../components/ProviderLogo'
import { Icon } from '../components/Icon'
import { Sheet, SheetContent, SheetHeader, SheetBody, SheetTitle, SheetFooter } from '@ui'
import { store } from '../state/store'
import { fetchRequests, type RequestPage, type RequestRow } from '../api/gateway'
import { ellipsisMiddle, formatClock, formatDuration, formatInt } from '../lib/format'
import { providerLabel } from '../lib/providers'
import { usePolling } from '../lib/usePolling'
import { t } from '../i18n'

/**
 * 请求日志。
 *
 * ── 九列宽表 → 一请求一卡 ────────────────────────────────────
 * 桌面端是「时间 / 提供商·账号 / 重试 / 状态 / 模型 / 客户端 / 用时 / 用量 /
 * 错误 / 详情」十列。手机上每张卡三行：
 *   ① 状态码 + 模型 + 时间
 *   ② 提供商图标 + 名称 + 账号
 *   ③ 用时 · Token · 客户端 · 重试次数
 * 换号链、退避重试、上游报错原本要悬停才看得到，手机上直接铺成卡内提示条
 * —— 这是排查时最需要的信息。
 */

type Filter = 'all' | 'error' | 'retry' | 'running'

const PAGE_SIZE = 20

export function RequestsPage() {
  const [filter, setFilter] = useState<Filter>('all')
  const [page, setPage] = useState<RequestPage | null>(null)
  const [extra, setExtra] = useState<RequestRow[]>([])
  const [error, setError] = useState('')
  const [detail, setDetail] = useState<RequestRow | null>(null)
  const [loadingMore, setLoadingMore] = useState(false)

  const load = async () => {
    try {
      const next = await fetchRequests({
        limit: PAGE_SIZE,
        status: filter === 'error' ? 'error' : undefined,
      })
      setPage(next)
      setExtra([])
      setError('')
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
    }
  }

  usePolling(load, 30_000, [filter])

  const rows = [...(page?.entries || []), ...extra]
  const filtered = rows.filter(row => {
    if (filter === 'retry') return (row.attempts || 1) > 1
    if (filter === 'running') return !row.status
    return true
  })

  const loadMore = async () => {
    setLoadingMore(true)
    try {
      const next = await fetchRequests({ offset: rows.length, limit: PAGE_SIZE })
      setExtra(prev => [...prev, ...(next.entries || [])])
    } catch (failure) {
      store.toast(failure instanceof Error ? failure.message : String(failure), 'err')
    } finally {
      setLoadingMore(false)
    }
  }

  const total = Number(page?.total) || 0
  const matched = Number(page?.matched) || 0
  const running = Number(page?.running) || 0

  return (
    <div className='m-page'>
      <PageIntro actions={<IconButton name='refresh' label={t('刷新')} onClick={() => void load()} />}
      />

      <div className='m-section' style={{ marginTop: 0 }}>
        <div
          style={{
            display: 'flex',
            gap: 2,
            padding: 3,
            borderRadius: 11,
            background: 'var(--m-surface-3)',
          }}
        >
          {(
            [
              { value: 'all', label: t('全部') },
              { value: 'error', label: t('失败') },
              { value: 'retry', label: t('有重试') },
              { value: 'running', label: running ? t('在途 {n}', { n: running }) : t('在途') },
            ] as Array<{ value: Filter; label: string }>
          ).map(item => (
            <button
              key={item.value}
              type='button'
              onClick={() => setFilter(item.value)}
              style={{
                flex: 1,
                height: 30,
                borderRadius: 8,
                border: 0,
                fontSize: 12.5,
                background: filter === item.value ? 'var(--m-surface)' : 'transparent',
                color: filter === item.value ? 'var(--m-text)' : 'var(--m-text-2)',
                fontWeight: filter === item.value ? 600 : 400,
                boxShadow: filter === item.value ? 'var(--m-shadow-1)' : 'none',
              }}
            >
              {item.label}
            </button>
          ))}
        </div>
      </div>

      {error ? <ErrorNote text={error} /> : null}
      {!page ? <SkeletonList rows={4} height={104} /> : null}

      {page && !filtered.length ? (
        <div className='m-section'>
          <div className='m-card'>
            <Empty text={filter === 'all' ? t('暂无请求日志') : t('当前筛选下没有记录')} />
          </div>
        </div>
      ) : null}

      <div className='m-section' style={{ marginTop: 12 }}>
        {filtered.map((row, index) => (
          <RequestCard key={row.id || `${row.ts}-${index}`} row={row} onClick={() => setDetail(row)} />
        ))}
      </div>

      {page && rows.length < matched ? (
        <div className='m-section'>
          <button type='button' className='m-btn m-ghost' disabled={loadingMore} onClick={() => void loadMore()}>
            {loadingMore ? t('加载中…') : t('加载更早的请求')}
          </button>
        </div>
      ) : null}

      <RequestSheet row={detail} onClose={() => setDetail(null)} />
    </div>
  )
}

function RequestCard({ row, onClick }: { row: RequestRow; onClick: () => void }) {
  const status = Number(row.status) || 0
  const tone = !status ? 'warn' : status >= 200 && status < 400 ? 'ok' : 'bad'
  const note = buildNote(row)
  return (
    <button
      type='button'
      onClick={onClick}
      style={{
        display: 'block',
        width: '100%',
        textAlign: 'left',
        padding: '11px 13px',
        marginBottom: 9,
        borderRadius: 'var(--m-r-md)',
        background: 'var(--m-surface)',
        border: '1px solid var(--m-line)',
        boxShadow: 'var(--m-shadow-1)',
      }}
    >
      <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
        <span
          className='m-mono'
          style={{
            fontSize: 11,
            fontWeight: 600,
            padding: '2px 6px',
            borderRadius: 5,
            flex: 'none',
            background:
              tone === 'ok' ? 'var(--m-ok-soft)' : tone === 'warn' ? 'var(--m-warn-soft)' : 'var(--m-danger-soft)',
            color: tone === 'ok' ? 'var(--m-ok)' : tone === 'warn' ? 'var(--m-warn)' : 'var(--m-danger)',
          }}
        >
          {status || t('在途')}
        </span>
        <span
          className='m-mono'
          style={{ flex: 1, minWidth: 0, fontSize: 12, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}
        >
          {ellipsisMiddle(row.upstreamModel || row.model || '—', 30)}
        </span>
        <span style={{ fontSize: 11, color: 'var(--m-text-3)', flex: 'none', fontVariantNumeric: 'tabular-nums' }}>
          {formatClock(row.ts)}
        </span>
      </div>

      <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginTop: 8, fontSize: 11.5, color: 'var(--m-text-3)', minWidth: 0 }}>
        <ProviderLogo id={row.provider} />
        <span style={{ flex: 'none' }}>{providerLabel(row.provider, row.providerLabel)}</span>
        {row.accountName ? (
          <>
            <span style={{ opacity: 0.45 }}>·</span>
            <span style={{ minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
              {row.accountName}
            </span>
          </>
        ) : null}
      </div>

      <div style={{ display: 'flex', alignItems: 'center', gap: 6, marginTop: 4, fontSize: 11, color: 'var(--m-text-3)', fontVariantNumeric: 'tabular-nums' }}>
        <span>{formatDuration(row.durationMs)}</span>
        <span style={{ opacity: 0.45 }}>·</span>
        <span>{formatInt(Number(row.totalTokens) || 0)} tokens</span>
        {(row.attempts || 1) > 1 ? (
          <>
            <span style={{ opacity: 0.45 }}>·</span>
            <span>{t('重试 {n}', { n: (row.attempts || 1) - 1 })}</span>
          </>
        ) : null}
      </div>

      {note ? (
        <div
          style={{
            marginTop: 8,
            padding: '7px 9px',
            borderRadius: 8,
            fontSize: 11.3,
            lineHeight: 1.5,
            background:
              note.tone === 'bad' ? 'var(--m-danger-soft)' : note.tone === 'warn' ? 'var(--m-warn-soft)' : 'var(--m-surface-2)',
            color:
              note.tone === 'bad'
                ? 'color-mix(in srgb, var(--m-danger) 72%, var(--m-text))'
                : note.tone === 'warn'
                  ? 'color-mix(in srgb, var(--m-warn) 72%, var(--m-text))'
                  : 'var(--m-text-2)',
          }}
        >
          {note.text}
        </div>
      ) : null}
    </button>
  )
}

function buildNote(row: RequestRow): { text: string; tone: 'mute' | 'warn' | 'bad' } | null {
  const attempts = row.attempts || 1
  const details = Array.isArray(row.attemptDetails) ? row.attemptDetails : []
  const failed = details.filter(item => Number(item.status) >= 400)
  if (row.error) {
    return { text: row.error, tone: Number(row.status) >= 400 ? 'bad' : 'warn' }
  }
  if (attempts > 1) {
    const chain = details
      .map(item => `${providerLabel(item.provider)} ${item.status || ''}`.trim())
      .join(' → ')
    return {
      text: chain ? t('换号链：{chain}', { chain }) : t('重试 {n} 次', { n: attempts - 1 }),
      tone: failed.length ? 'warn' : 'mute',
    }
  }
  const hits = Array.isArray(row.sensitiveHits) ? row.sensitiveHits : []
  if (hits.length) {
    return { text: t('脱敏命中：{words}', { words: hits.map(item => item.word).join('、') }), tone: 'mute' }
  }
  if (row.clientModel && row.upstreamModel && row.clientModel !== row.upstreamModel) {
    return { text: t('模型映射：{from} → {to}', { from: row.clientModel, to: row.upstreamModel }), tone: 'mute' }
  }
  return null
}

function RequestSheet({ row, onClose }: { row: RequestRow | null; onClose: () => void }) {
  const details = Array.isArray(row?.attemptDetails) ? row?.attemptDetails || [] : []
  return (
    <Sheet open={Boolean(row)} onOpenChange={next => { if (!next) onClose() }}>
      <SheetContent>
        <SheetHeader>
          <SheetTitle>{t('请求详情')}</SheetTitle>
        </SheetHeader>
        <SheetBody>
          {row ? (
            <>
              <div className='m-kv'>
                <div className='m-kv-cell'>
                  <div className='k'>{t('状态')}</div>
                  <div
                    className='v'
                    style={{
                      color:
                        !row.status
                          ? 'var(--m-warn)'
                          : row.status < 400
                            ? 'var(--m-ok)'
                            : 'var(--m-danger)',
                    }}
                  >
                    {row.status || t('在途')}
                  </div>
                </div>
                <div className='m-kv-cell'>
                  <div className='k'>{t('用时')}</div>
                  <div className='v'>{formatDuration(row.durationMs)}</div>
                </div>
                <div className='m-kv-cell'>
                  <div className='k'>{t('输入 Token')}</div>
                  <div className='v'>{formatInt(Number(row.promptTokens) || 0)}</div>
                </div>
                <div className='m-kv-cell'>
                  <div className='k'>{t('输出 Token')}</div>
                  <div className='v'>{formatInt(Number(row.completionTokens) || 0)}</div>
                </div>
              </div>

              <div className='m-rows'>
                <div className='m-row'>
                  <span className='m-k'>{t('模型')}</span>
                  <span className='m-v m-mono' style={{ fontSize: 11 }}>
                    {row.model || '—'}
                  </span>
                </div>
                {row.clientModel ? (
                  <div className='m-row'>
                    <span className='m-k'>{t('客户端请求的模型')}</span>
                    <span className='m-v m-mono' style={{ fontSize: 11 }}>
                      {row.clientModel}
                    </span>
                  </div>
                ) : null}
                {row.upstreamModel ? (
                  <div className='m-row'>
                    <span className='m-k'>{t('发给上游的模型')}</span>
                    <span className='m-v m-mono' style={{ fontSize: 11 }}>
                      {row.upstreamModel}
                    </span>
                  </div>
                ) : null}
                <div className='m-row'>
                  <span className='m-k'>{t('提供商')}</span>
                  <span className='m-v'>{providerLabel(row.provider, row.providerLabel)}</span>
                </div>
                <div className='m-row'>
                  <span className='m-k'>{t('账号')}</span>
                  <span className='m-v'>{row.accountName || '—'}</span>
                </div>
                <div className='m-row'>
                  <span className='m-k'>{t('尝试次数')}</span>
                  <span className='m-v'>{row.attempts || 1}</span>
                </div>
                <div className='m-row'>
                  <span className='m-k'>{t('首响')}</span>
                  <span className='m-v'>{formatDuration(row.firstResponseMs ?? null)}</span>
                </div>
                <div className='m-row'>
                  <span className='m-k'>{t('时间')}</span>
                  <span className='m-v m-mono' style={{ fontSize: 11 }}>
                    {row.ts ? new Date(row.ts).toLocaleString('zh-CN', { hour12: false }) : '—'}
                  </span>
                </div>
              </div>

              {details.length ? (
                <>
                  <div style={{ fontSize: 12, fontWeight: 600, color: 'var(--m-text-2)' }}>{t('尝试链')}</div>
                  <div className='m-rows'>
                    {details.map((item, index) => (
                      <div key={index} className='m-row'>
                        <span className='m-k'>#{index + 1}</span>
                        <span className='m-v' style={{ flex: 1, minWidth: 0, textAlign: 'left' }}>
                          {providerLabel(item.provider)} · {item.status || '—'}
                          {item.error ? (
                            <span style={{ display: 'block', color: 'var(--m-danger)', fontSize: 11, marginTop: 2 }}>
                              {item.error}
                            </span>
                          ) : null}
                        </span>
                      </div>
                    ))}
                  </div>
                </>
              ) : null}

              {row.error ? <div className='m-error'>{row.error}</div> : null}
            </>
          ) : null}
        </SheetBody>
        <SheetFooter>
          <button type='button' className='m-btn m-ghost' onClick={onClose}>
            {t('关闭')}
          </button>
        </SheetFooter>
      </SheetContent>
    </Sheet>
  )
}

