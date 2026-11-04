import * as React from 'react'
import { mountOnPage } from '../island-lazy'
import { Alert, Badge, Button, Card, Empty, InputGroup, InputGroupAddon, InputGroupInput, NavItem, NavList, SegmentedControl, Sidebar, StatusDot, cn } from '@ui'
import { t } from '../i18n'
import { counts, getAgentStore, loadPanel, selectClient, selectedClient, setFilter, setQuery, subscribeAgentStore, visibleClients } from './agent-state'
import { detectionLabel } from './agent-model'
import { ProfileEditor } from './agent-editor'
import { PathDialog } from './agent-path-dialog'
import { ApplyDialog } from './agent-preview'
import { QuickConnectDialog } from './agent-quick-connect'
import { quickBlockReason } from './agent-quick-model'
import type { AgentClient, AgentOperation } from './agent-types'
import { AgentResourcesPanel } from '../../../ui-shared/agent-resources-panel'
import type { ResourceKind } from '../../../ui-shared/agent-resources'
import { callJson, toast } from './assistant-state'

function ClientAvatar({client,size = 'md'}:{client:AgentClient;size?:'md'|'lg'}) {
  const [broken,setBroken] = React.useState(false)
  const icon = Boolean(client.icon)&&!broken
  return <span className={cn('agent-avatar',size==='lg'&&'is-lg',icon&&'has-icon',icon&&client.iconFull&&'is-full')}
    style={{'--av-h':String(client.hue)} as React.CSSProperties} aria-hidden='true'>
    {icon?<img src={client.icon} alt='' loading='lazy' onError={()=>setBroken(true)}/>:client.letter}
  </span>
}

export function AgentPage() {
  const store = React.useSyncExternalStore(subscribeAgentStore,getAgentStore,getAgentStore)
  const client = selectedClient()
  const list = visibleClients(store)
  const stats = counts(store)
  const detected = list.filter(client=>client.detected)
  const missing = list.filter(client=>!client.detected)
  return <Card render={<section/>} className='panel agent-panel'>
    <div className='toolbar agent-toolbar'>
      <InputGroup className='agent-search'><InputGroupInput type='search' placeholder={t('搜索客户端 / 配置文件…')} aria-label={t('搜索客户端')} value={store.query} autoComplete='off' onChange={event=>setQuery(event.currentTarget.value)}/><InputGroupAddon aria-hidden='true'>⌕</InputGroupAddon></InputGroup>
      <SegmentedControl value={store.filter} onValueChange={setFilter} aria-label={t('按状态筛选')} options={[
        {value:'all',label:`${t('全部')} ${stats.all}`},{value:'via',label:`${t('接入本网关')} ${stats.via}`},{value:'direct',label:`${t('未接入本网关')} ${stats.direct}`},{value:'missing',label:`${t('未检测到')} ${stats.missing}`},
      ]}/>
    </div>
    {store.error&&<Alert variant='destructive' className='mx-4 mt-3'>{store.error}</Alert>}
    <div className='agent-body'>
      <Sidebar className='agent-rail p-0 gap-0' aria-label={t('客户端清单')}><NavList className='agent-rail-scroll'>
        {detected.length>0&&<><div className='agent-rail-label'>{t('已检测到')}（{detected.length}）</div>{detected.map(client=><ClientRow key={client.id} client={client} active={client.id===store.selected}/>)}</>}
        {missing.length>0&&<><div className='agent-rail-label'>{t('未检测到')}（{missing.length}）</div>{missing.map(client=><ClientRow key={client.id} client={client} active={client.id===store.selected}/>)}</>}
        {!list.length&&<Empty>{store.loading?t('正在检测客户端…'):t('没有匹配的客户端')}</Empty>}
      </NavList></Sidebar>
      <div className='agent-detail'>{client?<ClientDetail key={client.id} client={client}/>:<Empty>{t('选一个客户端开始')}
        <Button variant='outline' size='sm' disabled={store.loading||Boolean(store.busy)} onClick={()=>void loadPanel()}>{store.loading?t('检测中…'):t('重新检测')}</Button></Empty>}</div>
    </div>
  </Card>
}

function ClientRow({client,active}:{client:AgentClient;active:boolean}) {
  const live = client.live.find(profile=>profile.id===client.activeId)
  return <NavItem active={active} icon={<ClientAvatar client={client}/>} trailing={<StatusDot variant={client.viaGateway?'success':'muted'}/>}
    className={cn('agent-client',active&&'is-active',!client.detected&&'is-missing')} onClick={()=>selectClient(client.id)}>
    <span className='agent-client-main'><span className='agent-client-name'>{client.name}<span className='agent-client-ver'>{client.version}</span></span>
      <span className='agent-client-sub'>{live?.name||detectionLabel(client)}</span></span>
  </NavItem>
}

