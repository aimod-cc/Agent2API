import { useEffect, useMemo, useRef, useState } from 'react'
import { Alert, Badge, Button, Textarea, confirmDialog } from '@ui'
import { codingFailureIsDefinite, codingMessageId, codingPath, type CodingDelivery, type CodingRender, type CodingTranslate, type CodingTransport } from './assistant-coding-types'
import { decodeCodingMessages } from './assistant-coding-messages'
import { useCodingSession } from './assistant-coding-session'
import { CodingPermissionCard } from './assistant-coding-permissions'

export function CodingConversation({ id, transport, t, renderText, onChanged, draft, onDraftChange, delivery, onDeliveryChange }: {
  id: string; transport: CodingTransport; t: CodingTranslate; renderText: CodingRender
  onChanged: () => void; draft: string; onDraftChange: (text: string) => void
  delivery?: CodingDelivery; onDeliveryChange: (delivery: CodingDelivery | null) => void
}) {
  const state = useCodingSession(transport, id, true)
  const session = state.session
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  const uncertain = delivery?.state === 'uncertain'
  const gate = useRef(false)
  const mounted = useRef(true)
  const scroll = useRef<HTMLDivElement>(null)
  const nearBottom = useRef(true)
  const settled = useRef(new Set<string>())
  const blocks = useMemo(() => decodeCodingMessages(state.items), [state.items])
  const liveConnection = state.connected && session?.connected !== false
  const canSend = Boolean(session?.accessible && (!session.thinking || session.activeTurnId) && !session.pendingCount && liveConnection && !busy && !delivery)
  useEffect(() => { mounted.current = true; return () => { mounted.current = false } }, [])
  useEffect(() => {
    if (nearBottom.current && scroll.current) scroll.current.scrollTop = scroll.current.scrollHeight
  }, [blocks, session?.pendingCount, delivery])
  useEffect(() => {
    if (!delivery || !state.items.some(item => item.type === 'userMessage' && item.clientId === delivery.localId)) return
    settled.current.add(delivery.localId)
    onDeliveryChange(null); setError(''); setNotice('')
    if (draft === delivery.text) onDraftChange('')
  }, [state.items, draft, delivery, onDraftChange, onDeliveryChange])
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
  const send = async () => {
    if (gate.current || !canSend || !draft.trim()) return
    gate.current = true; setBusy(true); setError(''); setNotice('')
    const input = { text: draft, localId: codingMessageId(), ...(session?.thinking && session.activeTurnId ? { expectedTurnId: session.activeTurnId } : {}) }
    onDeliveryChange({ ...input, state: 'sending' })
    nearBottom.current = true
    try {
      await transport.json('POST', codingPath(id, '/messages'), input)
      onChanged()
      if (!settled.current.has(input.localId)) { onDeliveryChange({ ...input, state: 'accepted' }); onDraftChange('') }
      if (mounted.current) await state.refresh().catch(() => {})
    } catch (failure) {
      if (!settled.current.has(input.localId)) {
        onDeliveryChange(codingFailureIsDefinite(failure) ? null : { ...input, state: 'uncertain' })
      }
      if (mounted.current) { setError(failure instanceof Error ? failure.message : String(failure)); void state.refresh().catch(() => {}) }
    } finally { gate.current = false; if (mounted.current) setBusy(false) }
  }
  const dismissDelivery = async () => {
    if (!delivery) return
    if (!await confirmDialog(t('请确认这条消息没有被 Codex 执行。解除后再次发送可能重复修改文件或运行命令。'), { title: t('解除发送暂停') })) return
    onDeliveryChange(null); setError(''); setNotice('')
  }
  return <div className='flex min-h-0 flex-1 flex-col'>
    <div className='flex flex-wrap items-center gap-2 border-b border-border px-3 py-2'>
      <div className='min-w-0 flex-1'><div className='truncate text-sm font-medium'>{session?.title || t('Codex 会话')}</div>
        <div className='truncate text-xs text-muted-foreground' title={session?.directory}>{session?.directory || id}</div></div>
      {session?.model ? <Badge variant='outline'>{session.model}</Badge> : null}
      {session?.permissionMode ? <Badge variant='outline'>{session.permissionMode === 'read-only' ? t('只读') : session.permissionMode === 'default' ? t('项目内读写') : session.permissionMode === 'full-access' ? t('完全访问') : t('受限')}</Badge> : null}
      <Badge variant='outline'>{session?.pendingCount ? t('等待你处理') : session?.thinking ? t('正在执行') : session?.active ? t('就绪') : t('历史会话')}</Badge>
      {session?.terminalCount ? <Badge variant='outline'>{t('终端共享中 · {count}', { count: session.terminalCount })}</Badge> : null}
      <Button variant='outline' size='sm' disabled={busy || !session || session.terminalOpening || !liveConnection} onClick={() => void openTerminal()}>
        {session?.terminalOpening ? t('正在打开终端…') : t('打开官方终端')}
      </Button>
      <Button variant='ghost' size='sm' disabled={busy} onClick={() => void refresh()}>{t('刷新')}</Button>
      {session?.active ? <><Button variant='outline' size='sm' disabled={busy || !session.thinking || !liveConnection} onClick={() => void action('abort')}>{t('停止任务')}</Button>
        <Button variant='ghost' size='sm' disabled={busy || !liveConnection || session.terminalCount > 0 || session.terminalOpening} onClick={() => void action('archive')}>{t('结束会话')}</Button></> : null}
    </div>
    <div ref={scroll} onScroll={() => { const node = scroll.current; if (node) nearBottom.current = node.scrollHeight - node.scrollTop - node.clientHeight < 100 }} className='min-h-0 flex-1 overflow-auto'>
      <div className='mx-auto flex max-w-4xl flex-col gap-4 px-3 py-4'>
        {state.loading ? <p className='text-sm text-muted-foreground'>{t('正在读取会话…')}</p> : null}
        {state.error ? <Alert variant='destructive'>{state.error}</Alert> : null}
        {!liveConnection && !state.loading ? <p className='text-xs text-muted-foreground'>{t('实时连接已断开，正在重连；电脑上的任务不会因此停止。')}</p> : null}
        {session?.terminalError ? <Alert variant='warning'>{session.terminalError}</Alert> : null}
        {blocks.map(block => <div key={block.id} className={block.role === 'user' ? 'ml-auto max-w-[90%] rounded-lg bg-primary/10 px-3 py-2' : 'min-w-0'}>
          {block.role === 'user' ? <div className='whitespace-pre-wrap break-words text-sm'>{block.text}</div>
            : block.kind === 'text' ? <div className='min-w-0 break-words text-sm leading-relaxed'>{renderText(block.text)}</div>
            : block.kind === 'error' ? <Alert variant='destructive'><pre className='max-h-64 overflow-auto whitespace-pre-wrap break-all text-xs'>{block.text}</pre></Alert>
            : <details className='rounded-md border border-border bg-muted/20 px-3 py-2'>
              <summary className='cursor-pointer break-words text-xs text-muted-foreground'>{block.kind === 'thinking' ? t('思考过程') : block.kind === 'tool' ? t('执行操作') : block.kind === 'result' ? t('执行结果') : t('会话事件')}{block.title ? ` · ${block.title}` : ''}</summary>
              <pre className='mt-2 max-h-80 overflow-auto whitespace-pre-wrap break-all text-xs'>{block.text}</pre>
            </details>}
        </div>)}
        {delivery ? <div className='ml-auto max-w-[90%] rounded-lg bg-primary/10 px-3 py-2'>
          <div className='whitespace-pre-wrap break-words text-sm'>{delivery.text}</div>
          <p className='mt-1 text-xs text-muted-foreground'>{delivery.state === 'sending' ? t('正在发送…') : delivery.state === 'uncertain' ? t('发送结果待确认') : t('Codex 已接收，正在同步会话。')}</p>
        </div> : null}
        {session?.permissions.map(request => <CodingPermissionCard key={request.id} request={request} sessionId={id} transport={transport} t={t}
          disabled={!session.active || !liveConnection} onDone={() => { void refresh() }} />)}
        {!blocks.length && !delivery && !state.loading && !state.error ? <p className='py-10 text-center text-sm text-muted-foreground'>{t('发送任务，开始使用本机 Codex。')}</p> : null}
        {session?.thinking ? <p className='text-xs text-muted-foreground'>{t('Codex 正在电脑上执行…')}</p> : null}
      </div>
    </div>
    <div className='space-y-2 border-t border-border p-3'>
      {error ? <Alert variant='destructive'>{error}</Alert> : null}
      {notice ? <p role='status' className='text-xs text-muted-foreground'>{notice}</p> : null}
      {uncertain || delivery?.state === 'accepted' && session && !session.thinking ? <Alert variant='warning'><p>{t('为防止重复执行，已暂停发送。先刷新会话检查这条消息是否已出现。')}</p>
        <Button variant='outline' size='sm' className='mt-2' onClick={() => void dismissDelivery()}>{t('已核对，解除发送暂停')}</Button></Alert> : null}
      {session?.terminalCount ? <p className='text-xs text-muted-foreground'>{t('官方终端仍已连接。停止任务两端都会生效；结束会话前请先关闭终端。')}</p> : null}
      {session && !session.active ? <div className='flex flex-wrap items-center gap-2'>
        <Button variant='outline' size='sm' disabled={busy || !liveConnection} onClick={() => void action('resume')}>{t('恢复会话')}</Button>
        <p className='text-xs text-muted-foreground'>{t('发送时会自动恢复历史；本应用未记录权限的旧会话按只读恢复，不会擅自放宽权限。')}</p>
      </div> : null}
      <div className='flex items-end gap-2'><Textarea value={draft} onChange={e => onDraftChange(e.target.value)} rows={2} className='max-h-40 min-h-16 resize-y'
        placeholder={t('把任务发给本机 Codex…')} disabled={busy} onKeyDown={event => {
          if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) { event.preventDefault(); void send() }
        }} />
        <Button disabled={!canSend || !draft.trim()} onClick={() => void send()}>{session?.thinking ? t('追加指令') : t('发送')}</Button></div>
      <p className='text-xs text-muted-foreground'>{t('通过这里打开的官方终端与手机共享同一条会话。关闭终端或页面不停止任务；退出 Agent2API 会停止后台。')}</p>
    </div>
  </div>
}
