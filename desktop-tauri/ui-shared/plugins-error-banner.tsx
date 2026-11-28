import * as React from 'react'
import { Alert, Button } from '@ui'
import type { ScriptOperation, ScriptPackage, ScriptRecommendation } from './plugins'
import type { PluginTranslate } from './plugins-store'
import { reinstallRequest, scriptPluginName } from './plugins-script-display'

export type PluginErrorBannerState = {
  message: string
  retryDisabled?: boolean
  onRetry?: () => void
  onDismiss?: () => void
}

export function PluginErrorBanner({
  message,
  retryDisabled = false,
  onRetry,
  onDismiss,
  t,
}: PluginErrorBannerState & { t: PluginTranslate }): React.ReactElement | null {
  if (!message) return null
  return (
    <Alert variant='destructive' className='flex flex-wrap items-center justify-between gap-x-3 gap-y-1.5 py-2'>
      <p className='m-0 min-w-0 flex-1 whitespace-pre-wrap break-words'>{message}</p>
      {onRetry || onDismiss ? (
        <div className='flex shrink-0 items-center gap-1'>
          {onRetry ? (
            <Button variant='link' size='xs' className='text-inherit' disabled={retryDisabled} onClick={onRetry}>
              {t('重试')}
            </Button>
          ) : null}
          {onDismiss ? (
            <Button variant='link' size='xs' className='text-inherit' onClick={onDismiss}>
              {t('关闭')}
            </Button>
          ) : null}
        </div>
      ) : null}
    </Alert>
  )
}

function resolveTargetName(
  operation: ScriptOperation,
  packages: ScriptPackage[],
  recommended: ScriptRecommendation[],
  t: PluginTranslate,
): { name: string; pack?: ScriptPackage } {
  const target = operation.target
  const pack = target?.id ? packages.find(item => item.id === target.id) : undefined
  if (pack) return { name: scriptPluginName(pack, recommended, t), pack }
  const source = target?.source?.trim() || ''
  const rawName = target?.name?.trim() || ''
  const matched = recommended.find(item => (
    (source && ((item.source || item.name).toLowerCase() === source.toLowerCase()
      || (item.repository && `github:${item.repository}`.toLowerCase() === source.toLowerCase())))
    || (rawName && item.name === rawName)
  ))
  if (matched) return { name: matched.label }
  return { name: rawName || source }
}

export function formatScriptOperationFailure(
  operation: ScriptOperation,
  packages: ScriptPackage[],
  recommended: ScriptRecommendation[],
  t: PluginTranslate,
): string {
  const reason = (operation.error || '').trim()
  if (!reason) return ''
  const { name, pack } = resolveTargetName(operation, packages, recommended, t)
  const action = operation.action === 'install' && pack ? 'update' : (operation.action || '')
  if (action === 'update') {
    return name ? t('更新「{name}」失败：{reason}', { name, reason }) : t('更新脚本插件失败：{reason}', { reason })
  }
  if (action === 'install') {
    return name ? t('安装「{name}」失败：{reason}', { name, reason }) : t('安装脚本插件失败：{reason}', { reason })
  }
  if (action === 'reload') {
    return name ? t('重载「{name}」失败：{reason}', { name, reason }) : t('重载脚本插件失败：{reason}', { reason })
  }
  if (action === 'enable' || action === 'provider') {
    return name ? t('切换「{name}」状态失败：{reason}', { name, reason }) : t('切换脚本插件状态失败：{reason}', { reason })
  }
  if (action === 'remove') {
    return name ? t('卸载「{name}」失败：{reason}', { name, reason }) : t('卸载脚本插件失败：{reason}', { reason })
  }
  return name ? t('「{name}」操作失败：{reason}', { name, reason }) : reason
}

export function resolveScriptOperationRetry(
  operation: ScriptOperation,
  packages: ScriptPackage[],
  actions: {
    run: (path: string, body: Record<string, unknown>) => Promise<boolean>
    openInstall: (draft: { name: string; version: string; id?: string }) => void
  },
): (() => void) | undefined {
  const target = operation.target
  const pack = target?.id ? packages.find(item => item.id === target.id) : undefined
  const action = operation.action === 'install' && pack ? 'update' : (operation.action || '')
  if (action === 'update' && pack) {
    return () => {
      const next = reinstallRequest(pack)
      void actions.run('/api/plugins/scripts/install', {
        id: next.id,
        name: next.name,
        source: next.name,
        version: next.version,
        trusted: true,
        checkinAllowed: true,
      })
    }
  }
  if (action === 'install') {
    const source = target?.source?.trim() || target?.name?.trim() || ''
    if (!source) return undefined
    const version = target?.version?.trim() || ''
    return () => {
      actions.openInstall({ name: source, version })
    }
  }
  if (action === 'reload' && target?.id) {
    const id = target.id
    return () => { void actions.run('/api/plugins/scripts/change', { id, action: 'reload' }) }
  }
  if (action === 'enable' && target?.id && typeof target.enabled === 'boolean') {
    const id = target.id
    const enabled = target.enabled
    return () => { void actions.run('/api/plugins/scripts/change', { id, action: 'enable', enabled }) }
  }
  if (action === 'provider' && target?.id && target.provider && typeof target.enabled === 'boolean') {
    const id = target.id
    const provider = target.provider
    const enabled = target.enabled
    return () => { void actions.run('/api/plugins/scripts/change', { id, action: 'provider', provider, enabled }) }
  }
  if (action === 'remove' && target?.id) {
    const id = target.id
    return () => { void actions.run('/api/plugins/scripts/change', { id, action: 'remove' }) }
  }
  return undefined
}
