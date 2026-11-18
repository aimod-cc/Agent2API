export type ErrorKind = 'rate_limit' | 'quota' | 'auth' | 'balance' | 'transient' | 'unsupported' | 'bad_request' | 'content' | 'unknown'
export type ErrorAction = 'switch_account' | 'retry_then_switch' | 'refresh_then_switch' | 'switch_provider' | 'return_error'
export type CooldownMode = 'upstream' | 'fixed' | 'protection' | 'none'
export type CooldownScope = 'auto' | 'model' | 'account' | 'provider'
export type ErrorRule = {
  id: string; name: string; kind: ErrorKind; statuses: number[]; code: string; provider: string; model: string; enabled: boolean
  action: ErrorAction; cooldown: CooldownMode; fallbackSeconds: number; exponential: boolean; fallbackCapSeconds: number; scope: CooldownScope
}
export type ErrorSettings = {
  resends: number; switches: number; intervalSeconds: number; waitBudgetSeconds: number; cooldownFloorSeconds: number
  backoff: boolean; backoffSeconds: number; backoffCapSeconds: number; sink: boolean; sinkSeconds: number
  breaker: boolean; breakerFailures: number; breakerSeconds: number; breakerSuccesses: number
  rules: ErrorRule[]; overrides: ErrorRule[]
}
export type ErrorPolicyView = {
  settings: ErrorSettings; defaults: ErrorSettings; revision: string; configured: boolean; legacy: boolean
  providers: { id: string; label: string }[]
}
export type ErrorPolicyApi = { load(): Promise<ErrorPolicyView>; save(payload: { settings: ErrorSettings; revision: string }): Promise<ErrorPolicyView> }
export type ErrorPolicyRecord = { ruleId?: string; ruleName?: string; kind?: ErrorKind; action?: ErrorAction; scope?: CooldownScope;
  resetAt?: number | null; timeSource?: 'upstream' | 'fallback' | 'none'; exception?: boolean; reason?: string }
export type ErrorTranslate = (key: string, variables?: Record<string, string | number>) => string
export const ERROR_POLICY_CHANGED = 'wb:routing-errors-changed'

export function kindLabels(t: ErrorTranslate): Record<ErrorKind, string> {
  return { rate_limit: t('请求限流'), quota: t('额度耗尽'), auth: t('登录失效'), balance: t('余额不足'),
    transient: t('连接或服务异常'), unsupported: t('能力不支持'), bad_request: t('请求有误'), content: t('内容被拒绝'), unknown: t('其他错误') }
}
export function actionLabels(t: ErrorTranslate): Record<ErrorAction, string> {
  return { switch_account: t('直接换账号'), retry_then_switch: t('先重发，再换账号'), refresh_then_switch: t('续期一次，失败再换号'),
    switch_provider: t('跳过提供商'), return_error: t('直接返回错误') }
}
export function scopeLabels(t: ErrorTranslate): Record<CooldownScope, string> {
  return { auto: t('跟随上游限制范围'), model: t('当前账号的当前模型'), account: t('当前账号的全部模型'), provider: t('整个提供商') }
}
export function ruleLabel(rule: ErrorRule, t: ErrorTranslate): string {
  return rule.id === rule.kind ? kindLabels(t)[rule.kind] : rule.name
}
export function duration(seconds: number, t: ErrorTranslate): string {
  if (!seconds) return t('不冷却')
  if (seconds >= 3600 && seconds % 3600 === 0) return t('{n} 小时', { n: seconds / 3600 })
  if (seconds >= 60 && seconds % 60 === 0) return t('{n} 分钟', { n: seconds / 60 })
  return t('{n} 秒', { n: seconds })
}
export function cooldownSummary(rule: ErrorRule, settings: ErrorSettings, t: ErrorTranslate): string {
  if (rule.cooldown === 'none') return t('不冷却')
  if (rule.cooldown === 'protection') return settings.backoff ? t('失败后休息 {time}', { time: duration(settings.backoffSeconds, t) }) : t('不冷却')
  if (rule.cooldown === 'fixed') return t('固定 {time}', { time: duration(rule.fallbackSeconds, t) })
  return t('上游时间，缺失用 {time}', { time: duration(rule.fallbackSeconds, t) })
}
export function errorPolicyValidation(settings: ErrorSettings, t: ErrorTranslate): string | null {
  const fields: [number, number, number, string][] = [
    [settings.resends, 0, 10, t('原账号重发')], [settings.switches, 0, 10, t('最多再换账号')],
    [settings.intervalSeconds, 0, 300, t('重发间隔')], [settings.waitBudgetSeconds, 0, 300, t('累计等待上限')],
    [settings.cooldownFloorSeconds, 1, 60, t('最短限流冷却')], [settings.backoffSeconds, 1, 3600, t('首次休息时间')],
    [settings.backoffCapSeconds, settings.backoffSeconds, 3600, t('连续失败最长休息')], [settings.sinkSeconds, 30, 3600, t('最长保留时间')],
    [settings.breakerFailures, 1, 20, t('连续失败阈值')], [settings.breakerSeconds, 10, 600, t('保护时长')],
    [settings.breakerSuccesses, 1, 10, t('恢复所需成功次数')],
  ]
  for (const [value, min, max, name] of fields) if (!Number.isInteger(value) || value < min || value > max) return t('{name}必须是 {min}–{max} 的整数', { name, min, max })
  for (const rule of [...settings.rules, ...settings.overrides]) {
    if (!rule.name.trim()) return t('请填写规则名称。')
    if (rule.fallbackCapSeconds < rule.fallbackSeconds) return t('冷却上限不能短于首次冷却时间。')
  }
  return null
}
