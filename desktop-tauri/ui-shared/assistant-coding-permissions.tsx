import { useRef, useState } from 'react'
import { Alert, Badge, Button, Card, CardContent, CardHeader, CardTitle, Checkbox, Input, Label } from '@ui'
import { asRecord, codingFailureIsDefinite, codingPath, type CodingPermission, type CodingTranslate, type CodingTransport } from './assistant-coding-types'

type Question = { id: string; text: string; secret: boolean; other: boolean; options: Array<{ label: string; description: string }> }
function questionsOf(request: CodingPermission): Question[] {
  const input = asRecord(request.arguments)
  if (request.tool !== 'request_user_input' || !Array.isArray(input?.questions)) return []
  return input.questions.flatMap(raw => {
    const q = asRecord(raw)
    if (!q || typeof q.id !== 'string' || !q.id) return []
    return [{ id: q.id, text: String(q.question || q.header || ''), secret: q.isSecret === true, other: q.isOther === true,
      options: (Array.isArray(q.options) ? q.options : []).flatMap(raw => {
        const option = asRecord(raw)
        return typeof option?.label === 'string' ? [{ label: option.label, description: String(option.description || '') }] : []
      }) }]
  })
}
export function CodingPermissionCard({ request, sessionId, transport, t, disabled, onDone }: {
  request: CodingPermission; sessionId: string; transport: CodingTransport; t: CodingTranslate; disabled: boolean; onDone: () => void
}) {
  const [selected, setSelected] = useState<Record<string, string>>({})
  const [notes, setNotes] = useState<Record<string, string>>({})
  const [busy, setBusy] = useState(false)
  const [submitted, setSubmitted] = useState(false)
  const [uncertain, setUncertain] = useState(false)
  const [error, setError] = useState('')
  const gate = useRef(false)
  const question = request.tool === 'request_user_input'
  const questions = questionsOf(request)
  const pending = submitted || request.submitted
  const allowed = !disabled && !busy && !pending && !uncertain
  const decide = async (approve: boolean) => {
    if (gate.current || !allowed || approve && !request.canApprove) return
    const answers: Record<string, { answers: string[] }> = {}
    if (approve && question) {
      for (const q of questions) {
        const answer = notes[q.id]?.trim() || selected[q.id]
        if (!answer) { setError(t('请回答所有必填问题')); return }
        answers[q.id] = { answers: [answer] }
      }
      if (!questions.length) { setError(t('这个问题的格式暂不支持，可以停止当前任务。')); return }
    }
    gate.current = true; setBusy(true); setError('')
    try {
      await transport.json('POST', codingPath(sessionId, `/permissions/${encodeURIComponent(request.id)}/${approve ? 'approve' : 'deny'}`), question && approve ? { answers } : {})
      setSubmitted(true); setNotes({}); setSelected({}); onDone()
    } catch (failure) {
      if (!codingFailureIsDefinite(failure)) setUncertain(true)
      setError(failure instanceof Error ? failure.message : String(failure)); onDone()
    } finally { gate.current = false; setBusy(false) }
  }
  const title = request.tool === 'commandExecution' ? t('执行命令') : request.tool === 'fileChange' ? t('修改文件')
    : request.tool === 'permissions' ? t('扩展权限') : question ? t('回答问题') : request.tool
  return <Card className='border-warning/40'>
    <CardHeader className='pb-2'><CardTitle className='flex flex-wrap items-center gap-2 text-sm'>
      <Badge variant='outline'>{question ? t('等待回答') : t('等待审批')}</Badge><span>{title}</span>
    </CardTitle></CardHeader>
    <CardContent className='space-y-3'>
      {question && questions.length ? questions.map(q => <div key={q.id} className='space-y-2'>
        <p className='whitespace-pre-wrap text-sm'>{q.text}</p>
        {q.options.map((option, index) => <Label key={index} className='flex cursor-pointer items-start gap-2 rounded-md border border-border px-3 py-2'>
          <Checkbox checked={selected[q.id] === option.label} disabled={!allowed} onCheckedChange={checked => {
            setSelected(previous => ({ ...previous, [q.id]: checked ? option.label : '' }))
            setNotes(previous => ({ ...previous, [q.id]: '' }))
          }} />
          <span><span className='block text-sm'>{option.label}</span>{option.description ? <span className='block text-xs font-normal text-muted-foreground'>{option.description}</span> : null}</span>
        </Label>)}
        {q.other || !q.options.length ? <Input type={q.secret ? 'password' : 'text'} autoComplete='off' value={notes[q.id] || ''}
          onChange={e => { setNotes(previous => ({ ...previous, [q.id]: e.target.value })); setSelected(previous => ({ ...previous, [q.id]: '' })) }} disabled={!allowed}
          aria-label={q.text || t('补充回答')} placeholder={q.options.length ? t('其他回答或补充说明') : t('输入你的回答')} /> : null}
      </div>) : <pre className='max-h-60 overflow-auto whitespace-pre-wrap break-all rounded-md bg-muted/40 p-2 text-xs'>{JSON.stringify(request.arguments, null, 2)}</pre>}
      {!request.canApprove ? <p className='text-xs text-muted-foreground'>{t('此请求暂不支持在这里授权，可以拒绝，不会自动放行。')}</p> : null}
      {error ? <Alert variant='destructive'>{error}</Alert> : null}
      {uncertain ? <Alert variant='warning'>{t('审批结果尚未确认，已暂停重复提交，请刷新会话查看状态。')}</Alert> : null}
      {pending ? <p className='text-xs text-muted-foreground'>{t('决定已提交，等待电脑端确认。')}</p> : <div className='flex justify-end gap-2'>
        <Button variant='outline' size='sm' disabled={!allowed} onClick={() => void decide(false)}>{question ? t('停止任务') : t('拒绝')}</Button>
        {request.canApprove ? <Button size='sm' disabled={!allowed || question && !questions.length} onClick={() => void decide(true)}>{question ? t('提交回答') : t('允许这一次')}</Button> : null}
      </div>}
    </CardContent>
  </Card>
}
