import * as React from 'react'
import {
  Badge, Button, HtmlContent, StatusDot,
  Tooltip, TooltipContent, TooltipTrigger, cn,
} from '@ui'
import '../../styles/request-detail.css'
import { t } from '../../i18n'
import { requestApiKeyLabel, requestClientLabel } from '../../../../ui-shared/model-api-key'
import { ClientIdentificationDetail } from '../../../../ui-shared/client-identification-detail'
import { VisionRequestDetail, visionDetail } from '../../../../ui-shared/request-vision-detail'
import { describeRequestProtocol } from '../../../../ui-shared/request-protocol'
import type { ClientEvidence } from '../../../../ui-shared/client-identification'
import { errorPolicyDescription } from '../../../../ui-shared/routing-errors-record'
import { clientIdentificationApi } from '../settings-client-identification'
import { formatCost, pricingSourceLabel, usePricingDisplay, type ModelPricingInfo } from '../model-pricing'
import { RequestEmpty as Empty } from '../request-raw-panes'
import { RouteReplayFacts } from '../routing-replay'
import type { ReplayRow, RouteDecision } from '../routing-types'
import { numberText } from '../accounts-shared'
import { ClientIcon, ProviderIcon } from './request-brand'
import { formatModelWithReasoning } from './request-cells'
import { privacyActionLabel, privacyKindLabel } from '../settings-privacy-model'

export type AttemptDetail = {
  provider?: string
  providerLabel?: string
  account?: string
  status?: number | string | null
  error?: unknown
  retries?: { reason?: unknown; delayMs?: unknown }[]
  errorPolicy?: import('../../../../ui-shared/routing-errors-types').ErrorPolicyRecord
  notice?: unknown
  bodyBytes?: unknown
}

export type SensitiveHit = { word?: unknown; count?: unknown }
export type PrivacyHit = { kind?: unknown; count?: unknown; action?: unknown; restored?: unknown }
export type ProviderFields = { provider?: string; providerLabel?: string }

export type RequestRow = {
  rowId?: number
  id?: string
  detailsLoaded?: boolean
  ts?: number | string | null
  status?: number | string | null
  error?: unknown
  durationMs?: unknown
  firstResponseMs?: unknown
  attempts?: unknown
  promptTokens?: unknown
  completionTokens?: unknown
  totalTokens?: unknown
  cacheReadTokens?: unknown
  cacheCreationTokens?: unknown
  cacheCreation1hTokens?: unknown
  costUsd?: number | null
  costPriced?: boolean
  priceSource?: ModelPricingInfo['source']
  credit?: number | null
  model?: unknown
  clientModel?: unknown
  upstreamModel?: unknown
  clientReasoning?: unknown
  upstreamReasoning?: unknown
  clientProtocol?: unknown
  upstreamProtocol?: unknown
  accountId?: unknown
  accountName?: unknown
  attemptDetails?: AttemptDetail[]
  sensitiveHits?: SensitiveHit[]
  privacyHits?: PrivacyHit[]
  route?: unknown
  isTest?: boolean
  client?: string
  clientLabel?: string
  clientEvidence?: ClientEvidence
  apiKeyId?: string
  apiKeyName?: string
  apiKeySource?: string
  origin?: string
  localDirect?: {
    agentId?: string
    rootKind?: string
    conversationId?: string
    conversationTitle?: string
    workspaceUri?: string
    stepIndex?: number
    responseId?: string
    transcriptPath?: string
    dbPath?: string
  }
} & ProviderFields

type SharedWindow = {
  wbRequestPhase?: { badgeHtml?: (entry: unknown) => string; elapsedLineHtml?: (entry: unknown) => string }
  wbProviders?: { labelOf?: (id: string) => string }
  wbUnits?: { formatTokens?: (value: unknown) => string }
}

function shared(): SharedWindow {
  return window as unknown as SharedWindow
}

export function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

export function isOk(entry: RequestRow): boolean {
  const status = Number(entry.status) || 0
  return status >= 200 && status < 300 && !entry.error
}

