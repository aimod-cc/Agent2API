/** 桌面与移动端共用的动态登录元数据解析；只消费网关投影，不执行 provider。 */
import { t } from '../i18n'

export type Prompt = {
  key: string
  type: 'text' | 'select'
  label: string
  placeholder: string
  optional: boolean
  secret: boolean
  default?: string
  when?: { key: string; op: 'eq' | 'neq'; value: string }
  options: Array<{ value: string; label: string }>
}

export type CallbackInput = 'url' | 'code'

export type Method = {
  id: string
  type: 'oauth' | 'api' | 'manual' | 'desktop' | 'guest' | 'unsupported'
  label: string
  methodIndex?: number
  prompts: Prompt[]
  localOnly: boolean
  manualCallback: boolean
  /**
   * 插件要求这条 OAuth 流程只在应用内嵌窗口打开（例如登录页跑在本机回环
   * 地址上的 AStudio）。为真时界面不提供「系统浏览器」选项。
   */
  embeddedOnly: boolean
  callbackInput: CallbackInput
  instructions: string
  reason?: string
}

export type MethodsState = {
  phase: 'loading' | 'ready' | 'legacy' | 'error'
  methods: Method[]
  instructions: string
  reason: string
}

const UNSAFE_KEYS = new Set(['__proto__', 'prototype', 'constructor', 'signal'])
const textOf = (value: unknown): string => typeof value === 'string' ? value : ''
const objectOf = (value: unknown): Record<string, unknown> | undefined =>
  value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : undefined

/** 元数据与进度共用翻译；设备码只作为模板参数，不改动授权内容。 */
export function loginText(value: unknown): string {
  const text = textOf(value)
  const translated = t(text)
  if (translated !== text) return translated
  return text.split('\n').map(line => {
    const deviceCode = /^在 GitHub 输入设备码 (.+)，核对账号与授权权限。设备码过期后需要重新发起登录。$/.exec(line)
    if (deviceCode) return t('在 GitHub 输入设备码 {code}，核对账号与授权权限。设备码过期后需要重新发起登录。', { code: deviceCode[1] })
    const project = /^Google Cloud Project ID（必填，Standard \/ Enterprise 资格由上游判定）(?:；默认候选来自 (GOOGLE_CLOUD_PROJECT(?:_ID)?)，请明确确认)?$/.exec(line)
    if (project) return project[1]
      ? t('Google Cloud Project ID（必填，Standard / Enterprise 资格由上游判定）；默认候选来自 {variable}，请明确确认', { variable: project[1] })
      : t('Google Cloud Project ID（必填，Standard / Enterprise 资格由上游判定）')
    return t(line)
  }).join('\n')
}

function parsePrompt(value: unknown): Prompt {
  const prompt = objectOf(value)
  const key = textOf(prompt?.key)
  if (!prompt || !/^[A-Za-z][A-Za-z0-9_-]{0,63}$/.test(key) || UNSAFE_KEYS.has(key)
    || (prompt.type !== 'text' && prompt.type !== 'select')) {
    throw new Error(t('插件返回了无效的登录字段，请更新插件或联系作者'))
  }
  const options: Prompt['options'] = []
  if (prompt.type === 'select') {
    if (!Array.isArray(prompt.options)) throw new Error(t('插件登录选择字段缺少选项列表'))
    for (const raw of prompt.options) {
      const option = objectOf(raw)
      const value = textOf(option?.value)
      if (!option || !value || options.some(item => item.value === value)) {
        throw new Error(t('插件登录选择字段包含无效或重复的选项'))
      }
      options.push({ value, label: t(textOf(option.label) || value) })
    }
  }
  const condition = objectOf(prompt.when)
  const when = condition && typeof condition.key === 'string' && (condition.op === 'eq' || condition.op === 'neq') && typeof condition.value === 'string'
    ? { key: condition.key, op: condition.op as 'eq' | 'neq', value: condition.value } : undefined
  if (prompt.when !== undefined && !when) throw new Error(t('插件返回了无效的登录字段，请更新插件或联系作者'))
  const secret = prompt.secret === true
  const defaultValue = !secret && typeof prompt.default === 'string' ? prompt.default : undefined
  if (defaultValue !== undefined && prompt.type === 'select' && !options.some(option => option.value === defaultValue)) {
    throw new Error(t('插件返回了无效的登录字段，请更新插件或联系作者'))
  }
  return {
    key, type: prompt.type, label: loginText(textOf(prompt.label) || key),
    placeholder: loginText(prompt.placeholder), optional: prompt.optional === true,
    secret, options, ...(defaultValue === undefined ? {} : { default: defaultValue }), ...(when ? { when } : {}),
  }
}

