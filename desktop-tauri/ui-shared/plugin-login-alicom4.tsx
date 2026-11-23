import { Alert } from '@ui'
import {
  buildAlicom4Page,
  isValidAlicom4Config,
  parseAlicom4Result,
  type Alicom4Config,
} from './plugin-captcha-frames'
import { PluginCaptchaDialog } from './plugin-login-captcha-dialog'

export type { Alicom4Config }

export function Alicom4Captcha({ config, onChange, disabled = false, t }: {
  config: Alicom4Config
  onChange: (value: unknown) => void
  disabled?: boolean
  t: (key: string) => string
}) {
  if (!isValidAlicom4Config(config)) {
    return <Alert variant='destructive'>{t('人机验证地址无效，请重新发起登录')}</Alert>
  }
  return (
    <PluginCaptchaDialog
      buildPage={nonce => buildAlicom4Page(config.captchaId, nonce)}
      parseResult={parseAlicom4Result}
      onChange={onChange}
      disabled={disabled}
      t={t}
    />
  )
}
