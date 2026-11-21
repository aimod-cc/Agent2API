import { localizedText } from "./localization.mjs"
import { applies } from "./login-prompts.mjs"
import { untilCancelled } from "./fetch.mjs"

const WPS_SITE = "https://lingxi.wps.cn/"
const LOCAL_CLIENT_HINT = "读取的是网关电脑上已登录的客户端，不是打开面板的电脑；也可改用粘贴凭据。"
export const DEFAULT_FILE_ACCEPT = ".json,.info,application/json,text/plain"
export const MAX_TEXT_FILE_BYTES = 262144

const str = value => typeof value === "string" ? value.trim() : ""
const atMs = (at, inSec, now) => Number(at) > 0 ? (Number(at) < 1e11 ? Number(at) * 1000 : Number(at))
  : Number(inSec) > 0 ? now + Number(inSec) * 1000 : 0

function jwtExpMs(token) {
  const parts = str(token).split(".")
  if (parts.length !== 3) return 0
  try {
    const exp = Number(JSON.parse(Buffer.from(parts[1], "base64url").toString("utf8"))?.exp)
    return Number.isFinite(exp) && exp > 0 ? (exp < 1e11 ? exp * 1000 : exp) : 0
  } catch { return 0 }
}

function findCredentialObject(root) {
  const seen = new Set()
  const queue = [{ node: root, depth: 0, parent: null }]
  while (queue.length) {
    const { node, depth, parent } = queue.shift()
    if (!node || typeof node !== "object" || seen.has(node)) continue
    seen.add(node)
    if (Array.isArray(node)) {
      if (depth < 4) for (const item of node) queue.push({ node: item, depth: depth + 1, parent })
      continue
    }
    if (node.auth && typeof node.auth === "object" && ("accessToken" in node.auth || "refreshToken" in node.auth || "access_token" in node.auth)) {
      return { node: node.auth, account: node.account && typeof node.account === "object" ? node.account : node }
    }
    const hasToken = ["access", "accessToken", "access_token", "refresh", "refreshToken", "refresh_token", "apiKey", "key", "ptKey", "token"]
      .some(key => str(node[key]).length > 0)
    if (hasToken) return { node, account: parent && !Array.isArray(parent) ? parent : null }
    if (depth < 4) {
      for (const key of ["pluginSecret", "data", "accounts", "account", ...Object.keys(node)]) {
        if (key in node) queue.push({ node: node[key], depth: depth + 1, parent: node })
      }
    }
  }
  return null
}

function prepareRaccoonInput(rawText) {
  const text = str(rawText)
  if (!text.startsWith("{") && !text.startsWith("[")) return text
  try {
    const parsed = JSON.parse(text)
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed) && (str(parsed.access_token) || str(parsed.accessToken))) return text
    const hit = findCredentialObject(parsed)
    if (!hit) return text
    const access = str(hit.node.access_token ?? hit.node.accessToken ?? hit.node.access ?? hit.node.token)
    const refresh = str(hit.node.refresh_token ?? hit.node.refreshToken ?? hit.node.refresh)
    if (access) return JSON.stringify({ access_token: access, refresh_token: refresh })
  } catch {}
  return text
}

async function enrichCommandCodeKey(key, info, signal) {
  const cleanKey = str(key)
  if (!cleanKey) return { type: "failed", error: "请输入有效的 Command Code API Key" }
  let uid = str(info.uid)
  let email = str(info.email)
  let name = str(info.name)
  try {
    const response = await fetch("https://api.commandcode.ai/alpha/whoami", {
      headers: { Authorization: `Bearer ${cleanKey}`, "x-api-key": cleanKey, Accept: "application/json" },
      signal: signal ? AbortSignal.any([signal, AbortSignal.timeout(8_000)]) : AbortSignal.timeout(8_000),
    })
    if (response.status === 401 || response.status === 403) {
      return { type: "failed", error: "Command Code 拒绝了该 API Key，请核对密钥是否有效" }
    }
    if (response.ok) {
      const user = (await response.json().catch(() => null))?.user
      if (user && typeof user === "object") {
        uid = str(user.id) || uid
        email = str(user.email) || str(user.userName) || email
        name = str(user.userName) || str(user.email) || name
      }
    }
  } catch {}
  const who = name || email || uid
  const metadata = { ...info.metadata, ...(who ? { email: who } : {}), ...(uid ? { userId: uid } : {}) }
  return {
    type: "success", key: cleanKey,
    ...(uid ? { uid } : {}), ...(who ? { name: who, accountId: who } : {}), ...(email ? { email } : {}),
    metadata,
  }
}

