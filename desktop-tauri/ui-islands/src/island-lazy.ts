/**
 * 页面岛按需挂载。
 *
 * ── 为什么需要它 ────────────────────────────────────────────
 * 主面板的页面岛由 `island-registry` 一次性 eager 引入，每个模块在自己的
 * 求值末尾直接 `mount()`。结果是**首屏就把十几个页面全部渲染出来**：报表的
 * 图表、账号表、助手、设置、签到……哪怕用户只停在其中一个页面。页面越多，
 * 首屏 CPU、内存和隐藏页面顺手发出的接口请求就越多。
 *
 * 这个模块把「模块加载」和「挂到 DOM」拆开：模块仍然在同一个 bundle 里
 * （IIFE 产物做不了真正的代码分包），但 React root 只在页面第一次被切到时
 * 才创建。页面骨架（`.page[data-page=...]`）由桌面外壳统一渲染，所以选择器
 * 现在就能查到；真正要等的是 `app.js::showPage` 派发的 `wb:page` 事件。
 *
 * ── 为什么不用「当前 section 有 active 类就立即挂」 ────────────
 * 模块求值时 `showPage` 还没跑，外壳默认把 overview 标成 active。若按这个
 * 标记立即挂载，用户上次停在别的页面时 overview 仍会白挂一次。统一等
 * `wb:page` 事件，首屏页和多页切换走同一条路。
 *
 * ── 子面板（设置页里的「版本信息」「更新」）──────────────────
 * 这类挂载点由设置页组件在 React 提交后才创建，事件派发那一刻可能还不存在。
 * 因此 `tryMount` 找不到节点时挂一个 MutationObserver，等节点出现再挂；
 * 切到别的页面时停掉上一个页面的观察器，避免长期挂着。
 */

type PendingMount = {
  selector: string
  mount: () => void
  done: boolean
}

const pending = new Map<string, PendingMount[]>()
const observers = new Map<string, MutationObserver>()
let activePage: string | null = null

/**
 * 登记一个页面岛：`page` 第一次被激活时才执行 `mount`。
 *
 * 可重复调用（热重载 / 模块重复求值）不会重复挂载 —— 同一个页面里每个
 * selector 只认第一次成功的 mount。
 */
export function mountOnPage(page: string, selector: string, mount: () => void): void {
  const entry: PendingMount = { selector, mount, done: false }
  const list = pending.get(page)
  if (list) list.push(entry)
  else pending.set(page, [entry])

  // 兼容「模块在页面已经激活之后才求值」的路径（正常加载顺序不会走到，
  // 但热重载与插件页动态注册时可能发生）。
  if (activePage === page) mountReady(page)
}

function tryMount(entry: PendingMount): void {
  if (entry.done) return
  const node = document.querySelector<HTMLElement>(entry.selector)
  if (!node) return
  entry.done = true
  try {
    entry.mount()
  } catch (error) {
    console.error('[islands] 页面挂载失败:', entry.selector, error)
  }
}

/** 把某一页已登记、且挂载点已出现的岛挂上；还没出现的交给观察器。 */
function mountReady(page: string): void {
  const list = pending.get(page)
  if (!list || list.length === 0) return

  for (const entry of list) tryMount(entry)
  const waiting = list.filter(entry => !entry.done)
  if (waiting.length === 0) {
    pending.delete(page)
    observers.get(page)?.disconnect()
    observers.delete(page)
    return
  }

  // 同一页已经有一个观察器在等，不重复创建。
  if (observers.has(page)) return
  const observer = new MutationObserver(() => {
    for (const entry of waiting) tryMount(entry)
    if (waiting.every(entry => entry.done)) {
      observer.disconnect()
      observers.delete(page)
      pending.delete(page)
    }
  })
  observer.observe(document.body, { childList: true, subtree: true })
  observers.set(page, observer)
}

/** 页面切换：停掉上一页的观察器，再挂当前页已就绪的岛。 */
function activate(page: string): void {
  if (page !== activePage) {
    for (const [key, observer] of observers) {
      if (key === page) continue
      observer.disconnect()
      observers.delete(key)
    }
  }
  activePage = page
  mountReady(page)
}

// `app.js::showPage` 在切换页面时派发；首屏页由 index.tsx 在模块求值末尾
// 按 localStorage 里的页码提前派发一次，其余页面等第一次切到。
window.addEventListener('wb:page', event => {
  const page = (event as CustomEvent<unknown>).detail
  if (typeof page === 'string' && page) activate(page)
})
