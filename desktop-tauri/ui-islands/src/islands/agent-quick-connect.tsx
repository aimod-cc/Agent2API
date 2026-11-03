import * as React from 'react'
import { Alert, Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, Empty, Field, FieldLabel, Input, Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@ui'
import { t } from '../i18n'
import { loadCatalog, type CatalogModel } from './agent-catalog'
import { ClaudeModelFields } from './agent-fields'
import { ModelsFields } from './agent-model-fields'
import { protocolLabel, validateDraft } from './agent-model'
import { fillQuickProfile, quickModels, quickProfile } from './agent-quick-model'
import { applyProfile, errorMessage, getAgentStore, requestPreview } from './agent-state'
import { WritePreview } from './agent-preview'
import type { AgentClient, AgentPreview, AgentProfile } from './agent-types'

export function QuickConnectDialog({ client, onClose }: { client: AgentClient; onClose: () => void }) {
  const store = getAgentStore()
  const [draft,setDraft] = React.useState(() => quickProfile(client,store.gateway))
  const [catalog,setCatalog] = React.useState<CatalogModel[]>([])
  const [loading,setLoading] = React.useState(true)
  const [error,setError] = React.useState('')
  const [preview,setPreview] = React.useState<AgentPreview | null>(null)
  const [busy,setBusy] = React.useState(false)
  const [advanced,setAdvanced] = React.useState(false)
  const selectedKey = store.gatewayKeys.find(key => key.id === draft.gatewayKeyId) || (!draft.gatewayKeyId ? store.gatewayKeys[0] : undefined)
  const options = quickModels(catalog,selectedKey)
  const ids = client.supportsModels ? draft.models.map(model => model.id) : [draft.model]
  const validModels = ids.every(id => !id || !selectedKey?.allowedModels.length || selectedKey.allowedModels.some(model => model.toLowerCase() === id.toLowerCase()))
  const knownSelection = ids.every(id => !id || !catalog.some(model => model.id === id) || options.some(model => model.id === id))
  const missing = validateDraft(client,draft)
  React.useEffect(() => {
    let active = true
    void loadCatalog().then(catalog => {
      if (!active) return
      setCatalog(catalog)
      const key = store.gatewayKeys[0]
      setDraft(profile => fillQuickProfile(client,{ ...profile, gatewayKeyId:key?.id || '' },quickModels(catalog,key),store.defaultModel))
    }).catch(error => { if (active) setError(errorMessage(error)) }).finally(() => { if (active) setLoading(false) })
    return () => { active = false }
  }, [client.id])
  const update = (patch: Partial<AgentProfile>) => { setDraft(current => ({ ...current, ...patch })); setPreview(null); setError('') }
  const connect = async () => {
    if (busy || loading) return
    if (!store.gatewayKeys.length && draft.authMode === 'gateway') { setError(t('请先在「网关 Key」页创建并启用一把 Key')); return }
    if (missing) { setAdvanced(true); setError(missing); return }
    if (!validModels || !knownSelection) { setError(t('所选 Key 无权使用当前模型，请重新选择')); return }
    setBusy(true); setError('')
    try {
      if (!preview) {
        const next = await requestPreview(client,draft,'quick-connect')
        setPreview(next)
      } else {
        await applyProfile(client,draft,'quick-connect',preview)
        onClose()
      }
    } catch (error) { setError(errorMessage(error)); setPreview(null) }
    finally { setBusy(false) }
  }
  const keyLabel = selectedKey ? selectedKey.name || selectedKey.id : t('选择网关 Key')
  return <Dialog open onOpenChange={(open,details) => { if (!open && busy) details.cancel(); else if (!open) onClose() }}>
    <DialogContent className='agent-config-dialog' showCloseButton={!busy}>
      <DialogHeader><DialogTitle>{t('一键接入')} · {client.name}</DialogTitle></DialogHeader>
      <DialogBody>
        <Alert>{t('已预填网关地址、协议、Key 与模型；确认后直接写入客户端，不需要先保存方案。')}</Alert>
        <code className='break-all text-xs'>{client.resolvedPath}</code>
        {loading ? <Empty>{t('正在读取网关模型并填入配置…')}</Empty> : preview ? <WritePreview preview={preview} /> : <>
          <div className='rounded-md border border-border bg-surface-inset px-3 py-2 text-xs'>
            <code className='break-all'>{draft.baseUrl}</code><span className='ml-3 text-subtle'>{protocolLabel(draft.protocol)}</span>
          </div>
          {client.id !== 'zed' && <Field><FieldLabel>{t('网关 Key')}</FieldLabel>
            <Select value={draft.gatewayKeyId} disabled={busy || !store.gatewayKeys.length} onValueChange={value => {
              const key = store.gatewayKeys.find(key => key.id === value)
              setDraft(profile => fillQuickProfile(client,{ ...profile, gatewayKeyId:String(value), model:'' },quickModels(catalog,key),store.defaultModel))
              setError(''); setPreview(null)
            }}><SelectTrigger><SelectValue>{keyLabel}</SelectValue></SelectTrigger><SelectContent>
              {store.gatewayKeys.map(key => <SelectItem key={key.id} value={key.id}>{key.name || key.id}</SelectItem>)}
            </SelectContent></Select>
          </Field>}
          <Field><FieldLabel>{t('模型')}</FieldLabel>
            <Select value={options.some(model => model.id === draft.model) ? draft.model : ''} disabled={busy || !options.length} onValueChange={value => {
              setDraft(profile => fillQuickProfile(client,{ ...profile, model:String(value) },options,store.defaultModel,profile.model))
              setError(''); setPreview(null)
            }}><SelectTrigger><SelectValue>{options.find(model => model.id === draft.model)?.label || draft.model || t('请选择模型')}</SelectValue></SelectTrigger><SelectContent>
              {options.map(model => <SelectItem key={model.id} value={model.id}>{model.label}</SelectItem>)}
            </SelectContent></Select>
          </Field>
          {client.id === 'claude-code' && <ClaudeModelFields draft={draft} update={update} catalog={options} disabled={busy}
            note={t('Claude Code 的 Opus / Sonnet / Haiku / 子 Agent 四档默认都指向上面选的模型；要让某一档用别的模型，在这里单独改。')} />}
          {!options.length ? <Alert variant='warning'>{t('所选 Key 下没有可用的对话模型，请先启用模型或手动填写。')}</Alert> : null}
          {client.id === 'zed' ? <Alert variant='warning'>{t('Zed 的 API Key 仍需在客户端提供商设置中填写；本页不修改系统凭证。')}</Alert> : null}
          <Button variant='ghost' size='sm' className='self-start' disabled={busy} onClick={() => setAdvanced(value => !value)}>{advanced ? t('收起更多配置') : t('更多配置')}</Button>
          {advanced || missing ? <>
            <Field><FieldLabel>{t('模型 ID')}</FieldLabel><Input value={draft.model} disabled={busy} onChange={event => update({ model:event.currentTarget.value,
              ...(!client.supportsModels ? {} : { models:draft.models.map((model,index) => index === 0 ? { ...model, id:event.currentTarget.value } : model) }) })} /></Field>
            {client.supportsModels && <ModelsFields client={client} draft={draft} update={update} catalog={options} disabled={busy} />}
          </> : null}
        </>}
        {error && <Alert variant='destructive'>{error}</Alert>}
      </DialogBody>
      <DialogFooter>
        {preview && <Button variant='ghost' className='mr-auto' disabled={busy} onClick={() => { setPreview(null); setError('') }}>{t('返回编辑')}</Button>}
        <Button variant='outline' disabled={busy} onClick={onClose}>{t('取消')}</Button>
        <Button disabled={busy || loading} onClick={() => void connect()}>{busy ? t('处理中…') : preview ? t('确认接入') : t('接入客户端')}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}
