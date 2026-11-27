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
  if (entry.isTest && !evidence) return <div className='req-detail-id-card'><div className='req-detail-id-body text-xs'>{requestClientLabel(entry, t)}</div></div>

  const headerEntries = evidence ? Object.entries(evidence.headers || {}) : []
  const hasExtras = Boolean(
    evidence && (
      evidence.userAgentTruncated
      || evidence.invalidHeaders?.length
      || (evidence.declarationStatus && evidence.declarationStatus !== 'accepted')
      || evidence.method === 'ambiguous'
    ),
  )

  return (
    <div className='req-detail-id-card'>
      <div className='req-detail-id-head'>
        <div className='flex min-w-0 flex-wrap items-center gap-2'>
          <Badge variant='outline' shape='tag'>{requestClientLabel(entry, t)}</Badge>
          <span className='text-[11.5px] text-muted-foreground'>{clientMethodLabel(evidence?.method, t)}</span>
        </div>
        {evidence ? (
          <Button size='xs' variant='outline' disabled={busy} onClick={async () => {
            setBusy(true); setError(''); setSaved(false)
            try { const next = await api.load(); if (alive.current) setView(next) }
            catch (failure) { if (alive.current) setError(failure instanceof Error ? failure.message : String(failure)) }
            finally { if (alive.current) setBusy(false) }
          }}>
            {busy ? t('加载中…') : t('设置识别规则')}
          </Button>
        ) : null}
      </div>

      <div className='req-detail-id-body'>
        {evidence ? (
          <>
            {evidence.matchedHeader ? (
              <div className='req-detail-id-kv'>
                <span className='req-detail-id-k'>{t('命中依据')}</span>
                <span className='req-detail-id-v text-foreground'>
                  {evidence.matchedHeader} · {evidence.matchedPattern}
                  {evidence.ruleId ? <span className='ml-1.5 text-muted-foreground'>({evidence.ruleId})</span> : null}
                </span>
              </div>
            ) : evidence.ruleId ? (
              <div className='req-detail-id-kv'>
                <span className='req-detail-id-k'>{t('规则编号')}</span>
                <span className='req-detail-id-v'>{evidence.ruleId}</span>
              </div>
            ) : null}

            <div className='req-detail-id-kv'>
              <span className='req-detail-id-k'>User-Agent</span>
              <span className='req-detail-id-v'>{evidence.userAgent || '—'}</span>
            </div>

            {headerEntries.map(([header, value]) => (
              <div key={header} className='req-detail-id-kv'>
                <span className='req-detail-id-k'>{header}</span>
                <span className='req-detail-id-v'>
                  {value}{evidence.truncatedHeaders?.includes(header) ? ` · ${t('已截断')}` : ''}
                </span>
              </div>
            ))}

            {hasExtras ? (
              <div className='mt-1 space-y-1 border-t border-hairline pt-2 text-[11.5px] leading-relaxed text-muted-foreground'>
                {evidence.userAgentTruncated ? <p className='m-0'>{t('软件标识超过 1 KB，已截断保存；匹配仅使用保留部分。')}</p> : null}
                {evidence.invalidHeaders?.length ? <p className='m-0 break-all'>{t('无效或重复的标识请求头：{headers}', { headers: evidence.invalidHeaders.join(', ') })}</p> : null}
                {evidence.declarationStatus && evidence.declarationStatus !== 'accepted' ? (
                  <p className='m-0'>{evidence.declarationStatus === 'disabled' ? t('来源声明未启用，本次已忽略。') : t('来源声明无效或未登记，本次已忽略。')}</p>
                ) : null}
                {evidence.method === 'ambiguous' ? <p className='m-0'>{t('请求带有不同软件的标识，未自动归类；可用自定义规则明确归属。')}</p> : null}
              </div>
            ) : null}
          </>
        ) : (
          <p className='m-0 text-xs text-muted-foreground'>{t('这条历史请求没有保存识别依据，不能据此自动创建规则。')}</p>
        )}

        {error ? <Alert variant='destructive' className='mt-1'>{error}</Alert> : null}
        {saved ? <p className='m-0 mt-1 text-xs text-muted-foreground'>{t('规则已保存，对后续请求生效；当前历史记录未修改。')}</p> : null}
        {view && evidence ? <ClientRuleEditor api={api} view={view} evidence={evidence} t={t} onClose={() => setView(null)} onSaved={() => { setView(null); setSaved(true) }} /> : null}
      </div>
    </div>
  )
}
