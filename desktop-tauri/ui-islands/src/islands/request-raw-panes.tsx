import * as React from 'react'
import { Button, Empty, Spinner, Switch, Tooltip, TooltipContent, TooltipTrigger } from '@ui'
import { t } from '../i18n'
import { NOTES } from './settings-model'
import { loadDebug, saveDebug, useSettings } from './settings-state'
import {
  MemoryCopyButton,
  PagedTextViewer,
  formatByteSize,
  utf8ByteLength,
} from './requests/request-detail-guard'

export { PreviewPane } from './requests/request-preview-pane'

const SINGLE_COLLAPSE_BYTES = 512 * 1024
const TOTAL_COLLAPSE_BYTES = 1024 * 1024
const LARGE_TOTAL_SEG_BYTES = 64 * 1024

function DebugInfoIcon() {
  return (
    <svg width='13' height='13' viewBox='0 0 16 16' fill='none' stroke='currentColor' strokeWidth='1.5' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
      <circle cx='8' cy='8' r='6' />
      <path d='M8 7.2v3.6M8 4.8h.01' />
    </svg>
  )
}

export function RequestDebugSwitch() {
  const { debug, busy } = useSettings()
  const [loading, setLoading] = React.useState(true)
  const alive = React.useRef(false)
  const loadingRef = React.useRef(false)

  const refresh = React.useCallback(async () => {
    if (loadingRef.current) return
    loadingRef.current = true
    setLoading(true)
    try { await loadDebug() }
    finally {
      loadingRef.current = false
      if (alive.current) setLoading(false)
    }
  }, [])

  React.useEffect(() => {
    alive.current = true
    void refresh()
    return () => { alive.current = false }
  }, [refresh])

  const saving = busy === 'debug'
  const unavailable = !loading && debug.status === 'unavailable'
  return (
    <div className='ml-auto flex min-h-[30px] flex-wrap items-center justify-end gap-2'>
      <Tooltip>
        <TooltipTrigger
          render={
            <button
              type='button'
              className='inline-flex size-5 items-center justify-center rounded-full border-0 bg-transparent p-0 text-muted-foreground hover:bg-control-hover hover:text-foreground'
              aria-label={t('调试模式说明')}
            >
              <DebugInfoIcon />
            </button>
          }
        />
        <TooltipContent variant='plain' side='bottom'>{NOTES.debug}</TooltipContent>
      </Tooltip>
      <label className='inline-flex items-center gap-2 whitespace-nowrap text-[12px] text-subtle'>
        <span>{t('调试模式')}</span>
        <Switch checked={debug.on} disabled={loading || debug.status !== 'ready' || busy !== null}
          aria-label={t('调试模式')} aria-busy={loading || saving}
          onCheckedChange={next => void saveDebug(next)} />
      </label>
      {loading || saving ? <Spinner className='size-3.5 text-muted-foreground'
        aria-label={loading ? t('正在读取…') : t('保存中…')} /> : null}
      {unavailable ? <>
        <span className='text-[11.5px] text-destructive' role='status'
          title={t('未能读取调试模式设置，请稍后重试')}>{t('读取失败')}</span>
        <Button variant='ghost' size='xs' disabled={busy !== null} onClick={() => void refresh()}>{t('重试')}</Button>
      </> : null}
    </div>
  )
}

export type RawBody = Record<string, unknown>
export type DebugTraffic = Record<string, unknown>
export type RawState =
  | 'idle'
  | 'loading'
  | 'gone'
  | { error: string; retryable?: boolean }
  | { body: RawBody; truncated: boolean }
export type DebugState =
  | 'idle'
  | 'loading'
  | 'gone'
  | { error: string; retryable?: boolean }
  | { data: DebugTraffic }

function jsonText(value: unknown): string {
  if (value === null || value === undefined) return ''
  if (typeof value === 'string') return value
  try { return JSON.stringify(value, null, 2) } catch { return '' }
}

const DEBUG_SEGS = [
  { key: 'requestHeaders', label: t('请求头'), pick: (data: DebugTraffic) => jsonText(data.requestHeaders) },
  { key: 'requestBody', label: t('请求体'), pick: (data: DebugTraffic) => jsonText(data.requestBody) },
  { key: 'responseHeaders', label: t('响应头'), pick: (data: DebugTraffic) => jsonText(data.responseHeaders) },
  { key: 'responseBody', label: t('响应体'), pick: (data: DebugTraffic) => data.responseBody ? String(data.responseBody) : '' },
] as const

