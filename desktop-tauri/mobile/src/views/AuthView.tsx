import { useEffect, useRef, useState } from 'react'
import {
  authenticate,
  exchangeOauthTicket,
  fetchPanelStatus,
  pollOauthLoginState,
  solveCaptcha,
  startOauthLoginUrl,
} from '../core/auth'
import { probe, storeApiKey, url } from '../core/client'
import { apiBase, baseLabel, isPackaged, normalizeBase, openExternalUrl, serverIdentity } from '../core/servers'
import { store, useStore } from '../core/store'
import { CommonCard, DashboardCard, ListGroup, ListItem, TonalChip } from '../ui/components'
import { GlyphIcon } from '../ui/icons'

function TurnstileFrame({
  endpointUrl,
  resetKey,
  onToken,
  onState,
}: {
  endpointUrl: string
  resetKey: number
  onToken: (token: string | null) => void
  onState: (next: 'solving' | 'ready' | 'failed') => void
}) {
  const hostRef = useRef<HTMLDivElement>(null)
  const cbRef = useRef({ onToken, onState })
  cbRef.current = { onToken, onState }
  const [retry, setRetry] = useState(0)
  const [err, setErr] = useState('')

  useEffect(() => {
    const host = hostRef.current
    if (!host) return
    let alive = true
    let connected = false
    const frame = document.createElement('iframe')
    const channel = new MessageChannel()
    const tokenChannel = Array.from(crypto.getRandomValues(new Uint8Array(16)), b =>
      b.toString(16).padStart(2, '0'),
    ).join('')
    const endpoint = new URL(endpointUrl, window.location.href)
    endpoint.searchParams.set('channel', tokenChannel)

    setErr('')
    cbRef.current.onToken(null)
    cbRef.current.onState('solving')

    const timer = setTimeout(() => {
      if (!alive) return
      setErr('Turnstile 人机验证加载超时')
      cbRef.current.onState('failed')
    }, 90000)

    channel.port1.onmessage = ev => {
      if (!alive || !ev.data || typeof ev.data !== 'object') return
      const data = ev.data as { state?: string; token?: string; height?: number }
      if (data.state === 'size' && typeof data.height === 'number') {
        frame.style.height = `${Math.min(480, Math.max(65, data.height))}px`
      } else if (data.state === 'verified' && data.token) {
        clearTimeout(timer)
        setErr('')
        cbRef.current.onToken(data.token)
        cbRef.current.onState('ready')
      } else if (data.state === 'error') {
        clearTimeout(timer)
        setErr('Turnstile 验证失败，请点击重试')
        cbRef.current.onToken(null)
        cbRef.current.onState('failed')
      }
    }

    frame.title = 'Turnstile Verification'
    frame.style.cssText = 'display:block;width:100%;height:65px;border:0;background:transparent'
    frame.src = endpoint.href
    frame.referrerPolicy = 'no-referrer'
    frame.onload = () => {
      if (!alive || connected) return
      connected = true
      frame.contentWindow?.postMessage(
        {
          type: 'agent2api-turnstile-connect',
          channel: tokenChannel,
          language: document.documentElement.lang || 'zh-CN',
          theme: document.documentElement.dataset.theme || 'system',
        },
        endpoint.origin,
        [channel.port2],
      )
    }
    frame.onerror = () => {
      if (!alive) return
      clearTimeout(timer)
      setErr('无法加载 Turnstile 组件')
      cbRef.current.onState('failed')
    }

    host.appendChild(frame)
    return () => {
      alive = false
      clearTimeout(timer)
      channel.port1.close()
      channel.port2.close()
      frame.remove()
    }
  }, [endpointUrl, resetKey, retry])

  return (
    <div className='fc-inner-block' style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
      <div ref={hostRef} />
      {err ? (
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8 }}>
          <span style={{ fontSize: 12, color: 'var(--fc-danger)' }}>{err}</span>
          <button
            type='button'
            className='fc-btn is-tonal is-sm is-pill'
            onClick={() => setRetry(v => v + 1)}
          >
            重试
          </button>
        </div>
      ) : null}
    </div>
  )
}