export function isRunning(entry: RequestRow | null | undefined): boolean {
  return (Number(entry?.status) || 0) === 0 && !entry?.error
}

export function hasRoute(row: RequestRow | null | undefined): row is RequestRow & { route: RouteDecision } {
  return isPlainObject(row?.route)
}

export function providerName(entry: ProviderFields): string {
  const id = String(entry.provider ?? '').trim()
  const label = String(entry.providerLabel ?? '').trim()
  return label || (id ? shared().wbProviders?.labelOf?.(id) || id : '')
}

function fmtTime(ts: RequestRow['ts']): string {
  if (!ts) return '—'
  const d = new Date(ts)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`
    + ` ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
}

function fmtDuration(ms: unknown): string {
  const rounded = Math.round(Number(ms) || 0)
  if (rounded < 1000) return `${rounded}ms`
  const seconds = Math.floor(rounded / 1000)
  if (seconds >= 60) return t('{m}分{s}秒', { m: Math.floor(seconds / 60), s: seconds % 60 })
  const millis = rounded % 1000
  return millis > 0 ? t('{s}秒{ms}ms', { s: seconds, ms: millis }) : t('{n}秒', { n: seconds })
}

function fmtDurationCompact(ms: unknown): string {
  const value = Number(ms)
  if (!Number.isFinite(value) || value <= 0) return '—'
  const rounded = Math.round(value)
  if (rounded < 1000) return `${rounded}ms`
  if (rounded >= 60_000) {
    const seconds = Math.floor(rounded / 1000)
    return t('{m}分{s}秒', { m: Math.floor(seconds / 60), s: seconds % 60 })
  }
  const sec = rounded / 1000
  return t('{n}秒', { n: rounded % 1000 === 0 ? String(Math.floor(sec)) : sec.toFixed(1) })
}

function fmtFirstResponse(ms: unknown): string {
  const value = Number(ms)
  if (!Number.isFinite(value) || value <= 0) return '—'
  return fmtDuration(value)
}

const SPEED_TIP = t('生成速度：输出 token ÷ 流式段用时（总用时 − 首响）；流式段不足总用时一半时按总用时计')

function fmtTokensPerSecond(row: RequestRow): string {
  const tokens = Number(row.completionTokens) || 0
  const duration = Number(row.durationMs) || 0
  if (tokens <= 0 || duration <= 0) return ''
  const first = Number(row.firstResponseMs)
  const streamMs = Number.isFinite(first) && first > 0 && first < duration ? duration - first : 0
  const streaming = streamMs >= 500 && streamMs * 2 >= duration
  const rate = tokens / ((streaming ? streamMs : duration) / 1000)
  return `${rate >= 100 ? Math.round(rate) : rate.toFixed(1)} tok/s`
}

function fmtTokens(value: unknown): string {
  return shared().wbUnits?.formatTokens?.(value) ?? (Number(value) || 0).toLocaleString('zh-CN')
}

function fmtExactInt(value: unknown): string {
  return (Number(value) || 0).toLocaleString('zh-CN')
}

function fmtElapsed(ts: RequestRow['ts']): string {
  const elapsed = Math.max(0, Date.now() - (Number(ts) || 0))
  return t('{n}秒', { n: Math.max(1, Math.floor(elapsed / 1000)) })
}

function fmtBodyBytes(raw: unknown): { text: string; title?: string } {
  const bytes = Number(raw)
  if (!Number.isFinite(bytes) || bytes <= 0) return { text: '' }
  const text = bytes >= 1024 * 1024
    ? `${(bytes / 1024 / 1024).toFixed(1)} MB`
    : bytes >= 1024
      ? `${Math.round(bytes / 1024)} KB`
      : `${bytes} B`
  return { text, title: t('实际发给上游 {bytes} 字节', { bytes: bytes.toLocaleString() }) }
}

function FlowArrowIcon() {
  return (
    <svg width='12' height='12' viewBox='0 0 12 12' fill='none' stroke='currentColor' strokeWidth='1.5' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
      <path d='M2.5 6h7M6.5 3l3 3-3 3' />
    </svg>
  )
}

