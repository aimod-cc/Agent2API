import type { ErrorKind, ErrorRule, ErrorSettings, ErrorTranslate, PluginErrorRule } from './routing-errors-types'
import { actionLabels, duration, kindLabels, ruleLabel, scopeLabels } from './routing-errors-types'

export type ErrorScenario = { id: string; label: string; status: number; kind: ErrorKind; code: string; seconds: number | null; scope: 'model' | 'account' | 'provider'; local?: boolean }
export function errorScenarios(t: ErrorTranslate): ErrorScenario[] {
  return [
    { id: 'rate', label: t('429 · 有等待时间'), status: 429, kind: 'rate_limit', code: 'rate_limit_error', seconds: 90, scope: 'model' },
    { id: 'rate-unknown', label: t('429 · 没有时间'), status: 429, kind: 'rate_limit', code: 'rate_limit_error', seconds: null, scope: 'model' },
    { id: 'quota', label: t('429 · 额度用完'), status: 429, kind: 'quota', code: 'usage_limit_reached', seconds: 21600, scope: 'account' },
    { id: 'quota-unknown', label: t('429 · 额度重置未知'), status: 429, kind: 'quota', code: 'quota_exhausted', seconds: null, scope: 'account' },
    { id: 'unsupported', label: t('501 · 上游不支持'), status: 501, kind: 'unsupported', code: 'unsupported', seconds: null, scope: 'provider' },
    { id: 'gateway', label: t('501 · 网关缺能力'), status: 501, kind: 'unsupported', code: 'gateway_capability_missing', seconds: null, scope: 'model', local: true },
    { id: 'transient', label: t('503 · 服务异常'), status: 503, kind: 'transient', code: 'service_unavailable', seconds: null, scope: 'model' },
    { id: 'shared', label: t('429 · 共享限制'), status: 429, kind: 'rate_limit', code: 'provider_rate_limit', seconds: 90, scope: 'provider' },
    { id: 'auth', label: t('401 · 登录过期'), status: 401, kind: 'auth', code: 'token_expired', seconds: null, scope: 'account' },
    { id: 'input', label: t('413 · 请求太大'), status: 413, kind: 'bad_request', code: 'payload_too_large', seconds: null, scope: 'model' },
    { id: 'content', label: t('内容被拒绝'), status: 400, kind: 'content', code: 'content_policy_violation', seconds: null, scope: 'model' },
    { id: 'unknown', label: t('其他未识别错误'), status: 418, kind: 'unknown', code: 'unknown_upstream_error', seconds: null, scope: 'model' },
    { id: 'balance', label: t('402 · 余额不足'), status: 402, kind: 'balance', code: 'insufficient_balance', seconds: null, scope: 'account' },
  ]
}
export type ErrorPreviewInput = {
  scenario: string; provider: string; model: string; started: boolean; uncertain: boolean; pinned: boolean
  switchesUsed: number; resendsUsed: number; strikes: number; customStatus?: string; customCode?: string
}
export type ErrorPreview = {
  title: string; stopped: boolean; cooldown: number; deprioritize: number; source: string; rule: ErrorRule
  steps: { title: string; detail: string }[]; resendsLeft: number; switchesLeft: number
}

function ruleMatches(item: ErrorRule, kind: ErrorKind, provider: string, model: string, code: string, status: number): boolean {
  return item.enabled && item.kind === kind
    && (item.provider === '*' || item.provider === provider)
    && (!item.model || item.model === model)
    && (!item.code || item.code === code)
    && (!item.statuses.length || item.statuses.includes(status))
}

function specificityScore(item: ErrorRule): number {
  return (item.model ? 8 : 0) + (item.provider !== '*' ? 4 : 0) + (item.code ? 2 : 0) + (item.statuses.length ? 1 : 0)
}

