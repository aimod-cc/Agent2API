import { t } from '../i18n'
import type { Method } from './add-account-dynamic-fields'
import type { ProxyPayload } from './accounts-proxy-form'
import {
  accountCountOf, closeAddModals, describeError, normalizeLoginResult, postAccount, saveAccountProxySelection, shared, toast,
} from './add-account-bridge'
import { accountImportNotice } from '../../../ui-shared/account-import-result'
import {
  credentialFilePayloadForItem, inspectSoftwareExport,
  type CredentialUploadItem,
} from '../../../ui-shared/plugin-credential-file'

export type BatchImportFailure = {
  label: string
  reason: string
}

export type BatchImportProgress = {
  current: number
  total: number
  fileName: string
}

export type BatchImportOutcome = {
  added: number
  updated: number
  skipped: number
  otherProviderSkipped: number
  failed: number
  failures: BatchImportFailure[]
  failedUploadIds: Set<string>
  notices: string[]
  hasWarning: boolean
}

function objectOf(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : undefined
}

function appendBatchNotice(outcome: BatchImportOutcome, raw: string, warning = false): void {
  if (warning) outcome.hasWarning = true
  for (const part of raw.split('\n')) {
    const trimmed = part.trim()
    if (!trimmed) continue
    const line = t(trimmed)
    if (!outcome.notices.includes(line)) outcome.notices.push(line)
  }
}

export function buildCredentialAccountPayload({
  provider,
  method,
  item,
  otherFields,
  baseSettingsPayload,
  proxyPayload,
  exportProxyFields,
  remark,
}: {
  provider: string
  method: Method
  item: CredentialUploadItem
  otherFields: Record<string, string>
  baseSettingsPayload: Record<string, unknown>
  proxyPayload: ProxyPayload
  exportProxyFields: { proxy?: ProxyPayload }
  remark?: string
}): Record<string, unknown> {
  const fileField = method.file?.field
  const fields: Record<string, string> = { ...otherFields }
  if (fileField && item.text !== undefined) {
    fields[fileField] = item.text
  }
  const isExport = Boolean(item.exportKind)
  const payload: Record<string, unknown> = {
    provider,
    ...(isExport ? exportProxyFields : { proxy: proxyPayload }),
    ...(remark ? { name: remark } : {}),
    ...baseSettingsPayload,
    methodIndex: method.methodIndex,
    inputs: fields,
    ...credentialFilePayloadForItem(method, item, fields),
  }
  return payload
}

export async function runCredentialBatchImport({
  provider,
  method,
  items,
  otherFields,
  baseSettingsPayload,
  proxyPayload,
  exportProxyFields,
  isCancelled,
  onProgress,
}: {
  provider: string
  method: Method
  items: CredentialUploadItem[]
  otherFields: Record<string, string>
  baseSettingsPayload: Record<string, unknown>
  proxyPayload: ProxyPayload
  exportProxyFields: { proxy?: ProxyPayload }
  isCancelled: () => boolean
  onProgress: (progress: BatchImportProgress) => void
}): Promise<BatchImportOutcome> {
  const outcome: BatchImportOutcome = {
    added: 0,
    updated: 0,
    skipped: 0,
    otherProviderSkipped: 0,
    failed: 0,
    failures: [],
    failedUploadIds: new Set<string>(),
    notices: [],
    hasWarning: false,
  }
  const multiFiles = items.length > 1
  const batchOtherFields = Object.fromEntries(Object.entries(otherFields).filter(([key]) => key !== 'name'))
  for (let index = 0; index < items.length; index += 1) {
    if (isCancelled()) break
    const item = items[index]
    onProgress({ current: index + 1, total: items.length, fileName: item.name })
    const isExport = Boolean(item.exportKind)
    const payload = buildCredentialAccountPayload({
      provider,
      method,
      item,
      otherFields: batchOtherFields,
      baseSettingsPayload,
      proxyPayload,
      exportProxyFields,
    })
    try {
      const raw = await postAccount(payload)
      const data = objectOf(raw)
      if (data?.batch === true) {
        const added = Number(data.added) || 0
        const updated = Number(data.updated) || 0
        const skipped = Number(data.skipped) || 0
        const otherSkipped = Number(data.otherProviderSkipped) || 0
        const failed = Number(data.failed) || 0
        outcome.added += added
        outcome.updated += updated
        outcome.skipped += skipped
        outcome.otherProviderSkipped += otherSkipped
        outcome.failed += failed
        const errors = Array.isArray(data.errors) ? data.errors : []
        const failureItems = errors
          .map(objectOf)
          .filter((entry): entry is Record<string, unknown> => Boolean(entry) && entry?.skipped !== true)
        for (const entry of failureItems) {
          const itemId = typeof entry.id === 'string' && entry.id.trim() ? entry.id.trim() : t('未知账号')
          const reason = typeof entry.message === 'string' && entry.message.trim() ? entry.message.trim() : t('导入失败')
          outcome.failures.push({
            label: multiFiles && item.name ? `${item.name} · ${itemId}` : itemId,
            reason,
          })
        }
        if (failed > 0 && !failureItems.length) {
          outcome.failures.push({
            label: item.name || t('第 {n} 个文件', { n: index + 1 }),
            reason: t('部分账号导入失败'),
          })
        }
        if (added + updated === 0 && failed > 0) {
          outcome.failedUploadIds.add(item.id)
        }
        const notice = accountImportNotice(data)
        if (notice) appendBatchNotice(outcome, notice.text, notice.warning)
      } else {
        const normalized = isExport
          ? normalizeLoginResult(raw)
          : await saveAccountProxySelection(normalizeLoginResult(raw), proxyPayload)
        if (data?.created === false) outcome.updated += 1
        else outcome.added += 1
        const notice = accountImportNotice(data)
        if (notice) appendBatchNotice(outcome, notice.text, notice.warning)
        if (normalized.warning) appendBatchNotice(outcome, normalized.warning, true)
      }
    } catch (failure) {
      outcome.failed += 1
      outcome.failedUploadIds.add(item.id)
      outcome.failures.push({
        label: item.name || t('第 {n} 个文件', { n: index + 1 }),
        reason: describeError(failure),
      })
    }
  }
  return outcome
}

