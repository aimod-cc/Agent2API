import * as React from 'react'
import { Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle } from '@ui'
import { t } from '../i18n'
import { byPriorityOrder, providerOf } from './accounts-domain'
import { allAccounts, reorderQueue } from './accounts-data'
import { shared } from './accounts-shared'

export function AccountsGroupOrder({ onClose }: { onClose: () => void }) {
  const [providers, setProviders] = React.useState(() => [...new Set(allAccounts().slice().sort(byPriorityOrder).map(providerOf))])
  const [busy, setBusy] = React.useState(false)
  const move = (index: number, step: number) => setProviders(previous => {
    const next = [...previous]
    ;[next[index], next[index + step]] = [next[index + step], next[index]]
    return next
  })
  const save = async () => {
    setBusy(true)
    try {
      const accounts = allAccounts().slice().sort(byPriorityOrder)
      const ids = providers.flatMap(provider => accounts.filter(account => providerOf(account) === provider).map(account => account.id))
      if (await reorderQueue(ids)) onClose()
    } finally { setBusy(false) }
  }
  return <Dialog open onOpenChange={open => { if (!open && !busy) onClose() }}>
    <DialogContent className='max-w-lg' showCloseButton={!busy}>
      <DialogHeader><DialogTitle>{t('调整提供商顺序')}</DialogTitle></DialogHeader>
      <DialogBody>
        <div className='flex flex-col gap-2'>
          {providers.map((provider, index) => <div key={provider} className='flex items-center gap-3 rounded-md border border-hairline px-3 py-2'>
            <span className='text-xs text-subtle'>{index + 1}</span>
            <span className='min-w-0 flex-1 truncate'>{t(shared().wbProviders?.labelOf?.(provider) || provider)}</span>
            <Button size='sm' variant='outline' disabled={busy || index === 0} onClick={() => move(index, -1)}>{t('上移')}</Button>
            <Button size='sm' variant='outline' disabled={busy || index === providers.length - 1} onClick={() => move(index, 1)}>{t('下移')}</Button>
          </div>)}
        </div>
      </DialogBody>
      <DialogFooter>
        <Button variant='ghost' disabled={busy} onClick={onClose}>{t('取消')}</Button>
        <Button disabled={busy} onClick={() => void save()}>{busy ? t('保存中…') : t('保存')}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}
