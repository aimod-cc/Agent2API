import { useCallback, useEffect, useState } from 'react'
import {
  clearCheckinRecords,
  getCheckinRecords,
  type CheckinRecordFilters,
  type CheckinRecordRange,
  type RecordRow,
} from '../core/api'
import { formatDateTime } from '../core/format'
import { t } from '../core/i18n'
import { store } from '../core/store'
import { BottomSheet, CommonCard, EmptyBlock, SearchField, TonalChip } from '../ui/components'
import { GlyphIcon } from '../ui/icons'
import { PickerSheet, type PickerOption } from '../ui/pickers'

const PAGE_SIZE = 25

const RANGE_OPTIONS: PickerOption<CheckinRecordRange>[] = [
  { value: 'today', label: '今天' },
  { value: '7d', label: '近 7 天' },
  { value: '30d', label: '近 30 天' },
  { value: 'all', label: '全部时间' },
]

const OUTCOME_OPTIONS: PickerOption[] = [
  { value: 'all', label: '全部结果' },
  { value: 'succeeded', label: '仅成功' },
  { value: 'skipped', label: '已跳过' },
  { value: 'failed', label: '仅失败' },
]

const KIND_OPTIONS: PickerOption[] = [
  { value: 'all', label: '全部来源' },
  { value: 'schedule', label: '定时执行' },
  { value: 'manual', label: '手动执行' },
  { value: 'keepalive', label: '对话保活' },
  { value: 'travel', label: '猫猫旅行' },
  { value: 'plugin', label: '插件任务' },
]

export function CheckinRecordsStickyControls({
  filters,
  providerOptions,
  onChange,
  onClearRequest,
}: {
  filters: CheckinRecordFilters
  providerOptions: ReadonlyArray<{ id: string; label: string }>
  onChange: (patch: Partial<CheckinRecordFilters>) => void
  onClearRequest: () => void
}) {
  const [picker, setPicker] = useState<'range' | 'outcome' | 'kind' | 'provider' | null>(null)

  const rangeLabel = RANGE_OPTIONS.find(x => x.value === (filters.range || 'today'))?.label || t('今天')
  const outcomeLabel = OUTCOME_OPTIONS.find(x => x.value === (filters.outcome || 'all'))?.label || t('全部结果')
  const kindLabel = KIND_OPTIONS.find(x => x.value === (filters.kind || 'all'))?.label || t('全部来源')
  const providerList: PickerOption[] = [
    { value: 'all', label: t('全部提供商') },
    ...providerOptions.map(p => ({ value: p.id, label: p.label })),
  ]
  const providerLabel = providerList.find(x => x.value === (filters.provider || 'all'))?.label || t('全部提供商')

  return (
    <>
      <SearchField
        value={filters.keyword || ''}
        onChange={keyword => onChange({ keyword })}
        placeholder={t('搜索账号或签到说明')}
      />
      <div className='fc-filter-bar'>
        <button type='button' className='fc-filter-pill is-active' onClick={() => setPicker('range')}>
          <span>{t(rangeLabel)}</span>
          <GlyphIcon name='chevron-down' size={14} />
        </button>
        <button
          type='button'
          className={`fc-filter-pill ${(filters.outcome || 'all') !== 'all' ? 'is-active' : ''}`}
          onClick={() => setPicker('outcome')}
        >
          <span>{t(outcomeLabel)}</span>
          <GlyphIcon name='chevron-down' size={14} />
        </button>
        <button
          type='button'
          className={`fc-filter-pill ${(filters.kind || 'all') !== 'all' ? 'is-active' : ''}`}
          onClick={() => setPicker('kind')}
        >
          <span>{t(kindLabel)}</span>
          <GlyphIcon name='chevron-down' size={14} />
        </button>
        <button
          type='button'
          className={`fc-filter-pill ${(filters.provider || 'all') !== 'all' ? 'is-active' : ''}`}
          onClick={() => setPicker('provider')}
        >
          <span>{providerLabel}</span>
          <GlyphIcon name='chevron-down' size={14} />
        </button>
        <button type='button' className='fc-filter-pill' onClick={onClearRequest}>
          <GlyphIcon name='trash' size={14} />
          <span>{t('清空')}</span>
        </button>
      </div>

      <PickerSheet
        open={picker === 'range'}
        title={t('时间范围')}
        options={RANGE_OPTIONS.map(o => ({ ...o, label: t(o.label) }))}
        value={filters.range || 'today'}
        onSelect={v => onChange({ range: v as CheckinRecordRange })}
        onClose={() => setPicker(null)}
      />
      <PickerSheet
        open={picker === 'outcome'}
        title={t('执行结果')}
        options={OUTCOME_OPTIONS.map(o => ({ ...o, label: t(o.label) }))}
        value={filters.outcome || 'all'}
        onSelect={outcome => onChange({ outcome })}
        onClose={() => setPicker(null)}
      />
      <PickerSheet
        open={picker === 'kind'}
        title={t('记录来源')}
        options={KIND_OPTIONS.map(o => ({ ...o, label: t(o.label) }))}
        value={filters.kind || 'all'}
        onSelect={kind => onChange({ kind })}
        onClose={() => setPicker(null)}
      />
      <PickerSheet
        open={picker === 'provider'}
        title={t('按提供商筛选')}
        options={providerList}
        value={filters.provider || 'all'}
        onSelect={provider => onChange({ provider })}
        onClose={() => setPicker(null)}
      />
    </>
  )
}

