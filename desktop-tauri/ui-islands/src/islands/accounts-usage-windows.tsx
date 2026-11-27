import * as React from 'react'
import {
  Badge, BadgeDot, Button, Meter, Popover, PopoverContent, PopoverDescription,
  PopoverHeader, PopoverTitle, PopoverTrigger, SegmentedControl,
  Tooltip, TooltipContent, TooltipProvider, TooltipTrigger,
} from '@ui'
import { t } from '../i18n'
import { isFreePlan, planBadgeLabel } from './accounts-domain'
import { formatTime, numberText, type UsageEntry } from './accounts-shared'
import { usageAgeText } from '../../../ui-shared/usage-query-state'
import { usageWarningsOf } from '../../../ui-shared/usage-readings'
import {
  compactDurationText, exactResetLabel, formatResetCountdown, groupWindows,
  isExhaustedRow, isLongWindowCode, isShortWindowCode, pickTopResetSummary,
  supplementaryReadings, usageModelQuotas,
  type ModelQuotaReading, type UsageWindowReading, type WindowGroup,
} from './accounts-usage-windows-data'
import '../styles/accounts-usage-windows.css'

export { usageModelQuotas, usageWindows, type ModelQuotaReading, type UsageWindowReading } from './accounts-usage-windows-data'

function ResetIcon({ size = 9.5, className }: { size?: number; className?: string }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox='0 0 16 16'
      fill='none'
      stroke='currentColor'
      strokeWidth='1.75'
      strokeLinecap='round'
      strokeLinejoin='round'
      aria-hidden='true'
      className={className ? `usage-window-reset-ico ${className}` : 'usage-window-reset-ico'}
    >
      <path d='M13.5 8a5.5 5.5 0 1 1-1.4-3.65' />
      <path d='M13.5 2.75v2.9h-2.9' />
    </svg>
  )
}

function WindowResetRow({
  resetsAt,
  now,
  tone,
  layout,
}: {
  resetsAt: number
  now: number
  tone: 'ok' | 'warn' | 'muted'
  layout: 'tip' | 'pop'
}) {
  const reset = formatResetCountdown(resetsAt, now)
  const exact = exactResetLabel(resetsAt)
  const toneClass = reset.overdue ? 'muted' : tone
  if (layout === 'pop') {
    return (
      <div className='usage-window-pop-win-time'>
        <span className={`usage-window-pop-cd ${toneClass}`}>
          <ResetIcon size={10} />
          <span>{reset.verbose}</span>
        </span>
        <time dateTime={new Date(resetsAt).toISOString()} className='usage-window-pop-at'>
          {t('预计 {time}', { time: exact })}
        </time>
      </div>
    )
  }
  return (
    <div className='usage-window-tip-reset'>
      <span className={`usage-window-tip-cd ${toneClass}`}>
        <ResetIcon size={10} />
        <span>{reset.verbose}</span>
      </span>
      <span className='usage-window-tip-at'>{t('预计 {time}', { time: exact })}</span>
    </div>
  )
}

function effectiveWindowResetAt(row: UsageWindowReading): number | null {
  const isFull = row.unlimited || (row.percent !== null && row.percent >= 100)
  if (row.constrainedByModel && isLongWindowCode(row.windowCode)) return null
  if (row.constrainedByModel && isShortWindowCode(row.windowCode)) return row.modelResetsAt
  return row.resetsAt !== null && !isFull ? row.resetsAt : null
}

