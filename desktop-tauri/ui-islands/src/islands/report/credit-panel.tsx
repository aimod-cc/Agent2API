import * as React from 'react'
import { Card, CardContent, CardHeader, CardTitle, Empty, Meter, Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@ui'
import { t } from '../../i18n'

/** GET /api/stats/credits 的 accounts[] 里的一项（见 core::credit_snapshots::report 的文档） */
export type CreditAccount = {
  id: string
  label: string
  /** 区间内消耗的积分（相邻快照已用量增量之和，只累加上升的部分） */
  creditUsed: number
  /**
   * 区间内 `creditUsed` 回落的次数：到期套餐被上游收走（总额同步减少），或额度被重置。
   * 回落那一步的消耗算不出来，读数只能当下限看。充值只抬高余额和总额，不会让已用量回落。
   */
  resets: number
  /** 区间内有只有一条快照的日子：那几天的消耗算不出来 */
  partial: boolean
  daily?: { date: string; creditUsed: number; resets: number; partial: boolean }[]
}

export type CreditReport = {
  range: string
  startDate: string
  endDate: string
  currency: string
  totalCreditUsed: number
  /** 最早一条快照（毫秒）；null 表示还没有数据 */
  firstSnapshotAt: number | null
  accounts: CreditAccount[]
}

const COUNTS = [5, 10, 20, 0] as const

/** 积分数值：保留两位小数，整数部分加千分位。积分是「账面值」，不跟随货币单位。 */
export function formatCredit(value: number): string {
  const rounded = Math.round((Number(value) || 0) * 100) / 100
  return rounded.toLocaleString('zh-CN', { minimumFractionDigits: 0, maximumFractionDigits: 2 })
}

/** 没有名字快照的账号，后端回的是 81 字符的 id：显示末 6 位，悬停看全。
 * 只有确认 label 就是原始 id 时才缩写；正常邮箱和昵称保留完整内容，用 CSS 省略溢出。 */
export function creditAccountShortLabel(label: string, id: string): string {
  if (label === id && label.length > 16) return `…${label.slice(-6)}`
  return label
}

const TIP = t('按账号统计的实测积分消耗，来自定时记录的余额快照。上游按批结算，读数是采样下限而非精确值：跨零点的变化可能未计入任何一天，已用额度回落（套餐到期或重置）时的消耗也算不出来，标「偏低」的账号有这类情况。它记的是账号在上游的总消耗，不只是本网关用掉的部分。')

export function CreditPanel({ report, loading, error }: {
  report: CreditReport | null
  loading: React.ReactNode
  error: string | null
}) {
  const [count, setCount] = React.useState('10')
  const accounts = report?.accounts || []
  const shown = count === '0' ? accounts : accounts.slice(0, Number(count))
  // 进度条的占比按全部账号的最大值算，而不是合计：消耗高度集中在少数账号，
  // 用合计做分母会让第 10 名之后的条全部看不见
  const max = accounts.reduce((top, item) => Math.max(top, item.creditUsed), 0)
  const share = report && report.totalCreditUsed > 0
    ? shown.reduce((sum, item) => sum + item.creditUsed, 0) / report.totalCreditUsed * 100
    : 0
  const countLabel = count === '0' ? t('全部') : t('前 {n} 名', { n: count })
  const since = report?.firstSnapshotAt
    ? new Date(report.firstSnapshotAt).toLocaleDateString('zh-CN')
    : ''
  return <Card render={<section />} className='report-card report-credit-card' id='report-credit-panel'>
    <CardHeader className='report-card-header'>
      <CardTitle>{t('积分消耗')}</CardTitle><span className='tip-q' data-tip={TIP} />
      <Select value={count} onValueChange={value => setCount(String(value ?? '10'))}>
        <SelectTrigger className='report-rank-select ml-auto' aria-label={t('积分消耗显示数量')}><SelectValue>{countLabel}</SelectValue></SelectTrigger>
        <SelectContent>{COUNTS.map(value => <SelectItem key={value} value={String(value)}>{value ? t('前 {n} 名', { n: value }) : t('全部')}</SelectItem>)}</SelectContent>
      </Select>
    </CardHeader>
    <CardContent className='report-card-content'>
      {error ? <Empty className='report-error'>{t('读取积分报表失败：{error}', { error })}</Empty> : null}
      {!error && loading ? loading : null}
      {!error && !loading && report && !accounts.length ? (
        <Empty>{report.firstSnapshotAt
          ? t('所选范围内没有积分消耗')
          : t('还没有积分快照：余额查询成功后开始记录')}</Empty>
      ) : null}
      {!error && !loading && report && accounts.length ? (
        <>
          <div className='report-credit-summary'>
            <span>{t('合计')}</span>
            <strong>{formatCredit(report.totalCreditUsed)}</strong>
            <span>{t('积分')}</span>
            {report.startDate && report.endDate ? (
              <small className='report-credit-range'>{report.startDate} — {report.endDate}</small>
            ) : null}
            {since ? <small>{t('自 {date} 起记录', { date: since })}</small> : null}
          </div>
          <div className='report-rank-list' id='report-credits'>
            {shown.map((row, index) => {
              const short = creditAccountShortLabel(row.label, row.id)
              return <div className='report-rank-row' key={row.id}>
                <span className='report-rank-order'>{String(index + 1).padStart(2, '0')}</span>
                <div className='report-rank-name'>
                  <span title={row.label}>{short}</span>
                  {row.resets > 0 ? <span className='report-credit-flag' title={t('期间有 {n} 次已用额度回落（套餐到期或额度被重置），读数比实际略低', { n: row.resets })}>{t('偏低')}</span> : null}
                  {row.partial ? <span className='report-credit-flag' title={t('有只有一条快照的日子，那几天的消耗算不出来')}>{t('不完整')}</span> : null}
                </div>
                <Meter value={max ? row.creditUsed / max * 100 : 0} className='report-rank-meter'
                  trackClassName='report-rank-track' indicatorClassName='report-rank-fill' aria-label={row.label} />
                <strong className='report-rank-amount'>{formatCredit(row.creditUsed)}</strong>
                <small className='report-rank-share'>
                  {report.totalCreditUsed > 0 ? `${(row.creditUsed / report.totalCreditUsed * 100).toFixed(1)}%` : '—'}
                </small>
              </div>
            })}
          </div>
          <div className='report-rank-footer'>
            <span>{t('前 {n} 名占全部消耗 {pct}', { n: String(shown.length), pct: `${Math.min(100, share).toFixed(1)}%` })}</span>
            <span>{t('{n} 个账号有消耗', { n: String(accounts.length) })}</span>
          </div>
        </>
      ) : null}
    </CardContent>
  </Card>
}
