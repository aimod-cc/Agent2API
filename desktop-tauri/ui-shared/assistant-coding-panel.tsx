import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Alert, Button, Empty, Input, ResponsiveSheet } from '@ui'
import { CODING_BASE, codingConnection, type CodingConfig, type CodingDelivery, type CodingEngine, type CodingMachine, type CodingProject, type CodingRender, type CodingSession, type CodingTranslate, type CodingTransport } from './assistant-coding-types'
import { CodingSettings } from './assistant-coding-settings'
import { CodingNewSession } from './assistant-coding-new'
import { CodingConversation } from './assistant-coding-conversation'
import { CodingNewProject } from './assistant-coding-project-new'
import { CodingProjectList } from './assistant-coding-project-list'

export function CodingPanel({ engine, transport, t, renderText }: {
  engine: CodingEngine; transport: CodingTransport; t: CodingTranslate; renderText: CodingRender
}) {
  const [config, setConfig] = useState<CodingConfig | null>(null)
  const [sessions, setSessions] = useState<CodingSession[]>([])
  const [projects, setProjects] = useState<CodingProject[]>([])
  const [projectId, setProjectId] = useState('')
  const [projectOpen, setProjectOpen] = useState(false)
  const [machines, setMachines] = useState<CodingMachine[]>([])
  const [activeId, setActiveId] = useState('')
  const [drafts, setDrafts] = useState<Record<string, string>>({})
  const [deliveries, setDeliveries] = useState<Record<string, CodingDelivery[]>>({})
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [newOpen, setNewOpen] = useState(false)
  const [historyOpen, setHistoryOpen] = useState(false)
  const [loading, setLoading] = useState(true)
  const [refreshing, setRefreshing] = useState(false)
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  const [limited, setLimited] = useState(false)
  const [search, setSearch] = useState('')
  const mounted = useRef(true)
  const listRequest = useRef(0)
  const listBusy = useRef(false)
  const connectionIdentity = useRef('')
  const connectedTransport = useMemo(() => codingConnection(transport, config?.revision || ''), [transport, config?.revision])
  const load = useCallback(async () => {
    if (!mounted.current || listBusy.current) return
    listBusy.current = true
    const request = ++listRequest.current
    try {
      const settings = await transport.json<{ config: CodingConfig }>('GET', `${CODING_BASE}/config`)
      if (!mounted.current || request !== listRequest.current) return
      if (connectionIdentity.current && connectionIdentity.current !== settings.config.revision) {
        setActiveId(''); setProjectId(''); setProjects([]); setDrafts({}); setDeliveries({}); setSessions([]); setMachines([])
      }
      connectionIdentity.current = settings.config.revision
      setConfig(previous => previous?.revision === settings.config.revision ? previous : settings.config)
      const connected = codingConnection(transport, settings.config.revision)
      const projectList = await connected.json<{ projects: CodingProject[] }>('GET', `${CODING_BASE}/projects`)
      if (!mounted.current || request !== listRequest.current) return
      setProjects(projectList.projects)
      if (!settings.config.enabled) { setSessions([]); setMachines([]); setError(''); return }
      const [list, hosts] = await Promise.all([
        connected.json<{ sessions: CodingSession[]; projects: CodingProject[]; limited: boolean }>('GET', `${CODING_BASE}/sessions?engine=${engine}`),
        connected.json<{ machines: CodingMachine[] }>('GET', `${CODING_BASE}/machines`),
      ])
      if (!mounted.current || request !== listRequest.current) return
      setSessions(list.sessions); setProjects(list.projects); setLimited(list.limited); setMachines(hosts.machines); setError('')
    } catch (failure) {
      if (mounted.current && request === listRequest.current) { setError(failure instanceof Error ? failure.message : String(failure)); setMachines([]) }
    } finally {
      if (request === listRequest.current) { listBusy.current = false; if (mounted.current) setLoading(false) }
    }
  }, [transport, engine])
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; listRequest.current += 1; listBusy.current = false } }, [])
  useEffect(() => {
    setSessions([]); setSearch(''); setLoading(true)
    void load()
    const timer = setInterval(() => { if (document.visibilityState === 'visible') void load() }, 15_000)
    const foreground = () => { if (document.visibilityState === 'visible') void load() }
    document.addEventListener('visibilitychange', foreground)
    return () => { clearInterval(timer); listRequest.current += 1; listBusy.current = false; document.removeEventListener('visibilitychange', foreground) }
  }, [load])
  const openSession = useCallback((id: string) => {
    setActiveId(id); setHistoryOpen(false)
    const project = sessions.find(session => session.id === id)?.projectId
    if (project) setProjectId(project)
  }, [sessions])
  const newSession = (selectedProject?: string) => {
    if (selectedProject) setProjectId(selectedProject)
    setHistoryOpen(false)
    if (!projects.some(project => project.accessible)) setProjectOpen(true)
    else setNewOpen(true)
  }
  const changed = useCallback(() => { void load() }, [load])
  const refreshList = async () => {
    if (refreshing) return
    setRefreshing(true)
    try { await load() } finally { if (mounted.current) setRefreshing(false) }
  }
  const setDraft = useCallback((text: string) => {
    if (mounted.current && connectionIdentity.current === config?.revision) setDrafts(previous => ({ ...previous, [activeId]: text }))
  }, [activeId, config?.revision])
  const setDelivery = useCallback((localId: string, value: CodingDelivery | null) => {
    if (mounted.current && connectionIdentity.current === config?.revision) setDeliveries(previous => {
      const rows = previous[activeId] || []
      const next = value ? rows.some(row => row.localId === localId)
        ? rows.map(row => row.localId === localId ? value : row) : [...rows, value]
        : rows.filter(row => row.localId !== localId)
      return { ...previous, [activeId]: next }
    })
  }, [activeId, config?.revision])
  return <div className='flex min-h-0 flex-1 flex-col'>
    <div className='flex shrink-0 flex-wrap items-center gap-2 border-b border-border px-3 py-2'>
      <Button variant='ghost' size='sm' className='min-[900px]:hidden' onClick={() => setHistoryOpen(true)}>{t('历史会话')}</Button>
      <Button variant='ghost' size='sm' disabled={loading || refreshing} onClick={() => void refreshList()}>{refreshing ? t('读取中…') : t('刷新列表')}</Button>
      <Button variant='outline' size='sm' disabled={!config} onClick={() => setProjectOpen(true)}>{t('新建项目')}</Button>
      <Button variant='outline' size='sm' disabled={!config?.enabled || !machines.length} onClick={() => newSession()}>{t('新建会话')}</Button>
      <span className='flex-1' />
      <Button variant='ghost' size='sm' disabled={!config} onClick={() => setSettingsOpen(true)}>{t('本机设置')}</Button>
    </div>
    <div className='flex min-h-0 flex-1'>
      <ResponsiveSheet open={historyOpen} onOpenChange={setHistoryOpen} title={t('Codex 会话')} breakpoint={900} side='left'
        inlineAs='aside' inlineClassName='flex w-60 shrink-0 flex-col border-r border-border p-2' contentClassName='w-[min(320px,90vw)]' bodyClassName='gap-2'>
        <Input className='mb-2' value={search} onChange={e => setSearch(e.target.value)} placeholder={t('搜索会话或目录')} aria-label={t('搜索会话或目录')} />
        <CodingProjectList projects={projects} sessions={sessions} activeId={activeId} selectedProjectId={projectId} search={search} loading={loading} limited={limited}
          t={t} onOpen={openSession} onSelectProject={setProjectId} onNewSession={newSession} />
      </ResponsiveSheet>
      <div className='flex min-h-0 min-w-0 flex-1 flex-col'>
        {error ? <Alert variant='destructive' className='m-3'>{error}</Alert> : null}
        {notice ? <Alert variant='warning' className='m-3'>{notice}<Button size='sm' variant='ghost' onClick={() => setNotice('')}>{t('关闭')}</Button></Alert> : null}
        {loading && !config ? <Empty className='m-auto p-5'>{t('正在读取…')}</Empty>
          : !config ? <Empty className='m-auto p-5'><Button variant='outline' onClick={() => void load()}>{t('重试')}</Button></Empty>
          : !config.enabled ? <Empty className='m-auto max-w-xl p-5'>
            <h3 className='text-base font-medium'>{t('使用本机 Codex')}</h3>
            <p className='text-sm text-muted-foreground'>{t('直接连接这台电脑上已安装的 Codex，在电脑和手机之间继续工作，无需额外后台。')}</p>
            <Button onClick={() => setSettingsOpen(true)}>{t('设置本机 Codex')}</Button>
          </Empty> : activeId ? <CodingConversation key={`${config.revision}:${activeId}`} id={activeId} transport={connectedTransport} t={t} renderText={renderText}
            onChanged={changed} onForked={(id, warning) => { openSession(id); setNotice(warning || '') }} draft={drafts[activeId] || ''} onDraftChange={setDraft} deliveries={deliveries[activeId] || []} onDeliveryChange={setDelivery} />
            : <Empty className='m-auto max-w-xl p-5'>
              <h3 className='text-base font-medium'>{projects.length ? t('选择或新建 Codex 会话') : t('先新建一个项目')}</h3>
              <p className='text-sm text-muted-foreground'>{projects.length ? t('任务在 Agent2API 所在电脑上运行；手机只负责发送消息、查看进度和审批。') : t('选择一个文件夹作为项目，再在项目下开始对话。')}</p>
              {projects.length ? <Button disabled={!machines.length} onClick={() => newSession()}>{t('新建会话')}</Button>
                : <Button onClick={() => setProjectOpen(true)}>{t('新建项目')}</Button>}
              {!machines.length && !loading ? <p className='text-xs text-muted-foreground'>{t('本机 Codex 尚未就绪，请检查设置。')}</p> : null}
            </Empty>}
      </div>
    </div>
    <CodingSettings open={settingsOpen} onOpenChange={setSettingsOpen} config={config} transport={transport} t={t} onSaved={next => {
      listRequest.current += 1; listBusy.current = false
      if (next.revision !== config?.revision) { setActiveId(''); setProjectId(''); setProjects([]); setDrafts({}); setDeliveries({}) }
      connectionIdentity.current = next.revision
      setConfig(next); setSessions([]); setMachines([]); void load()
    }} />
    <CodingNewSession key={config?.revision || ''} open={newOpen} onOpenChange={setNewOpen} engine={engine} machines={machines} projects={projects} initialProjectId={projectId} transport={connectedTransport} t={t}
      onCreated={id => { openSession(id); void load() }} onNewProject={() => { setNewOpen(false); setProjectOpen(true) }} />
    <CodingNewProject key={`project:${config?.revision || ''}`} open={projectOpen} onOpenChange={setProjectOpen} transport={connectedTransport} t={t}
      initialDirectory={projects.find(project => project.id === projectId)?.directory} onCreated={project => {
        setProjects(previous => previous.some(item => item.id === project.id) ? previous.map(item => item.id === project.id ? project : item) : [...previous, project])
        setProjectId(project.id); setActiveId(''); setHistoryOpen(false); void load()
      }} />
  </div>
}
