import { useEffect, useRef, useState } from 'react'
import { Icon } from '../components/Icon'
import { store, useStore } from '../state/store'
import { authenticate, fetchPanelStatus, solveCaptcha } from '../api/auth'
import { storeApiKey } from '../api/client'
import { baseLabel } from '../lib/servers'
import { t } from '../i18n'

/**
 * 登录 / 首次注册（APK 独立整屏页）。
 *
 * ── 为什么是整屏而不是盖在主界面上的小弹窗 ────────────────────
 * 登录是一件「没完成就什么都做不了」的事，弹窗形态会透出后面的界面，用户
 * 容易以为还能点、也不知道自己在哪一步。整屏页给出明确的三个区块：品牌头、
 * 表单、底部的服务器入口。键盘弹起时整页跟着滚，不会把按钮顶到看不见的地方。
 *
 * ── 人机验证在后端，不在这一页 ────────────────────────────────
 * 网关开了机器人校验时，`POST /api/panel/login` 与 `/api/panel/setup` **在
 * 服务端**校验一次性 ALTCHA 载荷（见 `server::api::panel::check_captcha`），
 * 没带或算错一律 400。任何客户端 —— 桌面网页、手机 APK、curl —— 都绕不过去。
 * 这一页做的事只是**替用户把题算出来**：进页面领题、后台求解、提交时带上。
 *
 * 因此验证码求解状态必须显示出来。老版本把它做成一行小字，用户看到「登录」
 * 按钮灰着却不知道在等什么；这里做成一条带状态点的行，并且失败时给出重试。
 *
 * ── 令牌怎么落地 ──────────────────────────────────────────────
 * 登录请求带 `x-panel-auth-mode: body`，服务端据此把双令牌放进响应体
 * （见 `server::api::panel::tokens_in_body`）。拿到后按服务器 id 存进
 * localStorage，之后每条请求带 `x-panel-token` 头 —— 跨源请求没有别的
 * 办法带上凭证（`Allow-Origin: *` 下浏览器不带 cookie）。
 */
