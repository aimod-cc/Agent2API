import { apiCall, url, type PanelStatus } from './client'
import { activeServerId, readServerAuth, serverIdentity, storeServerAuth } from './servers'

type Challenge = {
  algorithm: string
  challenge: string
  salt: string
  maxnumber: number
  signature: string
}

export type SolveProgress = (ratio: number) => void

const K = new Int32Array([
  0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
  0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
  0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
  0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
  0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
  0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
  0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
  0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
])

function rotr(value: number, bits: number): number {
  return (value >>> bits) | (value << (32 - bits))
}

function sha256Bytes(bytes: Uint8Array): Uint8Array {
  const H = new Int32Array([
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
  ])
  const length = bytes.length
  const padded = new Uint8Array(((length + 9 + 63) >> 6) << 6)
  padded.set(bytes)
  padded[length] = 0x80
  const bitLength = length * 8
  const view = new DataView(padded.buffer)
  view.setUint32(padded.length - 4, bitLength >>> 0, false)
  view.setUint32(padded.length - 8, Math.floor(bitLength / 0x100000000), false)

  const w = new Int32Array(64)
  for (let offset = 0; offset < padded.length; offset += 64) {
    for (let i = 0; i < 16; i++) w[i] = view.getInt32(offset + i * 4, false)
    for (let i = 16; i < 64; i++) {
      const s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >>> 3)
      const s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >>> 10)
      w[i] = (w[i - 16] + s0 + w[i - 7] + s1) | 0
    }
    let a = H[0]
    let b = H[1]
    let c = H[2]
    let d = H[3]
    let e = H[4]
    let f = H[5]
    let g = H[6]
    let h = H[7]
    for (let i = 0; i < 64; i++) {
      const S1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25)
      const ch = (e & f) ^ (~e & g)
      const t1 = (h + S1 + ch + K[i] + w[i]) | 0
      const S0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22)
      const maj = (a & b) ^ (a & c) ^ (b & c)
      const t2 = (S0 + maj) | 0
      h = g
      g = f
      f = e
      e = (d + t1) | 0
      d = c
      c = b
      b = a
      a = (t1 + t2) | 0
    }
    H[0] = (H[0] + a) | 0
    H[1] = (H[1] + b) | 0
    H[2] = (H[2] + c) | 0
    H[3] = (H[3] + d) | 0
    H[4] = (H[4] + e) | 0
    H[5] = (H[5] + f) | 0
    H[6] = (H[6] + g) | 0
    H[7] = (H[7] + h) | 0
  }

  const out = new Uint8Array(32)
  const outView = new DataView(out.buffer)
  for (let i = 0; i < 8; i++) outView.setInt32(i * 4, H[i], false)
  return out
}

function toHex(bytes: Uint8Array): string {
  let text = ''
  for (const byte of bytes) text += byte.toString(16).padStart(2, '0')
  return text
}

function hasNativeCrypto(): boolean {
  return typeof crypto !== 'undefined' && Boolean(crypto.subtle) && typeof crypto.subtle.digest === 'function'
}

const encoder = new TextEncoder()

async function sha256Hex(text: string): Promise<string> {
  if (hasNativeCrypto()) {
    const digest = await crypto.subtle.digest('SHA-256', encoder.encode(text))
    return toHex(new Uint8Array(digest))
  }
  return toHex(sha256Bytes(encoder.encode(text)))
}

async function solve(challenge: Challenge, onProgress?: SolveProgress): Promise<number> {
  const target = String(challenge.challenge || '').toLowerCase()
  const salt = String(challenge.salt || '')
  const max = Number(challenge.maxnumber) || 0
  if (!target || !salt) throw new Error('验证数据不完整')
  for (let n = 0; n <= max; n++) {
    if ((await sha256Hex(salt + n)) === target) return n
    if (n % 2000 === 1999) {
      onProgress?.(Math.min(1, n / Math.max(1, max)))
      await new Promise(resolve => setTimeout(resolve, 0))
    }
  }
  throw new Error('验证计算超时')
}

export async function solveCaptcha(onProgress?: SolveProgress): Promise<string | null> {
  const response = await fetch(url('/api/panel/captcha'), {
    headers: { Accept: 'application/json' },
    cache: 'no-store',
  })
  const payload = await response.json()
  if (!response.ok) {
    if (response.status === 400 && readError(payload?.error) === '机器人校验未启用') return null
    throw new Error(readError(payload?.error) || '无法获取登录校验信息')
  }
  const challenge = payload as Challenge
  const number = await solve(challenge, onProgress)
  return btoa(
    JSON.stringify({
      algorithm: challenge.algorithm,
      challenge: challenge.challenge,
      number,
      salt: challenge.salt,
      signature: challenge.signature,
    }),
  )
}