export function ServerSetupView() {
  const [name, setName] = useState('')
  const [address, setAddress] = useState('')
  const [busy, setBusy] = useState(false)

  const handleConnect = async () => {
    const normalized = normalizeBase(address)
    if (!normalized) {
      store.toast('请输入合法的网关 IP 或域名', 'danger')
      return
    }
    setBusy(true)
    try {
      const check = await probe(normalized)
      store.upsertServer({ name: name.trim(), base: normalized }, true)
      if (check.ok) {
        store.toast(`已连接网关 (${check.latencyMs ?? 1} ms)`, 'success')
      } else {
        store.toast('已保存地址，请确认网关已启动', 'info')
      }
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className='fc-app-shell'>
      <div
        className='fc-scroll-body no-dock'
        style={{
          paddingTop: 'calc(var(--fc-safe-top) + 24px)',
        }}
      >
        <div style={{ margin: 'auto 0', width: '100%' }}>
          <DashboardCard
            icon='server'
            title='连接到 Agent2API 网关'
            subtitle='输入运行在电脑或服务器上的网关地址即可开始管理'
          >
            <input
              type='text'
              className='fc-input'
              value={name}
              onChange={e => setName(e.target.value)}
              placeholder='服务器名称（选填，如 我的电脑）'
            />
            <input
              type='text'
              className='fc-input fc-mono'
              value={address}
              onChange={e => setAddress(e.target.value)}
              placeholder='例如 192.168.1.10:3065 或 https://gw.example.com'
            />

            <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8 }}>
              <button
                type='button'
                className='fc-btn is-sm is-pill'
                onClick={() => {
                  setName('本地调试网关')
                  setAddress('http://127.0.0.1:3065')
                }}
              >
                填入 127.0.0.1:3065
              </button>
            </div>

            <button
              type='button'
              className='fc-btn is-primary'
              disabled={!address.trim() || busy}
              onClick={handleConnect}
            >
              <GlyphIcon name='bolt' size={16} />
              <span>{busy ? '正在连接网关…' : '立即连接'}</span>
            </button>
          </DashboardCard>
        </div>
      </div>
    </div>
  )
}

