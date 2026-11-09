import { promptOptional, promptSecret, loginMethods } from "./login-policy.mjs"
import { bounded } from "./entries.mjs"
import { scope } from "./fetch.mjs"

const safeKey = key => typeof key === "string" && /^[A-Za-z][A-Za-z0-9_-]{0,63}$/.test(key)
  && !["__proto__", "constructor", "prototype", "signal"].includes(key)

export function applies(prompt, inputs) {
  const rule = prompt.when ?? (typeof prompt.condition === "object" ? prompt.condition : null)
  if (rule) {
    if (!Object.hasOwn(inputs, rule.key)) return false
    const equal = inputs[rule.key] === rule.value
    if (rule.op === "eq" ? !equal : equal) return false
  }
  return typeof prompt.condition !== "function" || Boolean(prompt.condition(inputs))
}

export function promptView(provider, method, prompt) {
  if (!safeKey(prompt.key)) throw new Error("Plugin returned an invalid login field")
  const condition = prompt.when ?? (typeof prompt.condition === "object" ? prompt.condition : null)
  return {
    key: prompt.key, type: prompt.type === "select" ? "select" : "text",
    label: prompt.message || prompt.label || prompt.key, placeholder: prompt.placeholder || "",
    optional: promptOptional(provider, method, prompt), secret: promptSecret(prompt),
    options: (prompt.options ?? []).map(option => ({ value: option.value, label: option.label || option.value })),
    ...(!promptSecret(prompt) && typeof prompt.default === "string" ? { default: prompt.default } : {}),
    ...(condition ? { when: condition } : {}),
  }
}

export function keyPrompt(method) {
  return method.type === "api" && method.keyless !== true && typeof method.ask !== "function"
    && (!(method.prompts ?? []).length || typeof method.authorize !== "function")
    && !(method.prompts ?? []).some(prompt => prompt.key === "key")
    ? { key: "key", type: "text", label: "API Key", placeholder: method.placeholder || "", optional: false, secret: true, options: [] } : null
}

export async function resolvePrompts(provider, params) {
  const method = loginMethods(provider).find(method => method.methodIndex === params.methodIndex)
  if (!method) throw new Error("Unknown sign-in method")
  const raw = params.inputs ?? {}
  if (!raw || typeof raw !== "object" || Array.isArray(raw) || Object.keys(raw).length > 64
    || Object.entries(raw).some(([key, value]) => !safeKey(key) || typeof value !== "string" || value.length > 262144)) {
    throw new Error("Invalid sign-in fields")
  }
  const inputs = { ...raw }
  const context = scope.getStore()
  if (context) context.inputSecrets = Object.values(inputs).filter(Boolean)
  const prompts = []
  const key = keyPrompt(method)
  if (key) prompts.push(key)
  for (const prompt of method.prompts ?? []) {
    if (!applies(prompt, inputs)) continue
    if (!Object.hasOwn(inputs, prompt.key) && !promptSecret(prompt) && typeof prompt.default === "string") inputs[prompt.key] = prompt.default
    prompts.push({ ...promptView(provider, method, prompt), when: undefined })
  }
  if (typeof method.ask === "function") {
    const next = await bounded(() => method.ask(inputs), 10_000, "Provider login fields")
    if (next && !prompts.some(prompt => prompt.key === next.key)) prompts.push(promptView(provider, method, next))
  }
  return { prompts }
}
