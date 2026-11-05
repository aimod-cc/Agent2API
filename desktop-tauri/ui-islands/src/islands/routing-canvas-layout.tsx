import * as React from 'react'
import { Button, cn } from '@ui'
import { t } from '../i18n'
import { RouteIcon } from './routing-ui'

type Edge = { id: string; d: string; x: number; y: number }

/** 连线表达账号启用或路由参与关系，不把当前请求流量混入配置。 */
function RoutingCanvasEdges({ root }: { root: React.RefObject<HTMLDivElement | null> }) {
  const [drawing, setDrawing] = React.useState<{ width: number; height: number; x: number; y: number; edges: Edge[] } | null>(null)
  React.useEffect(() => {
    const canvas = root.current
    const card = canvas?.querySelector<HTMLElement>('[data-route-source]')
    const groups = canvas?.querySelector<HTMLElement>('[data-route-scroll]')
    if (!canvas || !card || !groups) return
    let frame = 0
    const draw = () => {
      if (!canvas.clientWidth || !canvas.clientHeight) { setDrawing(null); return }
      const bounds = canvas.getBoundingClientRect()
      const scaleX = bounds.width / canvas.offsetWidth || 1, scaleY = bounds.height / canvas.offsetHeight || 1
      const source = card.getBoundingClientRect()
      const anchor = card.querySelector<HTMLElement>('[data-route-anchor]')?.getBoundingClientRect() || source
      const list = groups.getBoundingClientRect()
      const column = card.querySelector<HTMLElement>('.routing-config-card-scroll')?.getBoundingClientRect() || source
      const sourceY = Math.min(source.bottom - 12, Math.max(source.top + 12, anchor.top + anchor.height / 2))
      if (list.left <= source.right || sourceY < Math.max(column.top, bounds.top) || sourceY > Math.min(column.bottom, bounds.bottom)) {
        setDrawing(null); return
      }
      const x = (source.right - bounds.left) / scaleX, y = (sourceY - bounds.top) / scaleY
      const edges = [...groups.querySelectorAll<HTMLElement>('[data-route-node]')].flatMap(node => {
        const header = node.querySelector('header')?.getBoundingClientRect() || node.getBoundingClientRect()
        const center = header.top + header.height / 2
        if (center < Math.max(list.top, bounds.top) || center > Math.min(list.bottom, bounds.bottom)) return []
        const endX = (header.left - bounds.left) / scaleX, endY = (center - bounds.top) / scaleY
        const mid = x + (endX - x) * .52
        return [{ id: node.dataset.routeNode!, x: endX, y: endY, d: `M ${x} ${y} C ${mid} ${y}, ${mid} ${endY}, ${endX} ${endY}` }]
      })
      setDrawing({ width: canvas.offsetWidth, height: canvas.offsetHeight, x, y, edges })
    }
    const schedule = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(draw) }
    const resize = new ResizeObserver(schedule)
    resize.observe(canvas); resize.observe(card); resize.observe(groups)
    const observeNodes = () => groups.querySelectorAll<HTMLElement>('[data-route-sort], [data-route-node]').forEach(node => resize.observe(node))
    observeNodes()
    const mutation = new MutationObserver(() => { resize.disconnect(); resize.observe(canvas); resize.observe(card); resize.observe(groups); observeNodes(); schedule() })
    mutation.observe(groups, { childList: true, subtree: true, attributes: true, attributeFilter: ['data-route-node', 'data-open', 'hidden'] })
    const sourceMutation = new MutationObserver(schedule)
    sourceMutation.observe(card, { childList: true, subtree: true })
    canvas.addEventListener('scroll', schedule, true)
    window.addEventListener('resize', schedule)
    schedule()
    return () => { resize.disconnect(); mutation.disconnect(); sourceMutation.disconnect(); cancelAnimationFrame(frame); canvas.removeEventListener('scroll', schedule, true); window.removeEventListener('resize', schedule) }
  }, [root])
  return <svg className='routing-canvas-edges' viewBox={drawing ? `0 0 ${drawing.width} ${drawing.height}` : undefined} aria-hidden='true'>
    {drawing?.edges.map(edge => <g key={edge.id}><path d={edge.d} /><circle cx={edge.x} cy={edge.y} r={3} /></g>)}
    {drawing?.edges.length ? <circle className='routing-canvas-origin' cx={drawing.x} cy={drawing.y} r={4} /> : null}
  </svg>
}

export function RoutingCanvasLayout({ title, children, groups, note, globalAccounts = false }: {
  title: string; children: React.ReactNode; groups: React.ReactNode; note: string; globalAccounts?: boolean
}) {
  const root = React.useRef<HTMLDivElement>(null)
  return <div className='routing-canvas' ref={root}>
    <RoutingCanvasEdges root={root} />
    <section className='routing-config-column'>
      <div className='routing-column-head'><RouteIcon name='route' size={14} /><b>{t('路由配置')}</b><span>{title}</span></div>
      {children}
      <p className='routing-canvas-note'><RouteIcon name='info' size={13} /><span>{note}</span></p>
    </section>
    {groups}
    <div className='routing-canvas-legend'><span><i />{globalAccounts ? t('连线表示账号组中有已启用账号') : t('连线表示参与本路由')}</span>
      <span>{globalAccounts ? t('账号启停立即生效；路由配置保存后生效') : t('保存后才会影响真实请求')}</span></div>
  </div>
}

export function RoutingDraftFooter({ dirty, inherited, busy, disabled, error, onDiscard, onSave }: {
  dirty: boolean; inherited?: boolean; busy: boolean; disabled: boolean; error?: string; onDiscard(): void; onSave(): void
}) {
  return <footer className={cn('routing-config-draft', dirty && 'is-dirty')}>
    {error ? <p className='routing-config-error' role='alert'>{error}</p> : null}
    <span className='routing-draft-state'>{dirty ? t('有未保存的修改') : inherited ? t('继承全局') : t('已保存')}</span>
    <div className='right'><Button variant='outline' size='sm' disabled={!dirty || disabled} onClick={onDiscard}>{t('撤销修改')}</Button>
      <Button size='sm' disabled={!dirty || disabled} onClick={onSave}>{busy ? t('保存中…') : t('保存路由')}</Button></div>
  </footer>
}