export async function importCredential(provider, rawText, options = {}) {
  const text = str(rawText)
  if (!text) return { type: "failed", error: "请提供有效的凭据内容" }
  const id = provider.originalID || provider.id
  if (typeof provider.hooks.auth?.import === "function") {
    if (provider.hooks.auth?.cli) {
      const { ensureLocalCliReady } = await import("./local-provider.mjs")
      await ensureLocalCliReady(provider, undefined, options.signal, untilCancelled)
    }
    const imported = await provider.hooks.auth.import([text], { signal: options.signal })
    const list = Array.isArray(imported) ? imported : imported ? [imported] : []
    const hit = list.find(item => item?.type === "success")
    if (hit) return hit
    const failed = list.find(item => typeof item?.error === "string" && item.error.trim())
    return { type: "failed", error: failed?.error || "未能从凭据文件中解析出有效账号" }
  }
  if (id === "raccoon") {
    const paste = (provider.hooks.auth?.methods ?? []).find(method =>
      method.type === "oauth" && typeof method.authorize === "function" && /粘贴|Token|callback/i.test(String(method.label || "")))
    if (paste) {
      const authorization = await paste.authorize({}, { signal: options.signal })
      if (authorization?.type === "failed") return authorization
      if (typeof authorization?.callback === "function") {
        return authorization.callback(prepareRaccoonInput(text), { signal: options.signal })
      }
    }
  }
  if (text.startsWith("{") || text.startsWith("[")) {
    let parsed
    try { parsed = JSON.parse(text) }
    catch { return { type: "failed", error: "凭据 JSON 格式无效，请检查内容是否完整" } }
    const secret = parsed && typeof parsed === "object" && !Array.isArray(parsed) && parsed.pluginSecret
      && typeof parsed.pluginSecret === "object" && !Array.isArray(parsed.pluginSecret) ? parsed.pluginSecret : null
    if (secret) {
      if ("refresh" in secret && typeof secret.access === "string" && Number.isFinite(secret.expires)) {
        return { ...secret, type: "success" }
      }
      if (typeof secret.key === "string" && secret.key.trim() && id !== "commandcode-plan") {
        return { ...secret, type: "success" }
      }
    }
    const hit = findCredentialObject(parsed)
    if (!hit) return { type: "failed", error: "凭据 JSON 中未找到有效的 Token 或 API Key" }
    const { node, account } = hit
    const root = parsed && typeof parsed === "object" && !Array.isArray(parsed) ? parsed : {}
    const md = { ...(typeof root.metadata === "object" ? root.metadata : {}), ...(typeof node.metadata === "object" ? node.metadata : {}) }
    const pub = typeof root.pluginPublic === "object" && root.pluginPublic ? root.pluginPublic : {}
    const access = str(node.access ?? node.accessToken ?? node.access_token)
    const refresh = str(node.refresh ?? node.refreshToken ?? node.refresh_token)
    const apiKey = str(node.apiKey ?? node.key ?? node.ptKey ?? (!refresh ? (node.token ?? node.accessToken ?? node.access_token) : ""))
    const uid = str(node.uid ?? node.userId ?? node.user_id ?? node.realUserID ?? md.userId ?? account?.uid ?? account?.userId ?? pub.uid ?? root.uid ?? root.userId)
    const email = str(node.email ?? md.email ?? node.userName ?? account?.email ?? pub.email ?? root.email)
    const name = str(node.name ?? node.userName ?? node.username ?? node.nickname ?? node.accountId ?? node.keyName ?? md.name ?? md.email ?? account?.nickname ?? account?.name ?? pub.name ?? root.name)
    const isOAuth = Boolean(refresh || (access && !str(node.apiKey ?? node.key ?? node.ptKey) && typeof provider.hooks.auth?.refresh === "function"))
    if (!isOAuth && (apiKey || access)) {
      const key = apiKey || access
      const metadata = {
        ...md, ...(options.inputs ?? {}),
        ...(uid ? { userId: uid } : {}), ...(email ? { email } : {}), ...(name ? { name } : {}),
        ...(str(node.keyName ?? root.keyName) ? { keyName: str(node.keyName ?? root.keyName) } : {}),
        ...(str(node.plan ?? root.plan) ? { plan: str(node.plan ?? root.plan) } : {}),
        ...(str(node.planId ?? root.planId) ? { planId: str(node.planId ?? root.planId) } : {}),
      }
      delete metadata.key
      delete metadata.file
      if (id === "commandcode-plan") return enrichCommandCodeKey(key, { uid, email, name, metadata }, options.signal)
      return {
        type: "success", key,
        ...(uid ? { uid } : {}), ...(name ? { name, accountId: name } : {}), ...(email ? { email } : {}),
        metadata,
      }
    }
    if (access || refresh) {
      const expires = atMs(node.expires ?? node.expiresAt ?? pub.expiresAt ?? root.expiresAt, node.expiresIn ?? node.expires_in, Date.now()) || jwtExpMs(access) || 0
      const domain = str(node.domain ?? account?.domain ?? root.domain)
      const enterpriseId = str(node.enterpriseId ?? account?.enterpriseId ?? root.enterpriseId)
      return {
        type: "success", access, refresh, expires,
        ...(uid ? { uid } : {}), ...(name ? { name, accountId: name } : {}), ...(email ? { email } : {}),
        ...(domain ? { domain } : {}), ...(enterpriseId ? { enterpriseId } : {}),
        ...(Object.keys(md).length ? { metadata: md } : {}),
      }
    }
    return { type: "failed", error: "凭据 JSON 中未找到有效的 Token 或 API Key" }
  }
  const metadata = { ...(options.inputs ?? {}) }
  delete metadata.key
  delete metadata.file
  if (id === "commandcode-plan") return enrichCommandCodeKey(text, { uid: "", email: "", name: "", metadata }, options.signal)
  return { type: "success", key: text, metadata }
}

