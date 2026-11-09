const EXCHANGE_PATH = "/deepai_code/desktop_auth/exchange/"

export function loginNetwork(provider, authorization) {
  if (provider.id !== "deepaicode" || !authorization.url) return null
  const url = new URL(authorization.url)
  if (url.pathname !== "/deepai_code/desktop_auth/") return null
  return { exchangeUrl: new URL(EXCHANGE_PATH, url.origin).href, hint: "", error: "" }
}

export function observeLoginRequest(context, url, method) {
  const login = context.loginNetwork
  if (!login || method.toUpperCase() !== "POST" || url.href !== login.exchangeUrl) return false
  login.hint = "已收到浏览器授权，正在获取账号凭据…"
  login.error = ""
  return true
}

export function loginNetworkError(context) {
  if (context.signal.aborted || !context.loginNetwork) return
  context.loginNetwork.error = context.proxy
    ? "浏览器授权已收到，但通过所选代理获取账号凭据失败。请检查该代理后重新登录。"
    : "浏览器授权已收到，但网关直连 DeepAI 失败。请在登录代理中填入系统代理或选择可用代理后重新登录。"
}