function WindowTooltipBody({
  row,
  models,
  now,
}: {
  row: UsageWindowReading
  models: ModelQuotaReading[]
  now: number
}) {
  const value = row.percentText || row.view
  const isFull = row.unlimited || (row.percent !== null && row.percent >= 100)
  const ownResetAt = row.resetsAt !== null && !isFull ? row.resetsAt : null
  const isConstrainedLong = row.constrainedByModel && isLongWindowCode(row.windowCode)
  const isConstrainedShort = row.constrainedByModel && isShortWindowCode(row.windowCode)
  const lowModels = isConstrainedShort && row.group
    ? models.filter(model => model.group === row.group && model.percent !== null && model.percent < 20)
    : []
  return (
    <div className='usage-window-tip-card'>
      <div className='usage-window-tip-head'>
        <span className='usage-window-tip-title'>{row.label}</span>
        <span className={`usage-window-tip-val ${row.tone}`}>{value}</span>
      </div>
      {isConstrainedLong ? (
        <>
          <div className='usage-window-tip-muted'>
            {t('上游汇总返回 {value}，但该组所有模型当前受限', { value })}
          </div>
          {row.modelResetsAt !== null ? <WindowResetRow resetsAt={row.modelResetsAt} now={now} tone='warn' layout='tip' /> : null}
          {ownResetAt !== null ? (
            <div className='usage-window-tip-muted'>
              {formatResetCountdown(ownResetAt, now).verbose} · {t('预计 {time}', { time: exactResetLabel(ownResetAt) })}
            </div>
          ) : null}
        </>
      ) : isConstrainedShort ? (
        <>
          {row.modelResetsAt !== null ? (
            <>
              <WindowResetRow resetsAt={row.modelResetsAt} now={now} tone={row.tone} layout='tip' />
              <div className='usage-window-tip-muted'>{t('受模型配额限制')}</div>
              {row.modelResetsAt <= now ? (
                <div className='usage-window-tip-muted'>{t('可刷新余额查看最新额度')}</div>
              ) : null}
            </>
          ) : (
            <div className='usage-window-tip-muted'>{t('受模型配额限制，未返回重置时间')}</div>
          )}
          {lowModels.length ? (
            <div className='usage-window-tip-models'>
              {lowModels.map(model => (
                <div key={model.id} className='usage-window-tip-model-row'>
                  <span>{model.name}</span>
                  <span className='usage-window-tip-val warn'>{model.percentText || model.view}</span>
                </div>
              ))}
            </div>
          ) : null}
        </>
      ) : ownResetAt !== null ? (
        <>
          <WindowResetRow resetsAt={ownResetAt} now={now} tone={row.tone} layout='tip' />
          {ownResetAt <= now ? (
            <div className='usage-window-tip-muted'>{t('可刷新余额查看最新额度')}</div>
          ) : null}
        </>
      ) : row.expiresAt !== null ? (
        <div className='usage-window-tip-muted'>{t('到期 {date}', { date: formatTime(row.expiresAt) })}</div>
      ) : isFull ? (
        <div className='usage-window-tip-muted'>{t('当前满额，无需等待恢复')}</div>
      ) : (
        <div className='usage-window-tip-muted'>{t('未返回重置时间')}</div>
      )}
    </div>
  )
}

