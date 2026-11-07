import * as React from 'react'

type Point = { x: number; y: number }
type Edge = { id: string; d: string; from: Point; to: Point; running: boolean }
type Drawing = { width: number; height: number; edges: Edge[] }

export function RoutingCanvasEdges({ root }: { root: React.RefObject<HTMLDivElement | null> }) {
  const svg = React.useRef<SVGSVGElement>(null)
  const [drawing, setDrawing] = React.useState<Drawing | null>(null)

  React.useEffect(() => {
    const canvas = root.current
    if (!canvas) return
    let frame = 0
    const draw = () => {
      const card = canvas.querySelector<HTMLElement>('[data-route-source]')
      const groups = canvas.querySelector<HTMLElement>('[data-route-scroll]')
      if (!card || !groups || !canvas.clientWidth || !canvas.clientHeight) { setDrawing(null); return }
      const bounds = canvas.getBoundingClientRect()
      const scaleX = bounds.width / canvas.offsetWidth || 1, scaleY = bounds.height / canvas.offsetHeight || 1
      const left = bounds.left + canvas.clientLeft * scaleX, top = bounds.top + canvas.clientTop * scaleY
      const point = (x: number, y: number): Point => ({ x: (x - left) / scaleX, y: (y - top) / scaleY })
      const visible = (rect: DOMRect, clip: DOMRect) => rect.width > 0 && rect.height > 0
        && rect.top + rect.height / 2 >= Math.max(clip.top, bounds.top)
        && rect.top + rect.height / 2 <= Math.min(clip.bottom, bounds.bottom)
      const source = card.getBoundingClientRect()
      const sourceY = source.top + source.height / 2
      const list = groups.getBoundingClientRect()
      const edges: Edge[] = []
      const connect = (id: string, from: Point, to: Point, running: boolean, entryX = to.x) => {
        if (entryX <= from.x) return
        const mid = from.x + (entryX - from.x) * .52
        const d = `M ${from.x} ${from.y} C ${mid} ${from.y}, ${mid} ${to.y}, ${entryX} ${to.y}`
        edges.push({ id, from, to, running, d: entryX === to.x ? d : `${d} L ${to.x} ${to.y}` })
      }
      if (visible(source, bounds)) {
        const selected = canvas.querySelector<HTMLElement>('.routing-config-scope-item[data-active]')
        const rail = canvas.querySelector<HTMLElement>('.routing-config-rail-list')
        if (selected && rail) {
          const start = selected.getBoundingClientRect()
          if (visible(start, rail.getBoundingClientRect())) connect('scope', point(start.right, start.top + start.height / 2),
            point(source.left, sourceY), card.hasAttribute('data-route-running'))
        }
        if (list.left > source.right) {
          const from = point(source.right, sourceY)
          for (const node of groups.querySelectorAll<HTMLElement>('[data-route-node]')) {
            const connected = node.hasAttribute('data-route-connected')
            const running = node.hasAttribute('data-route-running')
            if (!connected && !running) continue
            const nodeBounds = node.getBoundingClientRect()
            if (!nodeBounds.width || !nodeBounds.height) continue
            if (node.hasAttribute('data-route-expanded')) {
              for (const account of node.querySelectorAll<HTMLElement>('[data-route-account]')) {
                const active = account.hasAttribute('data-route-running')
                if (!active && !(connected && account.hasAttribute('data-route-connected'))) continue
                const target = account.getBoundingClientRect()
                if (!visible(target, list) || !visible(target, nodeBounds)) continue
                connect(JSON.stringify([node.dataset.routeNode, account.dataset.routeAccount]), from,
                  point(target.left, target.top + target.height / 2), active, point(nodeBounds.left - 4 * scaleX, 0).x)
              }
            } else {
              const header = node.querySelector('header')?.getBoundingClientRect() || nodeBounds
              if (!visible(header, list)) continue
              connect(JSON.stringify([node.dataset.routeNode]), from, point(nodeBounds.left, header.top + header.height / 2), running)
            }
          }
        }
      }
      setDrawing({ width: canvas.clientWidth, height: canvas.clientHeight, edges })
    }
    const schedule = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(draw) }
    const resize = new ResizeObserver(schedule)
    let observed = new Set<Element>()
    const resizeTargets = '[data-route-source], [data-route-scroll], [data-route-node], [data-route-account], .routing-config-column, .routing-groups-column, .routing-column-head, .routing-canvas-note, .routing-config-rail, .routing-config-rail-list, .routing-config-scope-item'
    const observeNodes = () => {
      const nodes = new Set<Element>([canvas, ...canvas.querySelectorAll(resizeTargets)])
      for (const node of observed) if (!nodes.has(node)) resize.unobserve(node)
      for (const node of nodes) if (!observed.has(node)) resize.observe(node)
      observed = nodes
    }
    observeNodes()
    const mutation = new MutationObserver(records => {
      if (!records.some(record => !svg.current?.contains(record.target))) return
      if (records.some(record => record.type === 'childList')) observeNodes()
      schedule()
    })
    mutation.observe(canvas, { childList: true, subtree: true, attributes: true,
      attributeFilter: ['data-active', 'data-route-connected', 'data-route-expanded', 'data-route-running', 'data-route-node', 'data-route-account', 'data-open', 'data-closed', 'hidden'] })
    canvas.addEventListener('scroll', schedule, true)
    window.addEventListener('resize', schedule)
    schedule()
    return () => {
      resize.disconnect(); mutation.disconnect(); cancelAnimationFrame(frame)
      canvas.removeEventListener('scroll', schedule, true); window.removeEventListener('resize', schedule)
    }
  }, [root])

  const origins = new Map<string, Point>()
  for (const edge of drawing?.edges || []) origins.set(`${edge.from.x}:${edge.from.y}`, edge.from)
  return <svg ref={svg} className='routing-canvas-edges' viewBox={drawing ? `0 0 ${drawing.width} ${drawing.height}` : undefined} aria-hidden='true'>
    {drawing?.edges.map(edge => <g key={edge.id} className={edge.running ? 'is-running' : undefined}>
      <path d={edge.d} /><circle cx={edge.to.x} cy={edge.to.y} r={3} />
    </g>)}
    {[...origins].map(([key, origin]) => <circle key={key} className='routing-canvas-origin' cx={origin.x} cy={origin.y} r={3} />)}
  </svg>
}
