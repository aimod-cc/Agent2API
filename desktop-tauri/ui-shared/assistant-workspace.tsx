import { useState, type ReactNode } from 'react'
import { SegmentedControl } from '@ui'
import { CodingPanel } from './assistant-coding-panel'
import type { AssistantEngine, CodingRender, CodingTranslate, CodingTransport } from './assistant-coding-types'

export function AssistantWorkspace({ children, transport, t, renderText, mobile = false }: {
  children: ReactNode; transport: CodingTransport; t: CodingTranslate; renderText: CodingRender; mobile?: boolean
}) {
  const [engine, setEngine] = useState<AssistantEngine>('assistant')
  const [codingMounted, setCodingMounted] = useState(false)
  const [codingEngine, setCodingEngine] = useState<'codex' | 'claude'>('codex')
  return <div className='flex h-full min-h-0 flex-1 flex-col overflow-hidden'>
    <div className='flex shrink-0 items-center justify-between gap-3 border-b border-border px-3 py-2'
      style={mobile ? { paddingTop: 'calc(var(--m-safe-t, 0px) + 8px)' } : undefined}>
      <SegmentedControl aria-label={t('对话方式')} value={engine} onValueChange={value => { setEngine(value); if (value !== 'assistant') { setCodingMounted(true); setCodingEngine(value) } }}
        options={[{ value: 'assistant', label: t('AI 助手') }, { value: 'codex', label: 'Codex' }, { value: 'claude', label: 'Claude Code' }]} />
    </div>
    <div style={{ display: engine === 'assistant' ? 'flex' : 'none', flex: 1, minHeight: 0, flexDirection: 'column' }}>{children}</div>
    {codingMounted ? <div style={{ display: engine !== 'assistant' ? 'flex' : 'none', flex: 1, minHeight: 0, flexDirection: 'column' }}>
      <CodingPanel engine={codingEngine} transport={transport} t={t} renderText={renderText} />
    </div> : null}
  </div>
}
