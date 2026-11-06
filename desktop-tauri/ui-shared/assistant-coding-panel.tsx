import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Alert, Badge, Button, Empty, Input, ResponsiveSheet } from '@ui'
import { CODING_BASE, codingConnection, type CodingConfig, type CodingDelivery, type CodingEngine, type CodingMachine, type CodingRender, type CodingSession, type CodingTranslate, type CodingTransport } from './assistant-coding-types'
import { CodingSettings } from './assistant-coding-settings'
import { CodingNewSession } from './assistant-coding-new'
import { CodingConversation } from './assistant-coding-conversation'

export function CodingPanel({ engine, transport, t, renderText }: {
  engine: CodingEngine; transport: CodingTransport; t: CodingTranslate; renderText: CodingRender
}) {
  const [config, setConfig] = useState<CodingConfig | null>(null)
  const [sessions, setSessions] = useState<CodingSession[]>([])
  const [machines, setMachines] = useState<CodingMachine[]>([])
  const [activeIds, setActiveIds] = useState<Record<CodingEngine, string>>({ codex: '', claude: '' })
  const [drafts, setDrafts] = useState<Record<string, string>>({})
  const [deliveries, setDeliveries] = useState<Record<string, CodingDelivery | undefined>>({})
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [newOpen, setNewOpen] = useState(false)
  const [historyOpen, setHistoryOpen] = useState(false)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [limited, setLimited] = useState(false)
  const [search, setSearch] = useState('')
  const mounted = useRef(true)
  const listRequest = useRef(0)
  const hubIdentity = useRef('')
  const activeId = activeIds[engine]
  const connectedTransport = useMemo(() => codingConnection(transport, config?.revision || ''), [transport, config?.revision])
  const load = useCallback(async () => {
    if (!mounted.current) return
    const request = ++listRequest.current
    try {
      const settings = await transport.json<{ config: CodingConfig }>('GET', `${CODING_BASE}/config`)
      if (!mounted.current || request !== listRequest.current) return
      if (hubIdentity.current && hubIdentity.current !== settings.config.revision) { setActiveIds({ codex: '', claude: '' }); setDrafts({}); setDeliveries({}); setSessions([]) }
      hubIdentity.current = settings.config.revision
      setConfig(previous => previous?.revision === settings.config.revision && previous.enabled === settings.config.enabled ? previous : settings.config)
      if (!settings.config.enabled) { setSessions([]); setMachines([]); setError(''); return }
      const connected = codingConnection(transport, settings.config.revision)
      const [list, hosts] = await Promise.all([
        connected.json<{ sessions: CodingSession[]; limited: boolean }>('GET', `${CODING_BASE}/sessions?engine=${engine}`),
        connected.json<{ machines: CodingMachine[] }>('GET', `${CODING_BASE}/machines`),
      ])
      if (!mounted.current || request !== listRequest.current) return
      setSessions(list.sessions); setLimited(list.limited); setMachines(hosts.machines); setError('')
    } catch (failure) { if (mounted.current && request === listRequest.current) setError(failure instanceof Error ? failure.message : String(failure)) }
    finally { if (mounted.current && request === listRequest.current) setLoading(false) }
  }, [transport, engine])
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; listRequest.current += 1 } }, [])
  useEffect(() => {
    setSessions([]); setSearch(''); setLoading(true)
    void load()
    const timer = setInterval(() => { if (document.visibilityState === 'visible') void load() }, 15_000)
    const foreground = () => { if (document.visibilityState === 'visible') void load() }
    document.addEventListener('visibilitychange', foreground)
    return () => { clearInterval(timer); listRequest.current += 1; document.removeEventListener('visibilitychange', foreground) }
  }, [load])
  const openSession = useCallback((id: string) => { setActiveIds(previous => ({ ...previous, [engine]: id })); setHistoryOpen(false) }, [engine])
  const resumeSession = useCallback((id: string) => {
    setDrafts(previous => ({ ...previous, [id]: previous[id] || previous[activeId] || '' }))
    setDeliveries(previous => ({ ...previous, [id]: previous[id] || previous[activeId] }))
    openSession(id)
  }, [activeId, openSession])
  const changed = useCallback(() => { void load() }, [load])
  const setDraft = useCallback((text: string) => {
    if (mounted.current && hubIdentity.current === config?.revision) setDrafts(previous => ({ ...previous, [activeId]: text }))
  }, [activeId, config?.revision])
  const setDelivery = useCallback((value: CodingDelivery | null) => {
    if (mounted.current && hubIdentity.current === config?.revision) setDeliveries(previous => ({ ...previous, [activeId]: value ?? undefined }))
  }, [activeId, config?.revision])
  const visible = sessions.filter(session => `${session.title} ${session.directory}`.toLocaleLowerCase().includes(search.toLocaleLowerCase()))
  return <div className='flex min-h-0 flex-1 flex-col'>
    <div className='flex shrink-0 items-center gap-2 border-b border-border px-3 py-2'>
      <Button variant='ghost' size='sm' onClick={() => setHistoryOpen(true)}>{t('历史会话')}</Button>
      <Button variant='outline' size='sm' disabled={!config?.enabled || !machines.length} onClick={() => setNewOpen(true)}>{t('新建会话')}</Button>
      <span className='flex-1' />
      <Button variant='ghost' size='sm' onClick={() => setSettingsOpen(true)}>{t('连接设置')}</Button>
    </div>
    <div className='flex min-h-0 flex-1'>
      <ResponsiveSheet open={historyOpen} onOpenChange={setHistoryOpen} title={t('编程会话')} breakpoint={900} side='left'
        inlineAs='aside' inlineClassName='flex w-60 shrink-0 flex-col border-r border-border p-2' contentClassName='w-[min(320px,90vw)]' bodyClassName='gap-2'>
        <Input className='mb-2' value={search} onChange={e => setSearch(e.target.value)} placeholder={t('搜索会话或目录')} aria-label={t('搜索会话或目录')} />
        <div className='min-h-0 flex-1 space-y-1 overflow-auto'>
          {visible.map(session => <Button key={session.id} variant={session.id === activeId ? 'secondary' : 'ghost'} size='default' className='h-auto w-full justify-start px-2 py-2 text-left' onClick={() => openSession(session.id)}>
            <span className='min-w-0 flex-1'><span className='block truncate text-sm'>{session.title || t('未命名会话')}</span>
              <span className='block truncate text-xs font-normal text-muted-foreground'>{session.directory}</span></span>
            {session.pendingCount ? <Badge variant='outline'>{session.pendingCount}</Badge> : session.thinking ? <span className='text-xs'>{t('执行中')}</span> : null}
          </Button>)}
          {!visible.length && !loading ? <p className='px-2 py-4 text-xs text-muted-foreground'>{t('没有匹配的会话')}</p> : null}
          {limited ? <p className='p-2 text-xs text-muted-foreground'>{t('这里只显示最近的 500 条会话。')}</p> : null}
        </div>
        <Button variant='ghost' size='sm' disabled={loading} onClick={() => void load()}>{t('刷新列表')}</Button>
      </ResponsiveSheet>
      <div className='flex min-h-0 min-w-0 flex-1 flex-col'>
        {error ? <Alert variant='destructive' className='m-3'>{error}</Alert> : null}
        {!config?.enabled ? <Empty className='m-auto max-w-xl p-5'>
          <h3 className='text-base font-medium'>{t('连接电脑上的编程工具')}</h3>
          <p className='text-sm text-muted-foreground'>{t('使用自己的 HAPI 后台，在电脑和手机之间继续 Codex、Claude Code 会话。')}</p>
          <Button onClick={() => setSettingsOpen(true)}>{t('设置连接')}</Button>
        </Empty> : activeId ? <CodingConversation key={`${config.revision}:${activeId}`} id={activeId} transport={connectedTransport} t={t} renderText={renderText}
          onResume={resumeSession} onChanged={changed} draft={drafts[activeId] || ''} onDraftChange={setDraft}
          delivery={deliveries[activeId]} onDeliveryChange={setDelivery} /> : <Empty className='m-auto max-w-xl p-5'>
          <h3 className='text-base font-medium'>{t('选择或新建编程会话')}</h3>
          <p className='text-sm text-muted-foreground'>{t('任务在选定电脑上运行；手机只负责发送消息、查看进度和审批。')}</p>
          <Button disabled={!machines.length} onClick={() => setNewOpen(true)}>{t('新建会话')}</Button>
          {!machines.length ? <p className='text-xs text-muted-foreground'>{t('尚无在线电脑。请先启动 HAPI Runner。')}</p> : null}
        </Empty>}
      </div>
    </div>
    <CodingSettings open={settingsOpen} onOpenChange={setSettingsOpen} config={config} transport={transport} t={t} onSaved={next => {
      if (next.revision !== config?.revision) { setActiveIds({ codex: '', claude: '' }); setDrafts({}); setDeliveries({}) }
      hubIdentity.current = next.revision
      setConfig(next); setSessions([]); setMachines([]); void load()
    }} />
    <CodingNewSession open={newOpen} onOpenChange={setNewOpen} engine={engine} machines={machines} transport={connectedTransport} t={t} onCreated={id => { openSession(id); void load() }} />
  </div>
}