export type AuthResult = { access: string; refresh: string } | { error: string }

export async function authenticate(input: {
  mode: 'login' | 'setup'
  username: string
  password: string
  captcha: string | null
}): Promise<AuthResult> {
  const identity = serverIdentity()
  const serverId = activeServerId()
  const body: Record<string, string> = {
    username: input.username,
    password: input.password,
  }
  if (input.captcha) body.captcha = input.captcha

  const path = input.mode === 'setup' ? '/api/panel/setup' : '/api/panel/login'
  try {
    const response = await fetch(url(path), {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Accept: 'application/json',
        'x-panel-auth-mode': 'body',
      },
      body: JSON.stringify(body),
    })
    const payload = (await response.json().catch(() => null)) as
      | {
          success?: boolean
          data?: { accessToken?: string; refreshToken?: string; expiresIn?: number }
          error?: unknown
        }
      | null
    if (!response.ok) {
      return { error: readError(payload?.error) || `HTTP ${response.status}` }
    }
    const data = payload?.data
    if (!data?.accessToken || !data?.refreshToken) {
      return { error: '网关没有返回登录令牌，请确认网关版本' }
    }
    if (serverIdentity() !== identity) return { error: '服务器已切换，请重新登录' }
    storeServerAuth(serverId, data.accessToken, data.refreshToken, data.expiresIn)
    return { access: data.accessToken, refresh: data.refreshToken }
  } catch (error) {
    return { error: `无法连接网关：${error instanceof Error ? error.message : String(error)}` }
  }
}

function readError(error: unknown): string {
  if (typeof error === 'string') return error
  if (error && typeof error === 'object') {
    const message = (error as { message?: unknown }).message
    if (typeof message === 'string') return message
  }
  return ''
}

export async function logout(): Promise<void> {
  const auth = readServerAuth(activeServerId())
  const headers: Record<string, string> = {
    Accept: 'application/json',
    'x-panel-auth-mode': 'body',
  }
  if (auth?.access) {
    headers['x-panel-token'] = auth.access
    headers.Authorization = `Bearer ${auth.access}`
  }
  if (auth?.refresh) headers['x-panel-refresh'] = auth.refresh
  try {
    await fetch(url('/api/panel/logout'), { method: 'POST', headers, body: '{}' })
  } catch {
    /* ignore */
  }
}

export async function fetchPanelStatus(): Promise<PanelStatus> {
  return apiCall<PanelStatus>('/api/panel/status', { quiet: true })
}

export async function startOauthLoginUrl(
  provider: 'google' | 'github',
  origin: string,
): Promise<{ state: string; authorizeUrl: string }> {
  return apiCall<{ state: string; authorizeUrl: string }>('/api/panel/oauth/start-url', {
    method: 'POST',
    body: { provider, origin },
    quiet: true,
  })
}

export async function pollOauthLoginState(state: string): Promise<{
  status: 'pending' | 'done' | 'error'
  ticket?: string
  identity?: string
  provider?: string
  error?: string
}> {
  return apiCall(`/api/panel/oauth/poll?state=${encodeURIComponent(state)}`, { quiet: true })
}

export async function exchangeOauthTicket(ticket: string): Promise<AuthResult> {
  const identity = serverIdentity()
  const serverId = activeServerId()
  try {
    const response = await fetch(url('/api/panel/oauth/exchange'), {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Accept: 'application/json',
        'x-panel-auth-mode': 'body',
      },
      body: JSON.stringify({ ticket }),
    })
    const payload = (await response.json().catch(() => null)) as
      | {
          success?: boolean
          data?: { accessToken?: string; refreshToken?: string; expiresIn?: number }
          error?: unknown
        }
      | null
    if (!response.ok) {
      return { error: readError(payload?.error) || `HTTP ${response.status}` }
    }
    const data = payload?.data
    if (!data?.accessToken || !data?.refreshToken) {
      return { error: '网关没有返回登录令牌，请重试' }
    }
    if (serverIdentity() !== identity) return { error: '服务器已切换，请重新登录' }
    storeServerAuth(serverId, data.accessToken, data.refreshToken, data.expiresIn)
    return { access: data.accessToken, refresh: data.refreshToken }
  } catch (error) {
    return { error: `无法连接网关：${error instanceof Error ? error.message : String(error)}` }
  }
}
