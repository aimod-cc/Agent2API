import { scope } from "./fetch.mjs"

export function safeMessage(error, context = scope.getStore()) {
  let message = String(error?.message || error || "Script plugin failed")
  if (context?.localProvider) {
    message = message.replace(/stderr tail:[\s\S]*/i, "stderr tail: [redacted]")
      .replace(/(["']?(?:prompt|messages|content|system|tool_result)["']?\s*[:=])[\s\S]*/i, "$1 [redacted]")
  }
  const values = [context?.auth, context?.expect, context?.loginAuth]
  const secrets = new Set(context?.inputSecrets ?? [])
  for (const value of values) {
    for (const record of [value, value?.metadata]) {
      for (const field of ["access", "refresh", "key", "accessToken", "refreshToken", "cookie", "sid", "password"]) {
        if (context?.localProvider && field === "key" && record?.[field] === "agy") continue
        if (typeof record?.[field] === "string" && record[field]) secrets.add(record[field])
      }
    }
  }
  if (context?.proxy?.password) secrets.add(context.proxy.password)
  for (const secret of [...secrets].sort((left, right) => right.length - left.length)) {
    if (secret) message = message.replaceAll(secret, "[redacted]")
  }
  message = message.replace(/([?&](?:code|authorization_code|pt_key|authKey|access_token|refresh_token|token)=)[^&\s]+/gi, "$1[redacted]")
    .replace(/((?:wps_sid|cookie|authorization|x-api-key)\s*[:=]\s*)[^\r\n;]+/gi, "$1[redacted]")
  return message.slice(0, 2000)
}
