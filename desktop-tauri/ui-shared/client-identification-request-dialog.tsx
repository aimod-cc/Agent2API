import { useEffect, useRef, useState } from 'react'
import { Alert, Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, Label, SearchSelect } from '@ui'
import type { Translate } from './assistant-read-types'
import { requestClientLabel, type RequestIdentity } from './model-api-key'
import { ClientRuleEditor } from './client-identification-editor'
import {
  clientMethodLabel, clientSettingsError, notifyIdentificationChanged,
  type ClientEvidence, type ClientIdentificationApi, type ClientIdentificationView, type ClientSettings,
} from './client-identification'

/**
 * 请求详情入口的「设置识别规则」弹窗。
 *
 * 识别管线是两层：请求头识别出真实客户端（来源声明 / 头匹配规则 / 内置规则），
 * 记账时再按供应商归属规则改写最终归属。弹窗按这条链路展示两段结果，主操作
 * 是给这条请求的供应商快捷维护归属规则；自定义头匹配规则降级为次入口，
 * 用于给难认的标识钉规则，不再像旧版那样直接铺一张头匹配表单。
 */
export function RequestIdentificationDialog({ api, view: initialView, entry, evidence, onClose, onSaved, t }: {
  api: ClientIdentificationApi
  view: ClientIdentificationView
  entry: RequestIdentity
  evidence: ClientEvidence
  onClose: () => void
  onSaved: (view: ClientIdentificationView) => void
  t: Translate
}) {
  const [view, setView] = useState(initialView)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  /** 次入口：展开旧的单条头匹配规则编辑器（new = 按证据新建，existing = 编辑命中的那条） */
  const [headerEditor, setHeaderEditor] = useState<'new' | 'existing' | null>(null)
  const provider = entry.provider?.trim() || ''
  const providerRule = provider
    ? view.config.providerRules.find(rule => rule.provider.trim().toLowerCase() === provider.toLowerCase())
    : undefined
  const [draftProvider, setDraftProvider] = useState(providerRule?.provider || provider)
  const [draftClient, setDraftClient] = useState(providerRule?.clientId || evidence.clientId || '')
  const alive = useRef(true)
  useEffect(() => { alive.current = true; return () => { alive.current = false } }, [])
  const providerOptions = view.providers.map(item => ({
    value: item.id, label: item.custom ? t('自定义 · {name}', { name: item.label }) : item.label,
  }))
  const clientOptions = [
    ...view.builtins.map(client => ({ value: client.id, label: client.name })),
    ...view.config.clients.map(client => ({ value: client.id, label: client.name })),
  ]
  const clientName = (id: string) => clientOptions.find(option => option.value === id)?.label || id
  const providerName = (id: string) => providerOptions.find(option => option.value === id)?.label || id
  // 归属来源与后端 attribution_client 同一优先级：规则 → 默认 → 透传识别结果
  const sourceLabel = providerRule ? t('供应商归属规则')
    : view.config.defaultClientId ? t('默认归属客户端') : t('未改写，保留识别结果')
  const existingHeaderRule = evidence.ruleId ? view.config.rules.find(rule => rule.id === evidence.ruleId) : undefined
  const apply = async (next: ClientSettings) => {
    const invalid = clientSettingsError(next, view, t)
    if (invalid) { setError(invalid); return }
    setBusy(true); setError('')
    try {
      const saved = await api.save({ config: next, revision: view.revision })
      notifyIdentificationChanged()
      if (alive.current) { setBusy(false); onSaved(saved) }
    } catch (failure) {
      if (alive.current) { setError(failure instanceof Error ? failure.message : String(failure)); setBusy(false) }
    }
  }
  const saveProviderRule = () => {
    const providerId = draftProvider.trim()
    if (!providerId) { setError(t('请选择供应商')); return }
    if (!draftClient) { setError(t('请选择归属客户端')); return }
    void apply({
      ...view.config,
      providerRules: providerRule
        ? view.config.providerRules.map(rule => rule === providerRule ? { provider: providerId, clientId: draftClient } : rule)
        : [...view.config.providerRules, { provider: providerId, clientId: draftClient }],
    })
  }
  const removeProviderRule = () => {
    if (!providerRule) return
    void apply({
      ...view.config,
      providerRules: view.config.providerRules.filter(rule => rule !== providerRule),
    })
  }
  const reload = async () => {
    setBusy(true)
    try { const next = await api.load(); if (alive.current) { setView(next); setError('') } }
    catch (failure) { if (alive.current) setError(failure instanceof Error ? failure.message : String(failure)) }
    finally { if (alive.current) setBusy(false) }
  }
  return <Dialog open onOpenChange={open => { if (!open && !busy && !headerEditor) onClose() }}>
    <DialogContent className='w-[min(640px,calc(100vw-24px))]'>
      <DialogHeader><DialogTitle>{t('设置识别规则')}</DialogTitle></DialogHeader>
      <DialogBody>
        <div className='rounded-lg bg-surface-inset p-3 text-[12.5px] leading-[1.7]'>
          <p className='text-muted-foreground'>{t('识别结果（请求头识别出的真实客户端）')}</p>
          <p>{evidence.clientName || evidence.clientId} · {clientMethodLabel(evidence.method, t)}</p>
          {evidence.matchedHeader ? <p className='break-all text-muted-foreground'>{t('命中依据：{header} · {pattern}', { header: evidence.matchedHeader, pattern: evidence.matchedPattern })}</p> : null}
        </div>
        <div className='rounded-lg bg-surface-inset p-3 text-[12.5px] leading-[1.7]'>
          <p className='text-muted-foreground'>{t('最终归属（记账时的客户端）')}</p>
          <p>{requestClientLabel(entry, t)} · {sourceLabel}</p>
          {providerRule ? <p className='break-all text-muted-foreground'>{t('命中规则：{provider} → {client}', { provider: providerName(providerRule.provider), client: clientName(providerRule.clientId) })}</p> : null}
        </div>
        {provider ? <>
          <Label>{t('供应商归属规则')}</Label>
          <div className='grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)] gap-2'>
            <SearchSelect value={draftProvider} options={providerOptions} disabled={busy}
              placeholder={t('选择供应商')} searchPlaceholder={t('搜索供应商')} aria-label={t('供应商')} className='min-w-0'
              onValueChange={value => { setDraftProvider(value); setError('') }} />
            <SearchSelect value={draftClient} options={clientOptions} disabled={busy}
              placeholder={t('选择归属客户端')} searchPlaceholder={t('搜索客户端')} aria-label={t('归属客户端')} className='min-w-0'
              onValueChange={value => { setDraftClient(value); setError('') }} />
          </div>
          {providerRule ? <p className='text-[12px] text-muted-foreground'>{t('该供应商已有归属规则，保存将覆盖它')}</p> : null}
        </> : <Alert>{t('这条请求没有记录供应商（旧数据或未走到转发），不能从这里改归属')}</Alert>}
        {error ? <Alert variant='destructive'><p>{error}</p><Button size='sm' variant='outline' disabled={busy} onClick={() => void reload()}>{t('保留草稿并读取最新设置')}</Button></Alert> : null}
      </DialogBody>
      <DialogFooter>
        <Button variant='outline' disabled={busy} onClick={() => setHeaderEditor(existingHeaderRule ? 'existing' : 'new')}>
          {existingHeaderRule ? t('编辑这条头匹配规则…') : t('添加自定义头匹配规则…')}
        </Button>
        {providerRule ? <Button variant='destructive' disabled={busy} onClick={removeProviderRule}>{t('删除归属规则')}</Button> : null}
        <Button disabled={busy || !draftProvider || !draftClient} onClick={saveProviderRule}>{busy ? t('保存中…') : t('保存归属规则')}</Button>
        <Button variant='ghost' disabled={busy} onClick={onClose}>{t('取消')}</Button>
      </DialogFooter>
    </DialogContent>
    {headerEditor ? <ClientRuleEditor api={api} view={view} evidence={evidence} rule={headerEditor === 'existing' ? existingHeaderRule : undefined}
      t={t} onClose={() => setHeaderEditor(null)} onSaved={next => { setView(next); setHeaderEditor(null) }} /> : null}
  </Dialog>
}
