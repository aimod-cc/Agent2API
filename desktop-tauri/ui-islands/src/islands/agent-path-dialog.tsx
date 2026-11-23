import * as React from 'react'
import { Alert, Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, Field, FieldDescription, FieldLabel, Input, Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@ui'
import { t } from '../i18n'
import { errorMessage, isDesktopEnvironment, pickConfigFile, savePath } from './agent-state'
import { dshTargetLabel } from './agent-dsh'
import { AgentIcon } from './agent-actions'
import type { AgentClient } from './agent-types'

export function PathDialog({client,onClose}: {client:AgentClient;onClose:()=>void}) {
  const [path,setPath] = React.useState(client.pathOverride || client.configTargets?.[0]?.path || client.configPath)
  const selectedTarget = client.configTargets?.find(target => target.path === path)
  const [saving,setSaving] = React.useState(false)
  const [picking,setPicking] = React.useState(false)
  const [error,setError] = React.useState('')
  const desktop = isDesktopEnvironment()
  const pathInputId = React.useId()
  const save = async (nextPath:string) => {
    setSaving(true);setError('')
    try { await savePath(client,nextPath);onClose() }
    catch(error) {setError(errorMessage(error));setSaving(false)}
  }
  const browse = async () => {
    if (saving || picking) return
    setPicking(true); setError('')
    try {
      const picked = await pickConfigFile(path || client.resolvedPath || client.configPath)
      if (!picked?.canceled && !picked?.unsupported && picked?.path) {
        setPath(picked.path)
      }
    } catch (error) {
      setError(errorMessage(error))
    } finally {
      setPicking(false)
    }
  }
  return <Dialog open onOpenChange={(open,details)=>{if(!open){if(saving||picking)details.cancel();else onClose()}}}>
    <DialogContent><DialogHeader><DialogTitle>{t('指定配置路径')} · {client.name}</DialogTitle></DialogHeader>
      <DialogBody>
        {!desktop && <Alert>{t('路径属于网关所在机器，不是浏览器所在设备。只改变检测目标，不修改客户端文件。')}</Alert>}
        {client.id === 'dsh' && Boolean(client.configTargets?.length) && <Field>
          <FieldLabel>{t('DSH 配置目标')}</FieldLabel>
          <Select value={path} disabled={saving || picking} onValueChange={value => { if (typeof value === 'string') setPath(value) }}>
            <SelectTrigger><SelectValue>{selectedTarget ? dshTargetLabel(selectedTarget.profile, selectedTarget.label) : t('自定义路径')}</SelectValue></SelectTrigger>
            <SelectContent>{client.configTargets?.map(target => <SelectItem key={target.path} value={target.path}>{dshTargetLabel(target.profile, target.label)}{target.version ? ` · ${target.version}` : ''}</SelectItem>)}</SelectContent>
          </Select>
          <FieldDescription>{t('新版网页版和桌面版分别保存模型配置；旧版模型设置共享，MCP 始终只写入所选运行方式。')}</FieldDescription>
        </Field>}
        <Field>
          <FieldLabel htmlFor={pathInputId}>{t('完整配置文件路径')}</FieldLabel>
          <div className='flex items-center gap-2'>
            <Input id={pathInputId} className='flex-1' type='text' autoComplete='off' spellCheck={false} value={path}
              onChange={event => setPath(event.currentTarget.value)} disabled={saving || picking} />
            {desktop && <Button type='button' variant='outline' size='sm' disabled={saving || picking} onClick={() => void browse()}>
              <AgentIcon name='folder' />{picking ? t('选择中…') : t('浏览文件')}
            </Button>}
          </div>
          <FieldDescription>{t('只改变本页读取与写入的目标文件，不修改客户端自身启动设置。')}</FieldDescription>
        </Field>
        {error&&<Alert variant='destructive'>{error}</Alert>}
      </DialogBody>
      <DialogFooter><Button variant='ghost' disabled={saving || picking} onClick={()=>void save('')}>{t('恢复自动检测')}</Button><span className='mr-auto'/>
        <Button variant='outline' disabled={saving || picking} onClick={onClose}>{t('取消')}</Button><Button disabled={saving || picking} onClick={()=>void save(path)}>{t('保存路径')}</Button></DialogFooter>
    </DialogContent>
  </Dialog>
}
