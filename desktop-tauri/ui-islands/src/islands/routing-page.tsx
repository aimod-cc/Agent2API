import * as React from 'react'
import { createRoot } from 'react-dom/client'
import { Badge, Button, Empty, FieldGroup, Label, SegmentedControl, Spinner, cn, confirmDialog } from '@ui'
import { t } from '../i18n'
import { AccountSettingsDialog } from './accounts-settings-dialog'
import { providerOf } from './accounts-domain'
import { GroupSettingsDialog, manualOrderBlockedMessage } from './accounts-group-settings'
import { RoutingCanvas } from './routing-canvas'
import { ModelGatewayLink } from './routing-links'
import {
  AddRouteModelDialog, AddRouteProviderDialog, ProviderRoutingDialog, RoutingSettingsDialog,
} from './routing-settings'
import { RoutingTracePage } from './routing-trace-page'
import { RoutingStickyDialog } from './routing-sticky'
import { GlobalRoutingScopeDialog } from './routing-scope-dialog'
import {
  accountOrder, accountOrderLocked, accountOrderRules, addModel, cloneProfile, loadWorkspace, moveAccount, patch, saveProfile,
  selectModel, snapshot, subscribe,
} from './routing-state'
import { IconButton, RouteIcon } from './routing-ui'
import { host, type ReplayRow } from './routing-types'

type Modal = { kind: 'settings' | 'sticky' | 'add-model' | 'add-provider' | 'scope' }
  | { kind: 'provider' | 'sort'; id: string }
  | { kind: 'account'; id: string }
let requestTrace: ((row?: ReplayRow | null) => void) | undefined

function AccountRoutePosition({ id }: { id: string }) {
  const state = React.useSyncExternalStore(subscribe, snapshot)
  const account = state.workspace?.accounts.find(account => account.id === id)
  if (!account) return null
  const provider = providerOf(account)
  const ids = accountOrder(provider)
  const index = ids.indexOf(id)
  const locked = accountOrderLocked(provider)
  return <FieldGroup orientation='horizontal' className='field-row'>
    <Label>{t('当前路由位置')}</Label><span className='text-xs'>{state.model || t('全局配置')} · {t('第 {n} 位', { n: index + 1 })}</span>
    <IconButton name='arrowUp' title={t('提高优先级')} disabled={state.busy || index <= 0 || locked} onClick={() => void moveAccount(provider, id, -1)} />
    <IconButton name='arrowDown' title={t('降低优先级')} disabled={state.busy || index >= ids.length - 1 || locked} onClick={() => void moveAccount(provider, id, 1)} />
    <span className='detail'>{locked ? manualOrderBlockedMessage(accountOrderRules(provider)) : state.model ? t('顺序修改即时保存，仅作用于当前视图。') : t('全局顺序与账号页联动，修改即时保存。')}</span>
  </FieldGroup>
}

