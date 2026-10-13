/**
 * 面板登录页的机器人校验：领题 → 求解 → 交给页面提交。
 *
 * 协议与服务端实现见 `server::altcha`（题面、答案格式、防重放那边有完整说明）：
 * 网关签发 `{algorithm, challenge, maxnumber, salt, signature}`，客户端枚举 number
 * 使 `sha256(salt + number) === challenge`，把答案连同题面放进 base64(JSON) 提交。
 *
 * ── 为什么不用官方 altcha widget（先前用的是 altcha@1.5.1）─────────
 * 官方实现在**非安全上下文**下开工即抛
 * 「Web Crypto is not available. Secure context is required」：浏览器的
 * `crypto.subtle` 只在 HTTPS / localhost 提供，而局域网面板天然是
 * `http://<内网IP>`（这正是「允许局域网访问」要支持的场景）。结果是组件永远停在
 * 「验证中…」、登录按钮形同失效 —— 浏览器控制台之外看不出任何原因。
 * 本文件自己实现同一套协议：有 `crypto.subtle` 就走原生（HTTPS / localhost，
 * 快且不占主线程算法时间），没有就用内置的纯 JS SHA-256 兜底 —— 两条路径的
 * 数学完全一致，服务端不区分，校验强度也不打折（题目上限 5–10 万次枚举照旧）。
 *
 * ── 与登录页内联脚本的契约（login.html，两边改动要同步）──────────
 *   · 标记是 `<panel-captcha id="captcha">`，由登录岛（login-page.tsx）渲染；
 *   · `verified` 事件的 `detail.payload` = base64(JSON)，提交时放进 body.captcha；
 *   · `expired` 事件 = 当前答案作废（题目到期 / reset / 重新领题）；
 *   · `reset()` 方法：作废答案并重新领题求解（提交失败后由脚本调用）；
 *   · 校验开关关闭时服务端领题回 400，登录页自己把本元素隐藏。
 * 组件只认这三个接口，登录页那侧不必知道求解细节。
 */
