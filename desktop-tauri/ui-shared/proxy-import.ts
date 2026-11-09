export type ProxyImportDraft = {
  name: string
  protocol: 'http' | 'socks5'
  host: string
  port: number
  username: string
  password: string
  nonDirect: boolean
  enabled: boolean
}

export type ProxyImportReport = {
  added: Array<{ index: number; id: string }>
  duplicates: number[]
  errors: Array<{ index: number; error: string }>
}

export type ProxyImportIssue = { line: number; error: string }
export const PROXY_IMPORT_LIMIT = 500

type Translate = (key: string, values?: Record<string, string | number>) => string

export function proxyAddress(proxy: Pick<ProxyImportDraft, 'protocol' | 'host' | 'port'>): string {
  const host = proxy.host.includes(':') && !proxy.host.startsWith('[') ? `[${proxy.host}]` : proxy.host
  return `${proxy.protocol}://${host}:${proxy.port}`
}

export function proxyIdentity(proxy: ProxyImportDraft): string {
  return JSON.stringify([proxy.protocol, proxy.host.toLowerCase(), proxy.port, proxy.username, proxy.password, proxy.nonDirect])
}

export function proxyHost(value: string, t: Translate): string {
  const host = value.trim()
  if (!host) throw new Error(t('请填写代理主机地址'))
  if (/[\s\u0000-\u001f/@?#\\%]/.test(host) || host.length > 255) {
    throw new Error(t('代理主机只能填写域名或 IP 地址，不能包含协议、端口或路径'))
  }
  const formatted = host.includes(':') && !host.startsWith('[') ? `[${host}]` : host
  try {
    const parsed = new URL(`http://${formatted}:1`)
    if (!parsed.hostname || parsed.port !== '1' || parsed.username || parsed.password) throw new Error()
    return parsed.hostname.replace(/^\[|\]$/g, '')
  } catch {
    throw new Error(t('代理主机地址无效'))
  }
}

function parseLine(raw: string, nonDirect: boolean, t: Translate): ProxyImportDraft {
  let line = raw.trim()
  let name = ''
  const remark = /\{([^{}]*)\}$/.exec(line)
  if (remark) { name = remark[1].trim(); line = line.slice(0, remark.index).trim() }
  // 刷新链接不是代理地址的一部分；导入时忽略，不自动访问。
  line = line.replace(/\[https?:\/\/[^\]]*\]$/i, '').trim()
  const scheme = /^([a-z][a-z\d+.-]*):\/\//i.exec(line)
  const protocol = scheme ? scheme[1].toLowerCase() : 'http'
  if (protocol !== 'http' && protocol !== 'socks5') throw new Error(t('代理协议只支持 HTTP 或 SOCKS5'))
  if (scheme) line = line.slice(scheme[0].length)
  let username = ''
  let password = ''
  const legacy = /^(\[[^\]]+\]|[^:@]+):(\d+):([^:]*):(.*)$/.exec(line)
  if (legacy) {
    username = legacy[3]
    password = legacy[4]
    line = `${legacy[1]}:${legacy[2]}`
  }
  const at = legacy ? -1 : line.lastIndexOf('@')
  const decode = (text: string): string => {
    try { return decodeURIComponent(text) } catch { throw new Error(t('代理账号或密码的 URL 编码无效')) }
  }
  if (at !== -1) {
    const auth = line.slice(0, at)
    const colon = auth.indexOf(':')
    username = decode(colon === -1 ? auth : auth.slice(0, colon))
    password = colon === -1 ? '' : decode(auth.slice(colon + 1))
    line = line.slice(at + 1)
  }
  const match = /^(\[[^\]]+\]|[^:]+):(\d+)\/?$/.exec(line)
  if (!match) throw new Error(t('代理格式无效，请按示例填写'))
  const host = proxyHost(match[1], t)
  const port = Number(match[2])
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error(t('代理端口必须是 1-65535 的整数'))
  if ([...username].length > 200 || [...password].length > 200) throw new Error(t('代理账号和密码不能超过 200 个字符'))
  if (protocol === 'socks5' && (new TextEncoder().encode(username).length > 255 || new TextEncoder().encode(password).length > 255)) {
    throw new Error(t('SOCKS5 用户名和密码不能超过 255 字节'))
  }
  if ([...name].length > 60) throw new Error(t('代理名称不能超过 60 个字符'))
  const draft: ProxyImportDraft = { name, protocol, host, port, username, password, nonDirect, enabled: true }
  if (!name) draft.name = [...proxyAddress(draft)].slice(0, 60).join('')
  return draft
}

export function parseProxyList(text: string, nonDirect: boolean, existing: ProxyImportDraft[], t: Translate): {
  items: ProxyImportDraft[]; issues: ProxyImportIssue[]; remaining: string; duplicates: number
} {
  const items: ProxyImportDraft[] = []
  const issues: ProxyImportIssue[] = []
  const remaining: string[] = []
  const seen = new Set(existing.map(proxyIdentity))
  let duplicates = 0
  for (const [index, raw] of text.split(/\r?\n/).entries()) {
    if (!raw.trim()) continue
    try {
      const proxy = parseLine(raw, nonDirect, t)
      const key = proxyIdentity(proxy)
      if (seen.has(key)) { duplicates += 1; continue }
      if (existing.length + items.length >= PROXY_IMPORT_LIMIT) throw new Error(t('每次最多添加 {n} 条代理', { n: PROXY_IMPORT_LIMIT }))
      seen.add(key)
      items.push(proxy)
    } catch (error) {
      remaining.push(raw)
      issues.push({ line: index + 1, error: error instanceof Error ? error.message : String(error) })
    }
  }
  return { items, issues, remaining: remaining.join('\n'), duplicates }
}