export function methodFile(method, prompts, dynamicPrompts) {
  if (method.type !== "api" || dynamicPrompts || method.localOnly === true || method.manualCallback === true
    || method.embeddedOnly === true || !method.file) return undefined
  const raw = typeof method.file === "object" && !Array.isArray(method.file) ? method.file : {}
  const candidate = typeof raw.field === "string" ? raw.field.trim() : ""
  const prompt = prompts.find(item => item.type === "text" && !item.when && (!candidate || item.key === candidate))
    || prompts.find(item => item.type === "text" && !item.when)
  if (!prompt) return undefined
  const accept = typeof raw.accept === "string" && raw.accept.length <= 512 && !/[\u0000-\u001f\u007f]/.test(raw.accept)
    ? raw.accept : DEFAULT_FILE_ACCEPT
  const maxBytes = Number.isSafeInteger(raw.maxBytes) && raw.maxBytes > 0 && raw.maxBytes <= MAX_TEXT_FILE_BYTES
    ? raw.maxBytes : MAX_TEXT_FILE_BYTES
  return { field: prompt.key, accept, maxBytes, binary: false }
}

export function loginMethods(provider, locale = "zh-CN") {
  const original = provider.hooks.auth?.methods ?? []
  const methods = original.map((method, methodIndex) => ({ ...method, methodIndex,
    label: localizedText(method.label, method.labels, locale) }))
  if (provider.id === "wps-lingxi") {
    const paste = methods.find(method => method.type === "oauth" && method.prompts?.some(prompt => prompt.key === "cookie"))
    for (const method of methods) {
      method.browserMode = method.prompts?.length ? "none" : /导入|客户端/.test(method.label) ? "none" : "plugin"
      if (method.browserMode === "plugin") {
        method.label = "由插件打开浏览器登录"
        method.instructions = "插件会在网关电脑上启动独立浏览器并读取登录结果，不使用系统默认浏览器。"
      } else if (!method.prompts?.length) method.localOnly = true
    }
    if (paste && typeof paste.authorize === "function") {
      methods.unshift({
        methodIndex: original.length, type: "oauth", label: "内嵌窗口登录", embeddedOnly: true,
        desktopOnly: true, cookieCapture: "wps-lingxi", browserMode: "choice",
        instructions: "在应用内嵌窗口完成 WPS 登录，登录凭据仅交给原插件校验并保存到当前网关。",
        async authorize(_inputs, options) {
          return { url: WPS_SITE, method: "code", instructions: this.instructions,
            async callback(cookie) {
              const authorization = await paste.authorize({ cookie }, options)
              if (authorization?.type === "failed") return authorization
              if (typeof authorization?.callback !== "function") throw new Error("WPS 插件未返回凭据校验流程")
              return authorization.callback(undefined, options)
            },
          }
        },
      })
    }
  } else if (["intern-inkstone", "mclaw"].includes(provider.id)) {
    // 这两家的登录与续期依赖插件自己的浏览器环境，不能替换成宿主开页。
    for (const method of methods) if (method.type === "oauth" && !method.embeddedOnly
      && (!method.browserMode || method.browserMode === "choice")) method.browserMode = "plugin"
  } else if (provider.id === "agnes-code") {
    for (const method of methods) if (method.type === "oauth") {
      method.localOnly = true
      method.browserMode = "none"
      method.instructions = LOCAL_CLIENT_HINT
    }
  }
  for (const method of methods) {
    if (provider.id === "deepaicode" && method.type === "oauth") {
      method.instructions = "浏览器显示登录完成后，网关还需联网获取账号凭据。若浏览器或 Magpie 使用系统代理，请先在下方点击「填入系统代理」；选择直连不会跟随系统代理。"
    }
    if (method.type === "api" && (method.keyless || provider.id === "deepaicode" && method.prompts?.some(prompt => prompt.key === "dir"))) {
      method.instructions = LOCAL_CLIENT_HINT
    }
  }
  const id = provider.originalID || provider.id
  const hasFileMethod = methods.some(method => method.type === "api" && method.file)
  if (!hasFileMethod) {
    const hasImportHook = typeof provider.hooks.auth?.import === "function"
    const hasRaccoonPaste = id === "raccoon" && methods.some(method =>
      method.type === "oauth" && typeof method.authorize === "function" && /粘贴|Token|callback/i.test(String(method.label || "")))
    const isCommandCode = id === "commandcode-plan"
    if (hasImportHook || hasRaccoonPaste || isCommandCode) {
      const nextIndex = methods.reduce((max, item) => Math.max(max, item.methodIndex), -1) + 1
      const instructions = hasImportHook
        ? localizedText(provider.hooks.auth.importFrom, provider.hooks.auth.importFroms, locale)
        : hasRaccoonPaste
          ? "选择或粘贴商汤小浣熊的凭据 JSON（含 access_token / refresh_token）、导出的账号文件或 JWT access_token。"
          : "选择或粘贴 Command Code 的 ~/.commandcode/auth.json、导出的账号 JSON 或 API Key。"
      methods.push({
        methodIndex: nextIndex, type: "api", label: "导入凭据文件", instructions: instructions || "",
        prompts: [{ key: "file", type: "text", label: "凭据内容", optional: false, secret: true }],
        file: { field: "file", accept: DEFAULT_FILE_ACCEPT, maxBytes: MAX_TEXT_FILE_BYTES, binary: false },
        authorize: (inputs, options) => importCredential(provider, inputs?.file ?? inputs?.key ?? "", { ...options, inputs }),
      })
    }
  }
  return methods
}

