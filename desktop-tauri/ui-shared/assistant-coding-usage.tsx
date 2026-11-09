import { useCallback, useEffect, useState } from 'react'
import { Alert, Button, Popover, PopoverContent, PopoverTitle, PopoverTrigger, Progress } from '@ui'
import { CodingIcon } from './assistant-coding-icons'
import { CODING_BASE, type CodingAccountUsage, type CodingRateLimit, type CodingRateLimitWindow, type CodingSession, type CodingTokenBreakdown, type CodingTranslate, type CodingTransport } from './assistant-coding-types'

function number(value: number | undefined): string {
  return typeof value === 'number' && Number.isFinite(value) ? value.toLocaleString() : '—'
}
function percentage(value: number): number { return Math.max(0, Math.min(100, value)) }
function duration(minutes: number | null, t: CodingTranslate): string {
  if (!minutes) return t('额度窗口')
  if (minutes % 1440 === 0) return t('{count} 天额度', { count: minutes / 1440 })
  if (minutes % 60 === 0) return t('{count} 小时额度', { count: minutes / 60 })
  return t('{count} 分钟额度', { count: minutes })
}
function Tokens({ value, t }: { value: CodingTokenBreakdown; t: CodingTranslate }) {
  return <dl className='grid grid-cols-2 gap-x-4 gap-y-1 text-xs sm:grid-cols-3'>
    {[[t('输入'), value.inputTokens], [t('输出'), value.outputTokens], [t('合计'), value.totalTokens],
      [t('其中缓存输入'), value.cachedInputTokens], [t('其中思考输出'), value.reasoningOutputTokens]].map(([label, count]) =>
      <div key={String(label)} className='flex flex-wrap justify-between gap-1'><dt className='text-muted-foreground'>{label}</dt><dd className='tabular-nums'>{number(Number(count))}</dd></div>)}
  </dl>
}
function Window({ window, t }: { window: CodingRateLimitWindow; t: CodingTranslate }) {
  const remaining = percentage(100 - window.usedPercent)
  return <div className='space-y-1'>
    <div className='flex justify-between gap-2 text-xs'><span>{duration(window.windowDurationMins, t)}</span><span>{t('剩余 {percent}%', { percent: Math.round(remaining) })}</span></div>
    <Progress value={remaining} aria-label={duration(window.windowDurationMins, t)} />
    {window.resetsAt ? <p className='text-xs text-muted-foreground'>{t('重置时间：{time}', { time: new Date(window.resetsAt * 1000).toLocaleString() })}</p> : null}
  </div>
}
function Limit({ limit, t }: { limit: CodingRateLimit; t: CodingTranslate }) {
  return <div className='space-y-2 rounded-md border border-border p-3'>
    <p className='text-xs font-medium'>{limit.limitName || limit.limitId || 'Codex'}{limit.planType ? ` · ${limit.planType}` : ''}</p>
    {limit.primary ? <Window window={limit.primary} t={t} /> : null}
    {limit.secondary ? <Window window={limit.secondary} t={t} /> : null}
    {limit.credits ? <p className='text-xs'>{limit.credits.unlimited ? t('额度不限') : t('额外额度：{balance}', { balance: limit.credits.balance ?? '—' })}</p> : null}
    {!limit.primary && !limit.secondary && !limit.credits ? <p className='text-xs text-muted-foreground'>{t('Codex 暂未提供额度数据。')}</p> : null}
  </div>
}

