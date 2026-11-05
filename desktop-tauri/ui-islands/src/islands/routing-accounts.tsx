import * as React from 'react'
import { Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, confirmDialog } from '@ui'
import { t } from '../i18n'
import { confirmBatchResult, refreshSavedAccounts } from './accounts-update'
import type { RoutingEditorState } from './routing-editor-state'
import { bridge, errorMessage, notify, type RoutingWorkspace } from './routing-types'

export function useRoutingAccounts(editor: RoutingEditorState, onStart: () => void, onFinished: () => void) {
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const pending = React.useRef(false)
  const sync = async (readWorkspace: boolean) => {
    if (readWorkspace) {
      try { editor.syncWorkspace(await bridge().getRoutingWorkspace()) }
      catch (failure) { setError(t('账号已保存，但列表刷新失败：{error}', { error: errorMessage(failure) })) }
    }
    try { await refreshSavedAccounts() }
    catch (failure) { setError(t('账号已保存，但列表刷新失败：{error}', { error: errorMessage(failure) })) }
  }
  const setEnabled = async (ids: string[], enabled: boolean, batch = false) => {
    const workspace = editor.draft?.workspace
    if (pending.current || editor.busy || editor.loading || editor.scope || !workspace) return
    if (!workspace.globalAccountControl || workspace.legacyGlobalParticipation) return
    const selected = new Set(ids)
    const targets = workspace.accounts.filter(account => selected.has(account.id) && (account.enabled !== false) !== enabled)
    if (!targets.length) return
    pending.current = true; onStart(); setBusy(true); setError('')
    try {
      if (batch && !enabled && !await confirmDialog(t('将禁用当前所选的 {n} 个账号，所有路由都不能再选用它们。此操作立即保存，不会删除账号。', { n: targets.length }), {
        title: t('批量禁用账号'), confirmText: t('禁用账号'), destructive: true,
      })) return
      const payload = { action: enabled ? 'enable' : 'disable', ids: targets.map(account => account.id) }
      const result = confirmBatchResult(await bridge().batchAccounts(payload), payload)
      editor.syncAccounts(result.list!.accounts!)
      const failed = result.failed || [], skipped = result.skipped || []
      if (failed.length || skipped.length) {
        const details = [...failed.map(item => `${targets.find(account => account.id === item.id)?.name || item.id}: ${item.error}`), ...skipped.map(item => `${item.name || item.id}: ${item.reason}`)].join('；')
        setError(t('账号启停部分完成：成功 {ok} 个，失败 {fail} 个，跳过 {skip} 个。{details}', {
          ok: result.ok?.length || 0, fail: failed.length, skip: skipped.length, details,
        }))
      } else notify(enabled ? t('✅ 已启用 {n} 个账号', { n: targets.length }) : t('✅ 已禁用 {n} 个账号', { n: targets.length }))
      await sync(true)
    } catch (failure) { setError(errorMessage(failure)); void refreshSavedAccounts().catch(() => {}) }
    finally { pending.current = false; onFinished(); setBusy(false) }
  }
  const convert = async (revision: string): Promise<boolean> => {
    if (pending.current || editor.busy || editor.loading || editor.scope) return false
    pending.current = true; onStart(); setBusy(true); setError('')
    try {
      const workspace = await bridge().saveRoutingProfile({ migrateGlobalAccounts: revision })
      if (!workspace.globalAccountControl || workspace.legacyGlobalParticipation || !workspace.revision) {
        throw new Error(t('转换结果未确认，请重新读取全局配置'))
      }
      editor.syncAccounts(workspace.accounts)
      editor.syncWorkspace(workspace, true)
      notify(t('已转换为账号启停，独立路由参与名单保持不变'))
      await sync(false)
      return true
    } catch (failure) { setError(errorMessage(failure)); return false }
    finally { pending.current = false; onFinished(); setBusy(false) }
  }
  return { busy, error, setEnabled, convert }
}

export function RoutingLegacyAccountsDialog({ legacy, busy, error, onConfirm, onClose }: {
  legacy: NonNullable<RoutingWorkspace['legacyGlobalParticipation']>; busy: boolean; error: string
  onConfirm(): void; onClose(): void
}) {
  return <Dialog open onOpenChange={(open, details) => { if (!open) { if (busy) details.cancel(); else onClose() } }}>
    <DialogContent className='w-[min(620px,calc(100vw-40px))]'>
      <DialogHeader><DialogTitle>{t('转换旧版全局参与设置')}</DialogTitle></DialogHeader>
      <DialogBody>
        <p className='routing-config-note'>{t('以下 {n} 个账号原本被全局路由排除。确认后会将它们设为禁用，所有路由都不能再选用；原有独立路由的参与名单保留。', { n: legacy.accounts.length })}</p>
        {legacy.accounts.length ? <div className='routing-model-picker'>{legacy.accounts.map(account => <div key={account.id} className='routing-group-picker-row'>
          <div><b>{account.name || account.id}</b><small>{account.providerLabel}</small></div>
        </div>)}</div> : null}
        <p className='routing-config-note'>{t('转换只处理当前账号；之后新增账号按账号页的启用状态使用。不会启用任何已禁用的账号。')}</p>
        {error ? <p className='routing-config-error' role='alert'>{error}</p> : null}
      </DialogBody>
      <DialogFooter><Button variant='outline' disabled={busy} onClick={onClose}>{t('取消')}</Button>
        <Button variant={legacy.accounts.length ? 'destructive' : 'default'} disabled={busy} onClick={onConfirm}>{busy ? t('保存中…') : t('确认转换')}</Button></DialogFooter>
    </DialogContent>
  </Dialog>
}