export function LoginPage({ mode, onDone }: { mode: 'login' | 'key'; onDone?: () => void }) {
  const { server, packaged } = useStore()
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [apiKey, setApiKey] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  /** 该网关有没有管理员 —— 没有就是首次注册 */
  const [setup, setSetup] = useState(false)
  const [statusKnown, setStatusKnown] = useState(false)
  /**
   * 人机验证状态。
   *   idle    还没开始（首帧）
   *   solving 正在领题 / 求解
   *   ready   已拿到可提交的载荷
   *   off     网关没开校验（领题回 400）
   *   failed  求解失败（网络 / 组件问题），可点重试
   */
  const [captchaState, setCaptchaState] = useState<'idle' | 'solving' | 'ready' | 'off' | 'failed'>('idle')
  const [captcha, setCaptcha] = useState<string | null>(null)
  const [progress, setProgress] = useState(0)
  /** 代次：重试时作废上一轮，避免迟到的结果覆盖新状态 */
  const generation = useRef(0)

  const runCaptcha = async () => {
    const mine = ++generation.current
    setCaptchaState('solving')
    setCaptcha(null)
    setProgress(0)
    try {
      const payload = await solveCaptcha(ratio => {
        if (generation.current === mine) setProgress(ratio)
      })
      if (generation.current !== mine) return
      if (payload) {
        setCaptcha(payload)
        setCaptchaState('ready')
      } else {
        // 领题 400 = 网关没开校验；领题失败（网络）也按「不需要」继续 ——
        // 真需要的话提交时服务端会回一句明确的原因
        setCaptchaState('off')
      }
    } catch {
      if (generation.current === mine) setCaptchaState('failed')
    }
  }

  // 进页面就探测：有没有管理员、要不要人机验证
  useEffect(() => {
    let alive = true
    void (async () => {
      try {
        const status = await fetchPanelStatus()
        if (!alive) return
        setSetup(!status.registered)
        setStatusKnown(true)
      } catch {
        if (!alive) return
        setStatusKnown(true)
      }
      if (!alive) return
      await runCaptcha()
    })()
    return () => {
      alive = false
      generation.current += 1
    }
    // 只在挂载时跑一次
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const submit = async () => {
    if (mode === 'key') {
      const value = apiKey.trim()
      if (!value) return
      storeApiKey(value)
      store.setAuthLost(null)
      onDone?.()
      await store.refresh(true)
      return
    }

    if (!username.trim() || !password) {
      setError(t('请填写账号与密码'))
      return
    }
    // 验证还没算完就提交，服务端必然回 400 —— 先在这里拦一句，省一次往返
    if (captchaState === 'solving') {
      setError(t('人机验证还在计算，请稍候'))
      return
    }
    setBusy(true)
    setError('')
    const result = await authenticate({
      mode: setup ? 'setup' : 'login',
      username: username.trim(),
      password,
      captcha,
    })
    setBusy(false)
    if ('error' in result) {
      setError(result.error)
      // 载荷一次性：失败后重新解一道（密码错也一样，下次提交要用新的）
      void runCaptcha()
      return
    }
    store.setAuthLost(null)
    onDone?.()
    await store.refresh(true)
  }

  const submitDisabled =
    busy ||
    (mode === 'key'
      ? !apiKey.trim()
      : !statusKnown || !username.trim() || !password || captchaState === 'solving')

  return (
    <div
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 80,
        display: 'flex',
        flexDirection: 'column',
        background: 'var(--m-bg)',
        overflowY: 'auto',
      }}
    >
      {/* 品牌头：整屏页要有自己的顶，不然表单会悬在半空 */}
      <div
        style={{
          flex: 'none',
          padding: 'calc(var(--m-safe-t) + 40px) 26px 22px',
        }}
      >
        <div
          style={{
            width: 48,
            height: 48,
            borderRadius: 14,
            display: 'grid',
            placeItems: 'center',
            background: 'var(--m-primary)',
            color: '#fff',
            boxShadow: '0 10px 24px -12px var(--m-primary)',
          }}
        >
          <Icon name='shield' style={{ width: 24, height: 24 }} />
        </div>
        <h1 style={{ margin: '16px 0 0', fontSize: 23, letterSpacing: '-0.02em' }}>
          {mode === 'key'
            ? t('需要网关 API Key')
            : setup
              ? t('创建管理员账号')
              : t('登录 Agent2API')}
        </h1>
        <p style={{ margin: '6px 0 0', fontSize: 12.5, lineHeight: 1.6, color: 'var(--m-text-3)' }}>
          {mode === 'key'
            ? t('这台网关没有管理员，用一把已启用的 API Key 访问。')
            : setup
              ? t('这台网关还没有管理员。设置一个账号密码，之后用它登录。')
              : t('输入这台网关的管理员账号与密码。')}
        </p>
      </div>

      <div style={{ flex: 1, padding: '0 20px', display: 'flex', flexDirection: 'column', gap: 10 }}>
        {/* 当前服务器：登录前最该确认的一件事，给成一条可点的行 */}
        <button
          type='button'
          className='m-row'
          disabled={busy}
          onClick={() => packaged && store.push('servers')}
          style={{
            borderRadius: 'var(--m-r-md)',
            border: '1px solid var(--m-line)',
            background: 'var(--m-surface)',
            minHeight: 56,
          }}
        >
          <span
            className='m-logo'
            style={{ background: 'var(--m-primary-soft)', borderColor: 'transparent', color: 'var(--m-primary)' }}
          >
            <Icon name='server' />
          </span>
          <div className='m-grow' style={{ minWidth: 0 }}>
            <div className='m-t1' style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
              {server?.name || baseLabel(server?.base || '') || t('未选择服务器')}
            </div>
            <div className='m-t2 m-mono' style={{ fontSize: 11.5 }}>
              {server?.base ? baseLabel(server.base) : t('点此添加服务器')}
            </div>
          </div>
          {packaged ? (
            <span className='m-chev'>
              <Icon name='chev' />
            </span>
          ) : null}
        </button>

        {mode === 'key' ? (
          <Field
            label={t('API Key')}
            value={apiKey}
            onChange={setApiKey}
            placeholder='sk-…'
            type='password'
            autoComplete='off'
          />
        ) : (
          <>
            <Field
              label={t('账号')}
              value={username}
              onChange={setUsername}
              placeholder={t('管理员账号')}
              autoComplete='username'
              autoCapitalize='off'
              autoCorrect='off'
              spellCheck={false}
            />
            <Field
              label={t('密码')}
              value={password}
              onChange={setPassword}
              placeholder={setup ? t('设置密码（至少 8 位）') : t('密码')}
              type='password'
              autoComplete={setup ? 'new-password' : 'current-password'}
            />

            {/* 人机验证：这一行必须可见 —— 它是登录能不能点的前提 */}
            <div
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 9,
                minHeight: 46,
                padding: '10px 13px',
                borderRadius: 'var(--m-r-md)',
                border: '1px solid var(--m-line)',
                background: 'var(--m-surface)',
              }}
            >
              <span
                style={{
                  width: 20,
                  height: 20,
                  borderRadius: '50%',
                  flex: 'none',
                  display: 'grid',
                  placeItems: 'center',
                  background:
                    captchaState === 'ready'
                      ? 'var(--m-ok)'
                      : captchaState === 'failed'
                        ? 'var(--m-danger-soft)'
                        : 'var(--m-surface-3)',
                  color: captchaState === 'ready' ? '#fff' : captchaState === 'failed' ? 'var(--m-danger)' : 'var(--m-text-3)',
                  fontSize: 12,
                }}
              >
                {captchaState === 'ready' ? (
                  <Icon name='check' style={{ width: 12, height: 12 }} />
                ) : captchaState === 'failed' ? (
                  <Icon name='alert' style={{ width: 12, height: 12 }} />
                ) : (
                  <span
                    style={{
                      width: 10,
                      height: 10,
                      borderRadius: '50%',
                      border: '2px solid currentColor',
                      borderTopColor: 'transparent',
                      animation:
                        captchaState === 'solving' ? 'm-ptr-rotate 0.7s linear infinite' : undefined,
                    }}
                  />
                )}
              </span>
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontSize: 13 }}>
                  {captchaState === 'ready'
                    ? t('人机验证已完成')
                    : captchaState === 'solving'
                      ? t('正在完成人机验证…')
                      : captchaState === 'failed'
                        ? t('人机验证失败')
                        : captchaState === 'off'
                          ? t('这台网关未启用机器人校验')
                          : t('人机验证')}
                </div>
                {captchaState === 'solving' && progress > 0 ? (
                  <div style={{ marginTop: 4, height: 3, borderRadius: 2, background: 'var(--m-surface-3)', overflow: 'hidden' }}>
                    <i
                      style={{
                        display: 'block',
                        height: '100%',
                        width: `${Math.round(progress * 100)}%`,
                        background: 'var(--m-primary)',
                        transition: 'width .2s linear',
                      }}
                    />
                  </div>
                ) : null}
                {captchaState === 'failed' ? (
                  <div style={{ marginTop: 2, fontSize: 11, color: 'var(--m-text-3)' }}>
                    {t('通常是网络问题，点右侧重试')}
                  </div>
                ) : null}
              </div>
              {captchaState === 'failed' ? (
                <button type='button' className='m-btn m-sm m-ghost' onClick={() => void runCaptcha()}>
                  {t('重试')}
                </button>
              ) : null}
            </div>
          </>
        )}

        {error ? (
          <div
            style={{
              padding: '10px 13px',
              borderRadius: 'var(--m-r-md)',
              background: 'var(--m-danger-soft)',
              color: 'var(--m-danger)',
              fontSize: 12.5,
              lineHeight: 1.55,
            }}
          >
            {error}
          </div>
        ) : null}

        <button
          type='button'
          className='m-btn'
          style={{ marginTop: 4, height: 46 }}
          disabled={submitDisabled}
          onClick={() => void submit()}
        >
          {busy ? t('验证中…') : mode === 'key' ? t('保存并继续') : setup ? t('创建并进入') : t('登录')}
        </button>

        {/*
          登录页是整屏的，用户被登出后看不到「设置 → 服务器」——
          这里给一个直达入口：压进服务器页，选/加完一台后它会自己退回原页。
        */}
        {packaged ? (
          <button
            type='button'
            disabled={busy}
            onClick={() => store.push('servers')}
            style={{
              display: 'block',
              width: '100%',
              marginTop: 2,
              padding: '10px 0',
              border: 0,
              background: 'none',
              color: 'var(--m-primary)',
              fontSize: 12.5,
            }}
          >
            {t('换一台服务器')}
          </button>
        ) : null}
      </div>

      <div
        style={{
          flex: 'none',
          padding: '16px 24px calc(var(--m-safe-b) + 20px)',
          textAlign: 'center',
          fontSize: 11,
          color: 'var(--m-text-3)',
        }}
      >
        {t('凭证只保存在这台手机上，按服务器分别存放')}
      </div>
    </div>
  )
}