export function promptOptional(provider, method, prompt) {
  if (typeof prompt.optional === "boolean") return prompt.optional
  if (typeof prompt.required === "boolean") return !prompt.required
  if (provider.id === "deepaicode" && method.prompts?.some(item => item.key === "dir")) return true
  if (provider.id === "cloudflare-workers-ai" && prompt.key === "accountId") return false
  if (typeof prompt.validate === "function") {
    try {
      const result = prompt.validate("")
      if (result && typeof result.then === "function") { void result.catch(() => {}); return false }
      return !result
    } catch { return false }
  }
  return /^(username|tenant)$/i.test(prompt.key)
}

export function promptSecret(prompt) {
  if (prompt.type === "password") return true
  if (typeof prompt.secret === "boolean") return prompt.secret
  return prompt.type !== "select" && /key|token|secret|password|cookie|credential|sid/i.test(prompt.key)
}

export async function validateInputs(method, inputs) {
  for (const prompt of method.prompts ?? []) {
    if (!applies(prompt, inputs)) continue
    if (typeof prompt.validate !== "function") continue
    const error = await prompt.validate(inputs[prompt.key] ?? "")
    if (typeof error === "string" && error) throw new Error(error)
  }
}

const loopback = host => ["127.0.0.1", "localhost", "[::1]"].includes(host)