function ChevronDownIcon({ open }: { open: boolean }) {
  return (
    <svg width='11' height='11' viewBox='0 0 16 16' fill='none' stroke='currentColor' strokeWidth='1.8' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'
      className={cn('transition-transform duration-150', open && 'rotate-180')}>
      <path d='m4 6 4 4 4-4' />
    </svg>
  )
}

export function StatusSummary({ row }: { row: RequestRow }) {
  if (isRunning(row)) {
    const phase = shared().wbRequestPhase
    if (!phase?.badgeHtml) {
      return (
        <Badge variant='info' shape='tag' title={t('请求正在转发中，用时列显示的是已用时')}>
          <StatusDot pulse className='size-[5px] bg-current' />
          {t('进行中')}
        </Badge>
      )
    }
    const elapsed = phase.elapsedLineHtml?.(row) ?? ''
    return (
      <span className='inline-flex flex-wrap items-center justify-end gap-1.5'>
        <HtmlContent html={phase.badgeHtml(row)} render={<span />} />
        {elapsed ? (
          <HtmlContent className='text-[11.5px] text-muted-foreground' html={elapsed} render={<span />} />
        ) : null}
      </span>
    )
  }
  const status = Number(row.status) || 0
  const ok = isOk(row)
  if (ok) {
    return (
      <span className='req-status-ok'>
        <span className='req-ok-dot' aria-hidden='true' />
        <span>{status}</span>
      </span>
    )
  }
  const title = status >= 200 && status < 300 ? t('HTTP {status}，但响应体阶段出错', { status }) : undefined
  return <span className='req-tag-err' title={title}>{status || t('失败')}</span>
}

function AttemptCard({ item, index, runningReq }: { item: AttemptDetail; index: number; runningReq: boolean }) {
  const [retriesOpen, setRetriesOpen] = React.useState(false)
  const status = Number(item?.status)
  const hasStatus = item?.status !== null && item?.status !== undefined && Number.isFinite(status)
  const error = item?.error ? String(item.error) : ''
  const ok = !error && hasStatus && status >= 200 && status < 300
  const providerId = String(item?.provider ?? '').trim()
  const provider = providerName(item) || (providerId || t('未知'))
  const account = item?.account ? String(item.account) : ''
  const retries = Array.isArray(item?.retries) ? item.retries : []
  const notice = [String(item?.notice ?? '').trim(), errorPolicyDescription(item?.errorPolicy, t)].filter(Boolean).join(' · ')
  const body = fmtBodyBytes(item?.bodyBytes)
  const hasMeta = Boolean(body.text || retries.length > 0 || notice)

  return (
    <div className={cn('req-detail-attempt', error && 'is-failed', ok && 'is-ok')}>
      <div className='req-detail-attempt-top'>
        <div className='flex min-w-0 flex-1 items-center gap-2'>
          <span className='req-detail-attempt-num'>{index + 1}</span>
          <div className='flex min-w-0 flex-1 flex-wrap items-center gap-x-2 gap-y-0.5'>
            <span className='inline-flex min-w-0 items-center gap-1.5 text-[12.5px] font-medium text-foreground'>
              <ProviderIcon providerId={providerId} size={15} />
              <span className='truncate'>{provider}</span>
            </span>
            {account ? (
              <span className='truncate font-mono text-[11.5px] text-muted-foreground' title={account}>
                {account}
              </span>
            ) : null}
          </div>
        </div>

        <div className='shrink-0'>
          {error ? (
            <Badge variant='destructive' shape='tag'>
              {hasStatus ? t('失败 {status}', { status: String(status) }) : t('失败')}
            </Badge>
          ) : hasStatus ? (
            <Badge variant='success' shape='tag'>
              {t('成功 {status}', { status: String(status) })}
            </Badge>
          ) : (
            <Badge variant='outline' shape='tag'>
              {runningReq ? t('响应中 · 尚无结果') : t('无结果记录')}
            </Badge>
          )}
        </div>
      </div>

      {hasMeta ? (
        <div className='req-detail-attempt-meta'>
          {body.text ? (
            <span className='font-mono tabular-nums' title={body.title}>
              {t('请求体积 {size}', { size: body.text })}
            </span>
          ) : null}
          {retries.length > 0 ? (
            <button
              type='button'
              className='req-detail-retry-toggle'
              aria-expanded={retriesOpen}
              onClick={() => setRetriesOpen(prev => !prev)}
            >
              <span>{t('↻ 内部重试 {n} 次', { n: retries.length })}</span>
              <ChevronDownIcon open={retriesOpen} />
            </button>
          ) : null}
          {notice ? <span className='text-subtle'>{notice}</span> : null}
        </div>
      ) : null}

      {retriesOpen && retries.length > 0 ? (
        <div className='req-detail-retry-box'>
          {retries.map((retry, idx) => {
            const delayMs = Number(retry?.delayMs)
            const delay = Number.isFinite(delayMs) && delayMs > 0
              ? (delayMs >= 1000
                  ? t('{n}秒后重试', { n: delayMs % 1000 === 0 ? Math.round(delayMs / 1000) : Number((delayMs / 1000).toFixed(1)) })
                  : t('{n}毫秒后重试', { n: Math.round(delayMs) }))
              : ''
            return (
              <div key={idx} className='flex items-baseline justify-between gap-2'>
                <span className='min-w-0 break-words'>
                  {t('第 {n} 次：{reason}', { n: idx + 1, reason: String(retry?.reason || t('未知原因')) })}
                </span>
                {delay ? (
                  <span className='shrink-0 font-mono text-[11px] text-muted-foreground tabular-nums'>{delay}</span>
                ) : null}
              </div>
            )
          })}
        </div>
      ) : null}

      {error ? <div className='req-detail-attempt-err'>{error}</div> : null}
    </div>
  )
}

