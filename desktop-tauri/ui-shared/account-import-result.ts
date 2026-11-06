export type AccountImportNotice = {
  importNotice?: string
  warning?: string
}

export function accountImportNotice(value: unknown): { text: string; warning: boolean } | undefined {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return undefined
  const result = value as AccountImportNotice
  const warning = typeof result.warning === 'string' ? result.warning.trim() : ''
  const notice = typeof result.importNotice === 'string' ? result.importNotice.trim() : ''
  if (!notice && !warning) return undefined
  return { text: notice || warning, warning: Boolean(warning) }
}
