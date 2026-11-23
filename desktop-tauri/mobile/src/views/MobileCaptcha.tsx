import {
  buildAlicom4Page,
  buildGeetestPage,
  isValidAlicom4Config,
  parseAlicom4Result,
  parseGeetestResult,
  type Alicom4Config,
  type GeetestConfig,
} from '../../../ui-shared/plugin-captcha-frames'
import { useCaptchaPopupSession } from '../../../ui-shared/plugin-login-captcha-session'
import { t } from '../core/i18n'
import { BottomSheet } from '../ui/components'

function MobileCaptchaSheet({
  buildPage,
  parseResult,
  disabled = false,
  onChange,
}: {
  buildPage: (nonce: string) => string
  parseResult: (value: unknown) => unknown
  disabled?: boolean
  onChange: (value: unknown) => void
}) {
  const {
    frame,
    nonce,
    open,
    ready,
    verified,
    failed,
    attempt,
    start,
    dismiss,
  } = useCaptchaPopupSession({ onChange, parseResult })

  const page = open ? buildPage(nonce) : ''

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
      <button
        type='button'
        className={`fc-btn ${verified ? 'is-tonal' : ''}`}
        disabled={disabled}
        style={verified ? { color: 'var(--fc-success)', width: '100%' } : { width: '100%' }}
        onClick={start}
      >
        {verified ? t('人机验证已通过') : t('打开人机验证')}
      </button>
      {failed ? (
        <div style={{ fontSize: 12.5, color: 'var(--fc-danger)' }}>
          {t('登录步骤读取失败，请刷新当前步骤或重新发起登录。')}
        </div>
      ) : null}
      <BottomSheet open={open} title={t('人机验证')} onClose={dismiss}>
        <div
          style={{
            position: 'relative',
            width: '100%',
            height: 'min(480px, 68dvh)',
            borderRadius: 12,
            overflow: 'hidden',
          }}
        >
          {!ready ? (
            <div
              style={{
                position: 'absolute',
                inset: 0,
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                fontSize: 12.5,
                color: 'var(--fc-text-secondary)',
                background: 'var(--fc-block)',
                pointerEvents: 'none',
              }}
            >
              {t('正在进行人机验证…')}
            </div>
          ) : null}
          {open ? (
            <iframe
              key={attempt}
              ref={frame}
              title={t('人机验证')}
              sandbox='allow-scripts'
              referrerPolicy='no-referrer'
              style={{ display: 'block', width: '100%', height: '100%', border: 0, background: 'transparent' }}
              srcDoc={page}
            />
          ) : null}
        </div>
      </BottomSheet>
    </div>
  )
}

export function MobileAlicom4Captcha({
  config,
  disabled = false,
  onChange,
}: {
  config: Alicom4Config
  disabled?: boolean
  onChange: (value: unknown) => void
}) {
  if (!isValidAlicom4Config(config)) {
    return (
      <div style={{ fontSize: 12.5, color: 'var(--fc-danger)' }}>
        {t('人机验证地址无效，请重新发起登录')}
      </div>
    )
  }

  return (
    <MobileCaptchaSheet
      buildPage={nonce => buildAlicom4Page(config.captchaId, nonce)}
      parseResult={parseAlicom4Result}
      disabled={disabled}
      onChange={onChange}
    />
  )
}

export function MobileGeetestCaptcha({
  config,
  disabled = false,
  onChange,
}: {
  config: GeetestConfig
  disabled?: boolean
  onChange: (value: unknown) => void
}) {
  return (
    <MobileCaptchaSheet
      buildPage={nonce => buildGeetestPage(config, nonce)}
      parseResult={parseGeetestResult}
      disabled={disabled}
      onChange={onChange}
    />
  )
}
