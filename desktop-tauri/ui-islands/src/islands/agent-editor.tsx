import * as React from 'react'
import { Alert, Button, Empty, EmptyDescription, EmptyTitle, StatusDot, cn } from '@ui'
import { t } from '../i18n'
import { confirm, errorMessage, getAgentStore, loadPanel, removeConfiguration, subscribeAgentStore, toast } from './agent-state'
import { kindLabel, protocolLabel } from './agent-model'
import { quickBlockReason } from './agent-quick-model'
import { ProfileDialog } from './agent-profile-dialog'
import { ApplyDialog } from './agent-preview'
import { AgentActions, AgentIcon } from './agent-actions'
import type { AgentClient, AgentOperation, AgentProfile } from './agent-types'

export function ProfileEditor({
  client,
  onAdd,
  onQuickConnect,
  onOpenPath,
}: {
  client: AgentClient
  onAdd: () => void
  onQuickConnect: () => void
  onOpenPath: () => void
}) {
  const store = React.useSyncExternalStore(subscribeAgentStore, getAgentStore, getAgentStore)
  const [editor, setEditor] = React.useState<AgentProfile | null>(null)
  const [applying, setApplying] = React.useState<{ profile: AgentProfile; operation: AgentOperation } | null>(null)
  const busy = Boolean(store.busy)
  const disabled = busy || Boolean(client.storeError)
  const profiles = client.configurations
  const canQuickConnect = client.writable && !quickBlockReason(client)
  const remove = async (profile: AgentProfile) => {
    if (profile.liveId) { setApplying({ profile, operation: 'config-delete' }); return }
    if (!await confirm(t('删除配置？'), t('此配置未在客户端中使用，删除不会改变当前使用的配置。'), t('删除'))) return
    try { await removeConfiguration(client, profile); toast(t('配置已删除')) }
    catch (error) { toast(errorMessage(error), 'err') }
  }
  return <>
    <div className='agent-configurations' aria-label={t('配置列表')}>
      {client.readError && <Alert variant='destructive' className='flex flex-wrap items-center justify-between gap-3'>
        <div className='min-w-0 flex-1'>
          <div className='font-medium'>{t('配置文件读取失败')}</div>
          <div className='mt-0.5 text-xs opacity-90'>{client.readError}</div>
        </div>
        <div className='flex shrink-0 items-center gap-2'>
          <Button variant='outline' size='sm' disabled={busy} onClick={() => void loadPanel({ force: true })}>{t('重新检测')}</Button>
          <Button variant='outline' size='sm' disabled={busy} onClick={onOpenPath}>{t('指定配置文件')}</Button>
        </div>
      </Alert>}
      {client.storeError && <Alert variant='destructive'>{client.storeError}</Alert>}
      {!client.readError && !client.storeError && !profiles.length && <Empty className='agent-configuration-empty'>
        <EmptyTitle>{t('还没有配置')}</EmptyTitle>
        <EmptyDescription>
          {client.writable
            ? canQuickConnect
              ? t('可以直接一键接入本网关，或手动添加其他服务配置')
              : t('手动新增一套配置后即可应用到客户端')
            : t('未读取到可识别的客户端配置')}
        </EmptyDescription>
        {client.writable && <div className='mt-2 flex flex-wrap items-center justify-center gap-2'>
          {canQuickConnect && <Button size='sm' disabled={disabled} onClick={onQuickConnect}>
            <AgentIcon name='bolt' />{t('一键接入本网关')}
          </Button>}
          <Button variant={canQuickConnect ? 'outline' : 'default'} size='sm' disabled={disabled} onClick={onAdd}>
            <AgentIcon name='plus' />{t('手动新增配置')}
          </Button>
        </div>}
      </Empty>}
      {profiles.map(profile => {
        const active = profile.status === 'active' || profile.status === 'enabled'
        const status = profile.status === 'active' ? t('使用中') : profile.status === 'enabled' ? t('已启用')
          : profile.status === 'disabled' ? t('已停用') : ''
        const name = profile.name || profile.id
        const duplicateName = profiles.some(other => other !== profile && (other.name || other.id) === name)
        const claudeTierSummary = client.id === 'claude-code'
          ? (['opus', 'sonnet', 'haiku', 'subagent'] as const)
            .map(tier => {
              const value = profile.claudeModels?.[tier]
              if (!value) return ''
              const label = tier === 'subagent' ? t('子 Agent') : tier[0].toUpperCase() + tier.slice(1)
              return `${label} → ${value}`
            })
            .filter(Boolean)
          : []
        const uniqueClaudeTargets = new Set(Object.values(profile.claudeModels || {}).filter(Boolean))
        const models = client.id === 'claude-code' && uniqueClaudeTargets.size > 1
          ? claudeTierSummary
          : profile.models.length
            ? profile.models.map(model => model.id)
            : profile.model
              ? [profile.model]
              : claudeTierSummary
        const actionLabel = profile.status === 'disabled' ? t('启用') : client.supportsDefault ? t('使用此配置') : t('添加到客户端')
        return <div key={`${profile.liveId || 'saved'}:${profile.id}`} className={cn('agent-configuration-row', active && 'is-current')}>
          <span className='agent-configuration-mark' aria-hidden='true'>{profile.kind === 'gateway' ? 'A2' : name.slice(0, 2).toUpperCase()}</span>
          <div className='agent-configuration-main'>
            <div className='agent-configuration-title'><span title={name}>{name}</span>
              {duplicateName && <code>{profile.sourceId || profile.id}</code>}</div>
            <div className='agent-configuration-meta'><span className='agent-configuration-url' title={profile.baseUrl}>{profile.baseUrl || t('客户端默认地址')}</span>
              <span>{kindLabel(profile.kind)}</span><span>{protocolLabel(profile.protocol)}</span></div>
            {models.length > 0 && <p className='agent-configuration-models' title={models.join(' · ')}>{models.slice(0, 3).join(' · ')}{models.length > 3 ? ` · +${models.length - 3}` : ''}</p>}
            {profile.note && <p className='agent-configuration-note'>{profile.note}</p>}
            {profile.validationError && <p className='agent-configuration-error'>{t('请补全配置后使用')} · {profile.validationError}</p>}
          </div>
          <div className='agent-configuration-actions'>
            {status && <span className={cn('agent-configuration-status', active && 'is-current')} title={active ? t('状态来自配置文件，已打开的会话可能需要重新加载') : undefined}>
              <StatusDot variant={active ? 'success' : 'muted'} />{status}</span>}
            {!active && profile.editable && <Button variant='outline' size='sm' disabled={disabled || Boolean(profile.validationError)}
              onClick={() => setApplying({ profile, operation: 'config-use' })}>{actionLabel}</Button>}
            {profile.editable && <Button variant='ghost' size='icon-sm' disabled={disabled} title={t('编辑')} aria-label={t('编辑：{name}', { name })}
              onClick={() => setEditor(profile)}><AgentIcon name='edit' /></Button>}
            {profile.deletable && <AgentActions label={t('配置操作')} disabled={disabled || Boolean(client.readError)} actions={[
              ...(client.supportsToggle && profile.status === 'enabled' ? [{ label: t('停用'), onClick: () => setApplying({ profile, operation: 'config-disable' }) }] : []),
              { label: t('删除'), destructive: true, onClick: () => void remove(profile) },
            ]} />}
          </div>
        </div>
      })}
    </div>
    {editor && <ProfileDialog key={`${client.id}:${editor.id}`} client={client} profile={editor} creating={false} onClose={() => setEditor(null)} />}
    {applying && <ApplyDialog client={client} profile={applying.profile} operation={applying.operation} onClose={() => setApplying(null)} />}
  </>
}
