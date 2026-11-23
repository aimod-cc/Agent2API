import * as React from 'react'
import { Alert, Button, Popover, PopoverContent, PopoverTrigger, SegmentedControl, Spinner, confirmDialog } from '@ui'
import { ErrorIcon, ErrorNumber } from './routing-errors-controls'
import { ErrorRules } from './routing-errors-rules'
import { ErrorBoundaries, ErrorProtection } from './routing-errors-protection'
import { ErrorPreviewPanel } from './routing-errors-preview'
import { ERROR_POLICY_CHANGED, errorPolicyValidation, mergeProviderRules, type ErrorKind, type ErrorPolicyApi, type ErrorPolicyView, type ErrorSettings, type ErrorTranslate } from './routing-errors-types'
import type { ErrorPreviewInput } from './routing-errors-simulation'
import './routing-errors.css'

export function RoutingErrorsPanel({ api, t, active = true, identity = '', reloadToken = 0, initialTab = 'rules', onDirty, onSaved }: {
  api: ErrorPolicyApi; t: ErrorTranslate; active?: boolean; identity?: string; reloadToken?: number
  initialTab?: 'rules' | 'protection' | 'boundaries'; onDirty?(dirty: boolean): void; onSaved?(): void
}) {
  const [view, setView] = React.useState<ErrorPolicyView | null>(null)
  const [draft, setDraft] = React.useState<ErrorSettings | null>(null)
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const [message, setMessage] = React.useState('')
  const [tab, setTab] = React.useState(initialTab)
  const [ruleScope, setRuleScope] = React.useState<'global' | 'provider'>('global')
  const [input, setInput] = React.useState<ErrorPreviewInput>({ scenario: 'rate', provider: '', model: '', started: false, uncertain: false, pinned: false, switchesUsed: 0, resendsUsed: 0, strikes: 1, customStatus: '', customCode: '' })
  const generation = React.useRef(0)
  const pending = React.useRef(false)
  const dirtyRef = React.useRef(false)
  const currentApi = React.useRef(api)
  currentApi.current = api
  const dirty = Boolean(view && draft && JSON.stringify(view.settings) !== JSON.stringify(draft))
  dirtyRef.current = dirty
  React.useEffect(() => { onDirty?.(dirty) }, [dirty, onDirty])
  React.useEffect(() => { setTab(initialTab) }, [initialTab])
  const adopt = (next: ErrorPolicyView) => {
    setView(next); setDraft(structuredClone(next.settings)); setError(''); dirtyRef.current = false
    setInput(current => ({ ...current, provider: current.provider || next.providers[0]?.id || '*' }))
  }
  const load = React.useCallback(async (force = false) => {
    if (pending.current || !force && dirtyRef.current) return
    const id = generation.current
    pending.current = true; setBusy(true)
    try { const next = await currentApi.current.load(); if (generation.current === id) adopt(next) }
    catch (cause) { if (generation.current === id) setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { if (generation.current === id) { pending.current = false; setBusy(false) } }
  }, [])
  React.useEffect(() => {
    generation.current++; pending.current = false; dirtyRef.current = false
    setView(null); setDraft(null); setError(''); setMessage('')
    return () => { generation.current++ }
  }, [identity])
  React.useEffect(() => { if (active) void load() }, [active, identity, load])
  React.useEffect(() => { if (reloadToken) void load(true) }, [reloadToken, load])
  React.useEffect(() => {
    const reload = () => { if (active) void load() }
    window.addEventListener(ERROR_POLICY_CHANGED, reload)
    return () => window.removeEventListener(ERROR_POLICY_CHANGED, reload)
  }, [active, load])
  React.useEffect(() => {
    const protectDraft = (event: BeforeUnloadEvent) => { if (dirtyRef.current) event.preventDefault() }
    window.addEventListener('beforeunload', protectDraft)
    return () => window.removeEventListener('beforeunload', protectDraft)
  }, [])
  async function save() {
    if (!view || !draft || pending.current) return
    const validation = errorPolicyValidation(draft, t)
    if (validation) { setError(validation); return }
    const id = generation.current
    pending.current = true; setBusy(true); setError('')
    try {
      const next = await currentApi.current.save({ settings: draft, revision: view.revision })
      if (generation.current !== id) return
      adopt(next); setMessage(t('错误处理设置已保存，下一个请求生效'))
      window.dispatchEvent(new Event(ERROR_POLICY_CHANGED)); onSaved?.()
    } catch (cause) { if (generation.current === id) setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { if (generation.current === id) { pending.current = false; setBusy(false) } }
  }
  const patch = (next: Partial<ErrorSettings>) => { if (draft) { setDraft({ ...draft, ...next }); setMessage('') } }
  const inspect = (kind: ErrorKind) => {
    const map: Record<ErrorKind, string> = { rate_limit: 'rate', quota: 'quota', auth: 'auth', balance: 'balance', transient: 'transient', unsupported: 'unsupported', bad_request: 'input', content: 'content', unknown: 'unknown' }
    setInput(current => ({ ...current, scenario: map[kind], started: false, uncertain: false }))
  }
  const providerRuleCount = React.useMemo(
    () => (draft && view ? mergeProviderRules(draft, view.providerRules ?? []).length : 0),
    [draft, view],
  )
  if (!view || !draft) return <div className='re-panel'><div className='re-loading'>{busy ? <Spinner /> : <Button variant='outline' onClick={() => void load(true)}>{t('重新读取')}</Button>}</div>{error ? <Alert variant='destructive'>{error}</Alert> : null}</div>
  return <section className='re-panel' aria-label={t('错误处理')} hidden={!active}>
    <div className='re-toolbar'><SegmentedControl aria-label={t('错误处理分类')} value={tab} onValueChange={setTab} options={[
      { value: 'rules', label: t('处理规则') }, { value: 'protection', label: t('失败保护') }, { value: 'boundaries', label: t('安全边界') },
    ]} /><div className='re-actions'><Button variant='ghost' size='sm' disabled={busy} onClick={async () => {
      if (await confirmDialog(t('将填入建议值，保存后才生效；已有账号、冷却记录和其他路由保持不变。'), { title: t('恢复建议值'), confirmText: t('恢复') })) { setDraft(structuredClone(view.defaults)); setMessage('') }
    }}>{t('恢复建议值')}</Button><Button size='sm' disabled={!dirty || busy} onClick={() => void save()}><ErrorIcon name='check' size={14} />{busy ? t('保存中…') : t('保存修改')}</Button></div></div>
    <div className='re-limit-strip' role='group' aria-label={t('每条请求的重试上限')}><div className='re-compact-fields'>
      <ErrorNumber compact label={t('原账号重发')} unit={t('次')} value={draft.resends} max={10} disabled={busy} onChange={resends => patch({ resends })} />
      <ErrorNumber compact label={t('最多再换账号')} unit={t('个')} value={draft.switches} max={10} disabled={busy} onChange={switches => patch({ switches })} />
      <ErrorNumber compact label={t('重发间隔')} unit={t('秒')} value={draft.intervalSeconds} max={300} disabled={busy} onChange={intervalSeconds => patch({ intervalSeconds })} />
      <ErrorNumber compact label={t('累计等待上限')} unit={t('秒')} value={draft.waitBudgetSeconds} max={300} disabled={busy} onChange={waitBudgetSeconds => patch({ waitBudgetSeconds })} />
    </div><Popover><PopoverTrigger render={<Button variant='ghost' size='icon-sm' aria-label={t('重试计数说明')} />}><ErrorIcon name='info' size={14} /></PopoverTrigger><PopoverContent className='re-limit-help' align='end'>
      <p>{t('不含首发；重发次数由整条请求共用。每次换号都计数，包括 429。')}</p><p>{t('累计等待只限制本次重试，不会缩短上游要求的冷却时间。')}</p>
    </PopoverContent></Popover></div>
    {view.legacy ? <p className='re-legacy'>{t('已保留原重试与失败休息参数，错误处理已统一；429 也计入换号上限，已有冷却记录不清除。')}</p> : null}
    {error ? <Alert variant='destructive'>{error}</Alert> : null}{message ? <p className='re-saved' role='status'>{message}</p> : null}
    <div className='re-content-grid'><div className={`re-primary${tab === 'rules' ? ' is-rules' : ''}`}>
      {tab === 'rules' ? <>
        <div className='re-rules-switch'>
          <SegmentedControl
            aria-label={t('处理规则范围')}
            value={ruleScope}
            onValueChange={setRuleScope}
            options={[
              { value: 'global', label: t('全局处理规则') },
              { value: 'provider', label: t('提供商处理规则'), count: providerRuleCount },
            ]}
          />
        </div>
        <ErrorRules settings={draft} providers={view.providers} providerRules={view.providerRules ?? []} scope={ruleScope} t={t} onChange={setDraft} onPreview={inspect} />
      </> : tab === 'protection' ? <ErrorProtection settings={draft} t={t} onChange={setDraft} /> : <ErrorBoundaries t={t} />}
    </div><ErrorPreviewPanel settings={draft} providers={view.providers} providerRules={view.providerRules ?? []} t={t} input={input} onChange={setInput} /></div>
  </section>
}
