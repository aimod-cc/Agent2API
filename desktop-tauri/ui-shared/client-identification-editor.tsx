import { useEffect, useRef, useState } from 'react'
import { Alert, Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, Input, Label, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Switch } from '@ui'
import type { Translate } from './assistant-read-types'
import { blankRule, clientSettingsError, notifyIdentificationChanged, randomId, type ClientEvidence, type ClientIdentificationApi, type ClientIdentificationView, type ClientRule, type ClientSettings, type IdentificationPreview } from './client-identification'

const NEW_CLIENT = '__new_client__'

export function ClientRuleEditor({ api, view: initialView, rule, evidence, onClose, onSaved, t }: {
  api: ClientIdentificationApi
  view: ClientIdentificationView
  rule?: ClientRule
  evidence?: ClientEvidence
  onClose: () => void
  onSaved: (view: ClientIdentificationView) => void
  t: Translate
}) {
  const [view, setView] = useState(initialView)
  const [draft, setDraft] = useState<ClientRule>(() => rule ? { ...rule } : blankRule(evidence))
  const [name, setName] = useState(evidence?.candidate || '')
  const [customId] = useState(() => `custom:${randomId()}`)
  const [preview, setPreview] = useState<IdentificationPreview | null>(null)
  const [busy, setBusy] = useState<'preview' | 'save' | null>(null)
  const [error, setError] = useState('')
  const alive = useRef(true)
  useEffect(() => { alive.current = true; return () => { alive.current = false } }, [])
  const clients = [...view.builtins, ...view.config.clients]
  const clientName = draft.clientId === NEW_CLIENT ? t('新建客户端分类') : clients.find(client => client.id === draft.clientId)?.name || t('请选择归属客户端')
  const update = (patch: Partial<ClientRule>) => { setDraft(previous => ({ ...previous, ...patch })); setPreview(null); setError('') }
  const config = (): ClientSettings => {
    const next = { ...draft, pattern: draft.pattern.trim(), clientId: draft.clientId === NEW_CLIENT ? customId : draft.clientId }
    return {
      ...view.config,
      clients: draft.clientId === NEW_CLIENT ? [...view.config.clients, { id: customId, name: name.trim() }] : view.config.clients,
      // 新规则放在自定义规则最前面；预览使用的顺序与保存后的顺序完全相同。
      rules: rule ? view.config.rules.map(item => item.id === rule.id ? next : item) : [next, ...view.config.rules],
    }
  }
  const execute = async (operation: 'preview' | 'save') => {
    if (rule && !view.config.rules.some(item => item.id === rule.id)) { setError(t('这条规则已被删除，请关闭编辑后重新添加。')); return }
    const next = config()
    const invalid = clientSettingsError(next, view, t)
    if (invalid) { setError(invalid); return }
    setBusy(operation); setError('')
    try {
      if (operation === 'preview') {
        const result = await api.preview({ config: next, ruleId: draft.id, sample: evidence })
        if (alive.current) setPreview(result)
      } else {
        const saved = await api.save({ config: next, revision: view.revision })
        notifyIdentificationChanged()
        if (alive.current) onSaved(saved)
      }
    } catch (failure) {
      if (alive.current) setError(failure instanceof Error ? failure.message : String(failure))
    } finally { if (alive.current) setBusy(null) }
  }
  const modes = [
    ...(draft.header === 'user-agent' ? [{ value: 'product', label: t('产品名') }] : []),
    { value: 'prefix', label: t('前缀') }, { value: 'contains', label: t('包含') }, { value: 'exact', label: t('完全相同') },
  ]
  return <Dialog open onOpenChange={open => { if (!open && !busy) onClose() }}>
    <DialogContent className='w-[min(640px,calc(100vw-24px))]'>
      <DialogHeader><DialogTitle>{rule ? t('编辑识别规则') : t('设置识别规则')}</DialogTitle></DialogHeader>
      <DialogBody>
        <p className='max-w-[62ch] text-[12px] leading-[1.7] text-muted-foreground'>{t('规则忽略大小写，只影响新请求的日志与统计，不改变模型选择或权限。')}</p>
        {/* 与面板同一套行形态：底色分层而不是描边，开关在左、状态右对齐 */}
        <div className='flex items-center gap-3 rounded-lg bg-surface-inset px-4 py-3'>
          <Switch checked={draft.enabled} disabled={Boolean(busy)} onCheckedChange={enabled => update({ enabled })} aria-label={t('启用规则')} />
          <span className='text-[13px]'>{t('启用规则')}</span>
          <span className='ml-auto text-[12px] text-muted-foreground'>{draft.enabled ? t('已启用') : t('已停用')}</span>
        </div>
        <Label>{t('识别依据')}</Label>
        <Select value={draft.header} disabled={Boolean(busy)} onValueChange={value => update({ header: String(value), matchType: value === 'user-agent' ? 'product' : 'exact' })}>
          <SelectTrigger aria-label={t('识别依据')}><SelectValue>{draft.header}</SelectValue></SelectTrigger>
          <SelectContent>{view.headers.map(header => <SelectItem key={header} value={header}>{header}</SelectItem>)}</SelectContent>
        </Select>
        <div className='grid grid-cols-[120px_minmax(0,1fr)] gap-3'>
          <div className='flex flex-col gap-2'><Label>{t('匹配方式')}</Label>
            <Select value={draft.matchType} disabled={Boolean(busy)} onValueChange={value => update({ matchType: value as ClientRule['matchType'] })}>
              <SelectTrigger aria-label={t('匹配方式')}><SelectValue>{modes.find(mode => mode.value === draft.matchType)?.label}</SelectValue></SelectTrigger>
              <SelectContent>{modes.map(mode => <SelectItem key={mode.value} value={mode.value}>{mode.label}</SelectItem>)}</SelectContent>
            </Select>
          </div>
          <div className='flex flex-col gap-2'><Label>{t('匹配内容')}</Label><Input value={draft.pattern} maxLength={256} disabled={Boolean(busy)} onChange={event => update({ pattern: event.currentTarget.value })} aria-label={t('匹配内容')} placeholder={draft.header === 'user-agent' ? 'magpie' : 'Cline'} /></div>
        </div>
        <Label>{t('归属客户端')}</Label>
        <Select value={draft.clientId || '__choose__'} disabled={Boolean(busy)} onValueChange={value => update({ clientId: String(value) })}>
          <SelectTrigger aria-label={t('归属客户端')}><SelectValue>{clientName}</SelectValue></SelectTrigger>
          <SelectContent><SelectItem value='__choose__' disabled>{t('请选择归属客户端')}</SelectItem>
            {clients.map(client => <SelectItem key={client.id} value={client.id}>{client.name}</SelectItem>)}
            <SelectItem value={NEW_CLIENT}>{t('新建客户端分类')}</SelectItem>
          </SelectContent>
        </Select>
        {draft.clientId === NEW_CLIENT ? <Input value={name} maxLength={80} disabled={Boolean(busy)} onChange={event => { setName(event.currentTarget.value); setPreview(null); setError('') }} aria-label={t('客户端名称')} placeholder={t('客户端名称')} /> : null}
        {draft.header === view.declarationHeader && !view.config.acceptDeclarations ? <Alert>{t('来源声明当前关闭，这条规则不会生效。请先在客户端识别设置中开启。')}</Alert> : null}
        {error ? <Alert variant='destructive'><p>{error}</p><Button size='sm' variant='outline' disabled={Boolean(busy)} onClick={async () => {
          setBusy('preview')
          try { const next = await api.load(); if (alive.current) { setView(next); setPreview(null); setError('') } }
          catch (failure) { if (alive.current) setError(failure instanceof Error ? failure.message : String(failure)) }
          finally { if (alive.current) setBusy(null) }
        }}>{t('保留草稿并读取最新设置')}</Button></Alert> : null}
        {preview ? <div className='flex flex-col gap-2 rounded-lg bg-surface-inset p-3 text-[12px] leading-[1.7]'>
          <p>{t('最近 {window} 条请求中，{scanned} 条有识别依据，{matched} 条符合条件，{applied} 条会采用本规则。', { window: preview.window, scanned: preview.scanned, matched: preview.matched, applied: preview.applied })}</p>
          {preview.sample ? <p>{t('当前样本结果：{name}', { name: preview.sample.clientLabel || preview.sample.evidence.candidate || t('未知客户端') })}
            {!preview.sample.ruleApplied ? ` · ${t('未采用本规则，请检查开关、来源声明或规则顺序')}` : ''}</p> : null}
          {preview.samples.slice(0, 5).map((sample, index) => <div key={index} className='break-all border-t border-hairline pt-2'>
            <div>{sample.evidence.userAgent || sample.evidence.headers[draft.header] || '—'}</div>
            <div className='text-muted-foreground'>{sample.clientLabel || t('未知客户端')} · {sample.ruleApplied ? t('采用本规则') : t('被更高优先级覆盖')}</div>
          </div>)}
          {!preview.matched ? <p>{t('近期没有命中样本，不代表未来请求不会命中。')}</p> : null}
          <p className='text-muted-foreground'>{t('预览不发送模型请求，也不修改历史记录。')}</p>
        </div> : null}
      </DialogBody>
      <DialogFooter>
        <Button variant='ghost' disabled={Boolean(busy)} onClick={onClose}>{t('取消')}</Button>
        <Button variant='outline' disabled={Boolean(busy)} onClick={() => void execute('preview')}>{busy === 'preview' ? t('预览中…') : t('预览匹配')}</Button>
        <Button disabled={Boolean(busy) || !preview} onClick={() => void execute('save')}>{busy === 'save' ? t('保存中…') : t('保存规则')}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}
