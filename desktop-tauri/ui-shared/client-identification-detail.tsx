import { useEffect, useRef, useState } from 'react'
import { Alert, Badge, Button } from '@ui'
import type { Translate } from './assistant-read-types'
import { requestClientLabel, type RequestIdentity } from './model-api-key'
import { ClientRuleEditor } from './client-identification-editor'
import { clientMethodLabel, type ClientIdentificationApi, type ClientIdentificationView } from './client-identification'

export function ClientIdentificationDetail({ entry, api, t }: { entry: RequestIdentity; api: ClientIdentificationApi; t: Translate }) {
  const [view, setView] = useState<ClientIdentificationView | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [saved, setSaved] = useState(false)
  const alive = useRef(true)
  useEffect(() => { alive.current = true; return () => { alive.current = false } }, [])
  const evidence = entry.clientEvidence
  if (entry.isTest && !evidence) return <span>{requestClientLabel(entry, t)}</span>
  return <div className='space-y-2 text-xs'>
    <div className='flex flex-wrap items-center gap-2'><Badge variant='outline'>{requestClientLabel(entry, t)}</Badge><span className='text-muted-foreground'>{clientMethodLabel(evidence?.method, t)}</span></div>
    {evidence ? <>
      <div className='break-all'><span className='text-muted-foreground'>User-Agent: </span>{evidence.userAgent || '—'}</div>
      {evidence.userAgentTruncated ? <p className='text-muted-foreground'>{t('软件标识超过 1 KB，已截断保存；匹配仅使用保留部分。')}</p> : null}
      {Object.entries(evidence.headers || {}).map(([header, value]) => <div key={header} className='break-all'><span className='text-muted-foreground'>{header}: </span>{value}{evidence.truncatedHeaders?.includes(header) ? ` · ${t('已截断')}` : ''}</div>)}
      {evidence.invalidHeaders?.length ? <p className='break-all text-muted-foreground'>{t('无效或重复的标识请求头：{headers}', { headers: evidence.invalidHeaders.join(', ') })}</p> : null}
      {evidence.matchedHeader ? <p className='break-all text-muted-foreground'>{t('命中依据：{header} · {pattern}', { header: evidence.matchedHeader, pattern: evidence.matchedPattern })}</p> : null}
      {evidence.ruleId ? <p className='break-all text-muted-foreground'>{t('规则编号：{id}', { id: evidence.ruleId })}</p> : null}
      {evidence.declarationStatus && evidence.declarationStatus !== 'accepted' ? <p className='text-muted-foreground'>{evidence.declarationStatus === 'disabled' ? t('来源声明未启用，本次已忽略。') : t('来源声明无效或未登记，本次已忽略。')}</p> : null}
      {evidence.method === 'ambiguous' ? <p className='text-muted-foreground'>{t('请求带有不同软件的标识，未自动归类；可用自定义规则明确归属。')}</p> : null}
      <Button size='sm' variant='outline' disabled={busy} onClick={async () => {
        setBusy(true); setError(''); setSaved(false)
        try { const next = await api.load(); if (alive.current) setView(next) }
        catch (failure) { if (alive.current) setError(failure instanceof Error ? failure.message : String(failure)) }
        finally { if (alive.current) setBusy(false) }
      }}>{busy ? t('加载中…') : t('设置识别规则')}</Button>
    </> : <p className='text-muted-foreground'>{t('这条历史请求没有保存识别依据，不能据此自动创建规则。')}</p>}
    {error ? <Alert variant='destructive'>{error}</Alert> : null}
    {saved ? <p className='text-muted-foreground'>{t('规则已保存，对后续请求生效；当前历史记录未修改。')}</p> : null}
    {view && evidence ? <ClientRuleEditor api={api} view={view} evidence={evidence} t={t} onClose={() => setView(null)} onSaved={() => { setView(null); setSaved(true) }} /> : null}
  </div>
}
