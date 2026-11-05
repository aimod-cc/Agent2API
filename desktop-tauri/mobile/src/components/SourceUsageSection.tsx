import { useEffect, useState } from 'react'
import { Badge, Button, Card, Empty, SegmentedControl, Sheet, SheetBody, SheetContent, SheetHeader, SheetTitle } from '@ui'
import { fetchStatsSummary, type StatsDimension, type StatsSummary } from '../api/gateway'
import { store } from '../state/store'
import { formatInt, formatPercent, formatTokens } from '../lib/format'
import { t } from '../i18n'

export function reportSourceName(row: StatsDimension, key: boolean): string {
  if (!key) return !row.id ? t('未记录客户端') : row.id === 'other' ? t('其它') : String(row.label || row.id)
  if (row.source === 'none') return t('免鉴权调用')
  if (row.source === 'env') return t('环境变量 Key')
  if (row.source !== 'key') return t('历史未记录 Key')
  return row.label || row.id || t('未命名')
}

function value(amount: number | null | undefined): string {
  return typeof amount === 'number' ? formatTokens(amount) : '—'
}

export function openMobileRequestLogs(summary: StatsSummary, query: Record<string, string>): void {
  store.push('requests', { ...query, startDate: summary.startDate || '', endDate: summary.endDate || '', productionOnly: '1', completedOnly: '1' })
}

export function SourceUsageSection({ summary, apiKeys = false }: { summary: StatsSummary; apiKeys?: boolean }) {
  const [sort, setSort] = useState('tokens')
  const [expanded, setExpanded] = useState(false)
  const [target, setTarget] = useState<StatsDimension | null>(null)
  const [detail, setDetail] = useState<StatsSummary | null>(null)
  const [error, setError] = useState('')
  const list = [...(apiKeys ? summary.apiKeys || [] : summary.clients || [])].sort((left, right) => {
    const key = sort === 'tokens' ? 'totalTokens' : 'requests'
    return (Number(right[key]) || 0) - (Number(left[key]) || 0)
  })
  useEffect(() => {
    if (!target) return
    const controller = new AbortController()
    setDetail(null)
    setError('')
    void fetchStatsSummary('custom', controller.signal, {
      apiKey: target.filterValue || 'unknown', startDate: summary.startDate, endDate: summary.endDate,
    }).then(setDetail).catch(failure => { if (!controller.signal.aborted) setError(String(failure)) })
    return () => controller.abort()
  }, [target, summary.startDate, summary.endDate])
  const query = (row: StatsDimension): Record<string, string> => apiKeys ? { apiKey: row.filterValue || 'unknown' } : { client: row.id || '', clientLabel: row.label || '' }
  return (
    <div className='m-section'>
      <div className='flex flex-wrap justify-between items-center gap-2 mb-2'>
        <b className='text-sm'>{apiKeys ? t('API Key 统计') : t('客户端统计')}</b>
        <SegmentedControl value={sort} onValueChange={setSort} aria-label={t('统计排序')}
          options={[{ value: 'tokens', label: t('按 Token') }, { value: 'requests', label: t('按请求数') }]} />
      </div>
      <Card className='m-card p-3'>
        {!list.length ? <Empty>{t('所选范围内暂无请求')}</Empty> : (expanded ? list : list.slice(0, 5)).map(row => (
          <div className='border-b border-border py-2 last:border-0' key={row.filterValue || row.id || 'unknown'}>
            <div className='flex items-center gap-2'>
              <Button variant='link' size='sm' className='px-0 min-w-0 truncate' onClick={() => openMobileRequestLogs(summary, query(row))}>
                {reportSourceName(row, apiKeys)}
              </Button>
              {apiKeys && row.source === 'key' ? <Badge variant={row.state === 'enabled' ? 'success' : 'outline'}>
                {row.state === 'enabled' ? t('启用') : row.state === 'disabled' ? t('停用') : t('已删除')}
              </Badge> : null}
              <span className='ml-auto text-xs whitespace-nowrap'>{formatTokens(row.totalTokens || 0)}</span>
            </div>
            <div className='text-xs text-muted-foreground leading-6'>
              {t('{n} 次请求', { n: formatInt(row.requests || 0) })} · {t('成功 / 失败')} {formatInt(row.success || 0)} / {formatInt(row.failures || 0)}
              <br />{t('成功率')} {typeof row.successRate === 'number' ? formatPercent(row.successRate) : '—'} · {t('占比')} {summary.overview?.tokens ? formatPercent((row.totalTokens || 0) / summary.overview.tokens) : '—'}
              <br />{t('输入 / 输出')} {value(row.promptTokens)} / {value(row.completionTokens)}
              <br />{t('缓存读取')} {value(row.cacheReadTokens)} · {t('缓存命中率')} {typeof row.cacheRate === 'number' ? formatPercent(row.cacheRate) : '—'}
              {apiKeys ? <><br />{t('最近调用')} {row.lastUsedAt ? new Date(row.lastUsedAt).toLocaleString() : '—'}</> : null}
            </div>
            {apiKeys ? <Button variant='ghost' size='sm' onClick={() => setTarget(row)}>{t('用量详情')}</Button> : null}
          </div>
        ))}
        {list.length > 5 ? <Button variant='ghost' size='sm' className='w-full' onClick={() => setExpanded(!expanded)}>
          {expanded ? t('收起') : t('查看全部')}
        </Button> : null}
      </Card>
      <Sheet open={!!target} onOpenChange={open => { if (!open) setTarget(null) }}>
        <SheetContent><SheetHeader><SheetTitle>{t('API Key 用量详情')} · {target && reportSourceName(target, true)}</SheetTitle></SheetHeader>
          <SheetBody>
            {error ? <Empty>{error}</Empty> : !detail ? <Empty>{t('正在加载…')}</Empty> : (
              <>{(['clients', 'models'] as const).map(section => (
                <div key={section} className='mb-4'>
                  <b>{section === 'clients' ? t('客户端分布') : t('模型分布')}</b>
                  {detail.apiKeyDetail?.[section]?.length ? detail.apiKeyDetail[section]?.map((row, index) => (
                    <Button key={row.id || row.model || index} variant='ghost' className='w-full justify-between my-1' onClick={() => {
                      openMobileRequestLogs(summary, { apiKey: target?.filterValue || 'unknown',
                        ...(section === 'clients' ? { client: row.id || '', clientLabel: row.label || '' } : { model: row.model || '' }) })
                      setTarget(null)
                    }}><span className='truncate'>{section === 'clients' ? reportSourceName(row, false) : row.label || row.model}</span>
                      <span>{formatTokens(row.totalTokens || 0)} · {formatInt(row.requests || 0)}</span></Button>
                  )) : <Empty>{t('没有可恢复的维度明细')}</Empty>}
                </div>
              ))}</>
            )}
          </SheetBody>
        </SheetContent>
      </Sheet>
    </div>
  )
}
