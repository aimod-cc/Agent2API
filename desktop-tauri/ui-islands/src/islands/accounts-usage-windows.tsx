import * as React from 'react'
import {
  Badge, BadgeDot, Button, Meter, Popover, PopoverContent, PopoverDescription,
  PopoverHeader, PopoverTitle, PopoverTrigger,
} from '@ui'
import { t } from '../i18n'
import { isFreePlan, planBadgeLabel } from './accounts-domain'
import { formatTime, numberText, type UsageEntry } from './accounts-shared'
import { usageAgeText } from '../../../ui-shared/usage-query-state'
import {
  usageNumberOrNull, usageSupplementReadingsOf, usageWarningsOf,
  usageWindowLabel, usageWindowView,
} from '../../../ui-shared/usage-readings'
import '../styles/accounts-usage-windows.css'

type UsageWindowReading = {
  id: string
  group: string
  label: string
  window: string
  view: string
  percent: number | null
  percentText: string
  tone: 'ok' | 'warn' | 'muted'
  unlimited: boolean
  resetsAt: number | null
}

type WindowGroup = { id: string; name: string; rows: UsageWindowReading[] }

function windowLabel(code: string): string {
  switch (code.trim().toLowerCase()) {
    case '5h': return t('{n} 小时', { n: 5 })
    case 'weekly': return t('每周')
    case 'monthly': return t('每月')
    case 'daily': return t('每日')
    default: return code.trim()
  }
}

export function usageWindows(entry: UsageEntry): UsageWindowReading[] {
  if (!entry || typeof entry !== 'object' || Array.isArray(entry)) return []
  const rows = Array.isArray(entry.windows) ? entry.windows : []
  const readings: UsageWindowReading[] = []
  for (const [index, value] of rows.entries()) {
    if (!value || typeof value !== 'object' || Array.isArray(value)) continue
    const row = value as Record<string, unknown>
    const label = usageWindowLabel(row.label, t)
    const group = typeof row.group === 'string' ? row.group.trim() : ''
    const window = windowLabel(typeof row.window === 'string' ? row.window : '') || label
    if (!group && !window) continue
    const read = usageNumberOrNull(row.remainingPercent)
    const percent = read === null ? null : Math.max(0, Math.min(100, read))
    const unlimited = row.unlimited === true
    const onlyPercent = usageNumberOrNull(row.remaining) === null
      && usageNumberOrNull(row.limit) === null && !unlimited
    const resetsAt = usageNumberOrNull(row.resetsAt)
    readings.push({
      id: `${typeof row.key === 'string' ? row.key : [group, window].join('-')}-${index}`,
      group,
      label: label || [group, window].filter(Boolean).join(' · '),
      window,
      view: usageWindowView(row, t, numberText),
      percent,
      percentText: onlyPercent && percent !== null ? `${numberText(percent)}%` : '',
      tone: unlimited ? 'ok' : percent === null ? 'muted' : percent < 20 ? 'warn' : 'ok',
      unlimited,
      resetsAt: resetsAt !== null && resetsAt > 0 && resetsAt <= 8.64e15 ? resetsAt : null,
    })
  }
  return readings
}

function groupWindows(rows: UsageWindowReading[]): WindowGroup[] {
  const groups = new Map<string, WindowGroup>()
  for (const row of rows) {
    const key = row.group || row.id
    let group = groups.get(key)
    if (!group) {
      group = { id: key, name: row.group, rows: [] }
      groups.set(key, group)
    }
    group.rows.push(row)
  }
  return [...groups.values()]
}

function supplementaryReadings(entry: Record<string, unknown>, rows: UsageWindowReading[]): string[] {
  const labels = new Set(rows.map(row => row.label))
  const rawWindows = Array.isArray(entry.windows) ? entry.windows : []
  const keys = new Set(rawWindows.flatMap(row => row && typeof row === 'object'
    && typeof row.key === 'string' && row.key ? [row.key] : []))
  const wallets = Array.isArray(entry.wallets) ? entry.wallets.filter(wallet => {
    if (!wallet || typeof wallet !== 'object' || Array.isArray(wallet)) return false
    // 钱包与 windows 可描述同一份额度；去重只按结构标识，不解析上游展示串。
    return wallet.type !== 'quota_window' && !keys.has(wallet.key ?? wallet.type)
      && !labels.has(wallet.displayName)
  }) : []
  return usageSupplementReadingsOf({ ...entry, wallets, subscription: null }, t, numberText)
}

function WindowReading({ row }: { row: UsageWindowReading }) {
  const value = row.percentText || row.view
  const remaining = row.percent === null ? row.view : t('剩余 {percent}%', { percent: numberText(row.percent) })
  return (
    <span className={`usage-window-item ${row.tone}`} title={`${row.label} · ${remaining}`}>
      <span className='usage-window-line'>
        <span className='usage-window-label' title={row.window}>{row.window}</span>
        <span className='usage-window-value' title={row.view}>{value}</span>
      </span>
      {row.percent !== null && !row.unlimited ? (
        <Meter value={row.percent} variant={row.tone === 'warn' ? 'warning' : 'success'}
          className='mt-1' trackClassName='h-[3px]' indicatorClassName={row.tone === 'ok' ? 'opacity-75' : undefined}
          aria-label={`${row.label} · ${remaining}`} />
      ) : null}
    </span>
  )
}

