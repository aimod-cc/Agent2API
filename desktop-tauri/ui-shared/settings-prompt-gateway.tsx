import * as React from 'react'
import { Button, Collapsible, CollapsibleContent, CollapsibleTrigger, Switch } from '@ui'
import type { Translate } from './assistant-read-types'
import { PromptGatewayDialog } from './settings-prompt-body'
import type { PromptApi, PromptProviderOption, PromptView } from './settings-prompt-types'

export function PromptGatewaySection({ api, view, busy, t, onRefresh }: {
  api: PromptApi; view: PromptView; busy: boolean; t: Translate; onRefresh(): Promise<void>
}) {
  const [open, setOpen] = React.useState(false)
  const [editing, setEditing] = React.useState<PromptProviderOption | null>(null)
  const [saving, setSaving] = React.useState(false)
  const [error, setError] = React.useState('')
  const options = view.providers.filter(option => option.gatewayNote || option.gatewayBlocks)
  if (!options.length) return null

  async function save(payload: Parameters<PromptApi['saveGateway']>[0]): Promise<boolean> {
    setSaving(true); setError('')
    try { await api.saveGateway(payload); await onRefresh(); return true }
    catch (failure) { setError(failure instanceof Error ? failure.message : String(failure)); return false }
    finally { setSaving(false) }
  }

  return <Collapsible open={open} onOpenChange={setOpen} className='sp-gateway-section'>
    <CollapsibleTrigger render={<Button variant='ghost' className='sp-gateway-trigger' />}>
      <span>{t('网关自带提示词')}</span><span aria-hidden='true'>{open ? '⌃' : '⌄'}</span>
    </CollapsibleTrigger>
    <CollapsibleContent className='sp-gateway-body'>
      {options.map(option => {
        const enabled = view.gateway[option.id] ?? true
        const edited = Object.values(view.gatewayText[option.id] || {}).some(text => text?.trim())
        return <div className='sp-gateway-row' key={option.id}>
          <span className='sp-gateway-name'>{option.label}</span>
          <Switch checked={enabled} disabled={busy || saving} aria-label={t('{provider} 的网关自带提示词', { provider: option.label })}
            onCheckedChange={next => { void save({ promptGateway: { [option.id]: next } }) }} />
          <span className='sp-muted'>{edited ? t('已修改正文') : t('官方原文')}</span>
          {option.gatewayBlocks ? <Button variant='outline' size='sm' disabled={busy || saving} onClick={() => setEditing(option)}>{t('编辑正文')}</Button> : null}
          {!enabled ? <p className='sp-warning'>{t('已关闭，上游可能拒绝请求。')}</p> : null}
        </div>
      })}
      {error ? <p role='alert' className='sp-error'>{error}</p> : null}
    </CollapsibleContent>
    {editing?.gatewayBlocks ? <PromptGatewayDialog title={t('网关自带提示词 · {provider}', { provider: editing.label })}
      blocks={editing.gatewayBlocks} overrides={view.gatewayText[editing.id]} t={t}
      onSave={value => save({ promptGatewayText: { [editing.id]: value } })} onClose={() => setEditing(null)} /> : null}
  </Collapsible>
}