/** 带标签的输入框（整屏表单里标签比 placeholder 更稳：输入后仍看得见） */
function Field({
  label,
  value,
  onChange,
  placeholder,
  type,
  autoComplete,
  autoCapitalize,
  autoCorrect,
  spellCheck,
}: {
  label: string
  value: string
  onChange: (next: string) => void
  placeholder?: string
  type?: string
  autoComplete?: string
  autoCapitalize?: string
  autoCorrect?: string
  spellCheck?: boolean
}) {
  return (
    <label style={{ display: 'block' }}>
      <span style={{ display: 'block', fontSize: 12, color: 'var(--m-text-3)', marginBottom: 6, paddingLeft: 2 }}>
        {label}
      </span>
      <input
        value={value}
        onChange={event => onChange(event.target.value)}
        placeholder={placeholder}
        type={type}
        autoComplete={autoComplete}
        autoCapitalize={autoCapitalize}
        autoCorrect={autoCorrect}
        spellCheck={spellCheck}
        style={{
          width: '100%',
          boxSizing: 'border-box',
          height: 46,
          padding: '0 13px',
          borderRadius: 'var(--m-r-md)',
          border: '1px solid var(--m-line-strong)',
          background: 'var(--m-surface)',
          color: 'var(--m-text)',
          // 16px 是 iOS 的阈值：小于它聚焦时 Safari 会自动放大整页
          fontSize: 16,
          fontFamily: type === 'password' ? 'var(--m-mono)' : 'inherit',
        }}
      />
    </label>
  )
}
