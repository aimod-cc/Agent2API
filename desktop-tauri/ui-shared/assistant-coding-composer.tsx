import { useEffect, useRef } from 'react'
import { Button, Textarea } from '@ui'
import type { CodingSession, CodingTranslate, CodingTransport } from './assistant-coding-types'
import { CodingIcon } from './assistant-coding-icons'
import { CodingRunSettings } from './assistant-coding-run-settings'
import { CodingUsage } from './assistant-coding-usage'

export function CodingComposer({ session, transport, t, draft, onDraftChange, canSend, busy, connected, settingsSaving, onSavingChange, onSettingsChanged, onSend, onStop }: {
  session: CodingSession | null; transport: CodingTransport; t: CodingTranslate; draft: string; onDraftChange: (text: string) => void
  canSend: boolean; busy: boolean; connected: boolean; settingsSaving: boolean; onSavingChange: (saving: boolean) => void
  onSettingsChanged: () => Promise<void>; onSend: () => void; onStop: () => void
}) {
  const input = useRef<HTMLTextAreaElement>(null)
  useEffect(() => {
    const node = input.current
    if (!node) return
    node.style.height = 'auto'
    node.style.height = `${Math.min(node.scrollHeight + 2, 168)}px`
  }, [draft])
  const label = session?.thinking ? t('追加指令') : t('发送')
  return <div className='rounded-xl border border-control-border bg-surface-2 p-2 transition-colors focus-within:border-primary/50'>
    <Textarea ref={input} value={draft} onChange={event => onDraftChange(event.currentTarget.value)} rows={2}
      className='max-h-42 min-h-16 resize-none rounded-none border-0 bg-transparent px-2 py-1 font-sans text-[13px] leading-relaxed shadow-none focus:border-0 focus:shadow-none max-sm:text-base'
      placeholder={t('把任务发给本机 Codex…')} disabled={busy} onKeyDown={event => {
        if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) { event.preventDefault(); if (canSend && draft.trim()) onSend() }
      }} />
    <div className='flex flex-wrap items-center gap-1 pt-1'>
      {session ? <CodingUsage session={session} transport={transport} t={t} /> : null}
      <div className='ml-auto flex min-w-0 max-w-full items-center justify-end gap-1'>
        {session ? <CodingRunSettings session={session} transport={transport} t={t} disabled={busy || !connected}
          onChanged={onSettingsChanged} onSavingChange={onSavingChange} /> : null}
        {session?.thinking ? <Button variant='ghost' size='icon' className='size-8 shrink-0 rounded-full text-destructive max-sm:size-9' aria-label={t('停止任务')} title={t('停止任务')}
          disabled={busy || !connected} onClick={onStop}><CodingIcon name='stop' /></Button> : null}
        <Button size='icon' className='size-8 shrink-0 rounded-full shadow-none max-sm:size-9' aria-label={label} title={label}
          disabled={!canSend || settingsSaving || !draft.trim()} onClick={onSend}><CodingIcon name='send' /></Button>
      </div>
    </div>
    {settingsSaving ? <p role='status' className='px-2 pt-1 text-xs text-muted-foreground'>{t('等待 Codex 确认…')}</p> : null}
  </div>
}