function AllWindowsTooltipBoard({
  groups,
  models,
  now,
}: {
  groups: WindowGroup[]
  models: ModelQuotaReading[]
  now: number
}) {
  const lowModels = models.filter(model => model.percent !== null && model.percent < 20)
  return (
    <div className='usage-window-board'>
      <div className='usage-window-board-title'>{t('各窗口恢复时间')}</div>
      <div className='usage-window-board-groups'>
        {groups.map(group => (
          <div key={group.id} className='usage-window-board-group'>
            {group.name ? <div className='usage-window-board-gname'>{group.name}</div> : null}
            {group.rows.map(row => {
              const value = row.percentText || row.view
              const isFull = row.unlimited || (row.percent !== null && row.percent >= 100)
              const isConstrainedLong = row.constrainedByModel && isLongWindowCode(row.windowCode)
              const effectiveResetAt = effectiveWindowResetAt(row)
              const reset = effectiveResetAt !== null ? formatResetCountdown(effectiveResetAt, now) : null
              const exact = reset && effectiveResetAt !== null ? exactResetLabel(effectiveResetAt) : ''
              return (
                <div key={row.id} className='usage-window-board-row'>
                  <span className='usage-window-board-win'>{row.window}</span>
                  <span className={`usage-window-board-val ${row.tone}`}>{value}</span>
                  <span className={`usage-window-board-cd ${!reset ? 'muted' : reset.overdue ? 'muted' : row.tone}`}>
                    {reset ? reset.short : isConstrainedLong ? t('受限') : isFull ? t('满额') : '—'}
                  </span>
                  <span className='usage-window-board-at'>{exact || '—'}</span>
                </div>
              )
            })}
          </div>
        ))}
        {lowModels.length ? (
          <div className='usage-window-board-models'>
            <div className='usage-window-board-gname'>{t('受限模型')}</div>
            {lowModels.map(model => {
              const reset = model.resetsAt !== null ? formatResetCountdown(model.resetsAt, now) : null
              const exact = model.resetsAt !== null ? exactResetLabel(model.resetsAt) : ''
              return (
                <div key={model.id} className='usage-window-board-model-row'>
                  <span className='usage-window-board-mname' title={model.name}>{model.name}</span>
                  <span className={`usage-window-board-val ${model.tone}`}>{model.percentText || model.view}</span>
                  <span className={`usage-window-board-cd ${!reset ? 'muted' : reset.overdue ? 'muted' : model.tone}`}>
                    {reset ? reset.short : '—'}
                  </span>
                  <span className='usage-window-board-at'>{exact || '—'}</span>
                </div>
              )
            })}
          </div>
        ) : null}
      </div>
    </div>
  )
}

function WindowReading({
  row,
  models,
  now,
}: {
  row: UsageWindowReading
  models: ModelQuotaReading[]
  now: number
}) {
  const value = row.percentText || row.view
  const remaining = row.percent === null ? row.view : t('剩余 {percent}%', { percent: numberText(row.percent) })
  const effectiveResetAt = effectiveWindowResetAt(row)
  const reset = effectiveResetAt !== null ? formatResetCountdown(effectiveResetAt, now) : null
  const isZero = isExhaustedRow(row)
  const cdTone = reset?.overdue ? 'muted' : row.tone

  return (
    <Tooltip>
      <TooltipTrigger
        render={<span className={`usage-window-item usage-window-hoverable ${row.tone}`} />}
      >
        <span className='usage-window-line'>
          <span className='usage-window-label'>{row.window}</span>
          <span className='usage-window-reading'>
            <span className='usage-window-value'>{value}</span>
            {reset ? (
              <span className={`usage-window-reset-inline ${cdTone}${isZero && !reset.overdue ? ' is-zero' : ''}`}>
                <span className='usage-window-sep' aria-hidden='true'>·</span>
                <span className='usage-window-reset-text'>{reset.inline}</span>
              </span>
            ) : null}
          </span>
        </span>
        {row.percent !== null && !row.unlimited ? (
          <Meter value={row.percent} variant={row.tone === 'warn' ? 'warning' : row.tone === 'muted' ? 'muted' : 'success'}
            className='mt-[4px]' trackClassName='h-[3.5px]'
            indicatorClassName={row.tone === 'muted' ? 'opacity-45' : row.tone === 'ok' ? 'opacity-85' : undefined}
            aria-label={`${row.label} · ${remaining}${reset ? ` · ${reset.inline}` : ''}`} />
        ) : null}
      </TooltipTrigger>
      <TooltipContent variant='plain' side='top' sideOffset={6} className='p-2'>
        <WindowTooltipBody row={row} models={models} now={now} />
      </TooltipContent>
    </Tooltip>
  )
}

