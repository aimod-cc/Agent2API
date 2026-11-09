import * as React from 'react'
import { Alert, Button } from '@ui'

type Translate = (key: string, variables?: Record<string, string | number>) => string
type CaptchaState = 'verifying' | 'verified' | 'error'

export function TurnstileWidget({ url, t, onToken, onState, resetKey = 0 }: {
  url: string
  t: Translate
  onToken: (token: string | null) => void
  onState?: (state: CaptchaState) => void
  resetKey?: number
}) {
  const container = React.useRef<HTMLDivElement>(null)
  const callbacks = React.useRef({ onToken, onState })
  callbacks.current = { onToken, onState }
  const [error, setError] = React.useState('')
  const [retry, setRetry] = React.useState(0)

  React.useEffect(() => {
    const host = container.current
    if (!host) return
    let alive = true
    let connected = false
    let timer: ReturnType<typeof setTimeout> | undefined
    const frame = document.createElement('iframe')
    const connection = new MessageChannel()
    const channel = Array.from(crypto.getRandomValues(new Uint8Array(16)), byte => byte.toString(16).padStart(2, '0')).join('')
    const endpoint = new URL(url, window.location.href)
    endpoint.searchParams.set('channel', channel)
    setError('')
    callbacks.current.onToken(null)
    callbacks.current.onState?.('verifying')

    const fail = (message: string) => {
      if (!alive) return
      clearTimeout(timer)
      setError(message)
      callbacks.current.onToken(null)
      callbacks.current.onState?.('error')
    }
    connection.port1.onmessage = event => {
      if (!alive || !event.data || typeof event.data !== 'object') return
      const data = event.data as { state?: string; token?: unknown; height?: unknown; reason?: string; code?: string }
      if (data.state === 'size') {
        if (typeof data.height === 'number' && Number.isFinite(data.height)) frame.style.height = `${Math.min(520, Math.max(65, data.height))}px`
        return
      }
      if (data.state === 'verified' && typeof data.token === 'string' && data.token.length > 0 && data.token.length <= 2048) {
        clearTimeout(timer)
        setError('')
        callbacks.current.onToken(data.token)
        callbacks.current.onState?.('verified')
      } else if (data.state === 'verifying') {
        clearTimeout(timer)
        timer = setTimeout(() => fail(t('Turnstile 验证超时，请重试。')), 120000)
        setError('')
        callbacks.current.onToken(null)
        callbacks.current.onState?.('verifying')
      } else if (data.state === 'error') {
        const message = data.reason === 'configuration' ? t('机器人校验配置已变更或不完整，请刷新页面或联系管理员。')
          : data.reason === 'timeout' ? t('Turnstile 验证超时，请重试。')
            : data.reason === 'unsupported' ? t('当前浏览器不支持 Turnstile，请更新浏览器后重试。')
              : t('Turnstile 加载或验证失败，请检查网络、浏览器拦截和域名配置后重试。')
        fail(data.code && /^[0-9]{3,12}$/.test(data.code) ? `${message} (${data.code})` : message)
      }
    }
    frame.title = 'Cloudflare Turnstile'
    frame.style.cssText = 'display:block;width:100%;height:65px;border:0;background:transparent'
    frame.src = endpoint.href
    frame.referrerPolicy = 'no-referrer'
    frame.onload = () => {
      if (!alive || connected) return
      connected = true
      const locale = document.documentElement.lang
      const language = locale === 'zh-Hans' ? 'zh-CN' : locale === 'zh-Hant' ? 'zh-TW' : locale
      frame.contentWindow?.postMessage({
        type: 'agent2api-turnstile-connect', channel, language,
        theme: document.documentElement.dataset.theme || 'auto',
      }, endpoint.origin, [connection.port2])
    }
    frame.onerror = () => fail(t('Turnstile 加载或验证失败，请检查网络、浏览器拦截和域名配置后重试。'))
    timer = setTimeout(() => fail(t('Turnstile 验证超时，请重试。')), 120000)
    host.appendChild(frame)
    return () => {
      alive = false
      clearTimeout(timer)
      connection.port1.close()
      connection.port2.close()
      frame.remove()
    }
  }, [url, resetKey, retry, t])

  return <div className='flex w-full min-w-0 flex-col gap-2'>
    <div ref={container} className='w-full min-w-0' />
    {error ? <>
      <Alert variant='destructive' className='break-words text-xs'>{error}</Alert>
      <Button type='button' variant='outline' size='sm' onClick={() => setRetry(value => value + 1)}>{t('重新验证')}</Button>
    </> : null}
  </div>
}
