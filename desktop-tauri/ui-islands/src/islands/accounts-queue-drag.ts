/**
 * 分组模式下的**顺序拖拽**（组顺位 / 组内顺位）—— 指针事件实现，不引拖拽库。
 *
 * ── 为什么是指针事件，不是 HTML5 拖放 ─────────────────────────
 * 与列宽拖拽、列设置面板的排序同一取舍：拖拽过程中列表会被连接数轮询或虚拟滚动
 * 重绘，而 HTML5 拖放的会话绑在**那个节点**上，一重绘就断；指针事件的状态只在
 * JS 变量里，每帧从当前 DOM 现查同级节点与落点，并在靠近滚动容器边缘时自动滚动。
 *
 * ── 落点以目标 ID 回传，而不是纯下标 ─────────────────────────
 * 虚拟滚动与分页下，DOM 里只挂着当前视口 / 当前页那一段节点；直接回传
 * `targetProvider` / `targetAccountId` + `after`，页面只需在全量分组里定位该锚点，
 * 无论列表是否分页、折叠、筛选或边拖边滚，都不会算错顺位。
 */

/** 拖完的落点：由目标节点与相对前后决定插入位置 */
export type QueueDrop =
  | { kind: 'group'; provider: string; targetProvider: string; after: boolean }
  | { kind: 'row'; provider: string; accountId: string; targetAccountId: string; after: boolean }

export type QueueDragHandlers = {
  /** 整组拖动是否被当前视角锁定（例如只筛选了单个提供商） */
  isLocked?(): boolean
  /** 组顺位锁着时按下把手 */
  onLockedAttempt?(): void
  /** 当前页面可见的组不足 2 个，无法直接拖动调整顺序 */
  onGroupUnavailable?(): void
  /** 这一组是否开着排序规则（组内顺位被规则接管） */
  isRowLocked?(provider: string): boolean
  /** 组内把手被排序规则拦下：给一次提示 */
  onRowLockedAttempt?(provider: string): void
  /** 拖完了：把目标锚点交回去，顺序怎么写由调用方决定 */
  onDrop(drop: QueueDrop): void
}

type DragState = {
  kind: 'group' | 'row'
  provider: string
  accountId: string
  container: HTMLElement
  startX: number
  startY: number
  lastX: number
  lastY: number
  pointerId: number
  /** 跨过阈值后才算真正开始拖动，避免轻点把手时误触发或闪动 */
  active: boolean
  targetId: string
  after: boolean
  frame: number
}

/** 启动拖动的最小移动距离（px） */
const DRAG_THRESHOLD = 4
/** 拖动中的行 / 组压淡（与列设置面板排序时同一手法） */
const DRAGGING_CLASS = 'dragging'
/** 落点提示的两个类：插在这一项之前 / 之后 */
const DROP_CLASSES = ['drop-before', 'drop-after'] as const

let drag: DragState | null = null
let handlersRef: QueueDragHandlers | null = null

function clearDropMarks(container: ParentNode): void {
  container.querySelectorAll(`.${DROP_CLASSES[0]}, .${DROP_CLASSES[1]}`).forEach(element => {
    element.classList.remove(...DROP_CLASSES)
  })
}

function clearDraggingMarks(container: ParentNode): void {
  container.querySelectorAll(`.${DRAGGING_CLASS}`).forEach(element => {
    element.classList.remove(DRAGGING_CLASS)
  })
}

/** 现查当前 DOM 里的同级别兄弟项（组 = 各 tbody.acct-group，行 = 同组内的 tr.acct-row） */
function liveSiblings(container: HTMLElement, kind: 'group' | 'row', provider: string): HTMLElement[] {
  const table = container.querySelector<HTMLElement>('table.acct-table') || container
  if (kind === 'group') {
    return [...table.children].filter(element =>
      element.classList.contains('acct-group')) as HTMLElement[]
  }
  return [...table.querySelectorAll<HTMLElement>('tr.acct-row')].filter(row => row.dataset.provider === provider)
}

function itemIdOf(element: HTMLElement, kind: 'group' | 'row'): string {
  return kind === 'group' ? (element.dataset.provider || '') : (element.dataset.id || '')
}

/**
 * 指针落在哪一段：拿每一项的中线比，返回在其余项里的插入下标。
 */
function insertionIndex(others: HTMLElement[], clientY: number): number {
  for (let index = 0; index < others.length; index += 1) {
    const rect = others[index].getBoundingClientRect()
    if (clientY < rect.top + rect.height / 2) return index
  }
  return others.length
}

/** 根据当前 DOM 与指针位置刷新拖动态样式和目标落点 */
function updateTarget(state: DragState, paint = true): void {
  const selfId = state.kind === 'group' ? state.provider : state.accountId
  const items = liveSiblings(state.container, state.kind, state.provider)
  const dragged = items.find(item => itemIdOf(item, state.kind) === selfId) || null
  const others = items.filter(item => itemIdOf(item, state.kind) !== selfId)

  if (paint) {
    clearDraggingMarks(state.container)
    if (dragged) dragged.classList.add(DRAGGING_CLASS)
    clearDropMarks(state.container)
  }
  if (!others.length) return

  const index = insertionIndex(others, state.lastY)
  const after = index >= others.length
  const target = after ? others[others.length - 1] : others[index]
  const targetId = itemIdOf(target, state.kind)
  if (!targetId) return

  state.targetId = targetId
  state.after = after
  if (paint) target.classList.add(after ? DROP_CLASSES[1] : DROP_CLASSES[0])
}

