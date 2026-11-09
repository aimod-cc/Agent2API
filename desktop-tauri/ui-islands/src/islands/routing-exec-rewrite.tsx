/**
 * 请求回放里的「模型改写」块。
 *
 * ── 为什么单独一块、放在承载之前 ──────────────────────────────
 * 特殊请求路由（压缩 / 图片兼容 / 标题生成）只改写模型，不改选账号规则。
 * 回放页原来的结构是「最终承载 → 账号路由 → 当时配置」，读者看到的是
 * 「这次用了哪个账号」；而「客户端要的是 A，实际发出去的是 B」这条事实
 * 在别处看不出来。放在承载之前，读的顺序正好是请求真实经过的顺序。
 *
 * ── 数据从哪来 ────────────────────────────────────────────────
 * 后端在选路之前就把改写记录写进 route 快照（`compactRoute` / `titleRoute`），
 * 图片兼容则由 `visionCompatibility` 表达（它记录的是读图调用与接管方式）。
 * 三者都没有时这一块整块不渲染 —— 普通请求不该多出一个空壳。
 */

import { Badge } from '@ui'
import { t } from '../i18n'
import { RouteIcon } from './routing-ui'
import { requestModel } from './routing-trace-model'
import type { ReplayRow } from './routing-types'

type Rewrite = {
  kind: 'compact' | 'title' | 'vision'
  label: string
  from: string
  to: string
  detail: string
}

const CLIENT_LABEL: Record<string, string> = {
  zcode: 'ZCode',
  'claude-code': 'Claude Code',
  codex: 'Codex',
}

const clientText = (client?: string) => client ? CLIENT_LABEL[client] || client : t('客户端未记录')

/** 从 route 快照里读出这次请求发生过的模型改写。 */
export function requestRewrite(row: ReplayRow): Rewrite | null {
  const route = row.route as (NonNullable<ReplayRow['route']> & {
    compactRoute?: { source?: string; target?: string; client?: string }
    titleRoute?: { source?: string; target?: string; client?: string }
    visionCompatibility?: { mode?: string; calls?: unknown[]; target?: { model?: string } }
  }) | null | undefined
  if (!route) return null

  const compact = route.compactRoute
  if (compact?.source && compact.target) {
    return { kind: 'compact', label: t('压缩上下文'), from: compact.source, to: compact.target,
      detail: t('{client} 的上下文压缩请求', { client: clientText(compact.client) }) }
  }
  const title = route.titleRoute
  if (title?.source && title.target) {
    return { kind: 'title', label: t('标题生成'), from: title.source, to: title.target,
      detail: t('{client} 的标题请求', { client: clientText(title.client) }) }
  }
  const vision = route.visionCompatibility
  if (vision?.mode) {
    // 图片兼容不改「请求的模型」：辅助识图是另起一次读图调用，整体接管才是换模型。
    const calls = Array.isArray(vision.calls) ? vision.calls.length : 0
    const assist = vision.mode === 'assist_call' || vision.mode === 'assist'
    return {
      kind: 'vision', label: t('图片兼容'),
      from: requestModel(row),
      to: assist ? t('辅助识图 · {n} 次读图', { n: calls }) : (vision.target?.model || t('视觉模型接管')),
      detail: assist ? t('读图结果交回原模型继续回答') : t('整次请求交给视觉模型'),
    }
  }
  return null
}

export function RoutingExecRewrite({ row }: { row: ReplayRow }) {
  const rewrite = requestRewrite(row)
  if (!rewrite) return null
  const tone = rewrite.kind === 'vision' ? 'warning' : rewrite.kind === 'title' ? 'secondary' : 'brand'
  return <section className='routing-exec-rewrite' data-kind={rewrite.kind}>
    <header className='routing-exec-rewrite-head'>
      <RouteIcon name='route' size={14} />
      <b>{t('模型改写')}</b>
      <Badge variant={tone} shape='tag'>{rewrite.label}</Badge>
      <span>{rewrite.detail}</span>
    </header>
    <div className='routing-exec-rewrite-flow'>
      <div className='routing-exec-rewrite-node'><span>{t('客户端模型')}</span><b>{rewrite.from}</b></div>
      <span className='routing-exec-rewrite-arrow' aria-hidden='true'><RouteIcon name='caretRight' size={14} /></span>
      <div className='routing-exec-rewrite-node is-accent'><span>{t('实际使用')}</span><b>{rewrite.to}</b></div>
    </div>
  </section>
}