export function AuthGateView({ mode }: { mode: 'login' | 'key' }) {
  const { server, packaged } = useStore()
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [apiKey, setApiKey] = useState('')
  const [setup, setSetup] = useState(false)
  const [statusKnown, setStatusKnown] = useState(false)
  const [provider, setProvider] = useState<'altcha' | 'turnstile'>('altcha')
  const [turnstileReset, setTurnstileReset] = useState(0)
  const [captchaState, setCaptchaState] = useState<'idle' | 'solving' | 'ready' | 'off' | 'failed'>(
    'idle',
  )
  const [captcha, setCaptcha] = useState<string | null>(null)
  const [progress, setProgress] = useState(0)
  const [busy, setBusy] = useState(false)
  const [oauthGoogle, setOauthGoogle] = useState(false)
  const [oauthGithub, setOauthGithub] = useState(false)
  const [oauthBusy, setOauthBusy] = useState<'google' | 'github' | null>(null)
  const [error, setError] = useState('')
  const genRef = useRef(0)

  const prepareCaptcha = async () => {
    const mine = ++genRef.current
    const id = serverIdentity()
    setCaptchaState('solving')
    setCaptcha(null)
    setProgress(0)
    setError('')
    try {
      const status = await fetchPanelStatus()
      if (genRef.current !== mine || serverIdentity() !== id) return
      if (typeof status.registered === 'boolean') {
        setSetup(!status.registered)
        setStatusKnown(true)
      }
      setOauthGoogle(status.oauth?.google === true)
      setOauthGithub(status.oauth?.github === true)
      if (status.captchaEnabled === false) {
        setCaptchaState('off')
        return
      }
      if (status.captchaProvider === 'turnstile') {
        setProvider('turnstile')
        setTurnstileReset(v => v + 1)
        return
      }
      setProvider('altcha')
      const token = await solveCaptcha(r => {
        if (genRef.current === mine) setProgress(r)
      })
      if (genRef.current !== mine || serverIdentity() !== id) return
      setCaptcha(token)
      setCaptchaState(token ? 'ready' : 'off')
    } catch (err) {
      if (genRef.current !== mine || serverIdentity() !== id) return
      setCaptchaState('failed')
      setError(err instanceof Error ? err.message : '无法连接网关获取验证状态')
    }
  }

  useEffect(() => {
    if (mode === 'key') return
    void prepareCaptcha()
    return () => {
      genRef.current += 1
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mode, server?.base])

  const handleSubmit = async () => {
    if (mode === 'key') {
      if (!apiKey.trim()) return
      storeApiKey(apiKey.trim())
      store.setAuthLost(null)
      await store.refresh(true)
      return
    }
    if (!username.trim() || !password || busy) return
    setBusy(true)
    setError('')
    const res = await authenticate({
      mode: setup ? 'setup' : 'login',
      username: username.trim(),
      password,
      captcha,
    })
    setBusy(false)
    if ('error' in res) {
      setError(res.error)
      void prepareCaptcha()
      return
    }
    store.setAuthLost(null)
    store.toast('登录成功', 'success')
    await store.refresh(true)
  }

  const handleOauthLogin = async (oauthProvider: 'google' | 'github') => {
    if (busy || oauthBusy) return
    setError('')
    if (!isPackaged()) {
      window.location.href = `/api/panel/oauth/start?provider=${encodeURIComponent(oauthProvider)}&origin=${encodeURIComponent(window.location.origin)}&next=%2Fm`
      return
    }
    setOauthBusy(oauthProvider)
    try {
      const origin = apiBase()
      const { state, authorizeUrl } = await startOauthLoginUrl(oauthProvider, origin)
      void openExternalUrl(authorizeUrl)
      const startedAt = Date.now()
      while (Date.now() - startedAt < 180_000) {
        await new Promise(resolve => setTimeout(resolve, 1200))
        const res = await pollOauthLoginState(state)
        if (res.status === 'pending') continue
        if (res.status === 'done' && res.ticket) {
          const auth = await exchangeOauthTicket(res.ticket)
          if ('error' in auth) {
            setError(auth.error)
            return
          }
          store.setAuthLost(null)
          store.toast('登录成功', 'success')
          await store.refresh(true)
          return
        }
        setError(res.error || '第三方登录失败')
        return
      }
      setError('第三方登录超时，请重试')
    } catch (err) {
      setError(err instanceof Error ? err.message : '无法发起第三方登录')
    } finally {
      setOauthBusy(null)
    }
  }

  return (
    <div className='fc-app-shell'>
      <div
        className='fc-scroll-body no-dock'
        style={{
          paddingTop: 'calc(var(--fc-safe-top) + 20px)',
        }}
      >
        <ListGroup>
          <ListItem
            icon='server'
            title={server?.name || baseLabel(server?.base || '') || '当前服务器'}
            subtitle={server?.base || ''}
            trailing={<TonalChip tone='primary'>切换</TonalChip>}
            chevron={packaged}
            onClick={() => packaged && store.openSheet({ kind: 'servers' })}
          />
        </ListGroup>

        <DashboardCard
          icon='shield'
          title={
            mode === 'key'
              ? '输入网关 API Key'
              : setup
                ? '首次创建管理员账号'
                : '登录网关管理面板'
          }
          subtitle={
            mode === 'key'
              ? '使用一把已启用的网关 API Key 验证身份'
              : setup
                ? '设置管理员账号与密码以保护管理面板'
                : '请输入当前网关的管理员账号与密码'
          }
        >
          {mode === 'key' ? (
            <input
              type='password'
              className='fc-input fc-mono'
              value={apiKey}
              onChange={e => setApiKey(e.target.value)}
              placeholder='sk-...'
            />
          ) : (
            <>
              <input
                type='text'
                className='fc-input'
                value={username}
                onChange={e => setUsername(e.target.value)}
                placeholder='管理员账号'
                autoCapitalize='off'
                autoCorrect='off'
              />
              <input
                type='password'
                className='fc-input'
                value={password}
                onChange={e => setPassword(e.target.value)}
                placeholder={setup ? '设置密码（至少 8 位）' : '管理员密码'}
              />

              {captchaState !== 'off' ? (
                provider === 'turnstile' ? (
                  <TurnstileFrame
                    endpointUrl={url('/api/panel/turnstile')}
                    resetKey={turnstileReset}
                    onToken={setCaptcha}
                    onState={setCaptchaState}
                  />
                ) : (
                  <div
                    className='fc-inner-block'
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      justifyContent: 'space-between',
                      gap: 10,
                      fontSize: 12.5,
                    }}
                  >
                    <span style={{ color: 'var(--fc-text-secondary)', fontWeight: 600 }}>
                      {captchaState === 'solving'
                        ? `安全校验准备中 (${Math.round(progress * 100)}%)…`
                        : captchaState === 'ready'
                          ? '安全校验已就绪'
                          : '安全校验获取失败'}
                    </span>
                    {captchaState === 'failed' ? (
                      <button
                        type='button'
                        className='fc-btn is-tonal is-sm is-pill'
                        onClick={() => void prepareCaptcha()}
                      >
                        重试
                      </button>
                    ) : captchaState === 'ready' ? (
                      <TonalChip tone='success'>已就绪</TonalChip>
                    ) : null}
                  </div>
                )
              ) : null}
            </>
          )}

          {error ? (
            <CommonCard>
              <div style={{ fontSize: 12.5, color: 'var(--fc-danger)', fontWeight: 600 }}>
                {error}
              </div>
            </CommonCard>
          ) : null}

          <button
            type='button'
            className='fc-btn is-primary'
            disabled={
              busy ||
              Boolean(oauthBusy) ||
              (mode === 'key'
                ? !apiKey.trim()
                : !statusKnown ||
                  !username.trim() ||
                  !password ||
                  (captchaState !== 'ready' && captchaState !== 'off'))
            }
            onClick={handleSubmit}
          >
            {busy ? '正在验证…' : mode === 'key' ? '确认并继续' : setup ? '创建并登录' : '登录'}
          </button>

          {mode === 'login' && !setup && (oauthGoogle || oauthGithub) ? (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 8, paddingTop: 4 }}>
              <div style={{ textAlign: 'center', fontSize: 12, color: 'var(--fc-text-tertiary)' }}>
                或使用第三方账号登录
              </div>
              {oauthGoogle ? (
                <button
                  type='button'
                  className='fc-btn'
                  disabled={busy || oauthBusy !== null}
                  onClick={() => void handleOauthLogin('google')}
                >
                  {oauthBusy === 'google' ? '等待 Google 授权…' : '使用 Google 账号登录'}
                </button>
              ) : null}
              {oauthGithub ? (
                <button
                  type='button'
                  className='fc-btn'
                  disabled={busy || oauthBusy !== null}
                  onClick={() => void handleOauthLogin('github')}
                >
                  {oauthBusy === 'github' ? '等待 GitHub 授权…' : '使用 GitHub 账号登录'}
                </button>
              ) : null}
            </div>
          ) : null}
        </DashboardCard>
      </div>
    </div>
  )
}
