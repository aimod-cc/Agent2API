/*
 * Agent2API 手机端 Service Worker。
 *
 * ── 为什么放在 `ui/` 根、而不是 `ui/m/` ──────────────────────
 * SW 的默认 scope 上限是它自己所在的目录：`ui/m/sw.js` 最多只能控制
 * `/m/*`，而手机端的入口是 `/m.html` —— 它落在 scope 之外，注册成 `/`
 * 会被浏览器拒绝（`The path of the provided scope is not under the max
 * scope`），注册成 `/m/` 又管不到入口页。放在 `ui/` 根目录就没有这个
 * 矛盾：`/sw.js` 天然可以 scope 到 `/`。
 *
 * 构建脚本（vite.config.ts 的 swBuildStamp）把 `mobile/public/sw.js`
 * 复制到这里并写入时间戳，所以这个文件是产物 —— 改它请改源文件。
 *
 * ── 只拦截手机端，桌面端一字未变 ─────────────────────────────
 * scope 是 `/`，但下面的 fetch 处理只认 `/m` 与 `/m/*`，其余请求直接
 * return（浏览器按原样处理）。桌面面板与 `/api/*` 都不受影响。
 *
 * ── 三条策略，按资源性质分 ────────────────────────────────────
 *   · **入口页**（`/m`、`/m.html`）：network-first + 离线回退缓存。
 *     不能 stale-while-revalidate —— 入口页决定加载哪个版本的 JS，
 *     先给旧的再在后台更新会让「新 JS + 旧 HTML」错配一轮。
 *   · **外壳**（mobile.js / mobile.css / manifest / 图标）：stale-while-revalidate。
 *     首屏秒开，后台拉新版留给下次打开。
 *   · **数据**（`/api/*`、`/v1/*`）：**完全不碰**。网关数据必须实时，
 *     缓存一份只会让用户看到过期读数却以为是真的。
 *
 * ── 版本 ────────────────────────────────────────────────────
 * SW 的更新判据是文件内容。产物变了但 sw.js 不变时浏览器不会重装 SW，
 * 旧缓存会一直生效 —— 时间戳让每次构建都必然更新。
 * 新 SW 装好后不立刻 skipWaiting，而是等页面发消息 —— 否则用户正在
 * 填的表单会被一次自动刷新清掉。页面侧在设置页给「重启以更新」按钮。
 */

const CACHE = 'agent2api-mobile-5894f3e8fbc1'

/** 预缓存的外壳文件（相对 SW 所在目录，即 `ui/` 根） */
const SHELL = [
  './m.html',
  './m/mobile.js',
  './m/mobile.css',
  './m/manifest.webmanifest',
  './assets/app-icon-192.png',
  './assets/app-icon-512.png',
  './assets/app-icon-maskable-512.png',
]

/** 这个请求属不属于手机端（决定要不要拦） */
function isMobileAsset(pathname) {
  if (pathname === '/m' || pathname === '/m.html') return true
  return pathname.startsWith('/m/')
}

/** 是不是入口页导航（`/m` 或 `/m.html`，含 query / hash 之外的路径部分） */
function isEntryNavigation(request, url) {
  if (request.mode !== 'navigate') return false
  return url.pathname === '/m' || url.pathname === '/m.html'
}

/**
 * 预缓存一个文件。
 *
 * 不用 `cache.add` 是因为它会**跟随重定向**：未登录时 `/m.html` 会 302 到
 * `/login`，`cache.add` 会把那张登录页存成入口页 —— 之后离线打开就是登录页。
 * 这里显式取一次，跟随了重定向的响应直接丢弃（留给下次在线访问时再存）。
 */
async function precache(cache, item) {
  try {
    const response = await fetch(item, { cache: 'no-store' })
    if (response.ok && !response.redirected) await cache.put(item, response)
  } catch {
    /* 单个文件失败不影响其余（比如图标缺失） */
  }
}

self.addEventListener('install', event => {
  event.waitUntil(
    caches
      .open(CACHE)
      // 逐个而不是 addAll：任何一个文件缺失不该让整次预缓存失败
      .then(cache => Promise.all(SHELL.map(item => precache(cache, item))))
      .then(() => {
        // 不在这里 skipWaiting：等页面决定何时切换（见 message 分支）
      }),
  )
})

self.addEventListener('activate', event => {
  event.waitUntil(
    caches
      .keys()
      .then(keys => Promise.all(keys.filter(key => key !== CACHE).map(key => caches.delete(key))))
      .then(() => self.clients.claim()),
  )
})

/**
 * 页面发来的消息。
 *
 * `SKIP_WAITING`：用户点了「重启以更新」。新 SW 立刻接管，页面随后自己
 * reload 拿到新外壳。
 */
self.addEventListener('message', event => {
  const data = event.data
  if (data && data.type === 'SKIP_WAITING') {
    void self.skipWaiting()
  }
})

self.addEventListener('fetch', event => {
  const request = event.request
  if (request.method !== 'GET') return

  const url = new URL(request.url)
  if (url.origin !== self.location.origin) return
  // 桌面面板与接口一律不碰
  if (!isMobileAsset(url.pathname)) return

  // 入口页：network-first，离线回退缓存
  if (isEntryNavigation(request, url)) {
    event.respondWith(
      fetch(request)
        .then(response => {
          // 只缓存「直接命中」的响应。会话过期时服务器会 302 到 /login，
          // fetch 跟随重定向后拿到的是一张 200 的登录页 —— 存进入口页缓存
          // 的话，下次离线打开看到的就是登录页（而离线根本登不了）。
          if (response.ok && !response.redirected) {
            const copy = response.clone()
            void caches
              .open(CACHE)
              .then(cache => cache.put('./m.html', copy))
              .catch(() => undefined)
          }
          return response
        })
        .catch(async () => {
          const cached = await caches.match('./m.html')
          if (cached) return cached
          // 连外壳都没有（首次访问就断网）：给一句可读的话，别让浏览器报网络错误
          return new Response(
            '<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">' +
              '<body style="font:15px/1.7 -apple-system,system-ui,sans-serif;padding:40px 24px;color:#333">' +
              '<h1 style="font-size:18px;margin:0 0 8px">离线</h1>' +
              '<p style="margin:0;color:#666">连不上网关。确认手机与网关在同一局域网，然后刷新。</p>' +
              '</body>',
            { headers: { 'Content-Type': 'text/html; charset=utf-8' } },
          )
        }),
    )
    return
  }

  const isShell = SHELL.some(item => url.pathname.endsWith(item.slice(1)))
  if (isShell) {
    // 外壳：stale-while-revalidate —— 首屏秒开，后台拉新版留给下次
    event.respondWith(
      caches.open(CACHE).then(async cache => {
        const cached = await cache.match(request)
        const network = fetch(request)
          .then(response => {
            if (response.ok) void cache.put(request, response.clone())
            return response
          })
          .catch(() => cached)
        return cached || network
      }),
    )
    return
  }

  // 手机端其余资源：network-first，离线时回退缓存
  event.respondWith(
    fetch(request).catch(async () => {
      const cached = await caches.match(request)
      if (cached) return cached
      throw new Error('offline')
    }),
  )
})