export function UsageWindowsCell({ entry, rows, updatedAt, creditDetail, children }: {
  entry: UsageEntry
  rows: UsageWindowReading[]
  updatedAt: number
  creditDetail?: React.ReactNode
  children?: React.ReactNode
}) {
  const data = entry && typeof entry === 'object' ? entry : {}
  const subscription = data.subscription && typeof data.subscription === 'object'
    ? data.subscription as Record<string, unknown> : {}
  const planName = typeof subscription.planName === 'string' ? subscription.planName.trim() : ''
  const groups = groupWindows(rows)
  const grouped = rows.some(row => row.group)
  // 无分组、读数较长或没有比例的窗口（Kiro 的「剩 x / y 积分」、AStudio 的
  // 两个余额）按紧凑行排：标签在左、读数在右。不这样排的话，未分组的窗口会
  // 走「统计格」样式（标签在上一行、读数 15px 在下一行），单列时整格被一个
  // 大数字占满，多列时又像两张并排的卡片 —— 与 Antigravity 那种小行读数不一致。
  const compact = !grouped && (rows.length === 1 || rows.some(row => row.percent === null || row.view.length > 10))
  const warnings = usageWarningsOf(data)
  const supplementary = supplementaryReadings(data, rows)
  const low = rows.some(row => !row.unlimited && row.percent !== null && row.percent < 20)
  const unknown = rows.some(row => !row.unlimited && row.percent === null) || warnings.length > 0
  const status = low ? t('部分窗口偏低') : unknown ? t('部分读数未知') : t('额度充足')
  const tone = low ? 'warn' : unknown ? 'muted' : 'ok'
  const age = updatedAt ? usageAgeText(updatedAt, t, Date.now()) : ''
  const updateTitle = updatedAt ? t('上次成功查询：{time}', { time: formatTime(updatedAt) }) : ''
  const caption = rows.every(row => row.percentText || row.view === '—') ? t('剩余比例') : t('分窗口额度')
  return (
    <span className='usage-cell usage-window-cell'>
      <span className='usage-window-quota'>
        <span className='usage-window-top'>
          {planName ? <Badge variant={isFreePlan(planName) ? 'secondary' : 'brand'} shape='tag'
            className='usage-window-plan' title={planName}>{planBadgeLabel(planName)}</Badge> : null}
          <span className={`usage-window-status ${tone}`} title={status}>
            <BadgeDot /><span>{status}</span>
          </span>
        </span>
        <span className={`usage-window-grid${grouped ? ' grouped' : ' ungrouped'}${compact ? ' compact' : ''}`}
          style={{ '--quota-columns': Math.min(3, groups.length) } as React.CSSProperties}>
          {groups.map(group => (
            <span className='usage-window-group' key={group.id}>
              {group.name ? <span className='usage-window-group-name' title={group.name}>{group.name}</span> : null}
              {group.rows.map(row => <WindowReading key={row.id} row={row} />)}
            </span>
          ))}
        </span>
        <span className='usage-window-foot'>
          <span className='usage-window-updated' title={updateTitle}>
            <span className='usage-window-caption'>{caption}{age ? ' · ' : ''}</span>{age}
          </span>
          <Popover>
            <PopoverTrigger render={<Button type='button' size='xs' variant='ghost' className='usage-window-detail' />}>
              {t('重置详情')}<span aria-hidden='true' className='usage-window-caret'>⌄</span>
            </PopoverTrigger>
            <PopoverContent align='end' sideOffset={6} className='usage-window-pop'>
              <PopoverHeader><PopoverTitle>{t('窗口重置时间')}</PopoverTitle></PopoverHeader>
              <div className='usage-window-pop-body'>
                {rows.map(row => (
                  <div className='usage-window-reset' key={row.id}>
                    <div className='usage-window-reset-head'>
                      <span title={row.label}>{row.label}</span>
                      <span className={`usage-window-reset-value ${row.tone}`}>{row.percentText || row.view}</span>
                    </div>
                    {row.resetsAt !== null
                      ? <time dateTime={new Date(row.resetsAt).toISOString()}>{formatTime(row.resetsAt)}</time>
                      : <span className='usage-window-reset-unknown'>{t('未返回重置时间')}</span>}
                  </div>
                ))}
                {supplementary.length ? <div className='usage-window-supplementary'>
                  {supplementary.map(line => <p key={line}>{line}</p>)}
                </div> : null}
                {warnings.map(line => <p className='usage-window-warning' key={line}>{t(line)}</p>)}
                <PopoverDescription className='usage-window-explanation'>
                  {t('各窗口独立限制请求，不相加为总余额。')}
                </PopoverDescription>
              </div>
            </PopoverContent>
          </Popover>
        </span>
        {creditDetail ? <span className='usage-window-credit'>{creditDetail}</span> : null}
        {children}
      </span>
    </span>
  )
}