function AttemptsSection({ row, loading }: { row: RequestRow; loading?: boolean }) {
  const details = Array.isArray(row.attemptDetails) ? row.attemptDetails : []
  if (loading && !details.length) {
    return (
      <div className='req-detail-group'>
        <div className='req-detail-group-head'>
          <span className='req-detail-group-title'>{t('尝试明细')}</span>
        </div>
        <span className='px-0.5 text-xs text-muted-foreground'>{t('正在加载明细…')}</span>
      </div>
    )
  }
  if (!details.length) return null
  const attempts = Number(row.attempts) || details.length
  const failedCount = details.filter(item => Boolean(item?.error)).length
  const runningReq = isRunning(row)

  return (
    <div className='req-detail-group'>
      <div className='req-detail-group-head'>
        <span className='req-detail-group-title'>{t('尝试明细')}</span>
        <span className='font-mono text-[11px] text-muted-foreground tabular-nums'>
          {failedCount > 0
            ? t('共 {n} 轮 · 失败 {failed} 次', { n: attempts, failed: failedCount })
            : t('共 {n} 轮', { n: attempts })}
        </span>
      </div>
      <div className='req-detail-attempts'>
        {details.map((item, index) => (
          <AttemptCard key={index} item={item} index={index} runningReq={runningReq} />
        ))}
        {details.length < attempts ? (
          <div className='px-1 text-[11.5px] text-muted-foreground'>
            {t('另有 {missing} 次尝试未记录明细（只保留最早的 {kept} 条）', {
              missing: attempts - details.length, kept: details.length,
            })}
          </div>
        ) : null}
      </div>
    </div>
  )
}

export function RequestRoutePane({ row }: { row: RequestRow & { route: RouteDecision } }) {
  return (
    <div className='flex flex-col gap-3'>
      <RouteReplayFacts route={row.route}
        accountId={String(row.accountId || '')} accountName={String(row.accountName || '')} />
      <Button variant='outline' size='sm' className='self-start' onClick={() => {
        void window.wbRoutingPanel?.openReplay(row as unknown as ReplayRow)
      }}>{t('在路由页查看')}</Button>
    </div>
  )
}

