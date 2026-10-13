import * as React from 'react'
import { Badge, Button, Input, Pager, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Spinner } from '@ui'
import { formatTime, shared } from './accounts-shared'
import { t } from '../i18n'
import { getCheckinStore, type CheckinRecordRow } from './checkin-state'

/**
 * 签到中心的「签到记录」标签页：每账号一行的**明细台账**（schema v11）。
 *
 * ── 这页回答什么 ────────────────────────────────────────────
 * 「到底领到了没、失败的话错在哪、领了多少」—— 这些在改造前只有一条聚合的
 * lastResult 快照与转瞬即逝的运行日志，现在每执行一次签到（定时条目 / 启动补签 /
 * 手动批量 / 单账号 / 国际版活动），每个账号落一行：时间、来源、账号、提供商、
 * 结果档位、收益与错误原文。判据在后端唯一（复用签到执行体的同一对函数），
 * 界面只按 `outcome` 上色，不自己再判一遍。
 *
 * ── 数据 ────────────────────────────────────────────────────
 * GET /api/checkin-records（limit / offset 真分页 + outcome / provider / keyword
 * 筛选），响应 `{records, total}`：total 是同条件总数（Pager 与「清空」共用）。
 * 挂载时拉一次；无轮询 —— 签到动作后切回本页（重新挂载）即刷新，或手点「刷新」。
 *
 * ── 清空的护栏 ──────────────────────────────────────────────
 * 与运行日志同一条规则：带筛选 = 只删筛选命中的（后端与查询共用同一份过滤链）；
 * 不带筛选必须显式 `all=1`（后端护栏），且前端先过 wbConfirm —— Tauri 的
 * WebView 里原生 confirm 不弹窗直接放行，危险确认一律走自绘弹窗（见 confirm-dialog）。
 */

const PAGE_SIZE = 50

/** 结果档位的界面文案与徽章变体（outcome 由后端判，这里只管长得不一样） */
const OUTCOME_VIEW: Record<string, { label: string; variant: 'success' | 'brand' | 'warning' | 'destructive' }> = {
  success: { label: '成功领取', variant: 'success' },
  already: { label: '今日已领', variant: 'brand' },
  pending: { label: '等待窗口', variant: 'warning' },
  failed: { label: '失败', variant: 'destructive' },
}

/** 触发来源的界面文案（source 由后端给，值集合见 core::checkin_records 的模块头） */
const SOURCE_LABEL: Record<string, string> = {
  schedule: '定时',
  startup: '补签',
  manual: '手动',
  single: '单签',
  activity: '活动',
}

type Filters = { outcome: string; provider: string; keyword: string }

