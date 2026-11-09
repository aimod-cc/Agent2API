import { randomUUID } from "node:crypto"
import { rpc } from "./wire.mjs"
import { scope, untilCancelled } from "./fetch.mjs"

const MAX_MESSAGE = 2 * 1024 * 1024
const MAX_BUFFERED = 8 * 1024 * 1024

class GatewayWebSocket extends EventTarget {
  static CONNECTING = 0
  static OPEN = 1
  static CLOSING = 2
  static CLOSED = 3
  onopen = null
  onmessage = null
  onerror = null
  onclose = null
  #id = randomUUID()
  #url
  #protocol = ""
  #state = 0
  #binaryType = "blob"
  #buffered = 0
  #tail = Promise.resolve()
  #context
  #timer
  #abort
  #dispose

  constructor(address, protocolsOrOptions, options = {}) {
    super()
    const url = new URL(String(address))
    if (url.protocol === "http:") url.protocol = "ws:"
    if (url.protocol === "https:") url.protocol = "wss:"
    if (!["ws:", "wss:"].includes(url.protocol) || url.username || url.password || url.hash) throw new DOMException("Invalid WebSocket URL", "SyntaxError")
    if (protocolsOrOptions && typeof protocolsOrOptions === "object" && !Array.isArray(protocolsOrOptions)) {
      options = protocolsOrOptions
      protocolsOrOptions = options.protocols
    }
    const protocols = protocolsOrOptions === undefined ? [] : typeof protocolsOrOptions === "string" ? [protocolsOrOptions] : protocolsOrOptions
    if (!Array.isArray(protocols) || protocols.length > 32 || new Set(protocols).size !== protocols.length
      || protocols.some(value => typeof value !== "string" || !/^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/.test(value) || value.length > 256)) {
      throw new DOMException("Invalid WebSocket protocols", "SyntaxError")
    }
    if (options.proxy !== undefined || options.tls !== undefined || options.rejectUnauthorized !== undefined || options.perMessageDeflate) {
      throw new Error("WebSocket proxy and TLS settings are managed by the gateway")
    }
    const context = scope.getStore()
    if (!context || context.closed) throw new Error("Plugin WebSocket requires an active request")
    context.signal.throwIfAborted()
    const headers = new Headers(options.headers)
    if (context.userAgent) headers.set("user-agent", context.userAgent)
    for (const name of ["x-panel-token", "x-magpie-work-dir", "x-magpie-work-git", "x-magpie-fallback", "x-magpie-session", "x-magpie-subagent"]) headers.delete(name)
    this.#context = context
    this.#url = url.href
    this.#abort = () => this.#finish(1006, "", false)
    this.#dispose = () => this.#finish(1001, "", false)
    context.signal.addEventListener("abort", this.#abort, { once: true })
    context.sockets.add(this.#dispose)
    void this.#run(protocols, Object.fromEntries(headers)).catch(() => this.#fail())
  }

  get CONNECTING() { return 0 }
  get OPEN() { return 1 }
  get CLOSING() { return 2 }
  get CLOSED() { return 3 }
  get url() { return this.#url }
  get protocol() { return this.#protocol }
  get extensions() { return "" }
  get readyState() { return this.#state }
  get bufferedAmount() { return this.#buffered }
  get binaryType() { return this.#binaryType }
  set binaryType(value) { if (["blob", "arraybuffer", "nodebuffer"].includes(value)) this.#binaryType = value }

  #emit(event) {
    scope.run(this.#context, () => {
      const handler = this[`on${event.type}`]
      if (typeof handler === "function") {
        try { handler.call(this, event) } catch { console.error("Plugin WebSocket event handler failed") }
      }
      this.dispatchEvent(event)
    })
  }

  async #run(protocols, headers) {
    const opening = rpc("script.ws.open", { id: this.#id, url: this.#url, protocols, headers, proxy: this.#context.proxy ?? null })
    let head
    try { head = await untilCancelled(opening, this.#context.signal) }
    catch (error) {
      void opening.then(() => rpc("script.ws.abort", { id: this.#id })).catch(() => {})
      throw error
    }
    if (this.#state !== 0) { await rpc("script.ws.abort", { id: this.#id }).catch(() => {}); return }
    this.#protocol = head.protocol || ""
    this.#state = 1
    this.#emit(new Event("open"))
    while (this.#state !== 3) {
      const part = await untilCancelled(rpc("script.ws.read", { id: this.#id }), this.#context.signal)
      if (this.#state === 3) return
      if (part.type === "pending") continue
      if (part.type === "close") { this.#finish(part.code, part.reason || "", part.clean === true); return }
      if (part.type === "error") { this.#fail(); return }
      if (this.#state !== 1) continue
      let data = part.data
      if (part.type === "binary") {
        const bytes = Buffer.from(part.data, "base64")
        data = this.#binaryType === "nodebuffer" ? bytes : this.#binaryType === "arraybuffer"
          ? bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) : new Blob([bytes])
      }
      this.#emit(new MessageEvent("message", { data, origin: new URL(this.#url).origin }))
    }
  }

  send(value) {
    if (this.#state === 0) throw new DOMException("WebSocket is not open", "InvalidStateError")
    if (this.#state !== 1) return
    let data
    let size
    let type
    if (typeof value === "string") { type = "text"; data = value; size = Buffer.byteLength(value) }
    else if (value instanceof Blob) { type = "binary"; data = value; size = value.size }
    else if (value instanceof ArrayBuffer || ArrayBuffer.isView(value)) {
      type = "binary"
      data = Buffer.from(value instanceof ArrayBuffer ? new Uint8Array(value) : new Uint8Array(value.buffer, value.byteOffset, value.byteLength))
      size = data.byteLength
    } else { type = "text"; data = String(value); size = Buffer.byteLength(data) }
    if (size > MAX_MESSAGE || this.#buffered + size > MAX_BUFFERED) {
      this.#fail()
      throw new Error("WebSocket send buffer exceeded its limit")
    }
    this.#buffered += size
    this.#tail = this.#tail.then(async () => {
      if (this.#state === 3) return
      if (data instanceof Blob) data = Buffer.from(await data.arrayBuffer())
      await rpc("script.ws.send", { id: this.#id, type, data: type === "binary" ? data.toString("base64") : data })
    }).catch(() => this.#fail()).finally(() => { this.#buffered = Math.max(0, this.#buffered - size) })
  }

  close(code = 1000, reason = "") {
    if (!Number.isInteger(code) || (code !== 1000 && (code < 3000 || code > 4999))) throw new DOMException("Invalid WebSocket close code", "InvalidAccessError")
    reason = String(reason)
    if (Buffer.byteLength(reason) > 123) throw new DOMException("WebSocket close reason is too long", "SyntaxError")
    if (this.#state >= 2) return
    if (this.#state === 0) { this.#finish(1006, "", false); return }
    this.#state = 2
    this.#timer = setTimeout(() => this.#finish(1006, "", false), 10_000)
    this.#timer.unref()
    this.#tail = this.#tail.then(() => {
      if (this.#state !== 3) return rpc("script.ws.send", { id: this.#id, type: "close", code, reason })
    }).catch(() => this.#fail())
  }

  #fail() {
    if (this.#state === 3) return
    this.#emit(new Event("error"))
    this.#finish(1006, "", false)
  }

  #finish(code, reason, clean) {
    if (this.#state === 3) return
    this.#state = 3
    clearTimeout(this.#timer)
    this.#context.signal.removeEventListener("abort", this.#abort)
    this.#context.sockets?.delete(this.#dispose)
    const closed = rpc("script.ws.abort", { id: this.#id }).catch(() => {})
    const event = new Event("close")
    Object.defineProperties(event, { code: { value: code }, reason: { value: reason }, wasClean: { value: clean } })
    this.#emit(event)
    return closed
  }
}

export function installWebSocket() { globalThis.WebSocket = GatewayWebSocket }
