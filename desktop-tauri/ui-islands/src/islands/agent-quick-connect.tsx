import * as React from 'react'
import { Alert, Badge, Button, Dialog, DialogBody, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, Empty } from '@ui'
import { SearchableSelect } from '../../../ui-shared/searchable-select'
import { t } from '../i18n'
import { refreshCatalogModels, useAgentCatalog, type CatalogModel } from './agent-catalog'
import { SingleModelFields } from './agent-fields'
import { ModelsFields } from './agent-model-fields'
import { protocolLabel, validateDraft } from './agent-model'
import { fillQuickProfile, quickProfile } from './agent-quick-model'
import { applyProfile, errorMessage, getAgentStore, requestPreview, subscribeAgentStore } from './agent-state'
import { WritePreview } from './agent-preview'
import type { AgentClient, AgentPreview, AgentProfile } from './agent-types'

export function QuickConnectDialog({ client, onClose }: { client: AgentClient; onClose: () => void }) {
  const store = React.useSyncExternalStore(subscribeAgentStore, getAgentStore, getAgentStore)
  const [draft, setDraft] = React.useState(() => ({ ...quickProfile(client, store.gateway), gatewayKeyId: store.gatewayKeys[0]?.id || '' }))
  const { catalog, loading, error: catalogError, retry } = useAgentCatalog(client.id === 'zed' ? undefined : draft.gatewayKeyId)
  const [error, setError] = React.useState('')
  const [preview, setPreview] = React.useState<AgentPreview | null>(null)
  const [busy, setBusy] = React.useState(false)
  const initialized = React.useRef(false)
  const previousCatalog = React.useRef<CatalogModel[]>([])
  const selectedKey = store.gatewayKeys.find(key => key.id === draft.gatewayKeyId)
  React.useEffect(() => {
    if (loading || catalogError) return
    const previous = previousCatalog.current
    previousCatalog.current = catalog
    if (!initialized.current) {
      initialized.current = true
      setDraft(profile => fillQuickProfile(client, profile, catalog, store.defaultModel))
    } else if (client.supportsModels && previous !== catalog) {
      setDraft(profile => ({ ...profile, models: refreshCatalogModels(client, profile.models, previous, catalog) }))
    }
  }, [loading, catalogError, catalog, client, store.defaultModel])
  const update = (patch: Partial<AgentProfile>) => { setDraft(current => ({ ...current, ...patch })); setPreview(null); setError('') }
  const ids = [...(client.supportsModels ? draft.models.map(model => model.id) : [draft.model]), ...Object.values(draft.claudeModels)].filter(Boolean)
  const unavailable = !loading && !catalogError ? ids.filter(id => !catalog.some(model => model.id.toLowerCase() === id.toLowerCase())) : []
  const connect = async () => {
    if (busy || loading || catalogError) return
    if (draft.authMode === 'gateway' && !selectedKey) { setError(t('请先在「网关 Key」页创建并启用一把 Key')); return }
    const missing = validateDraft(client, draft)
    if (missing) { setError(missing); return }
    if (unavailable.length) { setError(t('部分已选模型已不可用或所选 Key 无权使用，请移除或更换 Key。')); return }
    setBusy(true); setError('')
    try {
      if (!preview) setPreview(await requestPreview(client, draft, 'quick-connect'))
      else { await applyProfile(client, draft, 'quick-connect', preview); onClose() }
    } catch (error) { setError(errorMessage(error)); setPreview(null) }
    finally { setBusy(false) }
  }
  return <Dialog open onOpenChange={(open, details) => { if (!open && busy) details.cancel(); else if (!open) onClose() }}>
    <DialogContent className='agent-config-dialog agent-quick-dialog' showCloseButton={!busy}>
      {/* 说明挂到标题下当副标题：原来它是正文的第一段，与下面三块信息抢同一层级 */}
      <DialogHeader className='agent-quick-head'><DialogTitle>{t('一键接入')} · {client.name}</DialogTitle>
        <DialogDescription>{t('选择要接入的模型；确认前展示文件改动，写入前自动备份。')}</DialogDescription>
      </DialogHeader>
      <DialogBody className='agent-quick-body'>
        {preview ? <WritePreview preview={preview} /> : <>
          {/* 接进哪里：一行一个字段（端点 / 写入文件 / 网关 Key），左标签右值 */}
          <section className='agent-quick-info'>
            <div className='agent-quick-info-row'>
              <span className='agent-quick-info-key'>{t('接入端点')}</span>
              <code className='agent-quick-info-value'>{draft.baseUrl}</code>
              <Badge variant='outline' shape='tag'>{protocolLabel(draft.protocol)}</Badge>
            </div>
            <div className='agent-quick-info-row'>
              <span className='agent-quick-info-key'>{t('写入文件')}</span>
              <code className='agent-quick-info-value is-muted'>{client.resolvedPath}</code>
            </div>
            {client.id !== 'zed' && <div className='agent-quick-info-row'>
              <span className='agent-quick-info-key'>{t('网关 Key')}</span>
              <SearchableSelect value={draft.gatewayKeyId} disabled={busy || !store.gatewayKeys.length}
                onValueChange={gatewayKeyId => update({ gatewayKeyId })}
                className='agent-quick-info-select' aria-label={t('网关 Key')}
                displayValue={selectedKey ? selectedKey.name || selectedKey.id : t('选择网关 Key')}
                options={store.gatewayKeys.map(key => ({ value: key.id, label: key.name || key.id }))} />
              <span className='agent-quick-info-hint'>{t('换 Key 后会按新 Key 的授权重新核对已选模型')}</span>
            </div>}
          </section>
          {loading ? <Empty>{t('正在读取网关模型并填入配置…')}</Empty> : catalogError ? <Alert variant='warning'>
            <span>{catalogError}</span><Button variant='outline' size='sm' disabled={busy} onClick={retry}>{t('重试')}</Button>
          </Alert> : <>
            {unavailable.length > 0 && <Alert variant='warning'>{t('部分已选模型已不可用或所选 Key 无权使用，请移除或更换 Key。')}<code className='block break-all'>{unavailable.join('、')}</code></Alert>}
            {client.supportsModels ? <ModelsFields client={client} draft={draft} update={update} catalog={catalog} disabled={busy} quick layout='split' />
              : <SingleModelFields client={client} draft={draft} update={update} catalog={catalog} disabled={busy} />}
            {!catalog.length && <Alert variant='warning'>{t('所选 Key 下没有可用的对话模型，请先启用模型或更换 Key。')}</Alert>}
          </>}
          {client.id === 'zed' && <Alert variant='warning'>{t('Zed 的 API Key 仍需在客户端提供商设置中填写；本页不修改系统凭证。')}</Alert>}
        </>}
        {error && <Alert variant='destructive'>{error}</Alert>}
      </DialogBody>
      <DialogFooter>
        {preview && <Button variant='ghost' className='mr-auto' disabled={busy} onClick={() => { setPreview(null); setError('') }}>{t('返回编辑')}</Button>}
        <Button variant='outline' disabled={busy} onClick={onClose}>{t('取消')}</Button>
        <Button disabled={busy || loading || Boolean(catalogError)} onClick={() => void connect()}>{busy ? t('处理中…') : preview ? t('确认接入') : t('接入客户端')}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}