/** 缺省只代表 URL；非法枚举不能猜成令牌输入。 */
export function parseCallbackInput(value: unknown): CallbackInput {
  if (value === undefined || value === 'url') return 'url'
  if (value === 'code') return 'code'
  throw new Error(t('插件返回了无效的登录方式，请更新插件或联系作者'))
}

export function parseMethods(value: unknown, provider = ''): MethodsState {
  const data = objectOf(value)
  if (!data || !Array.isArray(data.methods)) {
    throw new Error(t('网关没有返回有效的登录方式列表，无法确认该插件的登录能力'))
  }
  const indexes = new Set<number>()
  const methods = data.methods.map((value): Method => {
    const raw = objectOf(value)
    const index = raw?.methodIndex
    if (!raw || typeof index !== 'number' || !Number.isSafeInteger(index) || index < 0
      || indexes.has(index) || (raw.type !== 'oauth' && raw.type !== 'api') || !Array.isArray(raw.prompts)) {
      throw new Error(t('插件返回了无效的登录方式，请更新插件或联系作者'))
    }
    indexes.add(index)
    const prompts = raw.prompts.map(parsePrompt)
    if (new Set(prompts.map(prompt => prompt.key)).size !== prompts.length) {
      throw new Error(t('插件登录方式包含重复的输入字段'))
    }
    if (prompts.some(prompt => prompt.when && !prompts.some(source => source.key === prompt.when?.key))) {
      throw new Error(t('插件返回了无效的登录字段，请更新插件或联系作者'))
    }
    if ((raw.manualCallback !== undefined && typeof raw.manualCallback !== 'boolean')
      || (raw.localOnly !== undefined && typeof raw.localOnly !== 'boolean')
      || (raw.embeddedOnly !== undefined && typeof raw.embeddedOnly !== 'boolean')) {
      throw new Error(t('插件返回了无效的登录方式，请更新插件或联系作者'))
    }
    const callbackInput = parseCallbackInput(raw.callbackInput)
    const manualCallback = raw.manualCallback === true
    const embeddedOnly = raw.embeddedOnly === true
    if ((callbackInput === 'code' && !manualCallback)
      || (raw.type === 'api' && (raw.localOnly === true || manualCallback || embeddedOnly))) {
      throw new Error(t('插件返回了无效的登录方式，请更新插件或联系作者'))
    }
    const callbackContractMissing = provider === 'copilot-subscription' && index === 1
      && (!manualCallback || callbackInput !== 'code')
    return {
      id: `method:${index}`, methodIndex: index, type: raw.type,
      label: t(textOf(raw.label) || (raw.type === 'oauth' ? t('OAuth 登录') : t('API 凭证'))),
      prompts, localOnly: raw.localOnly === true, manualCallback, callbackInput, embeddedOnly,
      instructions: loginText(raw.instructions),
      reason: callbackContractMissing
        ? t('此 Copilot 手动令牌方式未声明回调输入契约，暂不发起；请改用设备码登录或只读本机引用。')
        : prompts.some(prompt => prompt.type === 'select' && !prompt.options.length)
          ? t('该登录方式没有可选项，无法提交；请更新插件或联系作者') : undefined,
    }
  })
  return {
    phase: 'ready', methods, instructions: loginText(data.instructions),
    reason: methods.length ? '' : t('插件没有声明可用的账号登录方式，请更新插件或联系插件作者'),
  }
}

export function methodLabel(method: Method): string {
  if (method.localOnly) return t('{label}（只读本机）', { label: method.label })
  return method.type === 'oauth' ? t('{label}（OAuth）', { label: method.label })
    : method.type === 'api' ? t('{label}（API）', { label: method.label }) : method.label
}

export function promptValue(prompt: Prompt, values: Record<string, string>): string {
  return values[prompt.key] ?? prompt.default ?? ''
}

export function promptEnabled(prompt: Prompt, method: Method, values: Record<string, string>): boolean {
  if (!prompt.when) return true
  const source = method.prompts.find(item => item.key === prompt.when?.key)
  const value = source ? promptValue(source, values) : ''
  const equal = value === prompt.when.value
  return prompt.when.op === 'eq' ? equal : !equal
}

export function readInputs(method: Method, values: Record<string, string>): Record<string, string> {
  const inputs: Record<string, string> = Object.create(null)
  if (method.reason) throw new Error(method.reason)
  for (const prompt of method.prompts) {
    if (!promptEnabled(prompt, method, values)) continue
    const raw = promptValue(prompt, values)
    const value = prompt.secret ? raw : raw.trim()
    if (!value.trim()) {
      if (!prompt.optional) throw new Error(t('请填写 {label}', { label: prompt.label }))
      continue
    }
    if (prompt.type === 'select' && !prompt.options.some(option => option.value === value)) {
      throw new Error(t('请选择 {label} 的有效选项', { label: prompt.label }))
    }
    inputs[prompt.key] = value
  }
  return inputs
}
