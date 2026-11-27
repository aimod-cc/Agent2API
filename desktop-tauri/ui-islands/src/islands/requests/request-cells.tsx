import * as React from 'react'
import { Button, HtmlContent } from '@ui'
import { t } from '../../i18n'
import { NameEyeButton } from '../accounts-panels'
import { maskName } from '../accounts-store'
import { numberText } from '../accounts-shared'
import { apiKeyRequestLabel, clientRequestLabel } from './source-filters'
import { ClientBrand, ProviderBrand } from './request-brand'
import { visionDetail } from '../../../../ui-shared/request-vision-detail'
import { describeRequestProtocol } from '../../../../ui-shared/request-protocol'
import { formatCost, pricingSourceLabel, usePricingDisplay, type ModelPricingInfo } from '../model-pricing'

export type RequestEntry = {
  rowId?: number
  id?: string; ts?: number; status?: number; error?: string | null
  provider?: string; providerLabel?: string; accountId?: string; accountName?: string
  model?: string; clientModel?: string; upstreamModel?: string
  clientReasoning?: string; upstreamReasoning?: string
  clientProtocol?: string; upstreamProtocol?: string
  durationMs?: number; firstResponseMs?: number | null
  promptTokens?: number; completionTokens?: number; totalTokens?: number; cacheReadTokens?: number
  cacheCreationTokens?: number; cacheCreation1hTokens?: number
  costUsd?: number | null; costPriced?: boolean; priceSource?: ModelPricingInfo['source']
  credit?: number | null
  route?: unknown
  attempts?: number; attemptDetails?: unknown[]; sensitiveHits?: unknown[]; privacyHits?: unknown[]
  detailsLoaded?: boolean
  hasRoute?: boolean
  visionMode?: string
  hasProcessFacts?: boolean
  internalRetries?: number
  hasSensitiveHits?: boolean
  hasPrivacyHits?: boolean
  privacyAction?: string
  hasClientEvidence?: boolean
  clientUserAgent?: string
  isTest?: boolean
  phase?: string; phaseElapsedMs?: number | null; phaseStartedAt?: number | null
  client?: string
  clientLabel?: string
  clientEvidence?: import('../../../../ui-shared/client-identification').ClientEvidence
  apiKeyId?: string; apiKeyName?: string; apiKeySource?: string
  origin?: string
}

export type VisibleColumn = {
  key: string
  label: string
  sel: string
  track: string
  align: 'left' | 'center' | 'right'
}

function shared() {
  return window as unknown as {
    wbProviders?: { labelOf?: (id: string) => string }
    wbUnits?: { formatTokens?: (value: unknown) => string }
    wbRequestHover?: { hasProcessFacts?: (entry: unknown) => boolean }
  }
}

/* ─── 读数格式化 ───────────────────────────── */

function formatDurationExact(ms: unknown): string {
  const rounded = Math.round(Number(ms) || 0)
  if (rounded < 1000) return `${rounded}ms`
  const seconds = Math.floor(rounded / 1000)
  if (seconds >= 60) return t('{m}分{s}秒', { m: Math.floor(seconds / 60), s: seconds % 60 })
  const millis = rounded % 1000
  return millis > 0 ? t('{s}秒{ms}ms', { s: seconds, ms: millis }) : t('{n}秒', { n: seconds })
}

/** 列表用紧凑耗时：小于 1s 给 ms，10s 内保留两位小数，10~60s 保留一位小数，分钟以上给分秒 */
function formatDurationCompact(ms: unknown): string {
  const rounded = Math.round(Number(ms) || 0)
  if (rounded < 1000) return `${rounded}ms`
  if (rounded < 10_000) return `${(rounded / 1000).toFixed(2)}s`
  if (rounded < 60_000) return `${(rounded / 1000).toFixed(1)}s`
  const seconds = Math.floor(rounded / 1000)
  return `${Math.floor(seconds / 60)}m ${seconds % 60}s`
}

const SPEED_TIP = t('生成速度：输出 token ÷ 流式段用时（总用时 − 首响）；流式段不足总用时一半时按总用时计')
const STREAM_MS_FLOOR = 500

type MetricTone = 'good' | 'info' | 'warn' | 'bad' | ''

function tokensPerSecondNumber(entry: RequestEntry): number | null {
  const tokens = Number(entry.completionTokens) || 0
  const duration = Number(entry.durationMs) || 0
  if (tokens <= 0 || duration <= 0) return null
  const first = Number(entry.firstResponseMs)
  const streamMs = Number.isFinite(first) && first > 0 && first < duration ? duration - first : 0
  const streaming = streamMs >= STREAM_MS_FLOOR && streamMs * 2 >= duration
  return tokens / ((streaming ? streamMs : duration) / 1000)
}

