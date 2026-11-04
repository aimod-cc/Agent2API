import * as React from 'react'
import { mountOnPage } from '../island-lazy'
import { createRoot } from 'react-dom/client'
import { Badge, Button, Empty, SegmentedControl, Spinner, confirmDialog } from '@ui'
import { t } from '../i18n'
import { AccountSettingsDialog } from './accounts-settings-dialog'
import { GroupSettingsDialog } from './accounts-group-settings'
import { AddRouteModelDialog, RoutingSettingsDialog } from './routing-settings'
import { RoutingStickyDialog } from './routing-sticky'
import { RoutingConfigPanel } from './routing-config-panel'
import { RoutingExecTimeline } from './routing-exec-timeline'
import { RoutingRequestList } from './routing-request-list'
import { useRoutingReplay } from './routing-replay-state'
import { draftDirty, useRoutingEditor } from './routing-editor-state'
import { RouteIcon, stickyLabel, strategyLabel } from './routing-ui'
import { isTraceRunning, requestKey } from './routing-trace-model'
import { bridge, host, type ReplayRow, type RoutingWorkspace } from './routing-types'

type ConfigTarget = { kind: 'model' | 'provider' | 'account'; id: string }

let requestTrace: ((row?: ReplayRow | null) => void) | undefined
let refreshWorkspace: (() => void) | undefined

/**
 * 路由页只有两个视图：
 *
 *   请求回放：左栏最近请求，右栏回放单条请求的候选、跳过原因、尝试与配置快照。
 *   配置：左栏作用范围，中间有序名单，右栏「此刻队列」预演当前模型会走哪套配置。
 *
 * 原来的聚合画布已移除：它回答的是「流量去哪了」，不是「这次为什么是它」。
 */
