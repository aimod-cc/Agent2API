import * as React from 'react'
import { Alert, Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, Field, FieldDescription, FieldLabel, Switch } from '@ui'
import { t } from '../i18n'
import { applyProfile, confirm, errorMessage, requestPreview, saveProfiles, toast } from './agent-state'
import { cloneProfile, defaultOperation, profileSignature, validateDraft } from './agent-model'
import { ConnectionFields, SingleModelFields, TextField } from './agent-fields'
import { ModelsFields } from './agent-model-fields'
import { refreshCatalogModels, useAgentCatalog, type CatalogModel } from './agent-catalog'
import { WritePreview } from './agent-preview'
import type { AgentClient, AgentPreview, AgentProfile } from './agent-types'

export function ProfileDialog({ client, profile, creating, native = false, onClose }: { client: AgentClient; profile: AgentProfile; creating: boolean; native?: boolean; onClose: () => void }) {
  const [draft, setDraft] = React.useState(() => cloneProfile(profile))
  const [error, setError] = React.useState('')
  const [saving, setSaving] = React.useState(false)
  const [preview, setPreview] = React.useState<AgentPreview | null>(null)
  const [previewing, setPreviewing] = React.useState(false)
  const { catalog, loading: catalogLoading, error: catalogError, retry: retryCatalog } = useAgentCatalog(
    draft.kind === 'gateway' && draft.authMode === 'gateway' ? draft.gatewayKeyId : undefined)
  const initial = React.useRef(profileSignature(profile))
  const nativeRevision = React.useRef(client.nativeRevision)
  const previewClient = native ? { ...client, nativeRevision:nativeRevision.current } : client
  const previewGeneration = React.useRef(0)
  const dirty = profileSignature(draft) !== initial.current
  const operation = native ? 'native-edit' : defaultOperation(client)
  const single = client.id === 'codex' && !draft.baseUrl && draft.authMode === 'preserve'
  const validationClient = { ...client, supportsModels: client.supportsModels && !single, ...(native && !profile.enabled ? { supportsDefault: false } : {}) }
  const previousCatalog = React.useRef<CatalogModel[]>([])
  const [confirming, setConfirming] = React.useState(false)
  React.useEffect(() => {
    if (catalogLoading || catalogError) return
    const previous = previousCatalog.current
    previousCatalog.current = catalog
    if (!previous.length || draft.kind !== 'gateway' || !client.supportsModels) return
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
    if (dirty && !await confirm(t('放弃未保存的修改？'), t('关闭后丢弃本次草稿，不会保存空方案，也不会修改客户端。'), t('放弃修改'))) return
    onClose()
  }
  const save = async () => {
    if (saving) return
    const message = validateDraft(validationClient,draft)
    if (message) { setError(message); return }
    if (native) {
      await showPreview(true)
      return
    }
    setSaving(true); setError('')
    try {
      const profiles = creating ? [...client.profiles,draft] : client.profiles.map(item => item.id === draft.id ? draft : item)
      await saveProfiles(client,profiles)
      onClose(); toast(t('方案已保存，尚未应用到客户端'))
    } catch (error) { setError(errorMessage(error)); setSaving(false) }
  }
  const showPreview = async (confirming = false) => {
    const message = validateDraft(validationClient,draft)
    if (message) { setError(message); return }
    const generation = ++previewGeneration.current
    setPreviewing(true); setError('')
    try { const result = await requestPreview(previewClient,draft,operation); if (generation === previewGeneration.current) { setPreview(result); setConfirming(confirming) } }
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
      <DialogHeader><DialogTitle>{native ? t('编辑客户端配置') : creating ? t('新建配置方案') : t('编辑配置方案')} · {client.name}</DialogTitle></DialogHeader>
      <DialogBody>
        <Alert>{native ? t('保存会直接修改客户端配置；确认前展示差异，写入前自动备份。') : t('保存只记录方案，不修改客户端。应用时会另外展示文件差异并请求确认。')}</Alert>
        {!confirming && <>
        <section className='agent-profile-connection flex min-w-0 flex-col gap-4'>
          <h3 className='text-sm font-semibold'>{t('连接配置')}</h3>
          <TextField label={t('方案名称')} fieldKey='' value={draft.name} onChange={name => update({name})} disabled={saving || previewing || (native && ['claude-code','gemini-cli','kimi-code','zed','cline','workbuddy'].includes(client.id))} />
          <ConnectionFields client={client} draft={draft} update={update} disabled={saving || previewing} />
          {draft.kind === 'gateway' && catalogLoading && <p className='text-xs text-subtle'>{t('正在读取网关模型并填入配置…')}</p>}
          {catalogError && draft.kind === 'gateway' && <Alert variant='warning'>{t('模型目录读取失败，可手动填写真实模型 ID。')} {catalogError}
            <Button variant='outline' size='sm' onClick={retryCatalog} disabled={saving || previewing}>{t('重试')}</Button></Alert>}
        </section>
        {validationClient.supportsModels ? <ModelsFields client={validationClient} draft={draft} update={update} catalog={catalog} disabled={saving || previewing || draft.kind === 'gateway' && catalogLoading} native={native} />
          : <section className='flex min-w-0 flex-col gap-4 border-t border-hairline pt-4'><h3 className='text-sm font-semibold'>{t('模型配置')}</h3>
            <SingleModelFields client={validationClient} draft={draft} update={update} catalog={catalog} disabled={saving || previewing} /></section>}
        {(client.supportsToggle || !native) && <section className='flex min-w-0 flex-col gap-4 border-t border-hairline pt-4'>
          {client.supportsToggle && <Field className='rounded-md border border-border bg-surface-inset p-3'>
            <div className='flex items-center justify-between gap-3'><FieldLabel>{t('客户端条目启用状态')}</FieldLabel>
              <Switch checked={draft.enabled} disabled={saving || previewing} onCheckedChange={enabled => update({enabled})} aria-label={t('客户端条目启用状态')} /></div>
            <FieldDescription>{native ? t('确认写入后会同步修改客户端开关；取消不会生效。') : t('这是待应用状态；保存不会立即改变客户端开关。')}</FieldDescription>
          </Field>}
          {!native && <TextField label={t('备注')} fieldKey='' value={draft.note} onChange={note => update({note})} disabled={saving || previewing} />}
        </section>}
        </>}
        {preview && <WritePreview preview={preview} />}
        {error && <Alert variant='destructive'>{error}</Alert>}
      </DialogBody>
      <DialogFooter className='agent-config-footer'>
        <Button variant='ghost' disabled={saving || previewing} onClick={() => confirming ? setConfirming(false) : void showPreview()}>{confirming ? t('返回编辑') : previewing ? t('生成中…') : t('预览草稿改动')}</Button><span className='mr-auto' />
        <Button variant='outline' disabled={saving || previewing} onClick={() => void close()}>{t('取消')}</Button>
        <Button disabled={saving || previewing || (!creating && !dirty) || (confirming && !preview?.hasChanges)} onClick={() => void (confirming ? apply() : save())}>{saving ? t('保存中…') : confirming ? t('确认写入') : native ? t('保存修改') : t('保存方案')}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}