function ModelQuotaCard({ row, now }: { row: ModelQuotaReading; now: number }) {
  const value = row.percentText || row.view
  const remaining = row.percent === null ? row.view : t('剩余 {percent}%', { percent: numberText(row.percent) })
  const isFull = row.percent !== null && row.percent >= 100
  const reset = row.resetsAt !== null && !isFull ? formatResetCountdown(row.resetsAt, now) : null
  const exact = row.resetsAt !== null && !isFull ? exactResetLabel(row.resetsAt) : ''
  const subMeta = [row.window, exact].filter(Boolean).join(' · ')
  return (
    <div className='usage-window-model-card'>
      <div className='usage-window-model-head'>
        <span className='usage-window-model-name' title={`${row.name} (${row.id})`}>{row.name}</span>
        <span className='usage-window-model-right'>
          <span className={`usage-window-model-val ${row.tone}`}>{value}</span>
          {reset && row.resetsAt !== null ? (
            <time dateTime={new Date(row.resetsAt).toISOString()} className={`usage-window-model-reset-inline${!reset.overdue && row.tone === 'warn' ? ' warn' : ''}`}>
              <span className='usage-window-sep' aria-hidden='true'>·</span>
              <span>{reset.inline}</span>
            </time>
          ) : null}
        </span>
      </div>
      <div className='usage-window-model-sub'>
        <span className='usage-window-model-id' title={row.id}>{row.id}</span>
        {subMeta ? <span className='usage-window-model-win'>{subMeta}</span> : null}
      </div>
      {row.percent !== null ? (
        <Meter value={row.percent} variant={row.tone === 'warn' ? 'warning' : 'success'}
          className='mt-1.5' trackClassName='h-[3.5px]' indicatorClassName={row.tone === 'ok' ? 'opacity-85' : undefined}
          aria-label={`${row.name} · ${remaining}${reset ? ` · ${reset.inline}` : ''}`} />
      ) : null}
    </div>
  )
}

function groupTitleFor(groupName: string, models: ModelQuotaReading[]): string | undefined {
  if (!groupName || !models.length) return undefined
  const matched = models.filter(model => model.group === groupName)
  if (!matched.length) return undefined
  const list = matched.map(model => `${model.name} ${model.percentText || model.view}`).join('\n')
  return `${t('{group}（{count} 个模型）', { group: groupName, count: matched.length })}\n${list}`
}

