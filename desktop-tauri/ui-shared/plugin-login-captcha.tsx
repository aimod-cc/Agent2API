import { buildGeetestPage, parseGeetestResult, type GeetestConfig } from './plugin-captcha-frames'
import { PluginCaptchaDialog } from './plugin-login-captcha-dialog'

export type { GeetestConfig }

export function Captcha({ config, onChange, disabled = false, t }: {
  config: GeetestConfig
  onChange: (value: unknown) => void
  disabled?: boolean
  t: (key: string) => string
}) {
  return (
    <PluginCaptchaDialog
      buildPage={nonce => buildGeetestPage(config, nonce)}
      parseResult={parseGeetestResult}
      onChange={onChange}
      disabled={disabled}
      t={t}
    />
  )
}