export function RoutingPage() {
  const state = React.useSyncExternalStore(subscribe, snapshot)
  const [modal, setModal] = React.useState<Modal | null>(null)
  const [mode, setMode] = React.useState<'config' | 'trace'>('config')
  const [initialTrace, setInitialTrace] = React.useState<ReplayRow | null>(null)
  const workspace = state.workspace
  React.useEffect(() => {
    requestTrace = row => { setModal(null); setInitialTrace(row || null); setMode('trace') }
    return () => { requestTrace = undefined }
  }, [])
  React.useEffect(() => {
    const timer = window.setInterval(() => {
      if (!document.hidden && mode === 'config' && host().wbApp?.currentPage === 'routing' && !state.busy && !modal) void loadWorkspace(true)
    }, 3000)
    return () => window.clearInterval(timer)
  }, [modal, state.busy, mode])
  const closeModal = () => { setModal(null); void loadWorkspace(true) }
  const configured = workspace?.configuredModels || []
  const models = state.model && !configured.some(model => model.toLowerCase() === state.model.toLowerCase()) ? [...configured, state.model] : configured
  const reset = async () => {
    const model = state.model
    if (!await confirmDialog(model ? t('确定删除「{model}」的独立路由配置？删除后该模型将使用全局路由。不会删除模型、账号或请求记录。', { model })
      : t('解除全局路由的参与范围限制，继续使用与账号页共用的手动顺序和自动消耗规则；模型独立配置保持不变。'), {
      title: model ? t('删除独立路由') : t('恢复默认路由'), confirmText: model ? t('删除') : t('恢复'), destructive: Boolean(model),
    })) return
    if (await saveProfile(null, model) && model && snapshot().model === model) await selectModel('')
  }
  const openReplay = () => { setInitialTrace(null); setMode('trace') }
  return <>
    <div className='routing-modebar'><SegmentedControl aria-label={t('路由视图')} value={mode} onValueChange={value => { closeModal(); setMode(value) }} options={[
      { value: 'config', label: <><RouteIcon name='settings' />{t('路由配置')}</> },
      { value: 'trace', label: <><RouteIcon name='requests' />{t('请求跟踪')}</> },
    ]} /><span>{mode === 'config' ? t('配置请求的参与范围与优先顺序') : t('查看实际请求的选路、尝试与结果')}</span></div>
    <div className='routing-config-view' hidden={mode !== 'config'}>
    {state.error ? <div className='routing-page-error' role='alert'><RouteIcon name='info' /><span>{state.error}</span><Button variant='ghost' size='sm' onClick={() => void loadWorkspace()}>{t('重试')}</Button></div> : null}
    <div className='routing-workspace'>
      {workspace ? <ModelGatewayLink model={state.model} zoom={state.zoom} revision={workspace.updatedAt} running={(workspace.metrics.running || 0) > 0} /> : null}
      <aside className='routing-model-rail'>
        <div className='routing-model-head'><span>{t('模型路由')}</span><IconButton name='plus' title={t('新增模型独立路由')} disabled={!workspace || state.busy} onClick={() => setModal({ kind: 'add-model' })} /></div>
        <Button variant='ghost' className={cn('routing-model-item', !state.model && 'is-selected')} disabled={state.busy} onClick={() => void selectModel('')}>
          <span className='routing-model-mark'><RouteIcon name='route' size={18} /></span><span><b>{t('全局配置')}</b><em>{t('未配置模型的默认路由')}</em></span>{!state.model ? <span className='routing-model-dot' /> : null}
        </Button>
        <div className='routing-model-divider'>{t('模型独立配置')}<Badge variant='secondary' shape='tag'>{configured.length}</Badge></div>
        <div className='routing-model-list'>{models.map(model => <Button key={model} variant='ghost' className={cn('routing-model-item', state.model.toLowerCase() === model.toLowerCase() && 'is-selected')} disabled={state.busy} onClick={() => void selectModel(model)}>
          <span className='routing-model-mark'><RouteIcon name='gateway' size={17} /></span><span><b title={model}>{model}</b><em>{configured.includes(model) ? t('独立路由') : t('继承全局')}</em></span>
          {state.model.toLowerCase() === model.toLowerCase() ? <span className='routing-model-dot' /> : null}
        </Button>)}</div>
        <Button variant='ghost' className='routing-add-model' disabled={!workspace || state.busy} onClick={() => setModal({ kind: 'add-model' })}><span>＋</span>{t('新增模型')}</Button>
        <div className='routing-model-help'><RouteIcon name='info' /><p>{t('切换模型后，右侧只展示该模型的提供商与账号路由。')}</p></div>
      </aside>
      <div className='routing-map-area'>
        <div className='routing-map-head'>
          <div className='routing-map-title'>
            <b>{state.model || t('全局配置')}</b>
            <Badge variant='brand' shape='tag'>{state.model ? workspace?.customized ? t('独立配置') : t('继承全局') : workspace?.profile.providerScope === 'all' ? t('自动参与') : t('指定提供商')}</Badge>
            {state.busy ? <span className='routing-save-state'><Spinner className='size-3' />{t('保存中…')}</span> : null}
          </div>
          <div className='routing-map-actions'>
            {!state.model ? <Button variant='outline' size='sm' disabled={!workspace || state.busy} onClick={() => setModal({ kind: 'scope' })}>{t('参与范围')}</Button> : null}
            {state.model && !workspace?.customized ? <Button variant='outline' size='sm' disabled={!workspace || state.busy} onClick={() => void saveProfile(cloneProfile())}>{t('创建独立配置')}</Button> : null}
            <Button variant='outline' size='sm' disabled={!workspace} onClick={openReplay}>
              <RouteIcon name='requests' /><span>{t('请求跟踪')}</span>
            </Button>
            <Button variant='outline' size='sm' disabled={!workspace} onClick={() => setModal({ kind: 'sticky' })}>
              <RouteIcon name='route' /><span>{t('会话粘滞')}</span>
              {workspace ? <Badge variant='secondary' shape='tag'>{workspace.stickyCount}</Badge> : null}
            </Button>
            <Button variant='outline' size='sm' disabled={!workspace} onClick={() => setModal({ kind: 'settings' })}>
              <RouteIcon name='settings' /><span>{t('设置')}</span>
            </Button>
            <IconButton name='refresh' title={t('刷新路由')} disabled={state.loading || state.busy} onClick={() => void loadWorkspace()} />
            <Button variant='ghost' size='sm' className={cn(state.model && 'text-destructive')} disabled={!workspace || state.busy || !(state.model ? workspace.customized : workspace.globalCustomized)} onClick={() => void reset()}>{state.model ? t('删除') : t('恢复默认')}</Button>
          </div>
        </div>
        {workspace && !state.model && workspace.profile.providerScope !== 'all' ? <div className='flex shrink-0 flex-wrap items-center gap-2 border-b border-border px-4 py-2 text-xs'>
          <span className='min-w-0 flex-1 text-muted-foreground'>{t('当前全局路由使用固定参与名单，新增提供商不会自动加入。')}</span>
          <Button variant='outline' size='sm' disabled={state.busy} onClick={() => setModal({ kind: 'scope' })}>{t('改为自动参与')}</Button>
        </div> : null}
        {workspace ? <RoutingCanvas workspace={workspace} search={state.search} zoom={state.zoom} selected={state.selectedProvider}
          onAccount={id => setModal({ kind: 'account', id })} onProvider={id => setModal({ kind: 'provider', id })}
          onSettings={() => setModal({ kind: 'settings' })} onAddProvider={() => setModal({ kind: 'add-provider' })}
          onSticky={() => setModal({ kind: 'sticky' })} onReplay={openReplay} />
          : <div className='routing-loading'><Empty>{state.loading ? <><Spinner />{t('正在读取路由…')}</> : t('路由工作台不可用')}</Empty></div>}
        <div className='routing-map-foot'><span>{state.model ? t('拖动提供商与账号调整顺序，修改即时保存。') : t('全局顺序与账号页联动，修改即时保存。')}</span><span>{t('队列为无会话预演；实际粘性命中和重试顺延以回放为准。')}</span><div><Button variant='ghost' size='sm' aria-label={t('缩小画布')} disabled={state.zoom <= 0.7} onClick={() => patch({ zoom: Math.max(0.7, state.zoom - 0.1) })}>−</Button><Button variant='ghost' size='sm' onClick={() => patch({ zoom: 1 })}>{Math.round(state.zoom * 100)}%</Button><Button variant='ghost' size='sm' aria-label={t('放大画布')} disabled={state.zoom >= 1.4} onClick={() => patch({ zoom: Math.min(1.4, state.zoom + 0.1) })}>＋</Button></div></div>
      </div>
    </div>
    </div>
    <div className='routing-trace-view' hidden={mode !== 'trace'}><RoutingTracePage active={mode === 'trace'} initialRow={initialTrace} onConfig={model => { void selectModel(model); setMode('config') }} /></div>
    {workspace && modal?.kind === 'scope' ? <GlobalRoutingScopeDialog workspace={workspace} onClose={closeModal} /> : null}
    {workspace && modal?.kind === 'settings' ? <RoutingSettingsDialog workspace={workspace} onClose={closeModal} /> : null}
    {workspace && modal?.kind === 'provider' ? <ProviderRoutingDialog key={`${state.model}:${modal.id}`} provider={modal.id} workspace={workspace} onSort={() => setModal({ kind: 'sort', id: modal.id })} onClose={closeModal} /> : null}
    {workspace && modal?.kind === 'sort' ? <GroupSettingsDialog provider={modal.id} onClose={closeModal} /> : null}
    {workspace && modal?.kind === 'add-provider' ? <AddRouteProviderDialog workspace={workspace} onClose={closeModal} /> : null}
    {workspace && modal?.kind === 'add-model' ? <AddRouteModelDialog workspace={workspace} onAdd={addModel} onClose={closeModal} /> : null}
    {modal?.kind === 'sticky' ? <RoutingStickyDialog model={state.model} onClose={closeModal} /> : null}
    {workspace && modal?.kind === 'account' ? <AccountSettingsDialog key={modal.id} id={modal.id} initialAccount={workspace.accounts.find(account => account.id === modal.id)}
      routeContext={<AccountRoutePosition id={modal.id} />} onClose={() => { closeModal(); void loadWorkspace() }} /> : null}
  </>
}

const selector = '.page[data-page="routing"]'
function mount() {
  const section = document.querySelector<HTMLElement>(selector)
  if (!section || section.dataset.routingMounted === '1') return
  section.dataset.routingMounted = '1'
  createRoot(section).render(<RoutingPage />)
  if (host().wbApp?.currentPage === 'routing') void loadWorkspace()
}
declare global {
  interface Window {
    wbRoutingPanel?: {
      load(): Promise<void>
      openReplay(row?: ReplayRow | null): Promise<void>
    }
  }
}
window.wbRoutingPanel = {
  load: () => loadWorkspace(),
  async openReplay(row) {
    host().wbRequestDetail?.close?.()
    host().wbApp?.showPage('routing')
    requestTrace?.(row)
  },
}
if (document.querySelector(selector)) mount()
else document.addEventListener('DOMContentLoaded', mount, { once: true })