/** 靠近滚动容器上下缘时平滑自动滚动，便于跨越长列表调整顺位 */
function stepAutoScroll(): void {
  if (!drag || !drag.active) return
  const host = drag.container
  const rect = host.getBoundingClientRect()
  const headHeight = host.querySelector('thead')?.getBoundingClientRect().height || 0
  const topEdge = rect.top + headHeight + 36
  const bottomEdge = rect.bottom - 40
  let delta = 0
  if (drag.lastY < topEdge && host.scrollTop > 0) {
    const overshoot = Math.min(80, topEdge - drag.lastY)
    delta = -Math.max(4, Math.round(overshoot / 4))
  } else if (drag.lastY > bottomEdge && host.scrollTop + host.clientHeight < host.scrollHeight) {
    const overshoot = Math.min(80, drag.lastY - bottomEdge)
    delta = Math.max(4, Math.round(overshoot / 4))
  }
  if (delta !== 0) {
    host.scrollTop += delta
    updateTarget(drag)
  }
  drag.frame = requestAnimationFrame(stepAutoScroll)
}

function onPointerMove(event: PointerEvent): void {
  if (!drag || event.pointerId !== drag.pointerId) return
  if (event.buttons === 0) { finish(false); return }
  drag.lastX = event.clientX
  drag.lastY = event.clientY
  if (!drag.active) {
    if (Math.hypot(event.clientX - drag.startX, event.clientY - drag.startY) < DRAG_THRESHOLD) return
    drag.active = true
    document.body.classList.add('queue-dragging')
    drag.frame = requestAnimationFrame(stepAutoScroll)
  }
  event.preventDefault()
  updateTarget(drag)
}

function finish(drop: boolean): void {
  const state = drag
  if (!state) return
  drag = null
  cancelAnimationFrame(state.frame)
  window.removeEventListener('pointermove', onPointerMove)
  window.removeEventListener('pointerup', onPointerUp)
  window.removeEventListener('pointercancel', onPointerCancel)
  window.removeEventListener('blur', onWindowBlur)
  window.removeEventListener('keydown', onKeyDown)
  document.body.classList.remove('queue-dragging')
  clearDraggingMarks(state.container)
  clearDropMarks(state.container)
  if (!drop || !state.active) return

  updateTarget(state, false)
  if (!state.targetId) return
  if (state.kind === 'group') {
    handlersRef?.onDrop({
      kind: 'group',
      provider: state.provider,
      targetProvider: state.targetId,
      after: state.after,
    })
  } else {
    handlersRef?.onDrop({
      kind: 'row',
      provider: state.provider,
      accountId: state.accountId,
      targetAccountId: state.targetId,
      after: state.after,
    })
  }
}

function onPointerUp(event: PointerEvent): void {
  if (!drag || event.pointerId !== drag.pointerId) return
  drag.lastX = event.clientX
  drag.lastY = event.clientY
  finish(true)
}

function onPointerCancel(): void { finish(false) }
function onWindowBlur(): void { finish(false) }

/** Esc = 放弃这次拖动（与列设置面板的排序同一手感） */
function onKeyDown(event: KeyboardEvent): void {
  if (event.key === 'Escape') {
    event.preventDefault()
    finish(false)
  }
}

/**
 * 绑定到滚动容器上（**委托**，一次即可）：拖拽结束后 React 会把 tbody 重画一遍，
 * 绑在具体行上的监听会随之失效，绑在容器上则一直有效 —— 与列宽拖拽同一理由。
 */
export function bindQueueDrag(host: HTMLElement | null, handlers: QueueDragHandlers): void {
  handlersRef = handlers
  if (!host) return
  if (host.dataset.queueDragBound === '1') return
  host.dataset.queueDragBound = '1'
  host.addEventListener('pointerdown', event => {
    if (event.button !== 0 || drag) return
    const target = event.target as Element | null
    const groupGrip = target?.closest?.('.queue-grip')
    const rowGrip = target?.closest?.('.order-grip')
    if (!groupGrip && !rowGrip) return
    event.preventDefault()

    if (groupGrip && handlersRef?.isLocked?.()) {
      handlersRef.onLockedAttempt?.()
      return
    }
    const item = (groupGrip ?? rowGrip)!.closest<HTMLElement>('tbody.acct-group, tr.acct-row')
    if (!item) return
    const kind: 'group' | 'row' = groupGrip ? 'group' : 'row'
    const provider = item.dataset.provider
      || item.closest<HTMLElement>('tbody.acct-group')?.dataset.provider
      || ''

    if (kind === 'row' && provider && handlersRef?.isRowLocked?.(provider)) {
      handlersRef.onRowLockedAttempt?.(provider)
      return
    }
    const accountId = item.dataset.id || ''
    if (kind === 'row' && !accountId) return

    const items = liveSiblings(host, kind, provider)
    if (items.length < 2) {
      if (kind === 'group') handlersRef?.onGroupUnavailable?.()
      return
    }

    drag = {
      kind,
      provider,
      accountId,
      container: host,
      startX: event.clientX,
      startY: event.clientY,
      lastX: event.clientX,
      lastY: event.clientY,
      pointerId: event.pointerId,
      active: false,
      targetId: '',
      after: false,
      frame: 0,
    }
    window.addEventListener('pointermove', onPointerMove, { passive: false })
    window.addEventListener('pointerup', onPointerUp)
    window.addEventListener('pointercancel', onPointerCancel)
    window.addEventListener('blur', onWindowBlur)
    window.addEventListener('keydown', onKeyDown)
  })
}