(function () {
  'use strict'

  /** 领题端点：与 login.html 的探测、服务端路由三处同名 */
  var CHALLENGE_URL = '/api/panel/captcha'
  /** 题目寿命：服务端 10 分钟过期，提前 30 秒重领，避免踩线提交拿到「已过期」 */
  var REFRESH_MS = 9.5 * 60 * 1000

  var TEXT = {
    idle: wbI18n.t('我不是机器人'),
    verifying: wbI18n.t('验证中…'),
    verified: wbI18n.t('验证成功'),
    error: wbI18n.t('验证失败，请重试'),
  }

  /* ── SHA-256：`crypto.subtle` 不可用时的兜底实现 ──────────────
     标准 FIPS 180-4 实现，一次处理整块输入（题面只有几十字节，够用）。
     只在 http://（非安全上下文）下走到；HTTPS / localhost 一律用原生。 */
  var K = new Int32Array([
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
  ])
  var H0 = new Int32Array([
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
  ])

  function rotr(value, bits) {
    return (value >>> bits) | (value << (32 - bits))
  }

  function sha256(bytes) {
    var length = bytes.length
    // 补位：0x80 + 零填充 + 64 位长度（bit），总长对齐到 64 字节
    var padded = new Uint8Array((((length + 9) >> 6) + 1) << 6)
    padded.set(bytes)
    padded[length] = 0x80
    var view = new DataView(padded.buffer)
    view.setUint32(padded.length - 8, Math.floor(length / 536870912))
    view.setUint32(padded.length - 4, (length << 3) >>> 0)

    var w = new Int32Array(64)
    var h = Int32Array.from(H0)
    for (var offset = 0; offset < padded.length; offset += 64) {
      for (var i = 0; i < 16; i++) w[i] = view.getInt32(offset + i * 4)
      for (var j = 16; j < 64; j++) {
        var x = w[j - 15]
        var y = w[j - 2]
        w[j] = (w[j - 16] + (rotr(x, 7) ^ rotr(x, 18) ^ (x >>> 3)) + w[j - 7]
          + (rotr(y, 17) ^ rotr(y, 19) ^ (y >>> 10))) | 0
      }
      var a = h[0], b = h[1], c = h[2], d = h[3], e = h[4], f = h[5], g = h[6], hh = h[7]
      for (var k = 0; k < 64; k++) {
        var t1 = (hh + (rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25)) + ((e & f) ^ (~e & g)) + K[k] + w[k]) | 0
        var t2 = ((rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22)) + ((a & b) ^ (a & c) ^ (b & c))) | 0
        hh = g; g = f; f = e; e = (d + t1) | 0; d = c; c = b; b = a; a = (t1 + t2) | 0
      }
      h[0] = (h[0] + a) | 0; h[1] = (h[1] + b) | 0; h[2] = (h[2] + c) | 0; h[3] = (h[3] + d) | 0
      h[4] = (h[4] + e) | 0; h[5] = (h[5] + f) | 0; h[6] = (h[6] + g) | 0; h[7] = (h[7] + hh) | 0
    }
    var out = new Uint8Array(32)
    var outView = new DataView(out.buffer)
    for (var m = 0; m < 8; m++) outView.setInt32(m * 4, h[m])
    return out
  }

  function toHex(bytes) {
    var text = ''
    for (var i = 0; i < bytes.length; i++) text += (bytes[i] < 16 ? '0' : '') + bytes[i].toString(16)
    return text
  }

  /** 有没有原生 Web Crypto：只在 HTTPS / localhost 下有 */
  function hasNativeCrypto() {
    return typeof crypto !== 'undefined' && !!crypto.subtle && typeof crypto.subtle.digest === 'function'
  }

  /** `sha256(文本)` → 小写 hex */
  async function sha256Hex(text) {
    var bytes = new TextEncoder().encode(text)
    if (hasNativeCrypto()) {
      var buf = await crypto.subtle.digest('SHA-256', bytes)
      return toHex(new Uint8Array(buf))
    }
    return toHex(sha256(bytes))
  }

  class PanelCaptcha extends HTMLElement {
    constructor() {
      super()
      this.state = 'idle'
      this.payload = null
      /** 求解代次：reset / 重连会 +1，旧的一轮跑完直接丢弃结果 */
      this.generation = 0
      this.timer = null
      this.onClick = () => { void this.start() }
    }

    connectedCallback() {
      if (!this.box) this.render()
      this.addEventListener('click', this.onClick)
      // 页面一加载就领题求解（与迁移前的 widget 同观感：进来就是「验证中…」）
      void this.start()
    }

    disconnectedCallback() {
      this.removeEventListener('click', this.onClick)
      clearTimeout(this.timer)
      this.generation += 1
    }

    /** 作废当前答案并重新领题求解（登录页在提交失败后调用） */
    reset() {
      this.dropPayload()
      void this.start()
    }

    dropPayload() {
      if (this.payload === null) return
      this.payload = null
      this.dispatchEvent(new CustomEvent('expired'))
    }

    /** 领一道题并求解；结果通过 `verified` 事件交给登录页 */
    async start() {
      if (this.state === 'verifying') return
      var generation = ++this.generation
      this.setState('verifying')
      var challenge
      try {
        var response = await fetch(CHALLENGE_URL)
        if (!response.ok) throw new Error('领题失败（HTTP ' + response.status + '）')
        challenge = await response.json()
      } catch (error) {
        if (generation === this.generation) this.fail(error)
        return
      }
      try {
        var number = await this.solve(challenge, generation)
        if (generation !== this.generation) return
        // 服务端要的是 base64(JSON)（std 与 url-safe 都认，btoa 给的是 std）。
        // 字段名与 server::altcha::verify 逐字对应，别改。
        this.payload = btoa(JSON.stringify({
          algorithm: challenge.algorithm,
          challenge: challenge.challenge,
          number: number,
          salt: challenge.salt,
          signature: challenge.signature,
        }))
        this.setState('verified')
        this.dispatchEvent(new CustomEvent('verified', { detail: { payload: this.payload } }))
        // 答案有寿命（服务端 10 分钟）：到点自动重领，省得用户停在过期页面上
        this.timer = setTimeout(() => { this.reset() }, REFRESH_MS)
      } catch (error) {
        if (generation === this.generation) this.fail(error)
      }
    }

    /**
     * 枚举 number 使 `sha256(salt + number) === challenge`。
     *
     * 每 2000 次让出一次主线程：纯 JS 路径的 await 只进微任务队列，不让出的话
     * 转圈动画会被冻住（原生路径每次 digest 本身就是异步的，天然让出）。题目上限
     * 5–10 万次、平均命中在一半，这个量级下一两百毫秒就跑完。
     */
    async solve(challenge, generation) {
      var target = String((challenge && challenge.challenge) || '').toLowerCase()
      var salt = String((challenge && challenge.salt) || '')
      var max = Number(challenge && challenge.maxnumber) || 0
      if (!target || !salt) throw new Error('题面不完整')
      for (var n = 0; n <= max; n++) {
        if (generation !== this.generation) return -1
        if ((await sha256Hex(salt + n)) === target) return n
        if (n % 2000 === 1999) await new Promise((resolve) => setTimeout(resolve, 0))
      }
      throw new Error('未找到答案（maxnumber=' + max + '）')
    }

    fail(error) {
      console.warn('机器人校验失败:', (error && error.message) || error)
      this.setState('error')
    }

    setState(state) {
      this.state = state
      this.setAttribute('state', state)
      if (this.label) this.label.textContent = TEXT[state] || TEXT.idle
    }

    render() {
      // 浅色 DOM（不用 shadow）：样式留在 login.html 的页面样式里，与迁移前
      // 官方 widget 的处理方式一致（那套 CSS 也是从页面层调它的内部 DOM）
      this.innerHTML =
        '<span class="pc-box"><span class="pc-indicator" aria-hidden="true"></span>'
        + '<span class="pc-label"></span></span>'
      this.box = this.querySelector('.pc-box')
      this.label = this.querySelector('.pc-label')
      this.setAttribute('role', 'button')
      this.setAttribute('tabindex', '0')
      this.setState(this.state)
    }
  }

  customElements.define('panel-captcha', PanelCaptcha)
})()
