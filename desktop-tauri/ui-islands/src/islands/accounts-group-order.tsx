import * as React from 'react'
import { createPortal } from 'react-dom'
import { Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, cn } from '@ui'
import { t } from '../i18n'
import { byPriorityOrder, providerOf } from './accounts-domain'
import { allAccounts, reorderQueue } from './accounts-data'
import { shared } from './accounts-shared'
import { useProviderIcon } from './provider-icon'

type OrderDragRef = {
  provider: string
  label: string
  pointerId: number
  startX: number
  startY: number
  lastX: number
  lastY: number
  active: boolean
  slot: number
  frame: number
}

type OrderDragView = {
  provider: string
  label: string
  slot: number
  targetProvider: string
  after: boolean
  x: number
  y: number
}

const DRAG_THRESHOLD = 4

const GRIP_ICON = (
  <svg viewBox='0 0 24 24' width={14} height={14} fill='currentColor' aria-hidden='true' className='block'>
    <circle cx='9' cy='6' r='1.6' />
    <circle cx='15' cy='6' r='1.6' />
    <circle cx='9' cy='12' r='1.6' />
    <circle cx='15' cy='12' r='1.6' />
    <circle cx='9' cy='18' r='1.6' />
    <circle cx='15' cy='18' r='1.6' />
  </svg>
)

function insertionIndex(others: HTMLElement[], clientY: number): number {
  for (let index = 0; index < others.length; index += 1) {
    const rect = others[index].getBoundingClientRect()
    if (clientY < rect.top + rect.height / 2) return index
  }
  return others.length
}

function ProviderBadgeIcon({ provider, label }: { provider: string; label: string }) {
  const { src, onError } = useProviderIcon(provider)
  if (src) {
    return <img src={src} alt='' loading='lazy' onError={onError} className='size-4 flex-none rounded-[3px] object-contain' />
  }
  return (
    <span className='inline-flex size-4 flex-none items-center justify-center rounded-[3px] bg-surface-3 text-[10px] font-bold text-muted-foreground'>
      {String(label || '?').trim().slice(0, 1).toUpperCase()}
    </span>
  )
}

