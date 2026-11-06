import { useEffect, useMemo, useRef, useState } from 'react'
import { Alert, Badge, Button, Textarea, confirmDialog } from '@ui'
import { codingMessageId, codingPath, engineLabel, type CodingDelivery, type CodingRender, type CodingTranslate, type CodingTransport } from './assistant-coding-types'
import { decodeCodingMessages } from './assistant-coding-messages'
import { useCodingSession } from './assistant-coding-session'
import { CodingPermissionCard } from './assistant-coding-permissions'

export function CodingConversation({ id, transport, t, renderText, onResume, onChanged, draft, onDraftChange, delivery, onDeliveryChange }: {
  id: string; transport: CodingTransport; t: CodingTranslate; renderText: CodingRender
  onResume: (id: string) => void; onChanged: () => void; draft: string; onDraftChange: (text: string) => void
  delivery?: CodingDelivery; onDeliveryChange: (delivery: CodingDelivery | null) => void
}) {
  const state = useCodingSession(transport, id, true)
  const session = state.session
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const uncertain = delivery?.state === 'uncertain'
  const [notice, setNotice] = useState('')
  const gate = useRef(false)
  const mounted = useRef(true)
  const scroll = useRef<HTMLDivElement>(null)
  const nearBottom = useRef(true)
  const pending = useRef<{ localId: string; text: string } | null>(delivery ?? null)
  const settled = useRef(new Set<string>())
  const blocks = useMemo(() => decodeCodingMessages(state.messages), [state.messages])
  useEffect(() => { mounted.current = true; return () => { mounted.current = false } }, [])
  useEffect(() => { if (session?.id && session.id !== id) onResume(session.id) }, [session?.id, id, onResume])
  useEffect(() => {
    if (nearBottom.current && scroll.current) scroll.current.scrollTop = scroll.current.scrollHeight
  }, [blocks.length, blocks.at(-1)?.text, session?.pendingCount])
  useEffect(() => {
    const item = delivery ?? pending.current
    if (!item || !state.messages.some(message => message.localId === item.localId)) return
    settled.current.add(item.localId)
    pending.current = null; onDeliveryChange(null); setError(''); setNotice('')
    if (draft === item.text) onDraftChange('')
  }, [state.messages, draft, delivery, onDraftChange, onDeliveryChange])
  const refresh = async () => {
    setError('')
    try { await state.refresh(); onChanged() }
    catch (failure) { if (mounted.current) setError(failure instanceof Error ? failure.message : String(failure)) }
  }
  const action = async (name: 'resume' | 'abort' | 'switch' | 'archive') => {
    if (gate.current) return
    if (name === 'archive' && !await confirmDialog(t('结束此编程会话？正在执行的任务会停止，历史记录仍然保留。'), { title: t('结束会话') })) return
    gate.current = true; setBusy(true); setError('')
    try {
      const result = await transport.json<{ sessionId: string }>('POST', codingPath(id, `/actions/${name}`), {})
      onChanged()
      if (!mounted.current) return
      if (result.sessionId !== id) onResume(result.sessionId)
      else await state.refresh()
    } catch (failure) { if (mounted.current) setError(failure instanceof Error ? failure.message : String(failure)) }
    finally { gate.current = false; if (mounted.current) setBusy(false) }
  }
  const send = async () => {
    if (gate.current || delivery?.state === 'sending' || !draft.trim() || !session?.active || uncertain || !state.connected
      || session.controlledByTerminal && !session.concurrentClients) return
    gate.current = true; setBusy(true); setError(''); setNotice('')
    const input = { text: draft, localId: codingMessageId() }
    pending.current = input
    onDeliveryChange({ ...input, state: 'sending' })
    nearBottom.current = true
    try {
      await transport.json('POST', codingPath(id, '/messages'), input)
      onChanged()
      if (!settled.current.has(input.localId)) { onDeliveryChange({ ...input, state: 'accepted' }); onDraftChange('') }
      if (!mounted.current) return
      if (pending.current?.localId === input.localId) setNotice(t('消息已交给电脑，等待编程工具处理。'))
      await state.refresh().catch(() => {})
    } catch (failure) {
      if (!settled.current.has(input.localId)) onDeliveryChange({ ...input, state: 'uncertain' })
      if (!mounted.current) return
      if (pending.current?.localId === input.localId) setError(failure instanceof Error ? failure.message : String(failure))
      void state.refresh().catch(() => {})
    } finally { gate.current = false; if (mounted.current) setBusy(false) }
  }
  const takeOver = session?.active && session.controlledByTerminal && !session.concurrentClients
  return <div className='flex min-h-0 flex-1 flex-col'>
    <div className='flex flex-wrap items-center gap-2 border-b border-border px-3 py-2'>
      <div className='min-w-0 flex-1'><div className='truncate text-sm font-medium'>{session?.title || t('编程会话')}</div>
        <div className='truncate text-xs text-muted-foreground' title={session?.directory}>{session?.directory || id}</div></div>
      <Badge variant='outline'>{session?.pendingCount ? t('等待你处理') : session?.thinking ? t('正在执行') : session?.active ? t('就绪') : t('未运行')}</Badge>
      <Button variant='ghost' size='sm' disabled={busy} onClick={() => void refresh()}>{t('刷新')}</Button>
      {session?.active ? <><Button variant='outline' size='sm' disabled={busy} onClick={() => void action('abort')}>{t('停止任务')}</Button>
        <Button variant='ghost' size='sm' disabled={busy} onClick={() => void action('archive')}>{t('结束会话')}</Button></> : null}
    </div>
    <div ref={scroll} onScroll={() => { const node = scroll.current; if (node) nearBottom.current = node.scrollHeight - node.scrollTop - node.clientHeight < 100 }} className='min-h-0 flex-1 overflow-auto'>
      <div className='mx-auto flex max-w-4xl flex-col gap-4 px-3 py-4'>
        {state.loading ? <p className='text-sm text-muted-foreground'>{t('正在读取会话…')}</p> : null}
        {state.error ? <Alert variant='destructive'>{state.error}</Alert> : null}
        {!state.connected && !state.loading ? <p className='text-xs text-muted-foreground'>{t('实时连接已断开，正在重连；电脑上的任务不会因此停止。')}</p> : null}
        {state.older?.hasMore ? <Button variant='outline' size='sm' className='self-center' disabled={state.loadingOlder} onClick={() => {
          const node = scroll.current; const before = node?.scrollHeight || 0
          void state.loadOlder().then(() => requestAnimationFrame(() => { if (node) node.scrollTop += node.scrollHeight - before }))
        }}>{state.loadingOlder ? t('正在读取…') : t('更早的消息')}</Button> : null}
        {blocks.map(block => <div key={block.id} className={block.role === 'user' ? 'ml-auto max-w-[90%] rounded-lg bg-primary/10 px-3 py-2' : 'min-w-0'}>
          {block.role === 'user' ? <><div className='whitespace-pre-wrap break-words text-sm'>{block.text}</div>
            {block.queued ? <p className='mt-1 text-xs text-muted-foreground'>{block.uncertain ? t('发送结果待确认') : t('等待电脑处理')}</p> : null}</>
            : block.kind === 'text' ? <div className='min-w-0 break-words text-sm leading-relaxed'>{renderText(block.text)}</div>
            : block.kind === 'error' ? <Alert variant='destructive'><pre className='max-h-64 overflow-auto whitespace-pre-wrap break-all text-xs'>{block.text}</pre></Alert>
            : <details className='rounded-md border border-border bg-muted/20 px-3 py-2'>
              <summary className='cursor-pointer break-words text-xs text-muted-foreground'>{block.kind === 'thinking' ? t('思考过程') : block.kind === 'tool' ? t('执行操作') : block.kind === 'result' ? t('执行结果') : t('会话事件')}{block.title ? ` · ${block.title}` : ''}</summary>
              <pre className='mt-2 max-h-80 overflow-auto whitespace-pre-wrap break-all text-xs'>{block.text}</pre>
            </details>}
        </div>)}
        {session?.permissions.map(request => <CodingPermissionCard key={request.id} request={request} sessionId={id} transport={transport} t={t}
          disabled={!session.active || !state.connected} onDone={() => { void refresh() }} />)}
        {!blocks.length && !state.loading && !state.error ? <p className='py-10 text-center text-sm text-muted-foreground'>{t('发送任务，开始使用电脑上的编程工具。')}</p> : null}
        {session?.thinking ? <p className='text-xs text-muted-foreground'>{t('{engine} 正在电脑上执行…', { engine: engineLabel(session.engine, t) })}</p> : null}
      </div>
    </div>
    <div className='space-y-2 border-t border-border p-3'>
      {error ? <Alert variant='destructive'>{error}</Alert> : null}
      {notice ? <p role='status' className='text-xs text-muted-foreground'>{notice}</p> : null}
      {uncertain ? <Alert variant='warning'><p>{t('为防止重复执行，已暂停发送。先刷新会话检查这条消息是否已出现。')}</p>
        <Button variant='outline' size='sm' className='mt-2' onClick={() => { pending.current = null; onDeliveryChange(null); setError('') }}>{t('已核对，解除发送暂停')}</Button></Alert> : null}
      {session && !session.active ? <Button disabled={busy} onClick={() => void action('resume')}>{t('恢复会话')}</Button>
        : takeOver ? <Button disabled={busy} onClick={() => void action('switch')}>{t('切换到远程控制')}</Button> : null}
      <div className='flex items-end gap-2'><Textarea value={draft} onChange={e => onDraftChange(e.target.value)} rows={2} className='max-h-40 min-h-16 resize-y'
        placeholder={t('把任务发给电脑上的编程工具…')} disabled={busy} onKeyDown={event => {
          if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) { event.preventDefault(); void send() }
        }} />
        <Button disabled={busy || delivery?.state === 'sending' || !session?.active || Boolean(takeOver) || !draft.trim() || uncertain || !state.connected} onClick={() => void send()}>{t('发送')}</Button></div>
      <p className='text-xs text-muted-foreground'>{t('关闭页面只断开查看；停止任务请使用上方按钮。模型和账号沿用电脑端配置。')}</p>
    </div>
  </div>
}