function formatTokensPerSecondValue(rate: number | null): string {
  if (rate === null) return ''
  return rate >= 100 ? String(Math.round(rate)) : rate.toFixed(1)
}

function speedTone(rate: number | null): MetricTone {
  if (rate === null) return ''
  if (rate >= 60) return 'good'
  if (rate >= 30) return 'info'
  if (rate >= 15) return 'warn'
  return 'bad'
}

function ttftTone(ms: unknown): MetricTone {
  const value = Number(ms)
  if (!Number.isFinite(value) || value <= 0) return ''
  if (value < 2000) return 'good'
  if (value < 5000) return 'info'
  if (value < 10_000) return 'warn'
  return 'bad'
}

function cacheTone(prompt: number, cacheRead: number, cacheWrite: number): MetricTone {
  if (cacheRead > 0) {
    if (prompt <= 0) return 'good'
    const pct = (cacheRead / prompt) * 100
    if (pct >= 70) return 'good'
    if (pct >= 30) return 'info'
    return 'warn'
  }
  return cacheWrite > 0 ? 'info' : ''
}

const toneClass = (tone: MetricTone): string => (tone ? ` is-tone-${tone}` : '')

function formatFirstResponseCompact(ms: unknown): string {
  const value = Number(ms)
  if (!Number.isFinite(value) || value <= 0) return ''
  return formatDurationCompact(value)
}

function formatTokens(value: unknown): string {
  return shared().wbUnits?.formatTokens?.(value) ?? (Number(value) || 0).toLocaleString('zh-CN')
}

function formatElapsed(ts: unknown): string {
  const elapsed = Math.max(0, Date.now() - (Number(ts) || 0))
  const seconds = Math.max(1, Math.floor(elapsed / 1000))
  if (seconds < 60) return `${seconds}s`
  return `${Math.floor(seconds / 60)}m ${seconds % 60}s`
}

function cacheRateText(entry: RequestEntry): string {
  const prompt = Number(entry.promptTokens) || 0
  if (prompt <= 0) return ''
  return `${((Number(entry.cacheReadTokens) || 0) / prompt * 100).toFixed(1)}%`
}

export function isOk(entry: RequestEntry): boolean {
  const status = Number(entry.status) || 0
  return status >= 200 && status < 300 && !entry.error
}

export function isRunning(entry: RequestEntry): boolean {
  return (Number(entry?.status) || 0) === 0 && !entry?.error
}

export const rowIdentity = (entry: RequestEntry): string => String(entry?.id || entry?.rowId || entry?.ts || '')

function countRetries(entry: RequestEntry): number {
  const details = Array.isArray(entry?.attemptDetails) ? entry.attemptDetails : []
  return details.reduce<number>((sum, item) => {
    const retries = (item as { retries?: unknown[] } | null)?.retries
    return sum + (Array.isArray(retries) ? retries.length : 0)
  }, 0)
}

export function formatModelWithReasoning(name: string, level: string, origin?: string): string {
  if (!level) return name
  const lowerName = name.toLowerCase()
  const lowerLevel = level.toLowerCase()
  if (lowerName.endsWith(`(${lowerLevel})`) || lowerName.endsWith(`（${lowerLevel}）`)) {
    return name
  }
  if (
    origin === 'local_direct'
    && lowerLevel !== 'max'
    && (lowerName.endsWith(`-${lowerLevel}`) || lowerName.endsWith(`_${lowerLevel}`))
    && name.length > level.length + 1
  ) {
    return `${name.slice(0, -(level.length + 1))}(${level})`
  }
  return `${name}(${level})`
}

export function providerLabelOf(id: string): string {
  return t(shared().wbProviders?.labelOf?.(id) || id)
}

export function uniqueRowKeys(rows: RequestEntry[]): string[] {
  const seen = new Set<string>()
  return rows.map((entry, index) => {
    const base = rowIdentity(entry) || `i${index}`
    let key = base
    let n = 1
    while (seen.has(key)) key = `${base}#${n++}`
    seen.add(key)
    return key
  })
}

function stopRowClick(event: React.MouseEvent): void {
  event.stopPropagation()
}

