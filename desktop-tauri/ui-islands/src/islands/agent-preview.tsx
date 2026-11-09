import * as React from 'react'
import { Alert, Button, Collapsible, CollapsibleContent, CollapsibleTrigger, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, Empty } from '@ui'
import { t } from '../i18n'
import { applyProfile, errorMessage, requestPreview, restoreClient } from './agent-state'
import { AgentIcon } from './agent-actions'
import type { AgentClient, AgentOperation, AgentPreview, AgentProfile } from './agent-types'

export function WritePreview({ preview, showWarnings = true }: { preview: AgentPreview; showWarnings?: boolean }) {
  return <div className='flex flex-col gap-3'>
    {showWarnings && preview.warnings.map((warning,index) => <Alert key={index} variant='warning'>{warning}</Alert>)}
    <p className='text-xs text-subtle'>{t('密钥已隐藏；绿色为新增，红色为删除。所有实际写入文件都列在这里。')}</p>
    {preview.files.map(file => <div key={file.path} className='agent-preview-file'>
      <div className='flex flex-wrap items-center gap-2 pb-2 text-xs'><code className='break-all'>{file.path}</code>
        <span className='ml-auto text-subtle'>{file.created ? t('新建文件') : file.deleted ? t('删除新建文件') : file.changed ? t('有改动') : t('无改动')}</span></div>
      <pre className='agent-code' aria-label={t('配置差异')}>
        {file.diff.map((line,index) => <span key={index} className={`agent-line ${line.kind === 'added' ? 'is-added' : line.kind === 'removed' ? 'is-removed' : ''}`}>
          <span className='agent-line-no'>{line.newLine ?? line.oldLine ?? ''}</span>
          <span className='agent-line-sign' aria-hidden='true'>{line.kind === 'added' ? '+' : line.kind === 'removed' ? '−' : ' '}</span>
          <span className='agent-line-text'>{line.text || ' '}</span>
        </span>)}
      </pre>
    </div>)}
  </div>
}

export function ApplyDialog({ client, profile, operation, onClose }: {
  client: AgentClient; profile?: AgentProfile; operation: AgentOperation; onClose: () => void
}) {
  const [preview, setPreview] = React.useState<AgentPreview | null>(null)
  const [error, setError] = React.useState('')
  const [loading, setLoading] = React.useState(true)
  const [saving, setSaving] = React.useState(false)
  const [generation, setGeneration] = React.useState(0)
  const nativeRevision = React.useRef(client.nativeRevision)
  const revision = React.useRef(client.revision)
  const configPath = React.useRef(client.configPath)
  const unified = operation.startsWith('config-')
  const deleting = operation === 'native-delete' || operation === 'config-delete'
  const [detailsOpen, setDetailsOpen] = React.useState(false)
  const previewClient = operation.startsWith('native-') || unified ? { ...client, nativeRevision: nativeRevision.current, revision: revision.current, configPath: configPath.current } : client
  const title = deleting ? t('删除配置') : operation === 'config-disable' ? t('停用配置') : operation === 'config-use' ? client.supportsDefault ? t('使用此配置') : t('启用配置')
    : operation === 'disconnect' ? t('解除接入本网关') : operation === 'restore' ? t('撤销上次应用') : operation === 'default' ? t('应用并设为默认') : t('应用到客户端')
  React.useEffect(() => {
    let alive = true
    setLoading(true); setPreview(null); setError('')
    void requestPreview(previewClient, profile, operation).then(result => { if (alive) setPreview(result) })
      .catch(error => { if (alive) setError(errorMessage(error)) }).finally(() => { if (alive) setLoading(false) })
    return () => { alive = false }
  }, [client.id, profile, operation, generation])
  const apply = async () => {
    if (!preview || saving || loading) return
    setSaving(true); setError('')
    try {
      if (operation === 'restore' || operation === 'disconnect') await restoreClient(client, operation, preview)
      else if (profile) await applyProfile(previewClient, profile, operation, preview)
      else throw new Error(t('没有选中配置'))
      onClose()
    } catch (error) { setError(errorMessage(error)); setPreview(null); setSaving(false) }
  }
  return <Dialog open onOpenChange={(open,details) => { if (open) return; if (saving) details.cancel(); else onClose() }}>
    <DialogContent className='agent-config-dialog'>
      <DialogHeader><DialogTitle>{title} · {client.name}</DialogTitle></DialogHeader>
      <DialogBody>
        {profile && <p className='text-sm font-medium'>{profile.name}</p>}
        {loading ? <Empty>{t('正在生成准确预览…')}</Empty> : preview ? unified ? <>
          <p className='text-sm leading-relaxed'>{deleting ? t('将删除此配置及客户端中的对应条目，其他配置不受影响。')
            : operation === 'config-disable' ? t('停用后将从客户端可选项中关闭此配置，其他配置不受影响。')
              : client.supportsDefault ? t('确认后将切换到此配置，原配置会保留在列表中。') : t('确认后将此配置加入客户端，已有配置会保留。')}</p>
          {preview.warnings.map((warning, index) => <p key={index} className='text-xs leading-relaxed text-subtle'>{warning}</p>)}
          <Collapsible open={detailsOpen} onOpenChange={setDetailsOpen}>
            <CollapsibleTrigger render={<Button variant='ghost' size='sm' />}><AgentIcon name='chevron' />{t('查看文件改动')}</CollapsibleTrigger>
            <CollapsibleContent><WritePreview preview={preview} showWarnings={false} /></CollapsibleContent>
          </Collapsible>
        </> : <WritePreview preview={preview} /> : null}
        {error && <Alert variant='destructive'>{error}</Alert>}
      </DialogBody>
      <DialogFooter>
        <Button variant='ghost' size='sm' disabled={saving || loading} onClick={() => setGeneration(value => value+1)}>{t('重新生成预览')}</Button>
        <span className='mr-auto' />
        <Button variant='outline' disabled={saving} onClick={onClose}>{t('取消')}</Button>
        <Button variant={deleting ? 'destructive' : 'default'} disabled={!preview || loading || saving || (!preview.hasChanges && !unified && !['restore','disconnect'].includes(operation))} onClick={() => void apply()}>{saving ? t('处理中…') : deleting ? t('确认删除') : operation === 'config-disable' ? t('确认停用') : operation === 'config-use' ? t('确认使用') : t('确认应用')}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}