export function UsageWindowsCell({ entry, rows, updatedAt, now: nowProp, creditDetail, children }: {
  entry: UsageEntry
  rows: UsageWindowReading[]
  updatedAt: number
  now?: number
  creditDetail?: React.ReactNode
  children?: React.ReactNode
}) {
  const [detailTab, setDetailTab] = React.useState<'models' | 'windows'>('models')
  const now = nowProp ?? Date.now()
  const data = entry && typeof entry === 'object' ? entry : {}
  const subscription = data.subscription && typeof data.subscription === 'object'
    ? data.subscription as Record<string, unknown> : {}
  const planName = typeof subscription.planName === 'string' ? subscription.planName.trim() : ''
  const models = React.useMemo(() => usageModelQuotas(entry), [entry])
  const groups = groupWindows(rows)
  const grouped = rows.some(row => row.group)
  const activeTab = models.length > 0 ? detailTab : 'windows'
  const warnings = usageWarningsOf(data)
  const supplementary = supplementaryReadings(data, rows)
  const measured = rows.filter(row => !row.aside && !row.unlimited)
  const low = measured.some(row => row.percent !== null ? row.percent < 20 : row.remaining !== null && row.remaining <= 0)
    || models.some(model => model.percent !== null && model.percent < 20)
  const unknown = measured.some(row => row.percent === null && row.remaining === null) || warnings.length > 0
  const informational = measured.length === 0 && rows.every(row => !row.unlimited)
  const status = informational ? t('额度详情') : low ? t('部分窗口偏低') : unknown ? t('部分读数未知') : t('额度充足')
  const tone = low ? 'warn' : unknown || informational ? 'muted' : 'ok'
  const topReset = low ? pickTopResetSummary(rows, models, now) : null
  const topResetText = (() => {
    if (!topReset) return ''
    const diff = topReset.resetsAt - now
    if (diff < 60_000) return t('即将恢复')
    const duration = compactDurationText(diff)
    return topReset.multiple
      ? t('最快 {time}后恢复', { time: duration })
      : t('{time}后恢复', { time: duration })
  })()
  const hasAnyReset = rows.some(row => row.resetsAt !== null || row.modelResetsAt !== null)
    || models.some(model => model.percent !== null && model.percent < 20)
  const age = updatedAt ? usageAgeText(updatedAt, t, now) : ''
  const updateTitle = updatedAt ? t('上次成功查询：{time}', { time: formatTime(updatedAt) }) : ''
  const caption = rows.every(row => row.percentText || row.view === '—') ? t('剩余比例') : t('分窗口额度')
  const maxCols = grouped ? Math.min(2, groups.length) : 1

  const topStatusNode = topReset ? (
    <Tooltip>
      <TooltipTrigger
        render={
          <span className='usage-window-status warn usage-window-top-reset'>
            <BadgeDot />
            <ResetIcon size={9.5} />
            <span>{topResetText}</span>
          </span>
        }
      />
      <TooltipContent variant='plain' side='top' sideOffset={6} className='p-2.5'>
        <AllWindowsTooltipBoard groups={groups} models={models} now={now} />
      </TooltipContent>
    </Tooltip>
  ) : hasAnyReset ? (
    <Tooltip>
      <TooltipTrigger
        render={
          <span className={`usage-window-status ${tone}`}>
            <BadgeDot />
            <span>{status}</span>
          </span>
        }
      />
      <TooltipContent variant='plain' side='top' sideOffset={6} className='p-2.5'>
        <AllWindowsTooltipBoard groups={groups} models={models} now={now} />
      </TooltipContent>
    </Tooltip>
  ) : (
    <span className={`usage-window-status ${tone}`} title={status}>
      <BadgeDot /><span>{status}</span>
    </span>
  )

  return (
    <TooltipProvider delay={150}>
      <span className='usage-cell usage-window-cell'>
        <span className='usage-window-quota'>
          <span className='usage-window-top'>
            {planName ? <Badge variant={isFreePlan(planName) ? 'secondary' : 'brand'} shape='tag'
              className='usage-window-plan' title={planName}>{planBadgeLabel(planName)}</Badge> : null}
            {topStatusNode}
          </span>
          <span
            className={`usage-window-grid${grouped ? ' grouped' : ' ungrouped'}${grouped && groups.length > 2 ? ' has-overflow-row' : ''}`}
            style={{ '--quota-columns': maxCols } as React.CSSProperties}
          >
            {grouped ? (
              groups.map((group, idx) => {
                const title = groupTitleFor(group.name, models)
                const spanFull = groups.length > 2 && groups.length % 2 === 1 && idx === groups.length - 1
                return (
                  <span className={`usage-window-group${spanFull ? ' is-span-full' : ''}`} key={group.id}>
                    {group.name ? <span className='usage-window-group-name' title={title || group.name}>{group.name}</span> : null}
                    <span className='usage-window-rows'>
                      {group.rows.map(row => <WindowReading key={row.id} row={row} models={models} now={now} />)}
                    </span>
                  </span>
                )
              })
            ) : (
              <span className='usage-window-group'>
                <span className='usage-window-rows'>
                  {rows.map(row => <WindowReading key={row.id} row={row} models={models} now={now} />)}
                </span>
              </span>
            )}
          </span>
          <span className='usage-window-foot'>
            <span className='usage-window-updated' title={updateTitle}>
              <span className='usage-window-caption'>{caption}{age ? ' · ' : ''}</span>{age}
            </span>
            <Popover>
              <PopoverTrigger render={<Button type='button' size='xs' variant='ghost' className='usage-window-detail' />}>
                {models.length > 0 ? t('模型配额（{count}）', { count: models.length }) : t('额度详情')}
                <span aria-hidden='true' className='usage-window-caret'>⌄</span>
              </PopoverTrigger>
              <PopoverContent align='end' sideOffset={6} className={models.length > 0 ? 'usage-window-pop usage-window-pop-wide' : 'usage-window-pop'}>
                <PopoverHeader>
                  <PopoverTitle>{models.length > 0 ? t('模型与窗口额度') : t('窗口额度与时间')}</PopoverTitle>
                </PopoverHeader>
                <div className='usage-window-pop-body'>
                  {models.length > 0 ? (
                    <div className='usage-window-pop-tabs'>
                      <SegmentedControl
                        aria-label={t('额度详情视图')}
                        value={activeTab}
                        onValueChange={setDetailTab}
                        options={[
                          { value: 'models', label: t('模型配额'), count: models.length },
                          { value: 'windows', label: t('分组窗口'), count: rows.length },
                        ]}
                      />
                    </div>
                  ) : null}
                  {activeTab === 'models' ? (
                    <div className='usage-window-model-grid'>
                      {models.map(model => <ModelQuotaCard key={model.id} row={model} now={now} />)}
                    </div>
                  ) : (
                    rows.map(row => {
                      const isFull = row.unlimited || (row.percent !== null && row.percent >= 100)
                      const value = row.percentText || row.view
                      const isConstrainedShort = row.constrainedByModel && isShortWindowCode(row.windowCode)
                      const isConstrainedLong = row.constrainedByModel && isLongWindowCode(row.windowCode)
                      const effectiveResetAt = effectiveWindowResetAt(row)
                      const inlineReset = effectiveResetAt !== null ? formatResetCountdown(effectiveResetAt, now) : null
                      return (
                        <div className='usage-window-reset' key={row.id}>
                          <div className='usage-window-reset-head'>
                            <span title={row.label}>{row.label}</span>
                            <span className='usage-window-reset-cluster'>
                              <span className={`usage-window-reset-value ${row.tone}`}>{value}</span>
                              {inlineReset ? (
                                <span className={`usage-window-pop-reset-short${!inlineReset.overdue && row.tone === 'warn' ? ' warn' : ''}`}>
                                  <span className='usage-window-sep' aria-hidden='true'>·</span>
                                  <span>{inlineReset.inline}</span>
                                </span>
                              ) : null}
                            </span>
                          </div>
                          {isConstrainedShort ? (
                            row.modelResetsAt !== null ? (
                              <>
                                <WindowResetRow resetsAt={row.modelResetsAt} now={now} tone={row.tone} layout='pop' />
                                <span className='usage-window-reset-unknown'>{t('受模型配额限制')}</span>
                              </>
                            ) : (
                              <span className='usage-window-reset-unknown'>{t('受模型配额限制，未返回重置时间')}</span>
                            )
                          ) : (
                            <>
                              {row.resetsAt !== null && !isFull ? (
                                <WindowResetRow resetsAt={row.resetsAt} now={now} tone={row.tone} layout='pop' />
                              ) : !isConstrainedLong && row.expiresAt === null ? (
                                <span className='usage-window-reset-unknown'>
                                  {isFull ? t('当前满额，无需等待恢复') : t('未返回重置时间')}
                                </span>
                              ) : null}
                              {isConstrainedLong ? (
                                <span className='usage-window-reset-unknown'>
                                  {t('上游汇总返回 {value}，但该组所有模型当前受限', { value })}
                                </span>
                              ) : null}
                            </>
                          )}
                          {row.expiresAt !== null ? <time dateTime={new Date(row.expiresAt).toISOString()}>
                            {t('到期 {date}', { date: formatTime(row.expiresAt) })}
                          </time> : null}
                        </div>
                      )
                    })
                  )}
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
    </TooltipProvider>
  )
}