function ProcessTags({ entry }: { entry: RequestEntry }) {
  const attempts = Number(entry.attempts) || 1
  const identity = rowIdentity(entry)
  const tags: React.ReactNode[] = []

  if (entry.origin === 'local_direct') {
    tags.push(
      <span key='local-direct' className='req-micro-tag is-local'
        title={t('本机直接发起（未经过本网关），由本地记录采集统计')}>
        {t('直连')}
      </span>,
    )
  }

  if (entry.isTest) {
    tags.push(
      <span key='test' className='req-micro-tag is-neutral'
        title={t('面板内发起的模型调用，消耗额度并记录 Key，不计入生产报表')}>
        {clientRequestLabel(entry)}
      </span>,
    )
  }

  const showChain = entry.hasProcessFacts ?? shared().wbRequestHover?.hasProcessFacts?.(entry) ?? attempts > 1
  if (showChain) {
    const count = attempts > 1 ? attempts : (Number(entry.internalRetries) || countRetries(entry))
    tags.push(
      <button key='chain' type='button'
        className='req-micro-tag is-warn req-hover-tag'
        data-req-hover='chain' data-req-id={identity}
        onClick={stopRowClick}>
        {count > 1 ? t('重试 {n}', { n: count }) : t('重试')}
      </button>,
    )
  }

  const hasSensitive = entry.hasSensitiveHits ?? (Array.isArray(entry.sensitiveHits) && entry.sensitiveHits.length > 0)
  if (hasSensitive) {
    tags.push(
      <button key='sensitive' type='button'
        className='req-micro-tag is-sens req-hover-tag'
        data-req-hover='sensitive' data-req-id={identity}
        aria-label={t('命中了关键词替换')}
        onClick={stopRowClick}>
        {t('替换')}
      </button>,
    )
  }

  const privacyHits = Array.isArray(entry.privacyHits) ? entry.privacyHits as { action?: unknown }[] : []
  const hasPrivacy = entry.hasPrivacyHits ?? privacyHits.length > 0
  if (hasPrivacy) {
    const action = String(
      entry.privacyAction
        || (privacyHits.some(hit => hit?.action === 'block')
          ? 'block'
          : privacyHits.some(hit => hit?.action === 'warn')
            ? 'warn'
            : 'mask'),
    )
    const tone = action === 'block'
      ? 'is-privacy-block'
      : action === 'warn'
        ? 'is-privacy-warn'
        : 'is-privacy'
    tags.push(
      <button key='privacy' type='button'
        className={`req-micro-tag ${tone} req-hover-tag`}
        data-req-hover='privacy' data-req-id={identity}
        aria-label={t('命中了隐私脱敏')}
        onClick={stopRowClick}>
        {t('脱敏')}
      </button>,
    )
  }

  if (!tags.length) return null
  return <span className='req-micro-tags'>{tags}</span>
}

function RunningStatusInline({ entry }: { entry: RequestEntry }) {
  const phase = window.wbRequestPhase
  if (!phase?.badgeHtml) {
    return (
      <span className='req-tag-run' title={t('请求正在转发中，用时列显示的是已用时')}>
        <span className='req-live-dot' aria-hidden='true' />
        {t('进行中')}
      </span>
    )
  }
  return <HtmlContent className='req-status-badge' html={phase.badgeHtml(entry)} render={<span />} />
}

/* ─── 单元格渲染 ───────────────────────────── */