function outcomeBadge(outcome?: string): { label: string; tone: 'success' | 'warning' | 'danger' | 'neutral' } {
  if (outcome === 'succeeded' || outcome === 'claimed' || outcome === 'ok') return { label: t('成功'), tone: 'success' }
  if (outcome === 'skipped') return { label: t('跳过'), tone: 'neutral' }
  if (outcome === 'confirming') return { label: t('待确认'), tone: 'warning' }
  if (outcome === 'failed' || outcome === 'error') return { label: t('失败'), tone: 'danger' }
  return { label: outcome || t('未知'), tone: 'neutral' }
}

export function CheckinRecordsPanel({
  filters,
  clearPromptOpen,
  onCloseClearPrompt,
  refreshNonce,
}: {
  filters: CheckinRecordFilters
  clearPromptOpen: boolean
  onCloseClearPrompt: () => void
  refreshNonce: number
}) {
  const [rows, setRows] = useState<RecordRow[]>([])
  const [total, setTotal] = useState(0)
  const [page, setPage] = useState(0)
  const [loading, setLoading] = useState(true)
  const [clearing, setClearing] = useState(false)

  useEffect(() => {
    setPage(0)
  }, [filters.range, filters.outcome, filters.kind, filters.provider, filters.keyword])

  const load = useCallback(async (signal?: AbortSignal) => {
    setLoading(true)
    try {
      const res = await getCheckinRecords(
        {
          ...filters,
          offset: page * PAGE_SIZE,
          limit: PAGE_SIZE,
        },
        signal,
      )
      setRows(res.records || [])
      setTotal(res.total || 0)
    } catch (err) {
      if (!signal?.aborted) {
        store.toast(err instanceof Error ? err.message : String(err), 'danger')
      }
    } finally {
      if (!signal?.aborted) setLoading(false)
    }
  }, [filters, page])

  useEffect(() => {
    const ac = new AbortController()
    void load(ac.signal)
    return () => ac.abort()
  }, [load, refreshNonce])

  const totalPages = Math.max(1, Math.ceil(total / PAGE_SIZE))

  const doClear = async (all: boolean) => {
    if (clearing) return
    setClearing(true)
    try {
      const res = await clearCheckinRecords(filters, all)
      onCloseClearPrompt()
      store.toast(t('已清理 {n} 条签到记录', { n: res.removed ?? 0 }), 'success')
      setPage(0)
      await load()
    } catch (err) {
      store.toast(err instanceof Error ? err.message : String(err), 'danger')
    } finally {
      setClearing(false)
    }
  }

  return (
    <>
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '0 2px' }}>
        <span style={{ fontSize: 12.5, color: 'var(--fc-text-muted)', fontWeight: 600 }}>
          {t('共 {n} 条记录', { n: total })}
        </span>
        {total > PAGE_SIZE ? (
          <span style={{ fontSize: 12.5, color: 'var(--fc-text-muted)' }}>
            {page + 1} / {totalPages}
          </span>
        ) : null}
      </div>

      {loading && !rows.length ? (
        <EmptyBlock icon='clock' title={t('正在加载签到记录…')} />
      ) : !rows.length ? (
        <EmptyBlock icon='history' title={t('暂无符合条件的签到记录')} />
      ) : (
        rows.map((row, idx) => {
          const badge = outcomeBadge(row.outcome)
          return (
            <CommonCard key={row.rowId ?? `${row.ts}-${idx}`}>
              <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8 }}>
                <div style={{ fontSize: 14.5, fontWeight: 700, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                  {row.accountName || row.accountId || t('系统任务')}
                </div>
                <TonalChip tone={badge.tone}>{badge.label}</TonalChip>
              </div>
              <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6, marginTop: 6 }}>
                {row.provider ? <TonalChip>{row.provider}</TonalChip> : null}
                {row.source ? <TonalChip>{row.source}</TonalChip> : null}
                {row.keptActive ? <TonalChip tone='primary'>{t('已保活')}</TonalChip> : null}
                <span style={{ fontSize: 12, color: 'var(--fc-text-dim)', alignSelf: 'center' }}>
                  {formatDateTime(row.ts)}
                </span>
              </div>
              {row.message || row.error || row.reason ? (
                <div
                  style={{
                    fontSize: 12.5,
                    color: row.error ? 'var(--fc-danger)' : 'var(--fc-text-muted)',
                    marginTop: 6,
                    lineHeight: 1.45,
                  }}
                >
                  {row.error || row.message || row.reason}
                </div>
              ) : null}
            </CommonCard>
          )
        })
      )}

      {totalPages > 1 ? (
        <div style={{ display: 'flex', gap: 10 }}>
          <button
            type='button'
            className='fc-btn is-sm'
            style={{ flex: 1 }}
            disabled={page === 0 || loading}
            onClick={() => setPage(p => Math.max(0, p - 1))}
          >
            {t('上一页')}
          </button>
          <button
            type='button'
            className='fc-btn is-sm'
            style={{ flex: 1 }}
            disabled={page + 1 >= totalPages || loading}
            onClick={() => setPage(p => p + 1)}
          >
            {t('下一页')}
          </button>
        </div>
      ) : null}

      <BottomSheet
        open={clearPromptOpen}
        title={t('清理签到记录')}
        onClose={onCloseClearPrompt}
        footer={
          <div style={{ display: 'flex', gap: 10, width: '100%' }}>
            <button type='button' className='fc-btn' style={{ flex: 1 }} onClick={() => void doClear(false)} disabled={clearing}>
              {t('仅清当前筛选')}
            </button>
            <button type='button' className='fc-btn is-danger' style={{ flex: 1 }} onClick={() => void doClear(true)} disabled={clearing}>
              {t('清空全部记录')}
            </button>
          </div>
        }
      >
        <div style={{ fontSize: 14, color: 'var(--fc-text-muted)', lineHeight: 1.5 }}>
          {t('你可以仅删除当前筛选命中的记录，或清空所有历史签到明细。清空后无法恢复。')}
        </div>
      </BottomSheet>
    </>
  )
}