export function AccountsGroupOrder({ onClose }: { onClose: () => void }) {
  const initial = React.useMemo(() => {
    const accounts = allAccounts().slice().sort(byPriorityOrder)
    const counts = new Map<string, number>()
    const order: string[] = []
    for (const account of accounts) {
      const provider = providerOf(account)
      if (!counts.has(provider)) order.push(provider)
      counts.set(provider, (counts.get(provider) || 0) + 1)
    }
    return { order, counts }
  }, [])

  const [providers, setProviders] = React.useState<string[]>(() => initial.order)
  const [busy, setBusy] = React.useState(false)
  const [dragView, setDragView] = React.useState<OrderDragView | null>(null)

  const bodyRef = React.useRef<HTMLDivElement | null>(null)
  const listRef = React.useRef<HTMLDivElement | null>(null)
  const dragRef = React.useRef<OrderDragRef | null>(null)
  /** 避免拖拽松手落在遮罩上时触发 Dialog 的点外关闭 */
  const justDraggedRef = React.useRef(false)
  const busyRef = React.useRef(busy)
  busyRef.current = busy

  const labelOf = React.useCallback((provider: string) => t(shared().wbProviders?.labelOf?.(provider) || provider), [])

  const moveTo = React.useCallback((from: number, to: number) => {
    setProviders(previous => {
      const clamped = Math.max(0, Math.min(previous.length - 1, to))
      if (from < 0 || from >= previous.length || clamped === from) return previous
      const next = [...previous]
      const [moved] = next.splice(from, 1)
      next.splice(clamped, 0, moved)
      return next
    })
  }, [])

  const dirty = providers.some((provider, index) => provider !== initial.order[index])

  React.useEffect(() => {
    const computeSlot = (current: OrderDragRef): OrderDragView | null => {
      const list = listRef.current
      if (!list) return null
      const rows = [...list.querySelectorAll<HTMLElement>('[data-order-provider]')]
      const others = rows.filter(row => row.dataset.orderProvider !== current.provider)
      if (!others.length) return null
      const slot = insertionIndex(others, current.lastY)
      const after = slot >= others.length
      const target = after ? others[others.length - 1] : others[slot]
      const targetProvider = target.dataset.orderProvider || ''
      current.slot = slot
      return {
        provider: current.provider,
        label: current.label,
        slot,
        targetProvider,
        after,
        x: current.lastX,
        y: current.lastY,
      }
    }

    const stepAutoScroll = () => {
      const current = dragRef.current
      const body = bodyRef.current
      if (!current || !current.active) return
      if (body && body.scrollHeight > body.clientHeight) {
        const rect = body.getBoundingClientRect()
        const topEdge = rect.top + 36
        const bottomEdge = rect.bottom - 36
        let delta = 0
        if (current.lastY < topEdge && body.scrollTop > 0) {
          delta = -Math.max(4, Math.round(Math.min(80, topEdge - current.lastY) / 4))
        } else if (current.lastY > bottomEdge && body.scrollTop + body.clientHeight < body.scrollHeight) {
          delta = Math.max(4, Math.round(Math.min(80, current.lastY - bottomEdge) / 4))
        }
        if (delta !== 0) {
          body.scrollTop += delta
          const view = computeSlot(current)
          if (view) setDragView(view)
        }
      }
      current.frame = requestAnimationFrame(stepAutoScroll)
    }

    const cleanup = (drop: boolean) => {
      const current = dragRef.current
      if (!current) return
      dragRef.current = null
      cancelAnimationFrame(current.frame)
      document.body.classList.remove('queue-dragging')
      const finalView = drop && current.active ? computeSlot(current) : null
      setDragView(null)
      if (!current.active) return
      // 推迟一拍清标记，拦截本次松手顺带触发的 backdrop 点击关闭
      justDraggedRef.current = true
      window.setTimeout(() => { justDraggedRef.current = false }, 0)
      if (!drop || !finalView || busyRef.current) return
      setProviders(previous => {
        const from = previous.indexOf(current.provider)
        if (from < 0) return previous
        const rest = previous.filter((_, index) => index !== from)
        const to = Math.max(0, Math.min(rest.length, finalView.slot))
        if (to === from) return previous
        return [...rest.slice(0, to), current.provider, ...rest.slice(to)]
      })
    }

    const onPointerMove = (event: PointerEvent) => {
      const current = dragRef.current
      if (!current || event.pointerId !== current.pointerId) return
      if (event.buttons === 0 || busyRef.current) { cleanup(false); return }
      current.lastX = event.clientX
      current.lastY = event.clientY
      if (!current.active) {
        if (Math.hypot(event.clientX - current.startX, event.clientY - current.startY) < DRAG_THRESHOLD) return
        current.active = true
        document.body.classList.add('queue-dragging')
        current.frame = requestAnimationFrame(stepAutoScroll)
      }
      event.preventDefault()
      const view = computeSlot(current)
      if (view) setDragView(view)
    }

    const onPointerUp = (event: PointerEvent) => {
      const current = dragRef.current
      if (!current || event.pointerId !== current.pointerId) return
      current.lastX = event.clientX
      current.lastY = event.clientY
      cleanup(true)
    }

    const onAbort = () => cleanup(false)
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && dragRef.current) {
        event.preventDefault()
        event.stopPropagation()
        cleanup(false)
      }
    }

    window.addEventListener('pointermove', onPointerMove, { passive: false })
    window.addEventListener('pointerup', onPointerUp)
    window.addEventListener('pointercancel', onAbort)
    window.addEventListener('blur', onAbort)
    window.addEventListener('keydown', onKeyDown, true)
    return () => {
      const current = dragRef.current
      if (current) cancelAnimationFrame(current.frame)
      dragRef.current = null
      document.body.classList.remove('queue-dragging')
      window.removeEventListener('pointermove', onPointerMove)
      window.removeEventListener('pointerup', onPointerUp)
      window.removeEventListener('pointercancel', onAbort)
      window.removeEventListener('blur', onAbort)
      window.removeEventListener('keydown', onKeyDown, true)
    }
  }, [])

  const startDrag = (provider: string, event: React.PointerEvent<HTMLElement>) => {
    if (busy || providers.length < 2 || event.button !== 0 || dragRef.current) return
    if ((event.target as Element | null)?.closest('button')) return
    event.preventDefault()
    dragRef.current = {
      provider,
      label: labelOf(provider),
      pointerId: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      lastX: event.clientX,
      lastY: event.clientY,
      active: false,
      slot: -1,
      frame: 0,
    }
  }

  const save = async () => {
    setBusy(true)
    try {
      const accounts = allAccounts().slice().sort(byPriorityOrder)
      const known = new Set(providers)
      const extra = [...new Set(accounts.map(providerOf))].filter(provider => !known.has(provider))
      const ids = [...providers, ...extra].flatMap(provider =>
        accounts.filter(account => providerOf(account) === provider).map(account => account.id),
      )
      if (await reorderQueue(ids)) onClose()
    } finally { setBusy(false) }
  }

  return (
    <>
      <Dialog open onOpenChange={open => { if (!open && !busy && !dragRef.current?.active && !justDraggedRef.current) onClose() }}>
        <DialogContent className='w-[min(560px,calc(100vw-48px))]' showCloseButton={!busy}>
          <DialogHeader><DialogTitle>{t('调整提供商顺序')}</DialogTitle></DialogHeader>
          <DialogBody ref={bodyRef} className='py-3.5'>
            <div ref={listRef} className='flex flex-col gap-1.5'>
              {providers.map((provider, index) => {
                const label = labelOf(provider)
                const count = initial.counts.get(provider) || 0
                const isDragging = dragView?.provider === provider
                const isDropBefore = dragView?.targetProvider === provider && !dragView.after
                const isDropAfter = dragView?.targetProvider === provider && dragView.after
                return (
                  <div key={provider} data-order-provider={provider}
                    onPointerDown={event => startDrag(provider, event)}
                    onKeyDown={event => {
                      if (busy || !event.altKey) return
                      if (event.key === 'ArrowUp') { event.preventDefault(); moveTo(index, index - 1) }
                      else if (event.key === 'ArrowDown') { event.preventDefault(); moveTo(index, index + 1) }
                    }}
                    className={cn(
                      'flex items-center gap-2 rounded-md border border-hairline bg-surface px-2.5 py-1.5 select-none transition-colors',
                      !busy && providers.length > 1 && 'cursor-grab hover:bg-nav-hover',
                      isDragging && 'opacity-45',
                      isDropBefore && 'shadow-[inset_0_2px_0_0_var(--primary)]',
                      isDropAfter && 'shadow-[inset_0_-2px_0_0_var(--primary)]',
                    )}>
                    <span tabIndex={0} role='button' aria-label={t('拖动调整 {name} 顺位', { name: label })}
                      className='inline-flex size-5 flex-none items-center justify-center rounded-xs text-subtle outline-none focus-visible:shadow-focus'>
                      {GRIP_ICON}
                    </span>
                    <span className='w-6 flex-none text-center text-xs font-semibold text-subtle tabular-nums'>{index + 1}</span>
                    <ProviderBadgeIcon provider={provider} label={label} />
                    <span className='min-w-0 flex-1 truncate text-[13px] font-medium' title={label}>{label}</span>
                    <span className='flex-none text-[11px] text-subtle tabular-nums'>{t('{n} 个账号', { n: count })}</span>
                    <div className='flex flex-none items-center gap-0.5'>
                      <Button size='xs' variant='ghost' disabled={busy || index === 0}
                        title={t('移到第一位')} onClick={() => moveTo(index, 0)}>{t('置顶')}</Button>
                      <Button size='icon-xs' variant='ghost' disabled={busy || index === 0}
                        title={t('上移')} aria-label={t('上移')} onClick={() => moveTo(index, index - 1)}>↑</Button>
                      <Button size='icon-xs' variant='ghost' disabled={busy || index === providers.length - 1}
                        title={t('下移')} aria-label={t('下移')} onClick={() => moveTo(index, index + 1)}>↓</Button>
                      <Button size='xs' variant='ghost' disabled={busy || index === providers.length - 1}
                        title={t('移到最后')} onClick={() => moveTo(index, providers.length - 1)}>{t('置底')}</Button>
                    </div>
                  </div>
                )
              })}
            </div>
          </DialogBody>
          <DialogFooter>
            <span className='mr-auto text-xs text-subtle'>{t('按住行或 ⠿ 上下拖动调整顺序')}</span>
            <Button variant='ghost' disabled={busy} onClick={onClose}>{t('取消')}</Button>
            <Button disabled={busy || !dirty} onClick={() => void save()}>{busy ? t('保存中…') : t('保存')}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      {dragView ? createPortal(
        <div className='pointer-events-none fixed z-50 flex items-center gap-2 rounded-md border border-primary bg-raised px-3 py-1.5 text-xs shadow-3'
          style={{ left: dragView.x + 16, top: dragView.y + 14 }}>
          <b className='font-semibold text-foreground'>{dragView.label}</b>
          <span className='text-primary-fg'>{t('放到第 {n} 位', { n: dragView.slot + 1 })}</span>
        </div>,
        document.body,
      ) : null}
    </>
  )
}