export function RequestCell({
  entry,
  column,
  namesHidden,
  statusHidden,
  onOpenDetail,
}: {
  entry: RequestEntry
  column: VisibleColumn
  namesHidden: boolean
  statusHidden: boolean
  onOpenDetail: (entry: RequestEntry) => void
}): React.ReactNode {
  const pricing = usePricingDisplay()
  const { align, key } = column
  const base = column.sel.slice(1) + (key === 'dur' || key === 'speed' || key === 'billing' ? ' req-num' : '')
  const className = `${base} ta-${align}`
  const running = isRunning(entry)
  const ok = isOk(entry)
  const assistCall = !ok && (entry.visionMode === 'assist_call' || visionDetail(entry.route)?.mode === 'assist_call')

  switch (key) {
    case 'time': {
      if (!entry.ts) return <span key={key} className={className}>—</span>
      const d = new Date(entry.ts)
      const nowYear = new Date().getFullYear()
      const pad = (n: number) => String(n).padStart(2, '0')
      const fullDate = `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`
      const shortDate = d.getFullYear() === nowYear ? `${pad(d.getMonth() + 1)}-${pad(d.getDate())}` : fullDate
      const clock = `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
      const status = Number(entry.status) || 0
      return (
        <span key={key} className={className} title={`${fullDate} ${clock}`}>
          <span className='req-time-clock'>
            <span>{clock}</span>
            {statusHidden && !running && !ok ? (
              <span className='req-tag-err'>{status || t('失败')}</span>
            ) : null}
          </span>
          <span className='req-time-date'>
            <span>{shortDate}</span>
            {statusHidden && running ? <RunningStatusInline entry={entry} /> : null}
          </span>
        </span>
      )
    }

    case 'status': {
      if (running) {
        const elapsed = window.wbRequestPhase?.elapsedLineHtml?.(entry) ?? ''
        return (
          <span key={key} className={className}>
            <RunningStatusInline entry={entry} />
            {elapsed ? <HtmlContent className='req-status-line' html={elapsed} render={<span />} /> : null}
          </span>
        )
      }
      const status = Number(entry.status) || 0
      const title = !ok && status >= 200 && status < 300
        ? t('HTTP {status}，但响应体阶段出错', { status })
        : undefined
      return (
        <span key={key} className={className}>
          {ok ? (
            <span className='req-status-ok'>
              <span className='req-ok-dot' aria-hidden='true' />
              <span>{status}</span>
            </span>
          ) : (
            <span className='req-tag-err' title={title}>{status || t('失败')}</span>
          )}
          <ProcessTags entry={entry} />
        </span>
      )
    }

    case 'source': {
      const clientLabel = clientRequestLabel(entry)
      const keyLabel = apiKeyRequestLabel(entry)
      const clientTitle = entry.clientEvidence?.userAgent || entry.clientUserAgent || t('来源客户端：{client}', { client: clientLabel })
      const showKeySub = keyLabel && keyLabel !== clientLabel
      return (
        <span key={key} className={className}>
          <ClientBrand entry={entry} label={clientLabel} title={clientTitle} />
          {showKeySub ? (
            <span className='req-sub-indent' title={entry.apiKeyId || keyLabel}>{keyLabel}</span>
          ) : null}
        </span>
      )
    }

    case 'target': {
      const id = String(entry.provider ?? '').trim()
      const fromBackend = String(entry.providerLabel ?? '').trim()
      const provider = fromBackend ? t(fromBackend) : (id ? providerLabelOf(id) : '')
      const providerTitle = id && id !== provider ? t('{name}（{id}）', { name: provider, id }) : provider
      const rawAccountId = String(entry.accountId ?? '')
      const localDirectTail = rawAccountId.startsWith('local-direct:')
        ? rawAccountId.split(':').slice(2).join(':')
        : ''
      const accountTitle = localDirectTail
        ? (localDirectTail.includes('@') ? localDirectTail : entry.accountName)
        : (entry.accountId || entry.accountName)
      return (
        <span key={key} className={className}>
          {provider ? (
            <ProviderBrand providerId={id} label={provider} title={providerTitle} />
          ) : (
            <span className='req-provider is-empty'
              title={t('这条明细没有记录提供商（旧数据或请求未走到转发）')}>—</span>
          )}
          {entry.accountName ? (
            <span className='req-acct req-sub-indent' title={namesHidden ? undefined : accountTitle}>
              {namesHidden ? maskName(entry.accountName) : entry.accountName}
            </span>
          ) : (
            <span className='req-acct req-sub-indent is-empty'
              title={t('请求在任何账号接手之前就失败了')}>—</span>
          )}
        </span>
      )
    }

    case 'model': {
      const client = String(entry.clientModel ?? '').trim()
      const upstream = String(entry.upstreamModel ?? '').trim()
      const shown = String(entry.model ?? '').trim()
      const clientLevel = String(entry.clientReasoning ?? '').trim()
      const upstreamLevel = String(entry.upstreamReasoning ?? '').trim()
      const tag = (name: string, level: string) => formatModelWithReasoning(name, level, entry.origin)
      const isMapped = Boolean(client && upstream && upstream.toLowerCase() !== client.toLowerCase())
      const mainText = isMapped ? tag(upstream, upstreamLevel) : tag(shown, upstreamLevel || clientLevel)
      const subText = isMapped ? tag(client, clientLevel) : ''
      return (
        <span key={key} className={`${className} req-model-stack`}>
          <span className='req-model-main-row'>
            <span className='req-model-line' title={isMapped ? t('转发到上游的模型：{model}', { model: mainText }) : mainText}>
              {mainText || '—'}
            </span>
            {statusHidden ? <ProcessTags entry={entry} /> : null}
          </span>
          {subText ? (
            <span className='req-model-line sub' title={t('下游请求的模型：{model}', { model: subText })}>
              ↳ {subText}
            </span>
          ) : null}
        </span>
      )
    }

    case 'protocol': {
      const proto = describeRequestProtocol(entry.clientProtocol, entry.upstreamProtocol, t)
      if (!proto.mainLabel) {
        return (
          <span key={key} className={`${className} req-protocol-stack`}>
            <span className='req-model-line is-empty'>—</span>
          </span>
        )
      }
      const mainTitle = proto.converted
        ? t('转发到上游的协议：{protocol}', { protocol: proto.mainFullLabel })
        : t('请求协议：{protocol}', { protocol: proto.mainFullLabel })
      return (
        <span key={key} className={`${className} req-protocol-stack`}>
          <span className='req-model-line' title={mainTitle}>
            {proto.mainLabel}
          </span>
          {proto.subLabel ? (
            <span className='req-model-line sub' title={t('下游请求的协议：{protocol}', { protocol: proto.subFullLabel })}>
              ↳ {proto.subLabel}
            </span>
          ) : null}
        </span>
      )
    }

    case 'dur': {
      if (running) {
        const first = formatFirstResponseCompact(entry.firstResponseMs)
        const firstCls = `req-dur-line sub${toneClass(ttftTone(entry.firstResponseMs))}`
        return (
          <span key={key} className={className}>
            <span className='req-dur-line is-tone-info' title={t('已用时（请求仍在转发中）')}>{formatElapsed(entry.ts)}</span>
            {first ? (
              <span className={firstCls} title={t('首响：上游首帧到达的耗时（请求仍在转发中）')}>
                {t('首字 {time}', { time: first })}
              </span>
            ) : (
              <span className='req-dur-line sub'>{t('进行中')}</span>
            )}
          </span>
        )
      }
      const first = formatFirstResponseCompact(entry.firstResponseMs)
      const firstCls = `req-dur-line sub${ok ? toneClass(ttftTone(entry.firstResponseMs)) : ''}`
      const tipLines = [
        `${t('总耗时')}：${formatDurationExact(entry.durationMs)}`,
        first ? `${t('首字响应')}：${formatDurationExact(entry.firstResponseMs)}` : '',
      ].filter(Boolean)
      return (
        <span key={key} className={className} title={tipLines.join('\n')}>
          <span className='req-dur-line'>{formatDurationCompact(entry.durationMs)}</span>
          {first ? <span className={firstCls}>{t('首字 {time}', { time: first })}</span> : null}
        </span>
      )
    }

    case 'speed': {
      if (running) return <span key={key} className={className}><span className='req-speed-line is-dim'>—</span></span>
      const rate = ok ? tokensPerSecondNumber(entry) : null
      const speed = formatTokensPerSecondValue(rate)
      const tip = speed ? `${t('生成速度')}：${speed} tok/s\n${SPEED_TIP}` : SPEED_TIP
      return (
        <span key={key} className={className} title={tip}>
          {speed ? (
            <span className={`req-speed-line${toneClass(speedTone(rate))}`}>{speed} tok/s</span>
          ) : (
            <span className='req-speed-line is-dim'>—</span>
          )}
        </span>
      )
    }

    case 'usage': {
      if (running) return <span key={key} className={className}><span className='req-usage-line is-dim'>—</span></span>
      if (!ok && !assistCall) {
        return (
          <span key={key} className={className} title={t('失败请求不记录用量')}>
            <span className='req-usage-line is-dim'>—</span>
          </span>
        )
      }
      const prompt = Number(entry.promptTokens) || 0
      const completion = Number(entry.completionTokens) || 0
      const total = Number(entry.totalTokens) || 0
      const cacheRead = Number(entry.cacheReadTokens) || 0
      const cacheWrite = Number(entry.cacheCreationTokens) || 0
      const rate = cacheRateText(entry)
      const isZero = prompt === 0 && completion === 0 && total === 0
      const cacheCls = `req-usage-line sub${toneClass(cacheTone(prompt, cacheRead, cacheWrite))}`
      const tipLines = [
        `in: ${formatTokens(prompt)} / out: ${formatTokens(completion)} / all: ${formatTokens(total)}`,
        t('缓存读取: {cache} / 命中率: {rate}', { cache: formatTokens(cacheRead), rate: rate || '-' }),
        cacheWrite > 0 ? t('缓存写入: {cw}', { cw: formatTokens(cacheWrite) }) : '',
      ].filter(Boolean)
      return (
        <span key={key} className={className} title={tipLines.join('\n')}>
          {isZero ? (
            <span className='req-usage-line is-dim'>0</span>
          ) : (
            <span className='req-usage-line'>
              <span className='req-tok-arrow'>↑</span>{formatTokens(prompt)}
              <span className='req-sep-dot'>·</span>
              <span className='req-tok-arrow'>↓</span>{formatTokens(completion)}
            </span>
          )}
          {cacheRead > 0 ? (
            <span className={cacheCls}>
              {t('缓存 {cache}', { cache: formatTokens(cacheRead) })}{rate ? ` (${rate})` : ''}
            </span>
          ) : cacheWrite > 0 ? (
            <span className={cacheCls}>
              {t('缓存写 {cw}', { cw: formatTokens(cacheWrite) })}
            </span>
          ) : !isZero ? (
            <span className='req-usage-line sub'>{t('共 {total}', { total: formatTokens(total) })}</span>
          ) : null}
        </span>
      )
    }

    case 'billing': {
      if (running) return <span key={key} className={className}><span className='req-billing-line is-dim'>—</span></span>
      if (!ok && !assistCall) {
        return <span key={key} className={className} title={t('失败请求不记录用量')}><span className='req-billing-line is-dim'>—</span></span>
      }
      const usd = entry.costUsd
      const hasCost = usd !== null && usd !== undefined && Number.isFinite(Number(usd))
      const credit = entry.credit
      const hasCredit = credit !== null && credit !== undefined && Number.isFinite(Number(credit))
      if (!hasCost && !hasCredit) {
        return <span key={key} className={className} title={t('未匹配到定价或无积分读数')}><span className='req-billing-line is-dim'>—</span></span>
      }
      const costText = hasCost ? formatCost(Number(usd), pricing.currency, pricing.exchangeRate) : ''
      const isZeroCost = hasCost && Number(usd) === 0
      const tipLines = [
        hasCost ? `${t('预估费用')}：${costText}` : '',
        hasCost && pricing.currency === 'cny' ? `USD: $${Number(usd).toFixed(5)}` : '',
        hasCost && entry.priceSource ? `${t('计价依据')}：${pricingSourceLabel(entry.priceSource)}` : '',
        hasCredit ? t('本次请求消耗 {credit} 积分', { credit: numberText(credit) }) : '',
      ].filter(Boolean)
      return (
        <span key={key} className={className} title={tipLines.join('\n')}>
          {hasCost ? (
            <span className={`req-billing-line${isZeroCost ? ' is-dim' : ' is-strong'}`}>{costText}</span>
          ) : (
            <span className='req-billing-line is-strong'>{t('{credit} 积分', { credit: numberText(credit) })}</span>
          )}
          {hasCost && hasCredit ? (
            <span className='req-billing-line sub'>{t('{credit} 积分', { credit: numberText(credit) })}</span>
          ) : null}
        </span>
      )
    }

    case 'error': {
      if (running) return <span key={key} className={className} />
      if (entry.error) {
        return <span key={key} className={className} title={String(entry.error)}>{String(entry.error)}</span>
      }
      return <span key={key} className={`req-none ${base} ta-${align}`}>—</span>
    }

    case 'detail': {
      const id = entry.id ? String(entry.id) : ''
      if (!id && !entry.hasClientEvidence && !entry.clientEvidence && !entry.rowId) {
        return <span key={key} className={`req-none ${base} ta-${align}`}>—</span>
      }
      return (
        <span key={key} className={className}>
          <Button size='xs' variant='outline' className='req-detail-btn' title={t('请求详情')}
            onClick={event => {
              event.stopPropagation()
              onOpenDetail(entry)
            }}>{t('详情')}</Button>
        </span>
      )
    }

    default:
      return null
  }
}

export function headRow(columns: VisibleColumn[], namesHidden: boolean): React.ReactNode {
  return (
    <div className='req-head' key='__head'>
      {columns.map(column => (
        <span key={column.key} data-col={column.key}
          className={`${column.sel.slice(1)}${column.key === 'dur' || column.key === 'speed' || column.key === 'billing' ? ' req-num' : ''}`}
          title={column.key === 'speed' ? SPEED_TIP : undefined}>
          {column.key === 'target' ? (
            <span className='req-target-label'>
              {column.label}
              <NameEyeButton hidden={namesHidden} />
            </span>
          ) : column.label}
        </span>
      ))}
    </div>
  )
}
