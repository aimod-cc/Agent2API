import * as React from 'react'
import { createPortal } from 'react-dom'
import { t } from '../i18n'

export type RoutingDrop = { kind: 'provider' | 'account' | 'member'; provider: string; id: string; target: string; after: boolean }
type Drag = Omit<RoutingDrop, 'target' | 'after'> & { pointer: number; x: number; y: number; active: boolean; list: HTMLElement; ids: string[]; label: string }
type Position = { kind: RoutingDrop['kind']; provider: string; id: string; target: string; after: boolean; index: number; x: number; y: number; label: string }

/** 只在把手起拖，落点按同级列表计算；取消与松手是两条独立路径。 */
export function useRoutingDrag(enabled: boolean, scope: string, onDrop: (drop: RoutingDrop) => void) {
  const drag = React.useRef<Drag | null>(null)
  const position = React.useRef<Position | null>(null)
  const [state, setState] = React.useState<Position | null>(null)
  const live = React.useRef({ enabled, onDrop })
  live.current = { enabled, onDrop }
  const cancel = React.useCallback(() => { drag.current = null; position.current = null; setState(null) }, [])
  React.useEffect(() => { if (!enabled) cancel() }, [enabled, cancel])
  React.useEffect(() => cancel, [scope, cancel])
  React.useEffect(() => {
    let frame = 0
    let lastY = 0
    const rows = (current: Drag) => [...current.list.querySelectorAll<HTMLElement>('[data-route-sort]')]
      .filter(row => row.closest('[data-route-list]') === current.list)
    const locate = (x: number, y: number) => {
      const current = drag.current
      if (!current?.active || !current.list.isConnected || !live.current.enabled) { cancel(); return }
      const list = rows(current)
      if (list.map(row => row.dataset.routeSort).join('\0') !== current.ids.join('\0')) { cancel(); return }
      const others = list.filter(row => row.dataset.routeSort !== current.id)
      const bounds = current.list.getBoundingClientRect()
      const scrollerBounds = current.list.closest<HTMLElement>('[data-route-scroll]')?.getBoundingClientRect() || bounds
      if (!others.length || x < bounds.left - 24 || x > bounds.right + 24 || y < scrollerBounds.top - 48 || y > scrollerBounds.bottom + 48) {
        position.current = null; setState(null); return
      }
      const found = others.findIndex(row => {
        const rect = (current.kind !== 'account' ? row.querySelector('header') || row : row).getBoundingClientRect()
        return y < rect.top + rect.height / 2
      })
      const index = found < 0 ? others.length : found
      const target = others[Math.min(index, others.length - 1)].dataset.routeSort!
      const next = { kind: current.kind, provider: current.provider, id: current.id, target, after: index === others.length, index, x, y, label: current.label }
      position.current = next; setState(next)
    }
    const scroll = () => {
      const current = drag.current
      if (!current?.active) return
      const scroller = current.list.closest<HTMLElement>('[data-route-scroll]')
      if (scroller) {
        const rect = scroller.getBoundingClientRect()
        const delta = lastY < rect.top + 48 ? -12 : lastY > rect.bottom - 48 ? 12 : 0
        if (delta) { scroller.scrollTop += delta; locate(position.current?.x ?? current.x, lastY) }
      }
      frame = requestAnimationFrame(scroll)
    }
    const move = (event: PointerEvent) => {
      const current = drag.current
      if (!current || event.pointerId !== current.pointer) return
      if (event.buttons === 0 || !live.current.enabled) { cancel(); return }
      if (!current.active && Math.hypot(event.clientX - current.x, event.clientY - current.y) < 7) return
      event.preventDefault()
      if (!current.active) { current.active = true; frame = requestAnimationFrame(scroll) }
      lastY = event.clientY
      locate(event.clientX, event.clientY)
    }
    const finish = (event: PointerEvent) => {
      const current = drag.current
      if (!current || event.pointerId !== current.pointer) return
      if (!current.active) { cancel(); return }
      locate(event.clientX, event.clientY)
      const drop = position.current
      cancelAnimationFrame(frame)
      cancel()
      if (current.active && drop && live.current.enabled) {
        live.current.onDrop({ kind: current.kind, provider: current.provider, id: current.id, target: drop.target, after: drop.after })
      }
    }
    const abort = () => { cancelAnimationFrame(frame); cancel() }
    const key = (event: KeyboardEvent) => { if (event.key === 'Escape' && drag.current) { event.preventDefault(); abort() } }
    window.addEventListener('pointermove', move, { passive: false })
    window.addEventListener('pointerup', finish)
    window.addEventListener('pointercancel', abort)
    window.addEventListener('blur', abort)
    window.addEventListener('keydown', key)
    return () => {
      cancelAnimationFrame(frame)
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', finish)
      window.removeEventListener('pointercancel', abort)
      window.removeEventListener('blur', abort)
      window.removeEventListener('keydown', key)
      drag.current = null
    }
  }, [cancel])
  const start = (event: React.PointerEvent, kind: RoutingDrop['kind'], provider: string, id: string) => {
    if (!live.current.enabled || event.button !== 0 || drag.current) return
    const row = event.currentTarget.closest<HTMLElement>('[data-route-sort]')
    const list = row?.closest<HTMLElement>('[data-route-list]')
    if (!list || !row) return
    const ids = [...list.querySelectorAll<HTMLElement>('[data-route-sort]')]
      .filter(item => item.closest('[data-route-list]') === list).map(item => item.dataset.routeSort!)
    if (ids.length < 2) return
    if (event.pointerType === 'mouse') event.preventDefault()
    event.currentTarget.setPointerCapture(event.pointerId)
    drag.current = { kind, provider, id, pointer: event.pointerId, x: event.clientX, y: event.clientY, active: false, list, ids,
      label: row.querySelector('b')?.textContent || id }
  }
  return { start, state, cancel }
}

export function RoutingDragOverlay({ state }: { state: Position | null }) {
  return state ? createPortal(<div className='routing-drag-preview' style={{ left: state.x + 18, top: state.y + 16 }}>
    <b>{state.label}</b><span>{t('放到第 {n} 位', { n: state.index + 1 })}</span>
  </div>, document.body) : null
}

export function reorderRoute(ids: string[], id: string, target: string, after: boolean) {
  const from = ids.indexOf(id)
  if (from < 0 || id === target || !ids.includes(target)) return
  ids.splice(from, 1)
  ids.splice(ids.indexOf(target) + (after ? 1 : 0), 0, id)
}
