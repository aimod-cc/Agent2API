import * as React from 'react'
import { createRoot } from 'react-dom/client'
import { Badge, Button, Checkbox, Empty, Input, Label, SegmentedControl, Spinner } from '@ui'
import { t } from '../i18n'
import { AccountSettingsDialog } from './accounts-settings-dialog'
import { GroupSettingsDialog } from './accounts-group-settings'
import { RoutingSettingsDialog } from './routing-settings'
import { RoutingTracePage } from './routing-trace-page'
import { RoutingStickyDialog } from './routing-sticky'
import { RoutingFlowCanvas } from './routing-flow-canvas'
import { useRoutingFlow } from './routing-flow-state'
import { useRoutingEditor } from './routing-editor-state'
import { RoutingEditor } from './routing-editor'
import { RoutingExecution } from './routing-execution'
import { RoutingSelect } from './routing-select'
import { selectedRequest } from './routing-flow-model'
import { RouteIcon, stickyLabel, strategyLabel } from './routing-ui'
import { host, type ReplayRow, type RoutingWorkspace } from './routing-types'
import type { ConfigTarget, FlowSelection } from './routing-flow-types'

let requestTrace: ((row?: ReplayRow | null) => void) | undefined
let refreshWorkspace: (() => void) | undefined
export function RoutingPage() {
  const [view, setView] = React.useState<'workbench' | 'requests'>('workbench')
  const [initial, setInitial] = React.useState<ReplayRow | null>(null)
  const flow = useRoutingFlow(view === 'workbench', initial)
  const editor = useRoutingEditor(flow.refresh)
  const [panel, setPanel] = React.useState<'execution' | 'config'>('execution')
  const [target, setTarget] = React.useState<ConfigTarget>({ kind: 'model', id: '' })
  const [focused, setFocused] = React.useState(false)
  const [showSticky, setShowSticky] = React.useState(true)
  const [modal, setModal] = React.useState<{ kind: 'sticky' } | { kind: 'account' | 'sort'; id: string } | { kind: 'protection'; workspace: RoutingWorkspace } | null>(null)
  const request = selectedRequest(flow.selection)
  const models = [...new Set([...(flow.workspace?.models || []), ...(editor.draft?.workspace.models || []), ...(flow.snapshot?.paths.map(path => path.model) || []), ...(flow.model ? [flow.model] : [])])].filter(Boolean)
  const openConfig = (next: ConfigTarget, model = request?.model || flow.model) => {
    setTarget(next); setPanel('config'); setView('workbench')
    void editor.load(next.kind === 'model' && next.id ? next.id : model)
  }
  const select = (selection: FlowSelection) => { flow.setSelection(selection); setPanel('execution') }
  const openRequest = (row: ReplayRow) => { setInitial({ ...row }); setView('workbench'); setPanel('execution'); setModal(null) }
  React.useEffect(() => {
    requestTrace = row => { if (row) openRequest(row); else setView('requests') }
    refreshWorkspace = flow.refresh
    return () => { requestTrace = undefined; refreshWorkspace = undefined }
  }, [flow.refresh])
  const close = () => { setModal(null); flow.refresh() }
  return <>
    <div className='routing-workbench-modebar'><SegmentedControl aria-label={t('路由视图')} value={view} onValueChange={setView} options={[
      { value: 'workbench', label: <><RouteIcon name='route' />{t('路由工作台')}</> },
      { value: 'requests', label: <><RouteIcon name='requests' />{t('请求记录')}</> },
    ]} /><div><Button variant='ghost' size='sm' onClick={() => setModal({ kind: 'sticky' })}><RouteIcon name='lock' />{t('会话粘滞')}</Button><Button variant='outline' size='sm' onClick={() => openConfig({ kind: 'model', id: flow.model })}><RouteIcon name='settings' />{t('路由设置')}{editor.dirtyCount ? <Badge variant='warning' shape='tag'>{editor.dirtyCount}</Badge> : null}</Button></div></div>
    <div className='routing-unified-workbench' hidden={view !== 'workbench'}>
      <div className='routing-workbench-toolbar'><Label>{t('请求模型')}<RoutingSelect label={t('请求模型')} value={flow.model || '__all'} options={[[ '__all', t('全部模型') ], ...models.map(model => [model, model] as [string, string])]} onChange={value => flow.selectModel(value === '__all' ? '' : value)} /></Label><Label>{t('已完成记录')}<RoutingSelect label={t('已完成请求时间范围')} value={String(flow.minutes)} options={[5, 10, 30, 60].map(minutes => [String(minutes), t('最近 {n} 分钟', { n: minutes })])} onChange={value => flow.setMinutes(Number(value))} /></Label><SegmentedControl aria-label={t('请求活动范围')} value={flow.scope} onValueChange={flow.setScope} options={[{ value: 'window', label: t('在途 + 近期') }, { value: 'running', label: t('仅在途') }]} /><Button variant='ghost' size='sm' disabled={flow.loading} onClick={flow.refresh}><RouteIcon name='refresh' />{t('刷新')}</Button><span className='routing-workbench-policy'>{flow.workspace ? `${strategyLabel(flow.workspace.settings.routingStrategy)} · ${t('会话粘滞')} ${stickyLabel(flow.workspace.settings.routingSticky)}` : ''}</span></div>
      {flow.error ? <div className='routing-page-error' role='alert'><RouteIcon name='info' /><span>{flow.error}</span><Button variant='ghost' size='sm' onClick={flow.refresh}>{t('重试')}</Button></div> : null}
      <div className='routing-workbench-layout'><div className='routing-workbench-main'><div className='routing-workbench-head'><b>{request ? t('请求执行回放') : t('实际请求路径')}</b><span>{flow.snapshot ? t('{paths} 条路径 · {sessions} 个会话', { paths: flow.snapshot.pathCount, sessions: flow.snapshot.sessions }) : ''}</span><Button variant='ghost' size='sm' onClick={() => { flow.setSelection(null); setFocused(false) }}>{t('全部路径')}</Button></div>
        <div className='routing-workbench-filters'><Input placeholder={t('搜索客户端、模型或账号')} aria-label={t('搜索客户端、模型或账号')} value={flow.search} onChange={event => { flow.setSearch(event.currentTarget.value); flow.setSelection(null) }} /><Label><Checkbox checked={showSticky} onCheckedChange={on => setShowSticky(on === true)} />{t('粘滞标记')}</Label><Label><Checkbox checked={focused} onCheckedChange={on => setFocused(on === true)} />{t('仅看选中路径')}</Label></div>
        {request ? <div className='routing-workbench-replay'><RouteIcon name='lock' size={13} /><code>{request.id}</code><span>{t('执行路径按当时快照保留')}</span><Button variant='link' size='sm' onClick={() => select(null)}>{t('返回全部流向')}</Button></div> : null}
        {flow.snapshot?.truncated ? <p className='routing-workbench-notice'>{t('路径过多，画布仅展示前 300 条；统计覆盖完整窗口，请筛选模型缩小范围。')}</p> : null}
        {flow.workspace ? <RoutingFlowCanvas snapshot={flow.snapshot} workspace={flow.workspace} selection={flow.selection} now={flow.now} focused={focused} sticky={showSticky} onSelect={select} onConfig={openConfig} /> : <div className='routing-workbench-empty'><Empty>{flow.loading ? <Spinner /> : t('正在读取路由…')}</Empty></div>}
        <div className='routing-workbench-legend'><span><i className='live' />{t('流动：有请求在途')}</span><span><i className='selected' />{t('静止蓝线：选中历史路径')}</span><span><i />{t('灰线：历史流量')}</span><span><i className='failed' />{t('实际尝试失败')}</span></div><div className='routing-workbench-foot'>{t('未调用不画请求线；模型限流与当前配置分别展示。')}</div>
      </div><aside className='routing-workbench-inspector'><div className='routing-inspector-switch'><SegmentedControl aria-label={t('右侧面板内容')} value={panel} onValueChange={value => { setPanel(value); if (value === 'config') void editor.load(request?.model || flow.model) }} options={[{ value: 'execution', label: request ? t('本次执行') : t('执行记录') }, { value: 'config', label: <>{t('当前配置')}{editor.dirtyCount ? <Badge variant='warning' shape='tag'>{editor.dirtyCount}</Badge> : null}</> }]} /></div>
        <div className='routing-inspector-pane' hidden={panel !== 'execution'}><RoutingExecution active={view === 'workbench' && panel === 'execution'} selection={flow.selection} query={flow.query} paths={flow.snapshot?.paths || []} sticky={flow.sticky} now={flow.now} workspace={flow.workspace} onSelect={select} onConfig={model => openConfig({ kind: 'model', id: model }, model)} /></div>
        <div className='routing-inspector-pane' hidden={panel !== 'config'}><RoutingEditor editor={editor} target={target} models={models} now={flow.now} request={request} onExecution={() => setPanel('execution')} onAccount={id => setModal({ kind: 'account', id })} onSort={id => setModal({ kind: 'sort', id })} onProtection={workspace => setModal({ kind: 'protection', workspace })} /></div>
      </aside></div>
    </div>
    <div className='routing-trace-view' hidden={view !== 'requests'}><RoutingTracePage active={view === 'requests'} onWorkbench={openRequest} onConfig={model => openConfig({ kind: 'model', id: model }, model)} /></div>
    {modal?.kind === 'sticky' ? <RoutingStickyDialog model={flow.model} onClose={close} /> : null}
    {modal?.kind === 'protection' ? <RoutingSettingsDialog protectionOnly workspace={modal.workspace} onClose={close} /> : null}
    {modal?.kind === 'sort' ? <GroupSettingsDialog provider={modal.id} onClose={close} /> : null}
    {modal?.kind === 'account' ? <AccountSettingsDialog id={modal.id} initialAccount={flow.workspace?.accounts.find(account => account.id === modal.id)} onClose={close} /> : null}
  </>
}

const selector = '.page[data-page="routing"]'
function mount() {
  const section = document.querySelector<HTMLElement>(selector)
  if (!section || section.dataset.routingMounted === '1') return
  section.dataset.routingMounted = '1'
  createRoot(section).render(<RoutingPage />)
}
declare global { interface Window { wbRoutingPanel?: { load(): Promise<void>; openReplay(row?: ReplayRow | null): Promise<void> } } }
window.wbRoutingPanel = {
  async load() { refreshWorkspace?.() },
  async openReplay(row) { host().wbRequestDetail?.close?.(); host().wbApp?.showPage('routing'); requestTrace?.(row) },
}
if (document.querySelector(selector)) mount()
else document.addEventListener('DOMContentLoaded', mount, { once: true })