export function authorizationPolicy(provider, method, authorization) {
  const url = authorization.url ? new URL(authorization.url) : null
  const targets = []
  if (url && authorization.method === "auto") {
    for (const key of ["redirect_uri", "redirect_url", "callback_url", "auth_callback_url", "return_url"]) {
      try {
        const target = new URL(url.searchParams.get(key))
        if (target.protocol === "http:" && loopback(target.hostname) && target.port && !target.username && !target.password) targets.push(target)
      } catch {}
    }
    if (provider.id === "deepaicode" && url.pathname === "/deepai_code/desktop_auth/") {
      const port = url.searchParams.get("port")
      if (/^\d+$/.test(port) && +port > 0 && +port < 65536) {
        const target = new URL(`http://127.0.0.1:${port}/auth-callback`)
        target.searchParams.set("state", url.searchParams.get("state") || "")
        targets.push(target)
      }
    }
    if (provider.id === "joycode" && url.hostname === "joycode.jd.com") {
      const port = url.searchParams.get("authPort")
      if (/^\d+$/.test(port) && +port > 0 && +port < 65536) {
        const target = new URL(`http://127.0.0.1:${port}/`)
        for (const key of ["authKey", "authPort"]) target.searchParams.set(key, url.searchParams.get(key) || "")
        targets.push(target)
      }
    }
  }
  const hosts = provider.id === "wps-lingxi" ? ["wps.cn", "kdocs.cn"]
    : provider.id === "joycode" ? ["jd.com", "jd.com.cn"] : []
  return {
    targets,
    ui: {
      browserMode: authorization.opened === true ? "plugin" : method.browserMode || "choice",
      manualCallback: !method.cookieCapture && (authorization.method === "code" || targets.length > 0),
      callbackInput: authorization.method === "code" && !method.cookieCapture ? "code" : "url",
      cookieCapture: method.cookieCapture || null,
      loopbackTargets: targets.map(target => target.href),
    },
    allowedHosts: targets.length ? [] : hosts,
  }
}

export function checkedCallback(targets, raw) {
  const url = new URL(raw)
  const target = targets.find(target => url.protocol === "http:" && loopback(url.hostname)
    && url.port === target.port && url.pathname === target.pathname && !url.username && !url.password)
  if (!target) throw new Error("回调地址不属于本次登录的本机接收端")
  for (const [key, value] of target.searchParams) {
    if (key === "state" && !url.searchParams.has(key)) continue
    if (url.searchParams.get(key) !== value) throw new Error("回调地址与当前登录任务不匹配")
  }
  url.hostname = target.hostname
  return url.href
}