export function CodingUsage({ session, transport, t }: { session: CodingSession; transport: CodingTransport; t: CodingTranslate }) {
  const [open, setOpen] = useState(false)
  const [account, setAccount] = useState<CodingAccountUsage | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState('')
  const [refreshKey, setRefreshKey] = useState(0)
  useEffect(() => {
    if (!open) return
    const controller = new AbortController()
    let running = false
    const load = async () => {
      if (running || controller.signal.aborted) return
      running = true; setLoading(true)
      try {
        const data = await transport.json<CodingAccountUsage>('GET', `${CODING_BASE}/usage`, undefined, controller.signal)
        if (!controller.signal.aborted) { setAccount(data); setError('') }
      } catch (failure) { if (!controller.signal.aborted) setError(failure instanceof Error ? failure.message : String(failure)) }
      finally { running = false; if (!controller.signal.aborted) setLoading(false) }
    }
    void load()
    const timer = setInterval(() => { if (document.visibilityState === 'visible') void load() }, 60_000)
    return () => { controller.abort(); clearInterval(timer) }
  }, [open, transport, refreshKey])
  const usage = session.tokenUsage
  const window = usage?.modelContextWindow
  const used = usage?.last.totalTokens
  const context = typeof window === 'number' && window > 0 && typeof used === 'number' ? used / window * 100 : null
  const buckets = account?.data?.rateLimitsByLimitId
  const limits = buckets && Object.keys(buckets).length ? Object.values(buckets) : account?.data?.rateLimits ? [account.data.rateLimits] : []
  const refresh = useCallback(() => setRefreshKey(key => key + 1), [])
  return <Popover open={open} onOpenChange={setOpen}>
    <PopoverTrigger render={<Button variant='ghost' size='sm' className='h-8 rounded-full px-2 font-normal max-sm:h-9' aria-label={t('Codex 用量')}
      title={usage ? t('会话累计：{count} Token', { count: number(usage.total.totalTokens) }) : t('用量尚未上报')} />}>
      <CodingIcon name='usage' className='size-3.5 text-muted-foreground' />
      <span className='tabular-nums'>{context !== null ? t('上下文：{percent}%', { percent: Math.round(context) }) : t('Codex 用量')}</span>
    </PopoverTrigger>
    <PopoverContent side='top' align='start' className='w-96 max-w-[calc(100vw-32px)]'>
      <div className='border-b border-border px-4 py-3'><PopoverTitle>{t('Codex 用量')}</PopoverTitle></div>
      <div className='min-h-0 space-y-4 overflow-y-auto overscroll-contain p-4'>
      <div className='space-y-2'><p className='text-xs font-medium'>{t('上下文占用')}</p>
        {context !== null ? <><Progress value={percentage(context)} aria-label={t('上下文占用')} />
          <p className='text-xs tabular-nums'>{number(used)} / {number(window!)} Token</p></> : <p className='text-xs text-muted-foreground'>{t('当前模型未上报上下文大小，无法计算占用比例。')}</p>}
        <p className='text-xs text-muted-foreground'>{t('按 Codex 最近一次上报估算，不是累计用量；未上报的输入和自动压缩会影响实际占用。')}</p>
      </div>
      {usage ? <><div className='space-y-2'><p className='text-xs font-medium'>{t('会话累计用量')}</p><Tokens value={usage.total} t={t} /></div>
        <div className='space-y-2'><p className='text-xs font-medium'>{t('最近一次模型请求')}</p><Tokens value={usage.last} t={t} /></div>
        <p className='text-xs text-muted-foreground'>{t('缓存包含在输入中，思考包含在输出中，不会重复相加。')}</p></> : null}
      {session.usageUpdatedAt ? <p className='text-xs text-muted-foreground'>{t('读数更新时间：{time}', { time: new Date(session.usageUpdatedAt).toLocaleString() })}{session.usageSource === 'history' ? ` · ${t('来自历史记录')}` : ''}</p> : null}
      <div className='space-y-2'><div className='flex items-center justify-between'><p className='text-xs font-medium'>{t('账号额度')}</p>
        <Button size='sm' variant='ghost' disabled={loading} onClick={refresh}>{loading ? t('读取中…') : t('刷新')}</Button></div>
        {limits.map((limit, index) => <Limit key={limit.limitId || index} limit={limit} t={t} />)}
        {account && !account.available ? <p className='text-xs text-muted-foreground'>{account.reason === 'account_type'
          ? t('当前使用 API 或自定义提供商，Codex 不提供订阅剩余额度；会话 Token 用量仍可查看。') : t('Codex 暂未提供额度数据。')}</p> : null}
        {account?.stale ? <p className='text-xs text-muted-foreground'>{t('额度刷新失败，当前显示上次读数。')}</p> : null}
        {error || account?.error ? <Alert variant='warning'>{error || account?.error}</Alert> : null}
      </div>
      </div>
    </PopoverContent>
  </Popover>
}
