import { Alert, Badge } from '@ui'
import type { Translate } from './assistant-read-types'

type VisionDetail = {
  mode: 'assist' | 'reroute' | 'assist_call'
  status?: string
  sourceProvider?: string
  sourceModel?: string
  targetProvider?: string
  targetModel?: string
  durationMs?: number
  imageCount?: number
  cachedImages?: number
  totalTokens?: number
  error?: string
  parentId?: string
  calls?: Array<{ id?: string; provider?: string; model?: string; account?: string; durationMs?: number; totalTokens?: number; status?: number; error?: string }>
}

export function visionDetail(route: unknown): VisionDetail | null {
  if (!route || typeof route !== 'object') return null
  const value = (route as { visionCompatibility?: VisionDetail }).visionCompatibility
  return value && ['assist', 'reroute', 'assist_call'].includes(value.mode) ? value : null
}

export function VisionRequestDetail({ route, t }: { route: unknown; t: Translate }) {
  const value = visionDetail(route)
  if (!value) return null
  const mode = value.mode === 'assist' ? t('辅助识图 · 原模型回答') : value.mode === 'reroute' ? t('切换视觉模型 · 视觉模型回答') : t('辅助识图调用')
  return <div className='space-y-2 text-xs'>
    <div className='flex flex-wrap items-center gap-2'><Badge variant='outline'>{mode}</Badge><span className='text-muted-foreground'>{value.status === 'failed' ? t('失败') : value.status === 'processing' ? t('处理中') : t('已完成')}</span></div>
    {value.sourceModel ? <p className='break-all'>{t('原模型')}：{value.sourceProvider} · {value.sourceModel}</p> : null}
    {value.targetModel ? <p className='break-all'>{t('视觉模型')}：{value.targetProvider} · {value.targetModel}</p> : null}
    {value.parentId ? <p className='break-all text-muted-foreground'>{t('所属请求')}：{value.parentId}</p> : null}
    <p className='text-muted-foreground'>{t('图片处理用时：{n} 秒', { n: ((Number(value.durationMs) || 0) / 1000).toFixed(2) })}</p>
    {value.mode === 'assist' ? <>
      <p>{t('共 {images} 张图片，复用 {cached} 张识读结果', { images: value.imageCount || 0, cached: value.cachedImages || 0 })}</p>
      <p>{t('额外识图用量：{n} Token', { n: (Number(value.totalTokens) || 0).toLocaleString() })}</p>
      <p className='text-muted-foreground'>{t('识图用量按实际视觉账号单独记录，不混入原模型的回答用量。')}</p>
      {value.calls?.map((call, index) => <div key={call.id || index} className='rounded border border-hairline p-2'>
        <p className='break-all'>{call.provider} · {call.model} · {call.account}</p>
        <p className='text-muted-foreground'>{((Number(call.durationMs) || 0) / 1000).toFixed(2)} s · {(Number(call.totalTokens) || 0).toLocaleString()} Token</p>
        {call.error ? <p className='break-words text-destructive'>{call.error}</p> : null}
      </div>)}
    </> : null}
    {value.error ? <Alert variant='destructive'>{value.error}</Alert> : null}
  </div>
}
