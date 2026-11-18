import { shared } from './accounts-shared'
import { patch, getStore } from './accounts-store'

let pending: Promise<void> | undefined
let requested = 0

export function refreshAccountIndex(): Promise<void> {
  requested++
  if (!pending) pending = (async () => {
    let completed: number
    do {
      completed = requested
      const index = await shared().workbuddyDesktop?.getAccounts({ view: 'index' })
      if (!index || !Array.isArray(index.accounts)) throw new Error('账号摘要响应异常')
      if (completed === requested) {
        shared().wbApp?.applyAccounts?.(index)
        if (getStore().listError) patch({ listError: '' })
      }
    } while (completed !== requested)
  })().catch(error => {
    patch({ listError: error instanceof Error ? error.message : String(error) })
    throw error
  }).finally(() => { pending = undefined })
  return pending
}
