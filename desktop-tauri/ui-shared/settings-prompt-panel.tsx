import * as React from 'react'
import { Alert, Badge, Button, Card, CardAction, CardContent, CardHeader, CardTitle, Select, SelectContent,
  SelectItem, SelectTrigger, SelectValue } from '@ui'
import type { Translate } from './assistant-read-types'
import { IDENTIFICATION_CHANGED } from './client-identification'
import { PromptBodyDialog } from './settings-prompt-body'
import { PromptRuleCreateDialog } from './settings-prompt-create'
import { PromptGatewaySection } from './settings-prompt-gateway'
import { PromptRuleRow } from './settings-prompt-rule'
import { PromptRuleTester } from './settings-prompt-tester'
import { PROMPT_CHANGED, configPayload, modeLabel, sourceLabel, type PromptApi, type PromptConfigView,
  type PromptContent, type PromptMode, type PromptRuleView, type PromptView } from './settings-prompt-types'
import './settings-prompt.css'

type Editing = { ruleId: string } | { fallback: true } | null

export function PromptSettingsPanel({ api, t, active = true }: { api: PromptApi; t: Translate; active?: boolean }) {
  const [view, setView] = React.useState<PromptView | null>(null)
  const [draft, setDraft] = React.useState<PromptConfigView | null>(null)
  const [open, setOpen] = React.useState<string | null>(null)
  const [editing, setEditing] = React.useState<Editing>(null)
  const [creating, setCreating] = React.useState(false)
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const [dirty, setDirty] = React.useState(false)
  const current = React.useRef<PromptView | null>(null)
  const alive = React.useRef(true)
  const inFlight = React.useRef(false)
  const loadRef = React.useRef<() => Promise<void>>(async () => {})
  const sequence = React.useRef(0)
  const ruleNodes = React.useRef(new Map<string, HTMLDivElement>())

  const adopt = (next: PromptView) => {
    const localized = { ...next, clients: next.clients.map(client => client.id === 'other' ? { ...client, label: t('未识别客户端') } : client) }
    current.current = localized; setView(localized); setDraft(structuredClone(localized.config)); setDirty(false)
  }

  const load = React.useCallback(async () => {
    if (inFlight.current) return
    const token = ++sequence.current
    inFlight.current = true; setBusy(true); setError('')
    try {
      const next = await api.load()
      if (alive.current && sequence.current === token) adopt(next)
    } catch (failure) {
      if (alive.current && sequence.current === token) setError(failure instanceof Error ? failure.message : String(failure))
    } finally {
      inFlight.current = false
      if (alive.current && sequence.current === token) setBusy(false)
    }
  }, [api, t])
  loadRef.current = load

  React.useEffect(() => { alive.current = true; return () => { alive.current = false; ++sequence.current } }, [])
  React.useEffect(() => { if (active && !dirty && !editing && !creating) void loadRef.current() }, [active])
  React.useEffect(() => {
    const refresh = () => { if (active && !dirty && !editing && !creating) void load() }
    window.addEventListener(IDENTIFICATION_CHANGED, refresh)
    window.addEventListener('wb:settings-load', refresh)
    return () => {
      window.removeEventListener(IDENTIFICATION_CHANGED, refresh)
      window.removeEventListener('wb:settings-load', refresh)
    }
  }, [active, dirty, editing, creating, load])

  async function commit(next: PromptConfigView): Promise<boolean> {
    if (!current.current || inFlight.current) return false
    setDraft(next); setDirty(true); setError('')
    inFlight.current = true; setBusy(true)
    try {
      const saved = await api.save({ config: configPayload(next), revision: current.current.revision })
      if (!alive.current) return false
      adopt(saved)
      window.dispatchEvent(new Event(PROMPT_CHANGED))
      return true
    } catch (failure) {
      if (alive.current) setError(failure instanceof Error ? failure.message : String(failure))
      return false
    } finally {
      inFlight.current = false
      if (alive.current) setBusy(false)
    }
  }

  const locate = (id: string) => {
    setOpen(id)
    requestAnimationFrame(() => ruleNodes.current.get(id)?.scrollIntoView({ behavior: 'smooth', block: 'nearest' }))
  }

  if (!view || !draft) return <Card render={<section />} className='panel sp-panel'>
    <CardHeader><CardTitle>{t('系统提示词')}</CardTitle><CardAction><Button variant='ghost' size='sm' disabled={busy} onClick={() => void load()}>{t('刷新')}</Button></CardAction></CardHeader>
    <CardContent><p className='sp-muted'>{busy ? t('加载中…') : t('系统提示词设置读取失败')}</p>{error ? <Alert variant='destructive'>{error}</Alert> : null}</CardContent>
  </Card>

  const replaceRule = (next: PromptRuleView) => ({ ...draft, rules: draft.rules.map(rule => rule.id === next.id ? next : rule) })
  const updateRule = (next: PromptRuleView) => { void commit(replaceRule(next)) }
  const capture = current.current!.config.fallback.mode !== 'passthrough'
    || current.current!.config.rules.some(rule => rule.enabled && rule.conditions.some(condition => condition.values.length > 0) && rule.mode !== 'passthrough')
  const editedRule = editing && 'ruleId' in editing ? draft.rules.find(rule => rule.id === editing.ruleId) : undefined
  const content = editedRule ?? (editing && 'fallback' in editing ? draft.fallback : undefined)

  function move(index: number, delta: -1 | 1) {
    const rules = [...draft!.rules]
    const target = index + delta
    if (target < 0 || target >= rules.length) return
    ;[rules[index], rules[target]] = [rules[target], rules[index]]
    void commit({ ...draft!, rules })
  }

  async function createRule(rule: PromptRuleView): Promise<string | null> {
    if (!current.current || inFlight.current) return t('保存中…')
    const next = { ...draft!, rules: [...draft!.rules, rule] }
    inFlight.current = true; setBusy(true)
    try {
      const saved = await api.save({ config: configPayload(next), revision: current.current.revision })
      if (!alive.current) return t('系统提示词设置读取失败')
      adopt(saved); setError(''); setOpen(null)
      window.dispatchEvent(new Event(PROMPT_CHANGED))
      return null
    } catch (failure) {
      return failure instanceof Error ? failure.message : String(failure)
    } finally {
      inFlight.current = false
      if (alive.current) setBusy(false)
    }
  }

  async function saveBody(content: PromptContent): Promise<boolean> {
    if (editedRule) return commit(replaceRule({ ...editedRule, ...content }))
    return commit({ ...draft!, fallback: { ...draft!.fallback, ...content } })
  }

  async function clearDegrade() {
    if (inFlight.current) return
    inFlight.current = true; setBusy(true); setError('')
    try {
      await api.saveGateway({ clearDegrade: true })
      const next = await api.load()
      if (alive.current) {
        const localized = { ...next, clients: next.clients.map(client => client.id === 'other' ? { ...client, label: t('未识别客户端') } : client) }
        current.current = localized; setView(localized)
      }
    } catch (failure) { if (alive.current) setError(failure instanceof Error ? failure.message : String(failure)) }
    finally { inFlight.current = false; if (alive.current) setBusy(false) }
  }

  return <Card render={<section />} className='panel sp-panel'>
    <CardHeader>
      <CardTitle>{t('系统提示词')}</CardTitle>
      <Badge variant={capture ? 'success' : 'outline'}>{capture ? t('已接管') : t('透传')}</Badge>
      <CardAction><Button variant='ghost' size='sm' disabled={busy} onClick={() => void load()}>{t('刷新')}</Button></CardAction>
    </CardHeader>
    <CardContent className='sp-panel-body'>
      {error ? <Alert variant='destructive' className='sp-save-error'>
        <span>{error}</span>
        {dirty ? <Button variant='outline' size='sm' disabled={busy} onClick={() => void commit(draft)}>{t('重试保存')}</Button> : null}
      </Alert> : null}
      {view.configError ? <Alert variant='warning'>{view.configError}</Alert> : null}
      <div className='sp-rules-heading'>
        <span className='sp-rules-title'>{t('匹配规则')}</span>
        <span className='sp-order-hint'>{t('从上到下匹配，第一条命中的生效')}</span>
        <Button variant='outline' size='sm' disabled={busy || draft.rules.length >= view.maxRules}
          onClick={() => setCreating(true)}>{t('＋ 新建规则')}</Button>
      </div>
      <div className='sp-rules'>
        {draft.rules.map((rule, index) => <div key={rule.id} ref={node => { if (node) ruleNodes.current.set(rule.id, node); else ruleNodes.current.delete(rule.id) }}>
          <PromptRuleRow rule={rule} index={index} total={draft.rules.length} open={open === rule.id} view={view} disabled={busy} t={t}
            onToggle={() => setOpen(current => current === rule.id ? null : rule.id)} onChange={updateRule}
            onMove={delta => move(index, delta)} onDelete={() => { setOpen(null); void commit({ ...draft, rules: draft.rules.filter(current => current.id !== rule.id) }) }}
            onEditBody={() => setEditing({ ruleId: rule.id })} />
        </div>)}
        {!draft.rules.length ? <div className='sp-empty-rules'>{t('没有规则，所有请求使用兜底设置。')}</div> : null}
        <div className='sp-fallback'>
          <span className='sp-fallback-badge'>{t('兜底')}</span><span className='sp-fallback-title'>{t('未命中任何规则')}</span>
          <Select value={draft.fallback.mode} disabled={busy} onValueChange={next => {
            if (next) void commit({ ...draft, fallback: { ...draft.fallback, mode: next as PromptMode } })
          }}>
            <SelectTrigger aria-label={t('兜底动作')}><SelectValue>{modeLabel(draft.fallback.mode, t)}</SelectValue></SelectTrigger>
            <SelectContent>{(['custom', 'append', 'passthrough'] as const).map(mode => <SelectItem key={mode} value={mode}>{modeLabel(mode, t)}</SelectItem>)}</SelectContent>
          </Select>
          <Button variant='outline' size='sm' disabled={busy} onClick={() => setEditing({ fallback: true })}>{t('编辑正文')}</Button>
          <span className='sp-muted'>{sourceLabel(draft.fallback, t)}</span>
        </div>
      </div>
      <datalist id='sp-model-choices'>{view.models.map(model => <option key={model} value={model} />)}</datalist>
      <PromptRuleTester api={api} config={draft} view={view} t={t} onLocate={locate} />
      <PromptGatewaySection api={api} view={view} busy={busy || dirty} t={t} onRefresh={load} />
      {view.degradeActive ? <div className='sp-degrade'>
        <span>{t('内容拦截降级中，将于 {until} 自动解除。', { until: view.degradeUntilText })}</span>
        <Button variant='outline' size='sm' disabled={busy} onClick={() => void clearDegrade()}>{t('立即解除')}</Button>
      </div> : null}
    </CardContent>
    {creating ? <PromptRuleCreateDialog view={view} t={t} onSave={createRule} onClose={() => setCreating(false)} /> : null}
    {content ? <PromptBodyDialog key={editedRule?.id || 'fallback'} title={editedRule ? t('编辑提示词 · {name}', { name: editedRule.name || t('未命名规则') }) : t('编辑提示词 · 兜底')}
      content={content} t={t} onSave={saveBody} onClose={() => setEditing(null)} /> : null}
  </Card>
}
