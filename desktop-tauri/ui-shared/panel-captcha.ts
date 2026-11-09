export type CaptchaProvider = 'altcha' | 'turnstile'

export type CaptchaSetting = {
  captchaDesktop?: boolean
  captchaWeb?: boolean
  localPanel?: boolean
  captchaProvider?: CaptchaProvider
  turnstileSiteKey?: string
  turnstileSecretSet?: boolean
  turnstileHostnames?: string[]
  error?: string | null
}

export type CaptchaPatch = {
  captchaDesktop?: boolean
  captchaWeb?: boolean
  captchaProvider?: CaptchaProvider
  turnstileSiteKey?: string
  turnstileSecretKey?: string | null
  turnstileHostnames?: string[]
}

export type PanelCaptchaStatus = {
  captchaEnabled?: boolean
  captchaProvider?: CaptchaProvider
  turnstileSiteKey?: string | null
  captchaError?: string | null
}