export function formatBatchImportToast(label: string, outcome: BatchImportOutcome): {
  message: string
  tone: 'ok' | 'warn' | 'err'
} {
  const parts: string[] = []
  if (outcome.added) parts.push(t('新增 {n} 个', { n: outcome.added }))
  if (outcome.updated) parts.push(t('更新 {n} 个', { n: outcome.updated }))
  if (outcome.skipped) parts.push(t('跳过 {n} 个', { n: outcome.skipped }))
  if (outcome.otherProviderSkipped) {
    parts.push(t('忽略 {n} 个其他提供商账号', { n: outcome.otherProviderSkipped }))
  }
  if (outcome.failed) parts.push(t('失败 {n} 个', { n: outcome.failed }))
  const detail = parts.join(t('、')) || t('未导入任何账号')
  const head = outcome.failed || outcome.hasWarning
    ? t('{label}导入完成：{summary}', { label, summary: detail })
    : t('✅ {label}导入完成：{summary}', { label, summary: detail })
  const notice = Array.from(new Set(outcome.notices.map(item => item.trim()).filter(Boolean))).slice(0, 2).join('\n')
  const message = notice ? `${head}\n${notice}` : head
  const tone: 'ok' | 'warn' | 'err' = outcome.failed
    ? 'err'
    : outcome.hasWarning || outcome.otherProviderSkipped > 0 || outcome.skipped > 0
      ? 'warn'
      : 'ok'
  return { message, tone }
}

export function resolveBatchImportItems(
  method: Method,
  uploads: CredentialUploadItem[],
  fields: Record<string, string>,
): { items: CredentialUploadItem[]; pastedExport?: ReturnType<typeof inspectSoftwareExport> } {
  const fileField = method.file?.field
  const pastedText = !uploads.length && fileField ? (fields[fileField] || '') : ''
  const pastedExport = pastedText ? inspectSoftwareExport(pastedText) : undefined
  if (method.type !== 'api' || !method.file) return { items: [], pastedExport }
  if (uploads.length) return { items: uploads, pastedExport }
  if (pastedExport?.kind === 'multi') {
    return {
      items: [{ id: 'paste', name: '', size: pastedText.length, text: pastedText, exportKind: 'multi', accountCount: pastedExport.accountCount }],
      pastedExport,
    }
  }
  return { items: [], pastedExport }
}

export async function finalizeBatchImport({
  provider,
  label,
  wasFirstAccount,
  itemsCount,
  uploads,
  outcome,
  onClearAll,
  onPartialFailure,
  onAllFailed,
}: {
  provider: string
  label: string
  wasFirstAccount: boolean
  itemsCount: number
  uploads: CredentialUploadItem[]
  outcome: BatchImportOutcome
  onClearAll: () => void
  onPartialFailure: (remaining: CredentialUploadItem[], failures: BatchImportFailure[]) => void
  onAllFailed: (errorText: string, failures: BatchImportFailure[]) => void
}): Promise<void> {
  const summary = formatBatchImportToast(label, outcome)
  if (outcome.added + outcome.updated > 0) {
    if (outcome.failed === 0) {
      onClearAll()
      closeAddModals()
      await shared().wbApp?.refresh?.()
      toast(summary.message, summary.tone)
      if (wasFirstAccount) {
        shared().wbModelsFetchModal?.open?.({
          providerId: provider,
          custom: false,
          name: label || provider,
          autoFetch: true,
          onRefreshed: () => { void shared().wbModelsPanel?.load?.({ force: true }) },
        })
      }
      return
    }
    const remaining = uploads.filter(item => outcome.failedUploadIds.has(item.id))
    onPartialFailure(remaining, outcome.failures)
    toast(summary.message, summary.tone)
    await shared().wbApp?.refresh?.()
    return
  }
  if (outcome.failures.length === 1) {
    const only = outcome.failures[0]
    onAllFailed(only.label && itemsCount > 1 ? `${only.label}：${only.reason}` : only.reason, [])
  } else if (outcome.failures.length > 1) {
    onAllFailed('', outcome.failures)
  } else {
    onAllFailed(summary.message, [])
  }
}

export function isProviderFirstAccount(provider: string): boolean {
  return accountCountOf(provider) === 0
}
