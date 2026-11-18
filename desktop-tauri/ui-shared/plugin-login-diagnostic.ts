export type LoginDiagnostic = {
  stage: string
  method: string
  url: string
  status?: number
  code?: string
  requestId?: string
  route?: 'direct' | 'proxy'
  headers: Record<string, string>
  body: string
  truncated: boolean
  redacted: boolean
  bodyNote?: string
}

function recordOf(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : undefined
}

export function readLoginDiagnostic(value: unknown): LoginDiagnostic | undefined {
  const record = recordOf(value)
  if (!record || typeof record.stage !== 'string' || !['token_exchange', 'token_refresh', 'identity_keys'].includes(record.stage)
    || !['GET', 'POST'].includes(String(record.method)) || typeof record.url !== 'string' || record.url.length > 2048
    || typeof record.body !== 'string' || record.body.length > 32768 || typeof record.truncated !== 'boolean'
    || typeof record.redacted !== 'boolean') return undefined
  let url: URL
  try { url = new URL(record.url) } catch { return undefined }
  if (url.protocol !== 'https:' || url.hostname !== 'auth.openai.com' || url.port && url.port !== '443'
    || url.username || url.password || url.search || url.hash) return undefined
  const headers = recordOf(record.headers)
  if (!headers || Object.keys(headers).length > 8 || Object.entries(headers).some(([name, value]) =>
    !['content-type', 'x-request-id', 'request-id', 'cf-ray', 'cf-mitigated', 'server', 'date', 'retry-after'].includes(name)
    || typeof value !== 'string' || value.length > 1024)) return undefined
  const text = (key: string, limit = 512) => typeof record[key] === 'string' && record[key].length <= limit
    ? record[key] as string : undefined
  return {
    stage: record.stage, method: record.method as string, url: record.url,
    status: typeof record.status === 'number' && Number.isInteger(record.status) && record.status >= 100 && record.status < 600
      ? record.status : undefined,
    code: text('code', 128), requestId: text('requestId'),
    route: record.route === 'direct' || record.route === 'proxy' ? record.route : undefined,
    headers: Object.fromEntries(Object.entries(headers)) as Record<string, string>, body: record.body,
    truncated: record.truncated, redacted: record.redacted, bodyNote: text('bodyNote', 64),
  }
}

export function loginDiagnosticOf(error: unknown, depth = 0): LoginDiagnostic | undefined {
  if (depth > 3) return undefined
  const record = recordOf(error)
  if (!record) return undefined
  return readLoginDiagnostic(record.diagnostic) || loginDiagnosticOf(record.payload, depth + 1)
    || loginDiagnosticOf(record.data, depth + 1) || loginDiagnosticOf(record.error, depth + 1)
}

export function loginFailure(message: string, source: unknown): Error & { diagnostic?: LoginDiagnostic } {
  const error: Error & { diagnostic?: LoginDiagnostic } = new Error(message)
  const diagnostic = loginDiagnosticOf(source)
  if (diagnostic) error.diagnostic = diagnostic
  return error
}
