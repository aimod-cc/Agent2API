/**
 * 面板认证：登录 / 首次注册 / 人机验证。
 *
 * ── 为什么要自己实现一遍 ALTCHA ───────────────────────────────
 * 桌面网页端用的是 `ui/panel-captcha.js`（自定义元素 `<panel-captcha>`），
 * 那是给 `login.html` 准备的经典脚本，APK 里没有那个页面、也不该为了它去
 * 加载桌面端的整套资源。协议本身很简单（服务端见 `server::altcha`）：
 *
 *   领题 → `{algorithm, challenge, salt, maxnumber, signature}`
 *   枚举 n 使 `sha256(salt + n) === challenge`
 *   提交 base64(JSON({algorithm, challenge, number, salt, signature}))
 *
 * 这里把求解实现成 Promise，不做成自定义元素 —— APK 的登录页是 React 组件，
 * 用状态驱动比 DOM 事件更自然。
 *
 * ── 为什么不用 crypto.subtle 一把梭 ────────────────────────────
 * `crypto.subtle` 只在安全上下文提供。APK 里页面 origin 是 `http://localhost`，
 * **算安全上下文**（localhost 被视为可信），所以这里可以放心用原生实现 ——
 * 与网页端那份「http 局域网要纯 JS 兜底」的顾虑不同。
 * 但为了在浏览器里调试时不炸，仍然保留了纯 JS 兜底。
 */

import { apiCall, url, type PanelStatus } from './client'
import { activeServerId, readServerAuth, storeServerAuth } from '../lib/servers'

/** 题面（服务端 `altcha::challenge()` 的原样输出） */
type Challenge = {
  algorithm: string
  challenge: string
  salt: string
  maxnumber: number
  signature: string
}

/** 求解进度回调（0–1），用于界面上给一点反馈 */
export type SolveProgress = (ratio: number) => void

/* ── SHA-256 ─────────────────────────────────────────────── */

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

/** 纯 JS SHA-256（UTF-8 输入 → 32 字节摘要） */
function sha256Bytes(bytes: Uint8Array): Uint8Array {
  const H = new Int32Array([
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
  ])
  const length = bytes.length
  // 补位：0x80 + 若干 0 + 64 位长度
  const padded = new Uint8Array(((length + 9 + 63) >> 6) << 6)
  padded.set(bytes)
  padded[length] = 0x80
  const bitLength = length * 8
  // 长度只有低 32 位可能非零（题面几十字节，够用）
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

/** 一次 sha256 的 hex 结果（优先原生实现） */
async function sha256Hex(text: string): Promise<string> {
  if (hasNativeCrypto()) {
    const digest = await crypto.subtle.digest('SHA-256', encoder.encode(text))
    return toHex(new Uint8Array(digest))
  }
  return toHex(sha256Bytes(encoder.encode(text)))
}

/* ── 求解 ─────────────────────────────────────────────────── */

/**
 * 枚举 number 使 `sha256(salt + number) === challenge`。
 *
 * 每 2000 次让出一次主线程：纯 JS 路径的 await 只进微任务队列，不让出的话
 * 界面会冻住。原生路径每次 digest 本身就是异步的，天然让出。
 * 服务端上限 5–10 万次，平均命中在一半。
 */
async function solve(challenge: Challenge, onProgress?: SolveProgress): Promise<number> {
  const target = String(challenge.challenge || '').toLowerCase()
  const salt = String(challenge.salt || '')
  const max = Number(challenge.maxnumber) || 0
  if (!target || !salt) throw new Error('题面不完整')
  for (let n = 0; n <= max; n++) {
    if ((await sha256Hex(salt + n)) === target) return n
    if (n % 2000 === 1999) {
      onProgress?.(Math.min(1, n / Math.max(1, max)))
      await new Promise(resolve => setTimeout(resolve, 0))
    }
  }
  throw new Error('未找到答案')
}

/**
 * 领题并求解，返回可直接放进登录请求体的 payload。
 *
 * 服务端关闭校验时领题回 400 —— 这时返回 `null`，调用方提交时不带 captcha 字段。
 */
export async function solveCaptcha(onProgress?: SolveProgress): Promise<string | null> {
  let challenge: Challenge
  try {
    const response = await fetch(url('/api/panel/captcha'), { headers: { Accept: 'application/json' } })
    if (!response.ok) return null
    challenge = (await response.json()) as Challenge
  } catch {
    // 网关不可达：交给登录请求自己报错，不在这里把「验证失败」当结论
    return null
  }
  const number = await solve(challenge, onProgress)
  // 服务端要的是 base64(JSON)（std 与 url-safe 都认，btoa 给的是 std）
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

/* ── 登录 / 注册 ─────────────────────────────────────────── */

export type AuthResult = { access: string; refresh: string } | { error: string }

/**
 * 登录或首次注册。
 *
 * 两者共用一条路径：服务端 `/api/panel/status` 说没注册过就走 setup，
 * 否则走 login。成功后令牌一定在响应体里 —— 请求带了 `x-panel-auth-mode:
 * body`，服务端据此把令牌放进 body（见 `tokens_in_body`）。
 *
 * 为什么必须要求令牌进 body：APK 的页面 origin 与网关不同源，服务端种的
 * HttpOnly cookie 属于网关那个 origin，WebView 的 fetch 跨源时不会带上它
 * （`Allow-Origin: *` 下浏览器拒绝带 cookie）。只有拿 body 里的令牌走请求头
 * 这条路。
 */
export async function authenticate(input: {
  mode: 'login' | 'setup'
  username: string
  password: string
  captcha: string | null
}): Promise<AuthResult> {
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
        // 令牌进响应体（两条通道都给：有的中转会剥自定义头）
        'x-panel-auth-mode': 'body',
      },
      body: JSON.stringify(body),
    })
    const payload = (await response.json().catch(() => null)) as
      | { success?: boolean; data?: { accessToken?: string; refreshToken?: string }; error?: unknown }
      | null
    if (!response.ok) {
      return { error: readError(payload?.error) || `HTTP ${response.status}` }
    }
    const data = payload?.data
    if (!data?.accessToken || !data?.refreshToken) {
      // 服务端没按标记给令牌（老版本网关）：告诉用户升级，而不是让他反复试
      return { error: '网关没有返回登录令牌，请确认网关版本（需要支持 x-panel-auth-mode 的版本）' }
    }
    // 先落盘再返回：调用方紧接着就会用当前服务器 id 去读
    storeServerAuth(activeServerId(), data.accessToken, data.refreshToken)
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

/** 退出登录：撤掉服务端会话（失败也无所谓），再清本地 */
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
    /* 网关不可达也照样清本地 */
  }
}

/** 读当前面板状态（未登录也能调） */
export async function fetchPanelStatus(): Promise<PanelStatus> {
  return apiCall<PanelStatus>('/api/panel/status', { quiet: true })
}
