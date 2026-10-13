/**
 * 分组模式下的**顺序拖拽**（组顺位 / 组内顺位）—— 指针事件实现，不引拖拽库。
 *
 * ── 为什么是指针事件，不是 HTML5 拖放 ─────────────────────────
 * 与列宽拖拽、列设置面板的排序同一取舍：拖拽过程中列表每 2 秒会被连接数轮询重绘
 * 一次，而 HTML5 拖放的会话绑在**那个节点**上，一重绘就断；指针事件的状态只在
 * JS 变量里，节点被 React 换掉也不影响会话（本模块每帧从 DOM 现查一次落点）。
 *
 * ── 落点提示为什么不插节点 ──────────────────────────────────
 * tbody 归 React 管，往里塞一个自造的 `<tr>` 会在下一次 diff 时被清掉。这里改成
 * 在**目标行上加一个类**（`.drop-before` / `.drop-after`），插入线由 CSS 的
 * inset box-shadow 画 —— 不动布局、不产生高度跳变，也不用与 React 抢节点。
 * 类能留在 DOM 上，靠的是那些行的 className 由 React 计算且拖动期间不变
 * （React 只在 prop 值变化时才写 DOM）。
 *
 * ── 本模块只报「把谁放到了第几位」────────────────────────────
 * 目标顺序由调用方（页面）从分组数据算：折叠起来的组在 DOM 里没有行，光看 DOM
 * 拼不出完整队列，而重排接口要求「全部账号的一个排列」（见 store_order.rs）。
 *
 * ── 什么时候不给拖 ─────────────────────────────────────────
 * 筛选 / 搜索生效时顺序不可见（行被裁过、组内序号也断了连续性），拖动必然错位。
 * 这时 `isLocked()` 为真：不开始拖，回调 onLockedAttempt 让页面提示一次。
 */

/** 拖完的落点：`index` 是目标位置（0 起，已在被拖项之外的那一串里数） */
export type QueueDrop =
  | { kind: 'group'; provider: string; index: number }
  | { kind: 'row'; provider: string; accountId: string; index: number }

export type QueueDragHandlers = {
  /** 顺序是否被锁住（筛选 / 搜索进行中） */
  isLocked(): boolean
  /** 锁着的时候用户还是按下了把手：给一次提示（由页面决定怎么说） */
  onLockedAttempt(): void
  /** 拖完了：把新位置交回去，顺序怎么写由调用方决定 */
  onDrop(drop: QueueDrop): void
}

type DragState = {
  kind: 'group' | 'row'
  provider: string
  accountId: string
  item: HTMLElement
  container: HTMLElement
  items: HTMLElement[]
  /** 当前落点（在被拖项之外的那一串里的下标） */
  index: number
}

/** 拖动中的行 / 组压淡（与列设置面板排序时同一手法） */
const DRAGGING_CLASS = 'dragging'
/** 落点提示的两个类：插在这一项之前 / 之后 */
const DROP_CLASSES = ['drop-before', 'drop-after']

let drag: DragState | null = null

function clearDropMarks(container: ParentNode): void {
  container.querySelectorAll(`.${DROP_CLASSES[0]}, .${DROP_CLASSES[1]}`).forEach(element => {
    element.classList.remove(...DROP_CLASSES)
  })
}

/** 一组「候选落点」：同级别的兄弟项（组 = 各 tbody，行 = 同一组内的各 tr） */
function siblingsOf(item: HTMLElement, kind: 'group' | 'row'): HTMLElement[] {
  if (kind === 'group') {
    const list = item.parentElement
    return list ? ([...list.children].filter(element =>
      element.classList.contains('acct-group')) as HTMLElement[]) : []
  }
  const tbody = item.parentElement
  return tbody ? ([...tbody.children].filter(element =>
    element.classList.contains('acct-row')) as HTMLElement[]) : []
}

/**
 * 指针落在哪一段：拿每一项的中线比，返回**在其余项里的插入下标**。
 *
 * 与「拖到哪一行」的直觉对齐：越过某项的中线就算排到它前面 / 后面，
 * 与列设置的拖动排序同一口径。
 */
function insertionIndex(others: HTMLElement[], clientY: number): number {
  for (let index = 0; index < others.length; index += 1) {
    const rect = others[index].getBoundingClientRect()
    if (clientY < rect.top + rect.height / 2) return index
  }
  return others.length
}