function RecordsTab(): React.ReactElement {
  const [rows, setRows] = React.useState<CheckinRecordRow[]>([])
  const [total, setTotal] = React.useState(0)
  const [page, setPage] = React.useState(1) // 1 起数（Pager 契约）
  const [filters, setFilters] = React.useState<Filters>({ outcome: '', provider: '', keyword: '' })
  /** 提交中的关键词（输入框即时改 filters.keyword 会边打字边查询，落焦/回车才提交） */
  const [keywordDraft, setKeywordDraft] = React.useState('')
  const [loading, setLoading] = React.useState(false)
  const [error, setError] = React.useState('')
  // 提供商下拉的选项来自签到中心快照（后端 CHECKIN_PROVIDERS 清单 + 展示名）
  const providerOptions = getCheckinStore().snapshot?.auto?.providerOptions ?? []
  const providerLabel = (id: string): string =>
    providerOptions.find(option => option.id === id)?.label ?? id

  const pageCount = Math.max(1, Math.ceil(total / PAGE_SIZE))
  const hasFilters = filters.outcome !== '' || filters.provider !== '' || filters.keyword !== ''

  const load = React.useCallback(async (target: {
    page: number
    filters: Filters
  }): Promise<void> => {
    setLoading(true)
    setError('')
    try {
      const api = shared().workbuddyDesktop as {
        getCheckinRecords?: (params: Record<string, unknown>) => Promise<{ records?: CheckinRecordRow[]; total?: number } | null | undefined>
      } | undefined
      const data = await api?.getCheckinRecords?.({
        limit: PAGE_SIZE,
        offset: (target.page - 1) * PAGE_SIZE,
        outcome: target.filters.outcome,
        provider: target.filters.provider,
        keyword: target.filters.keyword,
      })
      setRows(Array.isArray(data?.records) ? data!.records! : [])
      setTotal(Number(data?.total) || 0)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught))
    } finally {
      setLoading(false)
    }
  }, [])

  // 挂载拉一次；筛选 / 翻页变化时重拉
  React.useEffect(() => { void load({ page, filters }) }, [load, page, filters])

  /** 筛选变更：回到第 1 页（结果集已经换了一个总体，页码没有保留意义） */
  const applyFilters = (next: Filters): void => {
    setFilters(next)
    setPage(1)
  }

  /** 清空（带筛选删命中，不带删全部 —— 两条提示语分开说，与运行日志同款） */
  const clearRecords = async (): Promise<void> => {
    const ask = shared().wbConfirm?.ask
    if (!ask) return
    const message = hasFilters
      ? t('确定清空当前筛选出的签到记录？清空后无法恢复。')
      : t('确定清空<strong>全部</strong>签到记录？清空后无法恢复。')
    if (!(await ask({ title: t('清空签到记录'), html: message, okText: t('清空'), okClass: 'danger' }))) return
    try {
      const api = shared().workbuddyDesktop as {
        clearCheckinRecords?: (params: Record<string, unknown>) => Promise<{ removed?: number } | null | undefined>
      } | undefined
      // 与后端护栏对齐：无筛选必须显式 all=1（漏传会被 400 拦下，这里不给它机会）
      await api?.clearCheckinRecords?.(hasFilters
        ? { outcome: filters.outcome, provider: filters.provider, keyword: filters.keyword }
        : { all: '1' })
      toast('签到记录已清空')
      await load({ page: 1, filters })
      setPage(1)
    } catch (caught) {
      toast(t('清空失败：{error}', { error: caught instanceof Error ? caught.message : String(caught) }), 'err')
    }
  }

  return (
    <section className='panel'>
      <div className='panel-head'>
        <h2>{t('签到记录')}</h2>
        <span className='panel-sub'>{t('每执行一次签到 · 每个账号一条明细 · 结果与收益以后端落账为准')}</span>
        <div className='ml-auto flex items-center gap-2'>
          <Button size='sm' variant='outline' disabled={loading} onClick={() => void load({ page, filters })}>
            {loading ? t('刷新中…') : t('刷新')}
          </Button>
          <Button size='sm' variant='ghost' className='text-destructive' onClick={() => void clearRecords()}>
            {t('清空')}
          </Button>
        </div>
      </div>

      {/* ── 筛选行：结果 / 提供商 / 关键词 ── */}
      <div className='ck-records-filters'>
        <Select value={filters.outcome} onValueChange={value => applyFilters({ ...filters, outcome: value ?? '' })}>
          <SelectTrigger className='w-[128px]' aria-label={t('按结果筛选')}>
            <SelectValue>{filters.outcome ? OUTCOME_VIEW[filters.outcome]?.label ?? filters.outcome : t('全部结果')}</SelectValue>
          </SelectTrigger>
          <SelectContent>
            <SelectItem value=''>{t('全部结果')}</SelectItem>
            {Object.entries(OUTCOME_VIEW).map(([value, view]) => (
              <SelectItem key={value} value={value}>{view.label}</SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Select value={filters.provider} onValueChange={value => applyFilters({ ...filters, provider: value ?? '' })}>
          <SelectTrigger className='w-[148px]' aria-label={t('按提供商筛选')}>
            <SelectValue>{filters.provider ? providerLabel(filters.provider) : t('全部提供商')}</SelectValue>
          </SelectTrigger>
          <SelectContent>
            <SelectItem value=''>{t('全部提供商')}</SelectItem>
            {providerOptions.map(option => (
              <SelectItem key={option.id} value={option.id}>{option.label}</SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Input
          type='search'
          className='ck-records-search'
          placeholder={t('搜账号 / 收益 / 错误 / 触发原因')}
          value={keywordDraft}
          onChange={event => setKeywordDraft(event.currentTarget.value)}
          onBlur={() => { if (keywordDraft !== filters.keyword) applyFilters({ ...filters, keyword: keywordDraft.trim() }) }}
          onKeyDown={event => {
            if (event.key === 'Enter') event.currentTarget.blur()
            if (event.key === 'Escape') { setKeywordDraft(filters.keyword); event.currentTarget.blur() }
          }}
        />
        {hasFilters ? (
          <Button
            size='sm'
            variant='ghost'
            onClick={() => { setKeywordDraft(''); applyFilters({ outcome: '', provider: '', keyword: '' }) }}
          >
            {t('清除筛选')}
          </Button>
        ) : null}
        <span className='panel-sub'>{loading ? t('读取中…') : t('{n} 条', { n: total })}</span>
      </div>

      {error ? (
        <div className='ck-page-error'>
          <span>{t('读取签到记录失败：{error}', { error })}</span>
          <Button size='sm' variant='outline' onClick={() => void load({ page, filters })}>{t('重试')}</Button>
        </div>
      ) : null}

      {rows.length === 0 && !loading ? (
        <div className='ck-empty'>{t('还没有签到记录 —— 手动签到或等定时条目执行后，明细会出现在这里。')}</div>
      ) : (
        <div className='ck-records-scroll'>
          <table className='ck-table ck-records-table'>
            <thead>
              <tr>
                <th className='ck-col-time'>{t('时间')}</th>
                <th>{t('账号')}</th>
                <th>{t('提供商')}</th>
                <th>{t('来源')}</th>
                <th>{t('结果')}</th>
                <th>{t('说明')}</th>
              </tr>
            </thead>
            <tbody>
              {rows.map(row => {
                const outcome = OUTCOME_VIEW[row.outcome ?? ''] ?? { label: row.outcome || '—', variant: 'outline' as const }
                const detail = row.error || row.message || ''
                return (
                  <tr key={row.rowId}>
                    <td className='ck-col-time'>
                      {row.ts ? formatTime(row.ts) : '—'}
                      <span className='ck-record-date'>{row.date || ''}</span>
                    </td>
                    <td>{row.accountName || row.accountId || '—'}</td>
                    <td>{row.provider ? providerLabel(row.provider) : '—'}</td>
                    <td>
                      <span className='ck-record-source'>{SOURCE_LABEL[row.source ?? ''] ?? row.source ?? '—'}</span>
                      {row.reason ? <span className='ck-record-reason'>{row.reason}</span> : null}
                    </td>
                    <td>
                      <Badge variant={outcome.variant} shape='tag'>{outcome.label}</Badge>
                      {row.keptActive ? <Badge variant='outline' shape='tag' className='ml-1'>{t('保活')}</Badge> : null}
                    </td>
                    <td className={row.error ? 'ck-record-detail-fail' : ''}>{detail || '—'}</td>
                  </tr>
                )
              })}
            </tbody>
          </table>
        </div>
      )}

      <div className='ck-records-foot'>
        <Pager page={page} pageCount={pageCount} onPageChange={setPage} disabled={loading} />
      </div>
    </section>
  )
}

/** toast 的窄封装（accounts-shared 的 toast 需要参数对象，这里收窄成惯例签名） */
function toast(message: string, kind?: 'err'): void {
  shared().wbApp?.toast?.(message, kind)
}

export { RecordsTab }
