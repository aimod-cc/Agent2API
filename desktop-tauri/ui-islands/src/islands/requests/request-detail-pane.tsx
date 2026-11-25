import * as React from 'react'
import {
  Badge, Button, HtmlContent, StatusDot,
  Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
} from '@ui'
import { t } from '../../i18n'
import { requestApiKeyLabel, requestClientLabel } from '../../../../ui-shared/model-api-key'
import { ClientIdentificationDetail } from '../../../../ui-shared/client-identification-detail'
import { VisionRequestDetail, visionDetail } from '../../../../ui-shared/request-vision-detail'
import type { ClientEvidence } from '../../../../ui-shared/client-identification'
import { errorPolicyDescription } from '../../../../ui-shared/routing-errors-record'
import { clientIdentificationApi } from '../settings-client-identification'
import { formatCost, pricingSourceLabel, usePricingDisplay, type ModelPricingInfo } from '../model-pricing'
import { RequestEmpty as Empty } from '../request-raw-panes'
import { RouteReplayFacts } from '../routing-replay'
import type { ReplayRow, RouteDecision } from '../routing-types'
import { numberText } from '../accounts-shared'
import { ClientBrand, ProviderBrand } from './request-brand'
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

function fmtElapsed(ts: RequestRow['ts']): string {
  const elapsed = Math.max(0, Date.now() - (Number(ts) || 0))
  return t('{n}秒', { n: Math.max(1, Math.floor(elapsed / 1000)) })
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

function DetailGroup({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className='req-detail-group'>
      <div className='req-detail-group-title'>{title}</div>
      <div className='req-detail-dl'>{children}</div>
    </div>
  )
}

function DetailRow({
  label,
  children,
  mono = false,
  strong = false,
  total = false,
  muted = false,
  title,
}: {
  label: string
  children: React.ReactNode
  mono?: boolean
  strong?: boolean
  total?: boolean
  muted?: boolean
  title?: string
}) {
  return (
    <div className={`req-detail-row${total ? ' is-total' : ''}`}>
      <span className='req-detail-k'>{label}</span>
      <span
        className={`req-detail-v${mono ? ' is-mono' : ''}${strong ? ' is-strong' : ''}${muted ? ' is-muted' : ''}`}
        title={title}
      >
        {children}
      </span>
    </div>
  )
}

const ATTEMPT_CELL = 'border border-hairline px-2 py-1 align-top'
const ATTEMPT_HEAD = 'border border-hairline bg-surface-2 px-2 py-1 text-[11px] tracking-normal whitespace-nowrap'
const NO_HOVER = 'hover:[&>td]:bg-transparent'

function attemptResult(item: AttemptDetail): React.ReactNode {
  const status = Number(item?.status)
  const hasStatus = item?.status !== null && item?.status !== undefined && Number.isFinite(status)
  const error = item?.error ? String(item.error) : ''
  if (error) {
    const text = hasStatus
      ? t('失败（{status}）：{error}', { status: String(status), error })
      : t('失败：{error}', { error })
    return <span className='break-words text-destructive'>{text}</span>
  }
  if (hasStatus) return <span className='text-success'>{t('成功（{status}）', { status: String(status) })}</span>
  return <span className='text-muted-foreground'>{t('无结果记录')}</span>
}

function AttemptsTable({ row, loading }: { row: RequestRow; loading?: boolean }) {
  const details = Array.isArray(row.attemptDetails) ? row.attemptDetails : []
  if (loading && !details.length) return <span className='text-xs text-muted-foreground'>{t('正在加载明细…')}</span>
  if (!details.length) return null
  const attempts = Number(row.attempts) || 1
  return (
    <div className='overflow-x-auto rounded-md border border-border bg-surface'>
      <Table className='text-[11.5px]'>
        <TableHeader>
          <TableRow className={NO_HOVER}>
            {[t('轮次'), t('提供商'), t('账号'), t('结果'), t('体积'), t('内部重试'), t('提示')].map(label => (
              <TableHead key={label} className={ATTEMPT_HEAD}>{label}</TableHead>
            ))}
          </TableRow>
        </TableHeader>
        <TableBody>
          {details.map((item, index) => {
            const retries = Array.isArray(item?.retries) ? item.retries : []
            const retryTitle = retries.map(retry => {
              const delayMs = Number(retry?.delayMs)
              const delay = Number.isFinite(delayMs) && delayMs > 0
                ? (delayMs >= 1000
                    ? t('，{n}秒后重试', { n: Math.round(delayMs / 1000) })
                    : t('，{n}毫秒后重试', { n: Math.round(delayMs) }))
                : ''
              return `${String(retry?.reason || t('未知原因'))}${delay}`
            }).join('\n')
            const notice = [String(item?.notice ?? '').trim(), errorPolicyDescription(item?.errorPolicy, t)].filter(Boolean).join(' · ')
            const bodyRaw = Number(item?.bodyBytes)
            const bodyText = Number.isFinite(bodyRaw) && bodyRaw > 0
              ? bodyRaw >= 1024 * 1024
                ? `${(bodyRaw / 1024 / 1024).toFixed(1)} MB`
                : bodyRaw >= 1024
                  ? `${Math.round(bodyRaw / 1024)} KB`
                  : `${bodyRaw} B`
              : '—'
            const bodyTitle = Number.isFinite(bodyRaw) && bodyRaw > 0
              ? t('实际发给上游 {bytes} 字节', { bytes: bodyRaw.toLocaleString() })
              : undefined
            return (
              <TableRow key={index} className={NO_HOVER}>
                <TableCell className={ATTEMPT_CELL}>{index + 1}</TableCell>
                <TableCell className={ATTEMPT_CELL}>
                  {providerName(item) || (item?.provider ? String(item.provider) : t('未知'))}
                </TableCell>
                <TableCell className={ATTEMPT_CELL}>{item?.account ? String(item.account) : '—'}</TableCell>
                <TableCell className={ATTEMPT_CELL}>{attemptResult(item)}</TableCell>
                <TableCell className={`${ATTEMPT_CELL} whitespace-nowrap`} title={bodyTitle}>{bodyText}</TableCell>
                <TableCell className={`${ATTEMPT_CELL} whitespace-nowrap`} title={retryTitle || undefined}>
                  {retries.length ? t('↻ {n} 次', { n: retries.length }) : '-'}
                </TableCell>
                <TableCell className={ATTEMPT_CELL}>{notice || '—'}</TableCell>
              </TableRow>
            )
          })}
          {details.length < attempts ? (
            <TableRow className={NO_HOVER}>
              <TableCell colSpan={7} className={`${ATTEMPT_CELL} text-muted-foreground`}>
                {t('另有 {missing} 次尝试未记录明细（只保留最早的 {kept} 条）', {
                  missing: attempts - details.length, kept: details.length,
                })}
              </TableCell>
            </TableRow>
          ) : null}
        </TableBody>
      </Table>
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

  const clientModel = String(row.clientModel ?? '').trim()
  const upstreamModel = String(row.upstreamModel ?? '').trim()
  const shownModel = String(row.model ?? '').trim()
  const clientLevel = String(row.clientReasoning ?? '').trim()
  const upstreamLevel = String(row.upstreamReasoning ?? '').trim()
  const withLevel = (name: string, level: string) => (level ? `${name}(${level})` : name)
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
  const cacheToneCls = cachePct === null || cachePct >= 70
    ? 'is-tone-good'
    : cachePct >= 30
      ? 'is-tone-info'
      : 'is-tone-warn'

  const hasCost = row.costUsd !== null && row.costUsd !== undefined && Number.isFinite(Number(row.costUsd))
  const hasCredit = row.credit !== null && row.credit !== undefined && Number.isFinite(Number(row.credit))
  const speed = fmtTokensPerSecond(row)
  const attempts = Number(row.attempts) || 1
  const details = Array.isArray(row.attemptDetails) ? row.attemptDetails : []
  const hits = Array.isArray(row.sensitiveHits) ? row.sensitiveHits : []
  const privacyHits = Array.isArray(row.privacyHits) ? row.privacyHits : []
  const hasVision = Boolean(visionDetail(row.route))

  return (
    <div className='req-detail-sections'>
      {row.error ? (
        <div className='req-detail-err-banner'>
          <span className='req-detail-err-text'>{String(row.error)}</span>
          <Button variant='outline' size='xs' className='shrink-0' data-copy={String(row.error)}>{t('复制')}</Button>
        </div>
      ) : null}

      <DetailGroup title={t('基本信息')}>
        <DetailRow label={t('时间')} mono>{fmtTime(row.ts)}</DetailRow>
        <DetailRow label={t('状态')}><StatusSummary row={row} /></DetailRow>
        <DetailRow label={t('来源软件')}>
          <span className='inline-flex justify-end'><ClientBrand entry={row} label={clientLabel} /></span>
        </DetailRow>
        <DetailRow label='API Key'>{requestApiKeyLabel(row, t)}</DetailRow>
        <DetailRow label={t('提供商')}>
          {provider ? (
            <span className='inline-flex justify-end'><ProviderBrand providerId={providerId} label={provider} /></span>
          ) : '—'}
        </DetailRow>
        {account ? <DetailRow label={t('命中账号')}>{account}</DetailRow> : null}
        <DetailRow label={isMapped ? t('实际转发模型') : t('模型')} mono>{actualModelText || '—'}</DetailRow>
        {requestedModelText ? <DetailRow label={t('原请求模型')} mono muted>{requestedModelText}</DetailRow> : null}
        {attempts > 1 ? <DetailRow label={t('尝试次数')} mono>{String(attempts)}</DetailRow> : null}
        {row.origin === 'local_direct' ? (
          <DetailRow label={t('采集方式')}>
            <Badge variant='info' shape='tag'>{t('本机直连（未经过网关）')}</Badge>
          </DetailRow>
        ) : null}
        {row.isTest ? (
          <DetailRow label={t('请求性质')}>
            <Badge variant='brand' shape='tag'
              title={t('面板内发起的模型调用，消耗额度并记录 Key，不计入生产报表')}>
              {clientLabel}
            </Badge>
          </DetailRow>
        ) : null}
      </DetailGroup>

      <DetailGroup title={t('Token 用量')}>
        <DetailRow label={t('输入')} mono>{fmtTokens(prompt)}</DetailRow>
        <DetailRow label={t('输出')} mono>{fmtTokens(completion)}</DetailRow>
        <DetailRow label={t('缓存读取')} mono muted={cacheRead === 0}>
          {cacheRead > 0 ? (
            <span className={`req-usage-line ${cacheToneCls}`}>
              {fmtTokens(cacheRead)}{cacheRate ? ` (${cacheRate})` : ''}
            </span>
          ) : fmtTokens(cacheRead)}
        </DetailRow>
        {cacheWrite > 0 ? <DetailRow label={t('缓存写入')} mono>{fmtTokens(cacheWrite)}</DetailRow> : null}
        <DetailRow label={t('合计')} mono strong total>{fmtTokens(total)}</DetailRow>
      </DetailGroup>

      <DetailGroup title={t('费用与性能')}>
        <DetailRow label={t('预估费用')} mono>
          {hasCost ? (
            <span>
              {formatCost(Number(row.costUsd), pricing.currency, pricing.exchangeRate)}
              {pricing.currency === 'cny' ? (
                <span className='ml-1.5 text-[11px] text-muted-foreground'>(${Number(row.costUsd).toFixed(5)})</span>
              ) : null}
              {row.priceSource ? (
                <span className='ml-1.5 text-[11px] text-muted-foreground'>· {pricingSourceLabel(row.priceSource)}</span>
              ) : null}
            </span>
          ) : '—'}
        </DetailRow>
        {hasCredit ? <DetailRow label={t('积分消耗')} mono>{numberText(row.credit)}</DetailRow> : null}
        <DetailRow label={t('首字响应')} mono>{running ? '—' : fmtFirstResponse(row.firstResponseMs)}</DetailRow>
        <DetailRow label={t('生成速度')} mono title={SPEED_TIP}>{running ? '—' : speed || '—'}</DetailRow>
        <DetailRow label={t('总耗时')} mono strong total>
          {running ? (
            <>
              {fmtElapsed(row.ts)}
              <span className='ml-1 text-[11.5px] font-normal text-muted-foreground'>{t('（进行中）')}</span>
            </>
          ) : fmtDuration(row.durationMs)}
        </DetailRow>
      </DetailGroup>

      {hasVision ? (
        <div className='req-detail-group'>
          <div className='req-detail-group-title'>{t('图片兼容')}</div>
          <div className='req-detail-dl py-2.5'>
            <VisionRequestDetail route={row.route} t={t} />
          </div>
        </div>
      ) : null}

      {loadingDetail || details.length > 0 ? (
        <div className='req-detail-group'>
          <div className='req-detail-group-title'>{t('尝试明细')}</div>
          <AttemptsTable row={row} loading={loadingDetail} />
        </div>
      ) : null}

      {hits.length > 0 ? (
        <div className='req-detail-group'>
          <div className='req-detail-group-title'>{t('关键词替换')}</div>
          <div className='req-detail-dl py-2.5'>
            <span className='inline-flex flex-wrap gap-1.5'>
              {hits.map((hit, index) => (
                <Badge key={index} variant='sensitive' shape='tag'>
                  {String(hit?.word ?? '')} × {String(Number(hit?.count) || 0)}
                </Badge>
              ))}
            </span>
          </div>
        </div>
      ) : null}

      {privacyHits.length > 0 ? (
        <div className='req-detail-group'>
          <div className='req-detail-group-title'>{t('隐私脱敏')}</div>
          <div className='req-detail-dl py-2.5'>
            <span className='inline-flex flex-wrap gap-1.5'>
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
            </span>
          </div>
        </div>
      ) : null}

      {row.origin === 'local_direct' ? (
        row.localDirect ? (
          <div className='req-detail-group'>
            <div className='req-detail-group-title'>{t('本机直连信息')}</div>
            <div className='req-detail-dl'>
              {row.localDirect.conversationTitle ? (
                <DetailRow label={t('对话标题')}>{row.localDirect.conversationTitle}</DetailRow>
              ) : null}
              {row.localDirect.workspaceUri ? (
                <DetailRow label={t('工作区')} mono>{row.localDirect.workspaceUri}</DetailRow>
              ) : null}
              <DetailRow label={t('来源目录')}>{localRootKindLabel(row.localDirect.rootKind)}</DetailRow>
              {row.localDirect.transcriptPath || row.localDirect.dbPath ? (
                <DetailRow label={t('本地记录文件')} mono>
                  {row.localDirect.transcriptPath || row.localDirect.dbPath}
                </DetailRow>
              ) : null}
            </div>
          </div>
        ) : null
      ) : (!row.isTest || row.clientEvidence) ? (
        <div className='req-detail-group'>
          <div className='req-detail-group-title'>{t('来源识别')}</div>
          <div className='req-detail-dl py-2.5'>
            <ClientIdentificationDetail entry={row} api={clientIdentificationApi} t={t} />
          </div>
        </div>
      ) : null}
    </div>
  )
}
