import { Fragment, useEffect, useMemo, useRef, useState } from 'react'
import { Alert, Badge, Button, confirmDialog } from '@ui'
import { codingFailureIsDefinite, codingMessageId, codingPath, type CodingDelivery, type CodingRender, type CodingTranslate, type CodingTransport } from './assistant-coding-types'
import { decodeCodingMessages } from './assistant-coding-messages'
import { useCodingSession } from './assistant-coding-session'
import { CodingPermissionCard } from './assistant-coding-permissions'
import { CodingComposer } from './assistant-coding-composer'
import { AssistantMessageActions } from './assistant-message-actions'
import { CodingOutput } from './assistant-coding-output'

export function CodingConversation({ id, transport, t, renderText, onChanged, onForked, draft, onDraftChange, deliveries, onDeliveryChange }: {
  id: string; transport: CodingTransport; t: CodingTranslate; renderText: CodingRender
  onChanged: () => void; onForked: (id: string, warning?: string) => void; draft: string; onDraftChange: (text: string) => void
  deliveries: CodingDelivery[]; onDeliveryChange: (localId: string, delivery: CodingDelivery | null) => void
}) {
  const state = useCodingSession(transport, id, true)
  const session = state.session
  const [busy, setBusy] = useState(false)
  const [settingsSaving, setSettingsSaving] = useState(false)
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  const uncertain = deliveries.some(delivery => delivery.state === 'uncertain')
  const gate = useRef(false)
  const mounted = useRef(true)
  const scroll = useRef<HTMLDivElement>(null)
  const nearBottom = useRef(true)
  const settled = useRef(new Set<string>())
  const blocks = useMemo(() => decodeCodingMessages(state.items), [state.items])
  const liveConnection = state.connected && session?.connected !== false
  const forkableTurns = useMemo(() => new Set(state.turns.filter(turn => turn.canFork).map(turn => turn.id)), [state.turns])
  const forkTargets = useMemo(() => {
    const targets = new Map<string, string>()
    for (const block of blocks) {
      if (!block.turnId || !forkableTurns.has(block.turnId)) continue
      if (block.role === 'assistant' && block.kind === 'text' && !block.streaming) targets.set(block.turnId, block.id)
    }
    return targets
  }, [blocks, forkableTurns])
  const visibleDeliveries = deliveries.filter(delivery => !state.items.some(item => item.type === 'userMessage' && item.clientId === delivery.localId)
    && !state.submissions.some(item => item.localId === delivery.localId && item.state === 'consumed'))
  const canSend = Boolean(session?.accessible && (!session.thinking || session.activeTurnId) && liveConnection && !busy && !settingsSaving && !uncertain
    && !deliveries.some(delivery => delivery.state === 'sending') && visibleDeliveries.length < 20)
  useEffect(() => { mounted.current = true; return () => { mounted.current = false } }, [])
  useEffect(() => {
    if (nearBottom.current && scroll.current) scroll.current.scrollTop = scroll.current.scrollHeight
  }, [blocks, session?.pendingCount, deliveries])
  useEffect(() => {
    for (const delivery of deliveries) {
      const recorded = state.items.some(item => item.type === 'userMessage' && item.clientId === delivery.localId)
        || state.submissions.some(item => item.localId === delivery.localId && item.state === 'consumed')
      if (!recorded) continue
      settled.current.add(delivery.localId)
      onDeliveryChange(delivery.localId, null); setError('')
      if (delivery.state !== 'accepted' && draft === delivery.text) onDraftChange('')
    }
  }, [state.items, state.submissions, deliveries, draft, onDraftChange, onDeliveryChange])
  const refresh = async () => {
    setError('')
    try { await state.refresh(); onChanged() }
    catch (failure) { if (mounted.current) setError(failure instanceof Error ? failure.message : String(failure)) }
  }
  const action = async (name: 'resume' | 'abort' | 'archive') => {
    if (gate.current) return
    if (name === 'archive' && !await confirmDialog(t('结束此编程会话？正在执行的任务会停止，历史记录仍然保留。'), { title: t('结束会话') })) return
    gate.current = true; setBusy(true); setError('')
    try {
      await transport.json('POST', codingPath(id, `/actions/${name}`), {})
      onChanged()
      if (mounted.current) await state.refresh()
    } catch (failure) { if (mounted.current) setError(failure instanceof Error ? failure.message : String(failure)) }
    finally { gate.current = false; if (mounted.current) setBusy(false) }
  }
  const openTerminal = async () => {
    if (gate.current || !session || session.terminalOpening || !liveConnection) return
    gate.current = true; setBusy(true); setError(''); setNotice('')
    try {
      await transport.json('POST', codingPath(id, '/terminal'), {})
      if (mounted.current) {
        setNotice(t('已请求在电脑上打开官方 Codex 终端，连接成功后会显示共享状态。'))
        await state.refresh()
      }
    } catch (failure) { if (mounted.current) setError(failure instanceof Error ? failure.message : String(failure)) }
    finally { gate.current = false; if (mounted.current) setBusy(false) }
  }
  const fork = async (lastTurnId: string) => {
    if (gate.current || !liveConnection || !forkableTurns.has(lastTurnId)) return
    if (!await confirmDialog(t('复制到这一轮结束为止的对话，创建独立分支。原会话保留，项目文件不会回退；两个会话仍使用同一目录。'), { title: t('创建会话分支') })) return
    if (gate.current || !mounted.current) return
    gate.current = true; setBusy(true); setError(''); setNotice('')
    try {
      const result = await transport.json<{ sessionId: string; warning?: string }>('POST', codingPath(id, '/fork'), { lastTurnId })
      if (mounted.current) onForked(result.sessionId, result.warning)
      onChanged()
    } catch (failure) { if (mounted.current) setError(failure instanceof Error ? failure.message : String(failure)) }
    finally { gate.current = false; if (mounted.current) setBusy(false) }
  }
  const send = async () => {
    if (gate.current || !canSend || !draft.trim()) return
    gate.current = true; setBusy(true); setError(''); setNotice('')
    const input = { text: draft, localId: codingMessageId(), ...(session?.thinking && session.activeTurnId ? { expectedTurnId: session.activeTurnId } : {}) }
    onDeliveryChange(input.localId, { ...input, state: 'sending' })
    nearBottom.current = true
    try {
      await transport.json('POST', codingPath(id, '/messages'), input)
      onChanged()
      if (!settled.current.has(input.localId)) onDeliveryChange(input.localId, { ...input, state: 'accepted' })
      onDraftChange('')
      if (mounted.current) await state.refresh().catch(() => {})
    } catch (failure) {
      if (!settled.current.has(input.localId)) {
        onDeliveryChange(input.localId, codingFailureIsDefinite(failure) ? null : { ...input, state: 'uncertain' })
      }
      if (mounted.current) { setError(failure instanceof Error ? failure.message : String(failure)); void state.refresh().catch(() => {}) }
    } finally { gate.current = false; if (mounted.current) setBusy(false) }
  }
  const dismissDelivery = async (delivery: CodingDelivery) => {
    if (!await confirmDialog(t('请确认这条消息没有被 Codex 执行。解除后再次发送可能重复修改文件或运行命令。'), { title: t('解除发送暂停') })) return
    onDeliveryChange(delivery.localId, null); setError(''); setNotice('')
  }
  return <div className='flex min-h-0 flex-1 flex-col'>
    <div className='flex flex-wrap items-center gap-2 border-b border-border px-3 py-2'>
      <div className='min-w-0 flex-1'><div className='truncate text-sm font-medium'>{session?.title || t('Codex 会话')}</div>
        <div className='truncate text-xs text-muted-foreground' title={session?.directory}>{session?.directory || id}</div></div>
      {session?.permissionMode ? <Badge variant='outline'>{session.permissionMode === 'read-only' ? t('只读') : session.permissionMode === 'default' ? t('项目内读写') : session.permissionMode === 'full-access' ? t('完全访问') : t('受限')}</Badge> : null}
      <Badge variant='outline'>{session?.pendingCount ? t('等待你处理') : session?.thinking ? t('正在执行') : session?.active ? t('就绪') : t('历史会话')}</Badge>
      {session?.terminalCount ? <Badge variant='outline'>{t('终端共享中 · {count}', { count: session.terminalCount })}</Badge> : null}
      <Button variant='outline' size='sm' disabled={busy || !session || session.terminalOpening || !liveConnection} onClick={() => void openTerminal()}>
        {session?.terminalOpening ? t('正在打开终端…') : t('打开官方终端')}
      </Button>
      <Button variant='ghost' size='sm' disabled={busy} onClick={() => void refresh()}>{t('刷新')}</Button>
      {session?.active ? <Button variant='ghost' size='sm' disabled={busy || settingsSaving || !liveConnection || session.terminalCount > 0 || session.terminalOpening} onClick={() => void action('archive')}>{t('结束会话')}</Button> : null}
    </div>
    <div ref={scroll} onScroll={() => { const node = scroll.current; if (node) nearBottom.current = node.scrollHeight - node.scrollTop - node.clientHeight < 100 }} className='min-h-0 flex-1 overflow-auto'>
      <div className='mx-auto flex max-w-4xl flex-col gap-4 px-3 py-4'>
        {state.loading ? <p className='text-sm text-muted-foreground'>{t('正在读取会话…')}</p> : null}
        {state.error ? <Alert variant='destructive'>{state.error}</Alert> : null}
        {!liveConnection && !state.loading ? <p className='text-xs text-muted-foreground'>{t('实时连接已断开，正在重连；电脑上的任务不会因此停止。')}</p> : null}
        {session?.terminalError ? <Alert variant='warning'>{session.terminalError}</Alert> : null}
        {blocks.map((block, index) => <Fragment key={block.id}>
          <CodingOutput block={block} t={t} renderText={renderText} forkDisabled={busy || settingsSaving || !liveConnection}
            onFork={block.turnId && forkTargets.get(block.turnId) === block.id ? () => { void fork(block.turnId!) } : undefined} />
          {block.turnId && blocks[index + 1]?.turnId !== block.turnId && forkableTurns.has(block.turnId) && !forkTargets.has(block.turnId) ?
            <AssistantMessageActions text='' t={t} onFork={() => { void fork(block.turnId!) }} forkDisabled={busy || settingsSaving || !liveConnection} forkLabel={t('从这一轮创建分支')} /> : null}
        </Fragment>)}
        {visibleDeliveries.map(delivery => <div key={delivery.localId} className='ml-auto max-w-[90%] rounded-lg bg-primary/10 px-3 py-2'>
          <div className='whitespace-pre-wrap break-words text-sm'>{delivery.text}</div>
          <p className='mt-1 text-xs text-muted-foreground'>{delivery.state === 'sending' ? t('正在发送…') : delivery.state === 'uncertain' ? t('发送结果待确认') : t('Codex 已接收，正在同步会话。')}</p>
          {delivery.state === 'accepted' && !session?.thinking ? <p className='mt-1 text-xs text-muted-foreground'>{t('消息已接收，但尚未在历史中确认；请核对后再决定是否重发。')}</p> : null}
          {delivery.state === 'uncertain' || delivery.state === 'accepted' && !session?.thinking ? <Button variant='ghost' size='sm' onClick={() => void dismissDelivery(delivery)}>{t('已核对，移除此提示')}</Button> : null}
        </div>)}
        {session?.permissions.map(request => <CodingPermissionCard key={request.id} request={request} sessionId={id} transport={transport} t={t}
          disabled={!session.active || !liveConnection} onDone={() => { void refresh() }} />)}
        {!blocks.length && !deliveries.length && !state.loading && !state.error ? <p className='py-10 text-center text-sm text-muted-foreground'>{t('发送任务，开始使用本机 Codex。')}</p> : null}
        {session?.thinking ? <p className='text-xs text-muted-foreground'>{t('Codex 正在电脑上执行…')}</p> : null}
      </div>
    </div>
    <div className='mx-auto w-full max-w-4xl shrink-0 space-y-2 px-3 pb-3 pt-2'>
      {error ? <Alert variant='destructive'>{error}</Alert> : null}
      {notice ? <p role='status' className='text-xs text-muted-foreground'>{notice}</p> : null}
      {uncertain ? <Alert variant='warning'>{t('为防止重复执行，已暂停发送。先刷新会话检查这条消息是否已出现。')}</Alert> : null}
      {visibleDeliveries.length >= 20 ? <p className='text-xs text-muted-foreground'>{t('已有 20 条消息等待同步，请稍后继续发送。')}</p> : null}
      {session?.terminalCount ? <p className='text-xs text-muted-foreground'>{t('官方终端仍已连接。停止任务两端都会生效；结束会话前请先关闭终端。')}</p> : null}
      {session && !session.active ? <div className='flex flex-wrap items-center gap-2'>
        <Button variant='outline' size='sm' disabled={busy || !liveConnection} onClick={() => void action('resume')}>{t('恢复会话')}</Button>
        <p className='text-xs text-muted-foreground'>{t('发送时会自动恢复历史；本应用未记录权限的旧会话按只读恢复，不会擅自放宽权限。')}</p>
      </div> : null}
      {session?.thinking ? <p className='text-xs text-muted-foreground'>{t('执行中发送会追加到当前任务，不会另开一轮；等待审批时仍需处理审批。')}</p> : null}
      <CodingComposer session={session} transport={transport} t={t} draft={draft} onDraftChange={onDraftChange}
        canSend={canSend} busy={busy} connected={Boolean(liveConnection)} settingsSaving={settingsSaving} onSavingChange={setSettingsSaving}
        onSettingsChanged={state.refresh} onSend={() => { void send() }} onStop={() => { void action('abort') }} />
      <p className='text-center text-xs text-muted-foreground'>{t('通过这里打开的官方终端与手机共享同一条会话。关闭终端或页面不停止任务；退出 Agent2API 会停止后台。')}</p>
    </div>
  </div>
}