export function RoutingPage() {
  const [mode, setMode] = React.useState<'replay' | 'config'>('replay')
  const [selected, setSelected] = React.useState<ReplayRow | null>(null)
  const [refreshToken, setRefreshToken] = React.useState(0)
  const replay = useRoutingReplay(true)
  const editor = useRoutingEditor(replay.refresh)
  const [modal, setModal] = React.useState<
    | { kind: 'sticky' }
    | { kind: 'account' | 'sort'; id: string }
    | { kind: 'protection'; workspace: RoutingWorkspace }
    | { kind: 'model' }
    | null
  >(null)
  const settings = mode === 'config' ? (editor.draft?.workspace.settings || replay.workspace?.settings) : replay.workspace?.settings

  React.useEffect(() => {
    if (!selected || !isTraceRunning(selected)) return
    let alive = true
    const timer = window.setInterval(async () => {
      if (document.hidden || host().wbApp?.currentPage !== 'routing') return
      try {
        const next = await bridge().getRoutingTraceRequest({ id: selected.id, ts: selected.ts })
        if (alive) setSelected(current => current && requestKey(current) === requestKey(next) ? next : current)
      } catch { /* 下个周期再试 */ }
    }, 2500)
    return () => { alive = false; window.clearInterval(timer) }
  }, [selected?.id, selected?.ts, selected && isTraceRunning(selected)])

  const openRequest = (row: ReplayRow) => { setSelected(row); setMode('replay') }
  const openConfig = (target: ConfigTarget, model?: string) => {
    setMode('config')
    if (editor.busy) return
    const scope = target.kind === 'model' ? target.id : model || replay.model || ''
    void editor.load(scope || '')
  }
  const refreshReplay = async () => {
    replay.refresh()
    setRefreshToken(value => value + 1)
    if (!selected) return
    try {
      const next = await bridge().getRoutingTraceRequest({ id: selected.id, ts: selected.ts })
      setSelected(next)
    } catch { /* 列表下次刷新会带回新状态 */ }
  }
  const close = () => { setModal(null); replay.refresh() }
  const reloadConfig = async () => {
    if (editor.draft && draftDirty(editor.draft)
      && !await confirmDialog(t('重新读取会丢弃当前范围的未保存草稿，其他范围的草稿保留。'), { title: t('重新读取配置'), confirmText: t('重新读取') })) return
    void editor.load(editor.scope, true)
  }
  React.useEffect(() => {
    requestTrace = row => {
      if (!row) return
      setSelected({ ...row })
      setMode('replay')
    }
    refreshWorkspace = replay.refresh
    return () => { requestTrace = undefined; refreshWorkspace = undefined }
  }, [replay.refresh])

  return <>
    <div className='routing-page-bar'>
      <SegmentedControl aria-label={t('路由模式')} value={mode} onValueChange={value => {
        setMode(value as 'replay' | 'config')
        if (value === 'config' && !editor.draft && !editor.busy) void editor.load(editor.scope || '')
      }} options={[
        { value: 'replay', label: <><RouteIcon name='requests' />{t('请求回放')}</> },
        { value: 'config', label: <><RouteIcon name='route' />{t('配置')}{editor.dirtyCount ? <Badge variant='warning' shape='tag'>{editor.dirtyCount}</Badge> : null}</> },
      ]} />
      <span className='routing-page-policy'>{settings ? `${strategyLabel(settings.routingStrategy)} · ${t('粘滞 {mode}', { mode: stickyLabel(settings.routingSticky) })}` : ''}</span>
      <div className='routing-page-actions'>
        <Button variant='ghost' size='sm' onClick={() => setModal({ kind: 'sticky' })}>
          <RouteIcon name='lock' />{t('会话粘滞')}{replay.sticky.length ? ` · ${replay.sticky.length}` : ''}
        </Button>
        {mode === 'replay'
          ? <Button variant='ghost' size='sm' disabled={replay.loading} onClick={() => void refreshReplay()}><RouteIcon name='refresh' />{t('刷新')}</Button>
          : <Button variant='ghost' size='sm' disabled={editor.loading || editor.busy} onClick={() => void reloadConfig()}><RouteIcon name='refresh' />{t('重新读取')}</Button>}
      </div>
    </div>

    {replay.error && mode === 'replay' ? <div className='routing-page-error' role='alert'>
      <RouteIcon name='info' /><span>{replay.error}</span><Button variant='ghost' size='sm' onClick={replay.refresh}>{t('重试')}</Button>
    </div> : null}

    {mode === 'replay' ? <div className='routing-replay-layout'>
      <aside className='routing-request-rail'>
        <RoutingRequestList active={mode === 'replay'} query={replay.query}
          selectedKey={selected ? requestKey(selected) : ''} refreshToken={refreshToken}
          onOpenRequest={openRequest} onHistory={() => host().wbApp?.showPage('requests')}
          onModel={replay.selectModel} onMinutes={replay.setMinutes} onScope={replay.setScope} onSearch={replay.setSearch} />
      </aside>
      <section className='routing-replay-main'>
        <div className='routing-replay-head'>
          <b><RouteIcon name='requests' />{t('请求执行')}</b>
          <code>{selected?.id || '—'}</code>
          <div className='right'>
            <Button size='sm' disabled={!selected} onClick={() => selected && host().wbRequestDetail?.open(selected.id, selected)}>{t('完整请求详情')}</Button>
          </div>
        </div>
        <div className='routing-replay-scroll'>
          {selected
            ? <RoutingExecTimeline row={selected} now={replay.now}
              onDetail={row => host().wbRequestDetail?.open(row.id, row)}
              onConfig={model => openConfig({ kind: 'model', id: model }, model)} />
            : <Empty>{replay.loading ? <Spinner /> : t('选择左侧请求查看选路过程')}</Empty>}
        </div>
      </section>
    </div> : <div className='routing-config-view'>
      {editor.error && !editor.draft ? <div className='routing-page-error' role='alert'>
        <RouteIcon name='info' /><span>{editor.error}</span><Button variant='ghost' size='sm' onClick={() => void editor.load(editor.scope, true)}>{t('重新读取')}</Button>
      </div> : null}
      <RoutingConfigPanel editor={editor} scope={editor.scope} now={replay.now}
        onScope={scope => { if (!editor.busy) void editor.load(scope) }}
        onAccount={id => setModal({ kind: 'account', id })}
        onSort={provider => setModal({ kind: 'sort', id: provider })}
        onAddModel={() => setModal({ kind: 'model' })}
        onProtection={workspace => setModal({ kind: 'protection', workspace })} />
    </div>}

    {modal?.kind === 'sticky' ? <RoutingStickyDialog model={mode === 'config' ? editor.scope || replay.model : replay.model} onClose={close} /> : null}
    {modal?.kind === 'protection' ? <RoutingSettingsDialog protectionOnly workspace={modal.workspace} onClose={close} /> : null}
    {modal?.kind === 'sort' ? <GroupSettingsDialog provider={modal.id} onClose={close} /> : null}
    {modal?.kind === 'account' ? <AccountSettingsDialog id={modal.id} initialAccount={editor.draft?.workspace.accounts.find(account => account.id === modal.id) || replay.workspace?.accounts.find(account => account.id === modal.id)} onClose={close} /> : null}
    {modal?.kind === 'model' && editor.draft ? <AddRouteModelDialog workspace={editor.draft.workspace} onClose={() => setModal(null)}
      onAdd={editor.create} /> : null}
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
mountOnPage('routing', selector, mount)
