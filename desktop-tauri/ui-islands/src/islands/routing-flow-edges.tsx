import * as React from 'react'
import type { FlowPath, FlowSelection } from './routing-flow-types'
import { matchesSelection, selectedRequest, sumPaths } from './routing-flow-model'
import { attemptState, traceAttempts } from './routing-trace-model'
import { t } from '../i18n'

type Edge = { id: string; from: string; to: string; paths: FlowPath[]; failed?: boolean }
type DrawnEdge = Edge & { d: string; x: number; y: number; endX: number; endY: number }
export function RoutingFlowEdges({ root, paths, selection, collapsed, focused, onSelect }: {
  root: React.RefObject<HTMLDivElement | null>; paths: FlowPath[]; selection: FlowSelection;
  collapsed: ReadonlySet<string>; focused: boolean; onSelect(selection: FlowSelection): void
}) {
  const [drawn, setDrawn] = React.useState<DrawnEdge[]>([])
  React.useLayoutEffect(() => {
    const container = root.current
    if (!container) return
    let frame = 0
    const draw = () => {
      const groups = new Map<string, Edge>()
      const add = (from: string, to: string, path: FlowPath, failed = false) => {
        const id = JSON.stringify([from, to, failed])
        const edge = groups.get(id) || { id, from, to, paths: [], failed }
        edge.paths.push(path); groups.set(id, edge)
      }
      const row = selectedRequest(selection)
      const attempts = row ? traceAttempts(row) : []
      for (const path of paths) {
        if (focused && selection && !matchesSelection(path, selection)) continue
        add(`client:${path.client}`, `model:${path.model}`, path)
        const hasFailedAttempt = row && attempts.some(attempt => attempt.accountId === path.accountId && attemptState(row, attempt) === 'failed')
        if (path.provider && !hasFailedAttempt) add(`model:${path.model}`, collapsed.has(path.provider) ? `provider:${path.provider}` : `account:${path.accountId || path.id}`, path)
      }
      if (row && paths[0]) for (const attempt of attempts) {
        if (!attempt.accountId || attemptState(row, attempt) !== 'failed') continue
        add(`model:${paths[0].model}`, collapsed.has(attempt.provider || '') ? `provider:${attempt.provider}` : `account:${attempt.accountId}`, paths[0], true)
      }
      const bounds = container.getBoundingClientRect()
      const anchors = new Map(Array.from(container.querySelectorAll<HTMLElement>('[data-flow-node]')).map(node => [node.dataset.flowNode!, node.getBoundingClientRect()]))
      setDrawn(Array.from(groups.values()).flatMap(edge => {
        const from = anchors.get(edge.from), to = anchors.get(edge.to)
        if (!from || !to) return []
        const x = from.right - bounds.left, y = from.top + from.height / 2 - bounds.top
        const endX = to.left - bounds.left, endY = to.top + to.height / 2 - bounds.top
        const middle = (x + endX) / 2
        return [{ ...edge, d: `M${x} ${y}C${middle} ${y},${middle} ${endY},${endX} ${endY}`, x: middle, y: (y + endY) / 2, endX, endY }]
      }))
    }
    const schedule = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(draw) }
    const observer = new ResizeObserver(schedule)
    observer.observe(container)
    container.querySelectorAll('[data-flow-node]').forEach(node => observer.observe(node))
    schedule()
    window.addEventListener('resize', schedule)
    return () => { observer.disconnect(); cancelAnimationFrame(frame); window.removeEventListener('resize', schedule) }
  }, [root, paths, selection, collapsed, focused])
  return <svg className='routing-flow-edges' role='group' aria-label={t('实际请求路径')}>
    {drawn.map(edge => {
      const chosen = edge.paths.filter(path => matchesSelection(path, selection))
      const active = selection && chosen.length > 0
      const rows = active ? chosen : edge.paths
      const running = edge.failed ? 0 : sumPaths(rows, 'running')
      const count = sumPaths(rows, 'requests')
      const label = edge.failed ? t('尝试失败') : running ? t('{n} 在途', { n: running }) : t('{n} 已完成', { n: count })
      const className = edge.failed ? 'is-failed' : running ? 'is-running' : active ? 'is-selected' : ''
      const activate = () => {
        if (selectedRequest(selection)) return
        if (edge.paths.length === 1) { onSelect({ kind: 'path', id: edge.paths[0].id }); return }
        const path = edge.paths[0]
        onSelect(edge.from.startsWith('client:') ? { kind: 'edge', model: path.model, client: path.client }
          : { kind: 'edge', model: path.model, provider: path.provider, ...(edge.to.startsWith('account:') ? { accountId: path.accountId } : {}) })
      }
      return <g key={edge.id} className={className} role='button' tabIndex={0} aria-label={label} onClick={activate} onKeyDown={event => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); activate() } }}>
        <path d={edge.d} className='flow-edge' /><path d={edge.d} className='flow-edge-hit' />
        <path className='flow-arrow' d={`M${edge.endX - 5} ${edge.endY - 3}L${edge.endX} ${edge.endY}L${edge.endX - 5} ${edge.endY + 3}`} />
        <g transform={`translate(${edge.x},${edge.y})`}><rect x={-39} y={-11} width={78} height={22} rx={6} /><text textAnchor='middle' dominantBaseline='central'>{label}</text></g>
      </g>
    })}
  </svg>
}