export function simulateError(
  settings: ErrorSettings,
  input: ErrorPreviewInput,
  t: ErrorTranslate,
  providerRules: PluginErrorRule[] = [],
): ErrorPreview {
  const scenario = errorScenarios(t).find(item => item.id === input.scenario) || errorScenarios(t)[0]
  const parsedStatus = Number(input.customStatus?.trim())
  const status = Number.isInteger(parsedStatus) && parsedStatus >= 400 && parsedStatus <= 599 ? parsedStatus : scenario.status
  const code = input.customCode?.trim() || scenario.code
  const overriddenIds = new Set(settings.overrides.map(item => item.id))
  const tier1 = settings.overrides.find(item => ruleMatches(item, scenario.kind, input.provider, input.model, code, status))
  const tier2 = providerRules
    .filter(item => !overriddenIds.has(item.id) && !overriddenIds.has(item.pluginRuleId))
    .filter(item => ruleMatches(item, scenario.kind, input.provider, input.model, code, status))
    .sort((a, b) => specificityScore(b) - specificityScore(a))[0]
  const tier3 = settings.rules.find(item => item.enabled && item.kind === scenario.kind) || settings.rules.find(item => item.kind === 'unknown')!
  const rule = tier1 || tier2 || tier3

  const result: ErrorPreview = {
    title: actionLabels(t)[rule.action], stopped: false, cooldown: 0, deprioritize: 0, source: t('不冷却'), rule, steps: [],
    resendsLeft: Math.max(0, settings.resends - input.resendsUsed), switchesLeft: Math.max(0, settings.switches - input.switchesUsed),
  }
  const stop = (message: string) => { result.title = t('结束本次请求'); result.stopped = true; result.steps.push({ title: result.title, detail: message }); return result }
  if (input.started) return stop(t('回答已经交付给客户端，不从头重发或换号拼接另一份回答。'))
  if (input.uncertain) return stop(t('任务可能已执行。没有可靠的安全重放依据，不再次提交或换账号重复执行。'))
  if (scenario.local) return stop(t('错误来自网关自身缺少能力。换其他账号或提供商不能补齐宿主功能。'))
  result.steps.push({ title: t('识别错误'), detail: `HTTP ${status} · ${kindLabels(t)[scenario.kind]} · ${code}` })
  const ruleOrigin = tier1 ? t('自定义规则') : tier2 ? t('插件默认') : t('全局规则')
  result.steps.push({ title: `${ruleLabel(rule, t)}（${ruleOrigin}）`, detail: actionLabels(t)[rule.action] })

  const scopeOrder = { auto: 0, model: 0, account: 1, provider: 2 }
  const scope = rule.scope === 'auto' || scopeOrder[rule.scope] < scopeOrder[scenario.scope] ? scenario.scope : rule.scope
  if (rule.cooldown === 'deprioritize') {
    let deprioritizeSecs = rule.fallbackSeconds
    if (rule.exponential) deprioritizeSecs = Math.min(deprioritizeSecs * 2 ** Math.max(0, input.strikes - 1), rule.fallbackCapSeconds)
    result.deprioritize = deprioritizeSecs
    result.source = t('优先级调到最低（成功一次提前恢复）')
  } else {
    let cooling = rule.cooldown === 'protection' ? (settings.backoff ? settings.backoffSeconds : 0) : rule.cooldown === 'none' ? 0 : rule.fallbackSeconds
    if (cooling && (rule.exponential || rule.cooldown === 'protection')) cooling = Math.min(cooling * 2 ** Math.max(0, input.strikes - 1), rule.cooldown === 'protection' ? settings.backoffCapSeconds : rule.fallbackCapSeconds)
    const floor = scenario.kind === 'rate_limit' || scenario.kind === 'quota' ? settings.cooldownFloorSeconds : 0
    result.cooldown = cooling ? Math.max(floor, scenario.seconds || 0, rule.cooldown === 'upstream' && scenario.seconds !== null ? 0 : cooling) : 0
    result.source = result.cooldown ? (scenario.seconds !== null ? t('上游恢复时间') : t('当前规则的兜底时间（不是上游重置时间）')) : t('不冷却')
  }

  const next = () => {
    if (result.deprioritize) {
      result.steps.push({ title: t('优先级调到最低（{time} 后恢复）', { time: duration(result.deprioritize, t) }), detail: `${scopeLabels(t)[scope]} · ${result.source}` })
    } else if (result.cooldown) {
      result.steps.push({ title: t('记录 {time}冷却', { time: duration(result.cooldown, t) }), detail: `${scopeLabels(t)[scope]} · ${result.source}` })
    }
    if (rule.action === 'return_error') return stop(t('当前规则直接返回错误；必要的状态记录保留，不再发送。'))
    if (input.pinned) return stop(t('这份请求已明确指定账号，不擅自更换指定对象；状态记录仍保留。'))
    if (!result.switchesLeft) return stop(t('换号上限已经用完，429 同样受此上限约束。'))
    result.title = scope === 'provider' || rule.action === 'switch_provider' ? t('跳过提供商，继续选路') : t('换账号，继续请求')
    result.steps.push({ title: result.title, detail: t('选择当前路由中可用且不共享本次限制的对象，消耗 1 次换号次数，剩余 {n} 次。', { n: result.switchesLeft - 1 }) })
    return result
  }
  if (rule.action === 'retry_then_switch') {
    const delay = Math.max(settings.intervalSeconds, scenario.seconds || 0)
    const count = Math.min(result.resendsLeft, delay ? Math.floor(settings.waitBudgetSeconds / delay) : result.resendsLeft)
    if (count) result.steps.push({ title: t('原账号最多再发 {n} 次', { n: count }), detail: t('重发间隔 {time}，累计等待不超过 {n} 秒；所有账号共用重发次数。', { time: duration(delay, t), n: settings.waitBudgetSeconds }) })
    else result.steps.push({ title: t('跳过原账号重发'), detail: t('重发次数已用完或等待超限，不缩短上游时间提前发送。') })
  }
  if (rule.action === 'refresh_then_switch' && result.resendsLeft && settings.intervalSeconds <= settings.waitBudgetSeconds) {
    result.steps.push({ title: t('先续期一次'), detail: t('仅使用已授权的有效续期材料；续期后的再发送扣 1 次原账号重发次数。') })
  }
  return next()
}
