import net from "node:net"
import tls from "node:tls"
import http2 from "node:http2"
import { Duplex } from "node:stream"
import { once } from "node:events"
import { scope } from "./fetch.mjs"

async function exact(socket, size) {
  for (;;) {
    const data = socket.read(size)
    if (data) return data
    if (socket.destroyed || socket.readableEnded) throw new Error("Proxy closed the connection")
    await new Promise((resolve, reject) => {
      const ready = () => { clear(); resolve() }
      const failed = error => { clear(); reject(error) }
      const closed = () => failed(new Error("Proxy closed the connection"))
      const clear = () => { socket.off("readable", ready); socket.off("error", failed); socket.off("close", closed); socket.off("end", closed) }
      socket.once("readable", ready); socket.once("error", failed); socket.once("close", closed); socket.once("end", closed)
    })
  }
}

async function connect(proxy, host, port, signal) {
  signal?.throwIfAborted()
  const address = { host: proxy.host.replace(/^\[|\]$/g, ""), port: proxy.port }
  const socket = proxy.protocol === "https" ? tls.connect({ ...address, servername: net.isIP(address.host) ? undefined : address.host }) : net.connect(address)
  const abort = () => socket.destroy(signal.reason instanceof Error ? signal.reason : new Error("Request cancelled"))
  signal?.addEventListener("abort", abort, { once: true })
  socket.setTimeout(30_000, () => socket.destroy(new Error("Proxy connection timed out")))
  try {
    await once(socket, proxy.protocol === "https" ? "secureConnect" : "connect")
    if (["socks5", "socks5h"].includes(proxy.protocol)) {
      socket.write(Buffer.from(proxy.username ? [5, 2, 0, 2] : [5, 1, 0]))
      const hello = await exact(socket, 2)
      if (hello[0] !== 5 || ![0, 2].includes(hello[1])) throw new Error("SOCKS proxy rejected authentication")
      if (hello[1] === 2) {
        const user = Buffer.from(proxy.username || ""), password = Buffer.from(proxy.password || "")
        if (user.length > 255 || password.length > 255) throw new Error("SOCKS credentials are too long")
        socket.write(Buffer.concat([Buffer.from([1, user.length]), user, Buffer.from([password.length]), password]))
        const authenticated = await exact(socket, 2)
        if (authenticated[1] !== 0) throw new Error("SOCKS proxy rejected authentication")
      }
      const target = Buffer.from(host)
      if (target.length > 255) throw new Error("SOCKS target is too long")
      socket.write(Buffer.concat([Buffer.from([5, 1, 0, 3, target.length]), target, Buffer.from([port >> 8, port & 255])]))
      const response = await exact(socket, 4)
      if (response[0] !== 5 || response[1] !== 0) throw new Error(`SOCKS proxy connection failed (${response[1]})`)
      const length = response[3] === 1 ? 4 : response[3] === 4 ? 16 : response[3] === 3 ? (await exact(socket, 1))[0] : -1
      if (length < 0) throw new Error("Invalid SOCKS proxy response")
      await exact(socket, length + 2)
    } else {
      const target = `${host.includes(":") ? `[${host}]` : host}:${port}`
      let request = `CONNECT ${target} HTTP/1.1\r\nHost: ${target}\r\n`
      if (proxy.username) request += `Proxy-Authorization: Basic ${Buffer.from(`${proxy.username}:${proxy.password || ""}`).toString("base64")}\r\n`
      socket.write(request + "\r\n")
      let response = ""
      while (!response.endsWith("\r\n\r\n")) {
        response += (await exact(socket, 1)).toString("latin1")
        if (response.length > 16384) throw new Error("Proxy response headers exceeded their limit")
      }
      if (!/^HTTP\/\d(?:\.\d)? 2\d\d\b/.test(response)) throw new Error("Proxy refused the HTTP tunnel")
    }
    socket.setTimeout(0)
    return socket
  } catch (error) { socket.destroy(); throw error }
  finally { signal?.removeEventListener("abort", abort) }
}

function tunnel(proxy, host, port, signal) {
  let socket, ready = false, ended = false
  const queued = []
  const stream = new Duplex({
    read() { socket?.resume() },
    write(chunk, encoding, callback) { if (ready) socket.write(chunk, callback); else queued.push([chunk, callback]) },
    final(callback) { ended = true; if (ready) socket.end(callback); else callback() },
    destroy(error, callback) {
      signal?.removeEventListener("abort", abort)
      socket?.destroy()
      for (const [, done] of queued.splice(0)) done(error || new Error("Tunnel closed"))
      callback(error)
    },
  })
  const abort = () => stream.destroy(signal.reason instanceof Error ? signal.reason : new Error("Request cancelled"))
  signal?.addEventListener("abort", abort, { once: true })
  if (signal?.aborted) abort()
  void connect(proxy, host, port, signal).then(value => {
    socket = value
    if (stream.destroyed) { socket.destroy(); return }
    socket.on("data", chunk => { if (!stream.push(chunk)) socket.pause() })
    socket.on("end", () => stream.push(null))
    socket.on("error", error => stream.destroy(error))
    ready = true
    for (const [chunk, callback] of queued.splice(0)) socket.write(chunk, callback)
    if (ended) socket.end()
  }).catch(error => stream.destroy(error))
  return stream
}

export function installHTTP2() {
  const original = http2.connect
  http2.connect = function (authority, options, listener) {
    if (typeof options === "function") { listener = options; options = undefined }
    const context = scope.getStore()
    const proxy = context?.proxy
    const url = new URL(typeof authority === "string" ? authority : authority.href)
    const host = url.hostname.replace(/^\[|\]$/g, "")
    if (!proxy || options?.createConnection || ["localhost", "127.0.0.1", "::1"].includes(host)) return original.call(this, authority, options, listener)
    const raw = tunnel(proxy, host, Number(url.port) || (url.protocol === "https:" ? 443 : 80), context.closed ? context.lifetime : context.signal)
    const createConnection = () => url.protocol === "http:" ? raw : tls.connect({ ...options, socket: raw,
      servername: options?.servername || (net.isIP(host) ? undefined : host), ALPNProtocols: ["h2"] })
    const session = original.call(this, authority, { ...options, createConnection }, listener)
    raw.on("error", error => session.destroy(error))
    return session
  }
}