function localRootKindLabel(kind?: string): string {
  switch (kind) {
    case 'desktop': return t('桌面版')
    case 'insiders': return t('Insiders 版')
    case 'cli': return 'CLI (agy)'
    case 't3_acp': return 'T3 Code ACP'
    case 'wsl': return 'WSL'
    case 'custom': return t('自定义目录')
    default: return kind || '—'
  }
}

export function RequestDetailPane({ row, loadingDetail = false }: { row: RequestRow | null; loadingDetail?: boolean }) {
  const pricing = usePricingDisplay()
  if (!row && loadingDetail) return <Empty text={t('正在加载请求详情…')} />
  if (!row) return <Empty text={t('列表数据已刷新，请重试（关闭后重新打开这条详情）')} />

  const running = isRunning(row)
  const providerId = String(row.provider ?? '').trim()
  const provider = providerName(row)
  const account = String(row.accountName ?? '').trim()
  const clientLabel = requestClientLabel(row, t)
  const apiKeyLabel = requestApiKeyLabel(row, t)

  const clientModel = String(row.clientModel ?? '').trim()
  const upstreamModel = String(row.upstreamModel ?? '').trim()
  const shownModel = String(row.model ?? '').trim()
  const clientLevel = String(row.clientReasoning ?? '').trim()
  const upstreamLevel = String(row.upstreamReasoning ?? '').trim()
  const withLevel = (name: string, level: string) => formatModelWithReasoning(name, level, row.origin)
  const isMapped = Boolean(clientModel && upstreamModel && upstreamModel.toLowerCase() !== clientModel.toLowerCase())
  const actualModelText = isMapped ? withLevel(upstreamModel, upstreamLevel) : withLevel(shownModel, upstreamLevel || clientLevel)
  const requestedModelText = isMapped ? withLevel(clientModel, clientLevel) : ''

  const prompt = Number(row.promptTokens) || 0
  const completion = Number(row.completionTokens) || 0
  const total = Number(row.totalTokens) || 0
  const cacheRead = Number(row.cacheReadTokens) || 0
  const cacheWrite = Number(row.cacheCreationTokens) || 0
  const cachePct = prompt > 0 && cacheRead > 0 ? (cacheRead / prompt) * 100 : null
  const cacheRate = cachePct !== null ? `${cachePct.toFixed(1)}%` : ''
  const cacheToneCls = cachePct === null
    ? 'is-muted'
    : cachePct >= 70
      ? 'is-good'
      : cachePct >= 30
        ? 'is-info'
        : 'is-warn'

  const hasCost = row.costUsd !== null && row.costUsd !== undefined && Number.isFinite(Number(row.costUsd))
  const hasCredit = row.credit !== null && row.credit !== undefined && Number.isFinite(Number(row.credit))
  const formattedCost = hasCost ? formatCost(Number(row.costUsd), pricing.currency, pricing.exchangeRate) : ''
  const costMain = hasCost
    ? formattedCost
    : hasCredit
      ? t('{credit} 积分', { credit: numberText(row.credit) })
      : '—'
  const costSub = hasCost
    ? [
        pricing.currency === 'cny' ? `$${Number(row.costUsd).toFixed(5)}` : '',
        hasCredit
          ? t('{credit} 积分', { credit: numberText(row.credit) })
          : (row.priceSource ? pricingSourceLabel(row.priceSource) : ''),
      ].filter(Boolean).join(' · ')
    : ''

  const firstResp = fmtFirstResponse(row.firstResponseMs)
  const speed = fmtTokensPerSecond(row)
  const durMain = running ? fmtElapsed(row.ts) : fmtDurationCompact(row.durationMs)
  const durSub = running
    ? t('进行中')
    : [
        firstResp !== '—' ? t('首响 {time}', { time: firstResp }) : '',
        speed,
      ].filter(Boolean).join(' · ')

  const details = Array.isArray(row.attemptDetails) ? row.attemptDetails : []
  const hits = Array.isArray(row.sensitiveHits) ? row.sensitiveHits : []
  const privacyHits = Array.isArray(row.privacyHits) ? row.privacyHits : []
  const hasVision = Boolean(visionDetail(row.route))
  const hasProcessing = hits.length > 0 || privacyHits.length > 0
  const proto = describeRequestProtocol(row.clientProtocol, row.upstreamProtocol, t)
  const protoTitle = proto.converted
    ? t('协议转换：{from} → {to}', { from: proto.subFullLabel, to: proto.mainFullLabel })
    : proto.mainFullLabel
      ? t('请求协议：{protocol}', { protocol: proto.mainFullLabel })
      : ''

  return (
    <div className='req-detail-sections'>
      {row.error ? (
        <div className='req-detail-err-banner'>
          <span className='req-detail-err-text'>{String(row.error)}</span>
          <Button variant='outline' size='xs' className='shrink-0' data-copy={String(row.error)}>{t('复制')}</Button>
        </div>
      ) : null}

      {/* 顶部路由摘要卡 */}
      <div className='req-detail-hero'>
        <div className='req-detail-hero-top'>
          <div className='min-w-0 flex-1'>
            <div className='flex flex-wrap items-center gap-1.5'>
              <span className='req-detail-hero-model'>{actualModelText || '—'}</span>
              {proto.inlineText ? (
                <Badge variant='outline' shape='tag' className='font-mono' title={protoTitle}>
                  {proto.inlineText}
                </Badge>
              ) : null}
              {row.origin === 'local_direct' ? (
                <Badge variant='info' shape='tag'>{t('本机直连')}</Badge>
              ) : null}
              {row.isTest ? (
                <Badge variant='brand' shape='tag'
                  title={t('面板内发起的模型调用，消耗额度并记录 Key，不计入生产报表')}>
                  {clientLabel}
                </Badge>
              ) : null}
            </div>
            {requestedModelText ? (
              <div className='mt-0.5 font-mono text-[11px] text-muted-foreground'>
                {t('原请求 {model} → 实际转发', { model: requestedModelText })}
              </div>
            ) : null}
          </div>
          <span className='shrink-0 pt-0.5 font-mono text-[11.5px] text-muted-foreground tabular-nums'>
            {fmtTime(row.ts)}
          </span>
        </div>

        <div className='req-detail-flow'>
          <div className='req-detail-flow-node'>
            <ClientIcon client={row.client} label={clientLabel} size={20} />
            <div className='min-w-0 flex-1'>
              <div className='truncate text-[12.5px] font-medium text-foreground' title={clientLabel}>{clientLabel}</div>
              <div className='truncate text-[11px] text-muted-foreground' title={row.origin === 'local_direct' ? t('未经过网关') : apiKeyLabel}>
                {row.origin === 'local_direct' ? t('未经过网关') : apiKeyLabel}
              </div>
            </div>
          </div>

          <span className='req-detail-flow-arrow' aria-hidden='true'>
            <FlowArrowIcon />
          </span>

          <div className='req-detail-flow-node'>
            <ProviderIcon providerId={row.origin === 'local_direct' ? 'local_direct' : providerId} size={20} />
            <div className='min-w-0 flex-1'>
              <div className='truncate text-[12.5px] font-medium text-foreground' title={provider || '—'}>
                {provider || '—'}
              </div>
              <div className='truncate font-mono text-[11px] text-muted-foreground' title={account || '—'}>
                {account || '—'}
              </div>
            </div>
          </div>
        </div>
      </div>

      {/* 四宫格核心指标 */}
      <div className='req-detail-kpi-grid'>
        <Tooltip>
          <TooltipTrigger
            render={
              <div className='req-detail-kpi-card'>
                <span className='req-detail-kpi-label'>{t('总 Token')}</span>
                <span className={cn('req-detail-kpi-value', total === 0 && 'is-muted')}>
                  {fmtTokens(total)}
                </span>
                <span className='req-detail-kpi-sub'>
                  {t('入 {in} · 出 {out}', { in: fmtTokens(prompt), out: fmtTokens(completion) })}
                </span>
              </div>
            }
          />
          <TooltipContent variant='plain' side='bottom'>
            <div className='space-y-0.5 font-mono text-[11px] tabular-nums'>
              <div>{t('输入')}：{fmtExactInt(prompt)}</div>
              <div>{t('输出')}：{fmtExactInt(completion)}</div>
              <div>{t('合计')}：{fmtExactInt(total)}</div>
            </div>
          </TooltipContent>
        </Tooltip>

        <Tooltip>
          <TooltipTrigger
            render={
              <div className='req-detail-kpi-card'>
                <span className='req-detail-kpi-label'>{t('缓存命中')}</span>
                <span className={cn('req-detail-kpi-value', cacheToneCls)}>
                  {cacheRate || fmtTokens(cacheRead)}
                </span>
                <span className='req-detail-kpi-sub'>
                  {cacheWrite > 0
                    ? t('读 {read} · 写 {write}', { read: fmtTokens(cacheRead), write: fmtTokens(cacheWrite) })
                    : t('读 {read}', { read: fmtTokens(cacheRead) })}
                </span>
              </div>
            }
          />
          <TooltipContent variant='plain' side='bottom'>
            <div className='space-y-0.5 font-mono text-[11px] tabular-nums'>
              <div>{t('缓存读取')}：{fmtExactInt(cacheRead)}{cacheRate ? ` (${cacheRate})` : ''}</div>
              {cacheWrite > 0 ? <div>{t('缓存写入')}：{fmtExactInt(cacheWrite)}</div> : null}
            </div>
          </TooltipContent>
        </Tooltip>

        {hasCost || hasCredit ? (
          <Tooltip>
            <TooltipTrigger
              render={
                <div className='req-detail-kpi-card'>
                  <span className='req-detail-kpi-label'>{t('预估费用')}</span>
                  <span className='req-detail-kpi-value'>{costMain}</span>
                  {costSub ? <span className='req-detail-kpi-sub' title={costSub}>{costSub}</span> : null}
                </div>
              }
            />
            <TooltipContent variant='plain' side='bottom'>
              <div className='space-y-0.5 font-mono text-[11px] tabular-nums'>
                {hasCost ? (
                  <div>
                    {t('预估费用')}：{formattedCost}
                    {pricing.currency === 'cny' ? ` ($${Number(row.costUsd).toFixed(5)})` : ''}
                  </div>
                ) : null}
                {row.priceSource ? <div>{pricingSourceLabel(row.priceSource)}</div> : null}
                {hasCredit ? <div>{t('积分消耗')}：{numberText(row.credit)}</div> : null}
              </div>
            </TooltipContent>
          </Tooltip>
        ) : (
          <div className='req-detail-kpi-card'>
            <span className='req-detail-kpi-label'>{t('预估费用')}</span>
            <span className='req-detail-kpi-value is-muted'>—</span>
          </div>
        )}

        <Tooltip>
          <TooltipTrigger
            render={
              <div className='req-detail-kpi-card'>
                <span className='req-detail-kpi-label'>{t('总耗时')}</span>
                <span className={cn('req-detail-kpi-value', durMain === '—' && 'is-muted')}>{durMain}</span>
                {durSub ? <span className='req-detail-kpi-sub' title={durSub}>{durSub}</span> : null}
              </div>
            }
          />
          <TooltipContent variant='plain' side='bottom'>
            <div className='space-y-0.5 font-mono text-[11px] tabular-nums'>
              <div>
                {t('总耗时')}：{running ? `${fmtElapsed(row.ts)}${t('（进行中）')}` : fmtDuration(row.durationMs)}
              </div>
              <div>{t('首字响应')}：{running ? '—' : firstResp}</div>
              <div>{t('生成速度')}：{running ? '—' : speed || '—'}</div>
              <div className='pt-1 font-sans text-[10.5px] text-muted-foreground'>{SPEED_TIP}</div>
            </div>
          </TooltipContent>
        </Tooltip>
      </div>

      {hasVision ? (
        <div className='req-detail-group'>
          <div className='req-detail-group-head'>
            <span className='req-detail-group-title'>{t('图片兼容')}</span>
          </div>
          <div className='req-detail-dl py-2.5'>
            <VisionRequestDetail route={row.route} t={t} />
          </div>
        </div>
      ) : null}

      {loadingDetail || details.length > 0 ? <AttemptsSection row={row} loading={loadingDetail} /> : null}

      {hasProcessing ? (
        <div className='req-detail-group'>
          <div className='req-detail-group-head'>
            <span className='req-detail-group-title'>{t('内容处理')}</span>
          </div>
          <div className='req-detail-process-card'>
            {hits.length > 0 ? (
              <div className='req-detail-process-row'>
                <span className='req-detail-process-label'>{t('关键词替换')}</span>
                <div className='flex min-w-0 flex-1 flex-wrap gap-1.5'>
                  {hits.map((hit, index) => (
                    <Badge key={index} variant='sensitive' shape='tag'>
                      {String(hit?.word ?? '')} × {String(Number(hit?.count) || 0)}
                    </Badge>
                  ))}
                </div>
              </div>
            ) : null}
            {privacyHits.length > 0 ? (
              <div className='req-detail-process-row'>
                <span className='req-detail-process-label'>{t('隐私脱敏')}</span>
                <div className='flex min-w-0 flex-1 flex-wrap gap-1.5'>
                  {privacyHits.map((hit, index) => {
                    const action = String(hit?.action ?? 'mask')
                    const count = Number(hit?.count) || 0
                    const restored = Number(hit?.restored) || 0
                    const variant = action === 'block'
                      ? 'destructive'
                      : action === 'warn'
                        ? 'warning'
                        : 'info'
                    return (
                      <Badge key={index} variant={variant} shape='tag'>
                        {privacyKindLabel(hit?.kind)} · {privacyActionLabel(action)} × {String(count)}
                        {restored > 0 ? t('（已还原 {n}）', { n: restored }) : ''}
                      </Badge>
                    )
                  })}
                </div>
              </div>
            ) : null}
          </div>
        </div>
      ) : null}

      {row.origin === 'local_direct' ? (
        row.localDirect ? (
          <div className='req-detail-group'>
            <div className='req-detail-group-head'>
              <span className='req-detail-group-title'>{t('本机直连信息')}</span>
              <Badge variant='info' shape='tag'>{localRootKindLabel(row.localDirect.rootKind)}</Badge>
            </div>
            <div className='req-detail-id-card'>
              <div className='req-detail-id-body'>
                {row.localDirect.conversationTitle ? (
                  <div className='req-detail-id-kv'>
                    <span className='req-detail-id-k'>{t('对话标题')}</span>
                    <span className='font-sans text-[12px] text-foreground break-words'>{row.localDirect.conversationTitle}</span>
                  </div>
                ) : null}
                {row.localDirect.workspaceUri ? (
                  <div className='req-detail-id-kv'>
                    <span className='req-detail-id-k'>{t('工作区')}</span>
                    <span className='req-detail-id-v'>{row.localDirect.workspaceUri}</span>
                  </div>
                ) : null}
                {row.localDirect.transcriptPath || row.localDirect.dbPath ? (
                  <div className='req-detail-id-kv'>
                    <span className='req-detail-id-k'>{t('本地记录文件')}</span>
                    <span className='req-detail-id-v'>{row.localDirect.transcriptPath || row.localDirect.dbPath}</span>
                  </div>
                ) : null}
              </div>
            </div>
          </div>
        ) : null
      ) : (!row.isTest || row.clientEvidence) ? (
        <div className='req-detail-group'>
          <div className='req-detail-group-head'>
            <span className='req-detail-group-title'>{t('来源识别')}</span>
          </div>
          <ClientIdentificationDetail entry={row} api={clientIdentificationApi} t={t} />
        </div>
      ) : null}
    </div>
  )
}