function ClientDetail({client}:{client:AgentClient}) {
  const store = React.useSyncExternalStore(subscribeAgentStore,getAgentStore,getAgentStore)
  const [pathOpen,setPathOpen] = React.useState(false)
  const [restore,setRestore] = React.useState<AgentOperation|null>(null)
  const [tab, setTab] = React.useState<string>('config')
  const [quickOpen, setQuickOpen] = React.useState(false)
  const quickReason = quickBlockReason(client)
  const busy = Boolean(store.busy)||store.loading
  return <>
    <div className='agent-dhead'><ClientAvatar client={client} size='lg'/><div className='agent-dtitle'><h3>{client.name}<Badge variant={client.writable?'success':'outline'}>{detectionLabel(client)}</Badge></h3>
      <div className='agent-meta'><span>{client.vendor}</span>{client.version&&<span>{client.version}</span>}</div></div>
      <div className='agent-dhead-actions'><Button size='sm' disabled={busy || Boolean(quickReason)} title={quickReason || undefined} onClick={() => setQuickOpen(true)}>{t('一键接入')}</Button>
        <Button variant='outline' size='sm' disabled={busy} onClick={()=>void loadPanel()}>{store.loading?t('检测中…'):t('重新检测')}</Button>
        <Button variant='ghost' size='sm' disabled={busy} onClick={()=>setPathOpen(true)}>{t('指定配置路径')}</Button></div></div>
    <SegmentedControl className='mt-3' value={tab} onValueChange={setTab} aria-label={t('客户端管理分类')} options={[
      { value: 'config', label: t('模型配置') }, { value: 'prompt', label: t('提示词') }, { value: 'skills', label: 'Skill' }, { value: 'mcp', label: 'MCP' },
    ]} />
    <p className='agent-machine-note'>{t('操作目标：网关所在机器')}{store.hostName?` · ${store.hostName}`:''}</p>
    {tab !== 'config' ? <AgentResourcesPanel key={tab} clientId={client.id} kind={tab as ResourceKind} capabilities={client.resources}
      request={callJson} t={t} notify={(text, bad) => toast(text, bad ? 'err' : 'ok')} /> : <>
    <code className='agent-resolved-path'>{client.resolvedPath}</code>
    {client.evidence.length>0&&<ul className='agent-evidence'>{client.evidence.map((evidence,index)=><li key={index}>{evidence}</li>)}</ul>}
    {!client.detected&&<Empty className='agent-empty'><b>{t('未找到客户端程序或配置文件')}</b><p>{t('安装或初始化后可重新检测；自定义安装位置可指定配置路径。')}</p>
      <Button variant='outline' size='sm' disabled={busy} onClick={()=>void loadPanel()}>{t('重新检测')}</Button></Empty>}
    <Alert className='mt-3' variant={!client.writable?'warning':'default'}>{client.supportNote}</Alert>
    <ProfileEditor client={client}/>
    {client.applied.error&&<Alert variant='warning' className='mt-3'>{client.applied.error}</Alert>}
    {(client.applied.canUndo||client.applied.canDisconnect)&&<div className='agent-actions'>
      {client.applied.canUndo&&<Button variant='outline' size='sm' disabled={busy} onClick={()=>setRestore('restore')}>{t('撤销上次应用')}</Button>}
      {client.applied.canDisconnect&&<Button variant='outline' size='sm' disabled={busy} onClick={()=>setRestore('disconnect')}>{t('解除接入本网关')}</Button>}
      {client.applied.modifiedExternally&&<p className='text-xs text-warning'>{t('客户端文件已有外部修改，自动恢复会拒绝覆盖。')}</p>}
      {client.applied.pendingRecovery&&<p className='text-xs text-warning'>{t('上次应用未完整结束，请先查看恢复预览。')}</p>}
    </div>}
    </>}
    {pathOpen&&<PathDialog client={client} onClose={()=>setPathOpen(false)}/>}
    {quickOpen&&<QuickConnectDialog client={client} onClose={()=>setQuickOpen(false)}/>}
    {restore&&<ApplyDialog client={client} operation={restore} onClose={()=>setRestore(null)}/>}
  </>
}

export default AgentPage

const PAGE_SELECTOR = '.page[data-page="agent"]'
let mounted = false
function mount(): void {
  if (mounted) return
  const section = document.querySelector<HTMLElement>(PAGE_SELECTOR)
  if (!section) return
  mounted = true; section.replaceChildren()
  void import('react-dom/client').then(({createRoot})=>createRoot(section).render(<AgentPage/>))
}
mountOnPage('agent', PAGE_SELECTOR, mount)
declare global { interface Window { wbAgentPanel?:{load:(silent?:boolean)=>Promise<void>} } }
window.wbAgentPanel = {load:loadPanel}