export function hasDebugTraffic(data: DebugTraffic): boolean {
  return [data.requestHeaders, data.requestBody, data.responseHeaders, data.responseBody]
    .some(value => typeof value === 'string' ? value.trim() !== '' : value != null)
}

export function extractFallbackPayloadText(raw: RawState, debug: DebugState): string {
  const sections: string[] = []
  if (typeof debug === 'object' && debug !== null && 'data' in debug) {
    for (const seg of DEBUG_SEGS) {
      const text = seg.pick(debug.data)
      if (text) sections.push(`=== ${seg.label} ===\n${text}`)
    }
  }
  if (sections.length === 0 && typeof raw === 'object' && raw !== null && 'body' in raw) {
    const req = String(raw.body.requestBody ?? '')
    const res = String(raw.body.responseBody ?? '')
    if (req) sections.push(`=== ${t('请求体')} ===\n${req}`)
    if (res) sections.push(`=== ${t('响应体')} ===\n${res}`)
  }
  return sections.join('\n\n')
}

export function RequestEmpty({ text, loading = false }: { text: string; loading?: boolean }) {
  return (
    <Empty className='flex-row py-6'>
      {loading ? <Spinner className='size-3.5' /> : null}
      {text}
    </Empty>
  )
}

export function RawPane({ state, onRetry }: { state: DebugState; onRetry?: () => void }) {
  const data = typeof state === 'object' && state !== null && 'data' in state ? state.data : null

  const computed = React.useMemo(() => {
    if (!data) return { segments: [] as Array<{ key: string; label: string; text: string; bytes: number }>, totalBytes: 0 }
    let totalBytes = 0
    const segments = DEBUG_SEGS.map(seg => {
      const text = seg.pick(data)
      const bytes = text ? utf8ByteLength(text) : 0
      totalBytes += bytes
      return { key: seg.key, label: seg.label, text, bytes }
    })
    return { segments, totalBytes }
  }, [data])

  if (state === 'idle' || state === 'loading') {
    return <RequestEmpty text={t('正在读取…')} loading />
  }
  if (state === 'gone') {
    return <RequestEmpty text={t('该请求没有保存原始报文（未开启「调试模式」，或记录已超出保留条数）')} />
  }
  if ('error' in state) {
    return (
      <div className='flex flex-col items-center gap-2.5 py-6 text-center'>
        <div className='text-xs text-muted-foreground'>{state.error}</div>
        {state.retryable !== false && onRetry ? (
          <Button type='button' variant='outline' size='xs' onClick={onRetry}>{t('重试')}</Button>
        ) : null}
      </div>
    )
  }

  const traffic = state.data
  const status = traffic.status === null || traffic.status === undefined ? '-' : String(traffic.status)
  const meta = [
    traffic.url ? `URL: ${traffic.url}` : '',
    traffic.provider ? t('提供商: {provider}', { provider: String(traffic.provider) }) : '',
    t('上游状态码: {status}', { status }),
  ].filter(Boolean)

  return (
    <div className='flex flex-col gap-4'>
      {meta.length > 0 && (
        <div className='border-b border-hairline pb-2.5 text-xs leading-[1.7] break-all text-subtle'>
          {meta.map((line, index) => <div key={index}>{line}</div>)}
        </div>
      )}
      {computed.segments.map(seg => {
        const defaultCollapsed = seg.bytes > SINGLE_COLLAPSE_BYTES
          || (computed.totalBytes > TOTAL_COLLAPSE_BYTES && seg.bytes > LARGE_TOTAL_SEG_BYTES)
        return (
          <div key={seg.key} className='flex min-w-0 flex-col gap-1.5'>
            <div className='flex items-center gap-2'>
              <h3 className='text-[11.5px] font-semibold tracking-[.02em] text-muted-foreground'>{seg.label}</h3>
              {seg.bytes >= 1024 ? (
                <span className='font-mono text-[11px] text-muted-foreground tabular-nums'>
                  {formatByteSize(seg.bytes)}
                </span>
              ) : null}
              {seg.text ? <MemoryCopyButton text={seg.text} className='ml-auto' /> : null}
            </div>
            {seg.text ? (
              <PagedTextViewer
                text={seg.text}
                label={seg.label}
                defaultCollapsed={defaultCollapsed}
              />
            ) : (
              <RequestEmpty text={t('这条请求没有保存这一段报文')} />
            )}
          </div>
        )
      })}
    </div>
  )
}
