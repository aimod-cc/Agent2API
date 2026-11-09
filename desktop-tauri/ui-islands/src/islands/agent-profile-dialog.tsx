import * as React from 'react'
import { Alert, Button, Collapsible, CollapsibleContent, CollapsibleTrigger, Dialog, DialogBody, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, Field, FieldDescription, FieldLabel, Switch } from '@ui'
import { t } from '../i18n'
import { applyProfile, confirm, errorMessage, getAgentStore, requestPreview, saveConfiguration, toast } from './agent-state'
import { cloneProfile, profileSignature, validateDraft } from './agent-model'
import { ConnectionFields, SingleModelFields, TextField } from './agent-fields'
import { ModelsFields } from './agent-model-fields'
import { refreshCatalogModels, useAgentCatalog, type CatalogModel } from './agent-catalog'
import { WritePreview } from './agent-preview'
import { fillQuickProfile } from './agent-quick-model'
import { AgentIcon } from './agent-actions'
import type { AgentClient, AgentPreview, AgentProfile } from './agent-types'

export function ProfileDialog({ client, profile, creating, onClose }: { client: AgentClient; profile: AgentProfile; creating: boolean; onClose: () => void }) {
  const native = Boolean(profile.liveId)
  const [draft, setDraft] = React.useState(() => cloneProfile(profile))
  const [error, setError] = React.useState('')
  const [saving, setSaving] = React.useState(false)
  const [preview, setPreview] = React.useState<AgentPreview | null>(null)
  const [previewing, setPreviewing] = React.useState(false)
  const { catalog, loading: catalogLoading, error: catalogError, retry: retryCatalog } = useAgentCatalog(
    draft.kind === 'gateway' && draft.authMode === 'gateway' ? draft.gatewayKeyId : undefined)
  const initial = React.useRef(profileSignature(profile))
  const nativeRevision = React.useRef(client.nativeRevision)
  const revision = React.useRef(client.revision)
  const configPath = React.useRef(client.configPath)
  const previewClient = { ...client, nativeRevision: nativeRevision.current, revision: revision.current, configPath: configPath.current }
  const previewGeneration = React.useRef(0)
  const dirty = profileSignature(draft) !== initial.current
  const operation = 'config-edit' as const
  const single = ['codex', 'claude-code'].includes(client.id) && !draft.baseUrl && draft.authMode === 'preserve' && !draft.models.length
  const validationClient = { ...client, supportsModels: client.supportsModels && !single,
    ...(native && !profile.enabled && !profile.models.length && !profile.model ? { supportsDefault: false } : {}) }
  const previousCatalog = React.useRef<CatalogModel[]>([])
  const initialized = React.useRef(!creating)
  const [confirming, setConfirming] = React.useState(false)
  const [detailsOpen, setDetailsOpen] = React.useState(false)
  React.useEffect(() => {
    if (catalogLoading || catalogError) return
    const previous = previousCatalog.current
    previousCatalog.current = catalog
    if (draft.kind !== 'gateway') return
    if (!initialized.current) {
      initialized.current = true
      setDraft(profile => profile.models.length || profile.model ? profile : fillQuickProfile(client, profile, catalog, getAgentStore().defaultModel))
      return
    }
    if (!previous.length || !client.supportsModels) return
    previewGeneration.current += 1
    setPreview(null); setConfirming(false); setPreviewing(false)
    setDraft(profile => ({ ...profile, models: refreshCatalogModels(client, profile.models, previous, catalog) }))
  }, [catalogLoading, catalogError, catalog, client.id, client.supportsModels, draft.kind])
  React.useEffect(() => () => { previewGeneration.current += 1 }, [])
  const update = (values: Partial<AgentProfile>) => {
    previewGeneration.current += 1
    setDraft(current => ({ ...current, ...values })); setPreview(null); setConfirming(false); setPreviewing(false); setError('')
  }
  const close = async () => {
    if (saving) return
    if (dirty && !await confirm(t('放弃未保存的修改？'), t('关闭后丢弃本次修改，不会保存配置或改动客户端。'), t('放弃修改'))) return
    onClose()
  }
  const save = async () => {
    if (saving) return
    const message = validateDraft(validationClient,draft)
    if (message) { setError(message); return }
    if (native) {
      await showPreview()
      return
    }
    setSaving(true); setError('')
    try {
      await saveConfiguration(previewClient, draft, creating)
      onClose(); toast(t('配置已保存，可在列表中点击使用'))
    } catch (error) { setError(errorMessage(error)); setSaving(false) }
  }
  const showPreview = async () => {
    const message = validateDraft(validationClient,draft)
    if (message) { setError(message); return }
    const generation = ++previewGeneration.current
    setPreviewing(true); setError('')
    try { const result = await requestPreview(previewClient,draft,operation); if (generation === previewGeneration.current) { setPreview(result); setConfirming(true) } }
    catch (error) { if (generation === previewGeneration.current) setError(errorMessage(error)) }
    finally { if (generation === previewGeneration.current) setPreviewing(false) }
  }
  const apply = async () => {
    if (!preview || saving) return
    setSaving(true); setError('')
    try { await applyProfile(previewClient,draft,operation,preview); onClose() }
    catch (error) { setError(errorMessage(error)); setPreview(null); setConfirming(false) }
    finally { setSaving(false) }
  }
  return <Dialog open onOpenChange={(open,details) => { if (!open) { details.cancel(); void close() } }}>
    <DialogContent className='agent-config-dialog' showCloseButton={!saving}>
      <DialogHeader className='agent-config-head'><DialogTitle>{creating ? t('新增配置') : t('编辑配置')} · {client.name}</DialogTitle>
        <DialogDescription>{native ? t('保存后同步更新客户端；写入前可查看改动，并自动备份。') : t('保存后可在配置列表中切换，不影响当前使用的配置。')}</DialogDescription>
      </DialogHeader>
      <DialogBody>
        {!confirming && <>
        <section className='agent-profile-connection flex min-w-0 flex-col gap-4'>
          <div className='grid min-w-0 grid-cols-1 gap-4 sm:grid-cols-2'>
            <TextField label={t('配置名称')} fieldKey='' value={draft.name} onChange={name => update({name})} disabled={saving || previewing} />
            <TextField label={t('备注')} fieldKey='' value={draft.note} onChange={note => update({note})} disabled={saving || previewing} />
          </div>
          <ConnectionFields client={client} draft={draft} update={update} disabled={saving || previewing} />
          {draft.kind === 'gateway' && catalogLoading && <p className='text-xs text-subtle'>{t('正在读取网关模型并填入配置…')}</p>}
          {catalogError && draft.kind === 'gateway' && <Alert variant='warning'>{t('模型目录读取失败，可手动填写真实模型 ID。')} {catalogError}
            <Button variant='outline' size='sm' onClick={retryCatalog} disabled={saving || previewing}>{t('重试')}</Button></Alert>}
        </section>
        {validationClient.supportsModels ? <ModelsFields client={validationClient} draft={draft} update={update} catalog={catalog} disabled={saving || previewing || draft.kind === 'gateway' && catalogLoading} native={native} />
          : <section className='flex min-w-0 flex-col gap-4 border-t border-hairline pt-4'><h3 className='text-sm font-semibold'>{t('模型配置')}</h3>
            <SingleModelFields client={validationClient} draft={draft} update={update} catalog={catalog} disabled={saving || previewing} /></section>}
        {client.supportsToggle && native && <Field className='rounded-md border border-border bg-surface-inset p-3'>
          <div className='flex items-center justify-between gap-3'><FieldLabel>{t('启用此配置')}</FieldLabel>
            <Switch checked={draft.enabled} disabled={saving || previewing} onCheckedChange={enabled => update({enabled})} aria-label={t('启用此配置')} /></div>
          <FieldDescription>{t('确认写入后会同步修改客户端开关；取消不会生效。')}</FieldDescription>
        </Field>}
        </>}
        {preview && <>
          <Alert>{t('确认后将更新 {name} 的配置文件，原配置会自动备份。', { name: client.name })}</Alert>
          {preview.warnings.map((warning, index) => <p key={index} className='text-xs leading-relaxed text-subtle'>{warning}</p>)}
          <Collapsible open={detailsOpen} onOpenChange={setDetailsOpen}>
            <CollapsibleTrigger render={<Button variant='ghost' size='sm' />}><AgentIcon name='chevron' />{t('查看文件改动')}</CollapsibleTrigger>
            <CollapsibleContent><WritePreview preview={preview} showWarnings={false} /></CollapsibleContent>
          </Collapsible>
        </>}
        {error && <Alert variant='destructive'>{error}</Alert>}
      </DialogBody>
      <DialogFooter className='agent-config-footer'>
        {confirming && <Button variant='ghost' disabled={saving || previewing} onClick={() => { setConfirming(false); setPreview(null) }}>{t('返回编辑')}</Button>}<span className='mr-auto' />
        <Button variant='outline' disabled={saving || previewing} onClick={() => void close()}>{t('取消')}</Button>
        <Button disabled={saving || previewing || (!creating && !dirty) || (confirming && !preview)} onClick={() => void (confirming ? apply() : save())}>{saving ? t('保存中…') : previewing ? t('处理中…') : confirming ? t('确认保存') : t('保存')}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}
