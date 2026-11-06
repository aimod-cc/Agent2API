import { useRef, useState } from 'react'
import { Alert, Badge, Button, Card, CardContent, CardHeader, CardTitle, Checkbox, Input, Label } from '@ui'
import { asRecord, codingPath, type CodingPermission, type CodingTranslate, type CodingTransport } from './assistant-coding-types'

type Question = { id: string; text: string; multiple: boolean; required: boolean; options: Array<{ label: string; description: string }> }
function questionsOf(request: CodingPermission): Question[] {
  const input = asRecord(request.arguments)
  if (!Array.isArray(input?.questions)) return []
  return input.questions.flatMap((raw, index) => {
    const q = asRecord(raw)
    if (!q) return []
    const id = request.tool === 'request_user_input' ? String(q.id || '') : String(index)
    if (!id) return []
    return [{ id, text: String(q.question || q.header || ''), multiple: q.multiSelect === true || q.multiple === true, required: q.required !== false,
      options: (Array.isArray(q.options) ? q.options : []).flatMap(raw => {
        const option = asRecord(raw)
        return typeof option?.label === 'string' ? [{ label: option.label, description: String(option.description || '') }] : []
      }) }]
  })
}
export function CodingPermissionCard({ request, sessionId, transport, t, disabled, onDone }: {
  request: CodingPermission; sessionId: string; transport: CodingTransport; t: CodingTranslate; disabled: boolean; onDone: () => void
}) {
  const [selected, setSelected] = useState<Record<string, string[]>>({})
  const [notes, setNotes] = useState<Record<string, string>>({})
  const [busy, setBusy] = useState(false)
  const [submitted, setSubmitted] = useState(false)
  const [error, setError] = useState('')
  const gate = useRef(false)
  const question = ['AskUserQuestion', 'ask_user_question', 'request_user_input'].includes(request.tool)
  const questions = questionsOf(request)
  const allowed = !disabled && !busy && !submitted
  const decide = async (approve: boolean) => {
    if (gate.current || !allowed) return
    const answers: Record<string, string[] | { answers: string[] }> = {}
    if (approve && question) {
      for (const q of questions) {
        const values = [...(selected[q.id] || []), ...(notes[q.id]?.trim() ? [notes[q.id].trim()] : [])]
        if (q.required && !values.length) { setError(t('请回答所有必填问题')); return }
        answers[q.id] = request.tool === 'request_user_input' ? { answers: values } : values
      }
      if (!questions.length) { setError(t('这个问题的格式暂不支持，请在电脑端处理。')); return }
    }
    gate.current = true; setBusy(true); setError('')
    try {
      await transport.json('POST', codingPath(sessionId, `/permissions/${encodeURIComponent(request.id)}/${approve ? 'approve' : 'deny'}`), question && approve ? { answers } : {})
      setSubmitted(true); onDone()
    } catch (failure) { setError(failure instanceof Error ? failure.message : String(failure)); onDone() }
    finally { gate.current = false; setBusy(false) }
  }
  return <Card className='border-warning/40'>
    <CardHeader className='pb-2'><CardTitle className='flex flex-wrap items-center gap-2 text-sm'>
      <Badge variant='outline'>{question ? t('等待回答') : t('等待审批')}</Badge><span>{request.tool}</span>
    </CardTitle></CardHeader>
    <CardContent className='space-y-3'>
      {question && questions.length ? questions.map(q => <div key={q.id} className='space-y-2'>
        <p className='whitespace-pre-wrap text-sm'>{q.text}</p>
        {q.options.map((option, index) => <Label key={index} className='flex cursor-pointer items-start gap-2 rounded-md border border-border px-3 py-2'>
          <Checkbox checked={(selected[q.id] || []).includes(option.label)} disabled={!allowed} onCheckedChange={checked => {
            setSelected(previous => ({ ...previous, [q.id]: q.multiple
              ? checked ? [...(previous[q.id] || []), option.label] : (previous[q.id] || []).filter(v => v !== option.label)
              : checked ? [option.label] : [] }))
          }} />
          <span><span className='block text-sm'>{option.label}</span>{option.description ? <span className='block text-xs font-normal text-muted-foreground'>{option.description}</span> : null}</span>
        </Label>)}
        <Input value={notes[q.id] || ''} onChange={e => setNotes(previous => ({ ...previous, [q.id]: e.target.value }))} disabled={!allowed}
          aria-label={q.text || t('补充回答')} placeholder={q.options.length ? t('其他回答或补充说明') : t('输入你的回答')} />
      </div>) : <pre className='max-h-60 overflow-auto whitespace-pre-wrap break-all rounded-md bg-muted/40 p-2 text-xs'>{JSON.stringify(request.arguments, null, 2)}</pre>}
      {error ? <Alert variant='destructive'>{error}</Alert> : null}
      {submitted ? <p className='text-xs text-muted-foreground'>{t('决定已提交，等待电脑端确认。')}</p> : <div className='flex justify-end gap-2'>
        <Button variant='outline' size='sm' disabled={!allowed} onClick={() => void decide(false)}>{t('拒绝')}</Button>
        <Button size='sm' disabled={!allowed || question && !questions.length} onClick={() => void decide(true)}>{question ? t('提交回答') : t('允许这一次')}</Button>
      </div>}
    </CardContent>
  </Card>
}