function paintDropMark(state: DragState): void {
  const others = state.items.filter(item => item !== state.item)
  clearDropMarks(state.container)
  if (!others.length) return
  if (state.index >= others.length) others[others.length - 1].classList.add(DROP_CLASSES[1])
  else others[state.index].classList.add(DROP_CLASSES[0])
}

function onPointerMove(event: PointerEvent): void {
  if (!drag) return
  if (event.buttons === 0) { finish(false); return }
  const others = drag.items.filter(item => item !== drag!.item)
  const next = insertionIndex(others, event.clientY)
  if (next === drag.index) return
  drag.index = next
  paintDropMark(drag)
}

function finish(drop: boolean): void {
  const state = drag
  if (!state) return
  drag = null
  window.removeEventListener('pointermove', onPointerMove)
  window.removeEventListener('pointerup', onPointerUp)
  window.removeEventListener('pointercancel', onPointerCancel)
  window.removeEventListener('keydown', onKeyDown)
  document.body.classList.remove('queue-dragging')
  state.item.classList.remove(DRAGGING_CLASS)
  clearDropMarks(state.container)
  if (!drop) return
  if (state.kind === 'group') {
    handlersRef?.onDrop({ kind: 'group', provider: state.provider, index: state.index })
  } else {
    handlersRef?.onDrop({ kind: 'row', provider: state.provider, accountId: state.accountId, index: state.index })
  }
}

function onPointerUp(): void { finish(true) }
function onPointerCancel(): void { finish(false) }

/** Esc = 放弃这次拖动（与列设置面板的排序同一手感） */
function onKeyDown(event: KeyboardEvent): void {
  if (event.key === 'Escape') finish(false)
}

/** 最近一次 bind 的处理器（finish 在监听回调里跑，拿不到闭包变量） */
let handlersRef: QueueDragHandlers | null = null

/**
 * 绑定到滚动容器上（**委托**，一次即可）：拖拽结束后 React 会把 tbody 重画一遍，
 * 绑在具体行上的监听会随之失效，绑在容器上则一直有效 —— 与列宽拖拽同一理由。
 */
export function bindQueueDrag(host: HTMLElement | null, handlers: QueueDragHandlers): void {
  handlersRef = handlers
  if (!host) return
  // 同一个宿主只绑一次：调用方（页面）的 effect 可能在别处重跑，重复 addEventListener
  // 会积下多份监听 —— 起拖有 drag 守卫不会真的拖两次，但那是隐患不是设计
  if (host.dataset.queueDragBound === '1') return
  host.dataset.queueDragBound = '1'
  host.addEventListener('pointerdown', event => {
    if (event.button !== 0 || drag) return
    const target = event.target as Element | null
    const groupGrip = target?.closest?.('.queue-grip')
    const rowGrip = target?.closest?.('.order-grip')
    if (!groupGrip && !rowGrip) return
    event.preventDefault()
    if (handlers.isLocked()) {
      handlers.onLockedAttempt()
      return
    }
    const item = (groupGrip ?? rowGrip)!.closest<HTMLElement>('tbody.acct-group, tr.acct-row')
    if (!item) return
    const kind: 'group' | 'row' = groupGrip ? 'group' : 'row'
    const provider = item.dataset.provider
      || item.closest<HTMLElement>('tbody.acct-group')?.dataset.provider
      || ''
    const accountId = item.dataset.id || ''
    if (kind === 'row' && !accountId) return
    const items = siblingsOf(item, kind)
    if (items.length < 2) return
    // 落点下标一律按「**除被拖项之外**那一串」的数法（与 `insertionIndex` 同尺度，
    // 否则初始值与后续计算会差一位 —— 被拖项自己也在 items 里）
    const others = items.filter(candidate => candidate !== item)
    drag = {
      kind, provider, accountId, item,
      container: item.parentElement || host,
      items,
      index: insertionIndex(others, event.clientY),
    }
    item.classList.add(DRAGGING_CLASS)
    document.body.classList.add('queue-dragging')
    window.addEventListener('pointermove', onPointerMove)
    window.addEventListener('pointerup', onPointerUp)
    window.addEventListener('pointercancel', onPointerCancel)
    window.addEventListener('keydown', onKeyDown)
  })
}
