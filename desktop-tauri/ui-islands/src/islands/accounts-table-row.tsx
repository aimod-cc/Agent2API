import * as React from 'react'
import { Checkbox, Spinner, TableCell, TableRow, cn } from '@ui'
import { t } from '../i18n'
import type { AccountRecord } from './accounts-shared'
import { isDesktopAccount, isEnabled, providerOf } from './accounts-domain'
import { getStore, subscribe, togglePick, snapshot, groupSortRules } from './accounts-store'
import { usageRowOf } from './accounts-usage-cache'
import { tokenUsageOf } from './accounts-usage'
import { statesOf } from './accounts-limiter-model'
import { clearLimits } from './accounts-data'
import { activeGroupSortRules } from './accounts-group-settings'
import { useDisplayClock } from '../../../ui-shared/use-display-clock'
import {
  AccountCell, ActionsCell, ConnectionsCell, ExpiryCell, LimitsCell, OrderCell,
  ProxyCell, StatusCell, UsageCell,
} from './accounts-panels'

export type AccountRowColumn = { key: string; align: 'left' | 'center' | 'right' }

export const AccountTableRow = React.memo(function AccountTableRow({ account, rank, position, columns, columnsKey,
  index, displayRank, measure, namesHidden }: {
  account: AccountRecord
  rank: number
  position: number
  columns: AccountRowColumn[]
  columnsKey: string
  index?: number
  displayRank?: number
  measure?: (element: HTMLTableRowElement | null) => void
  namesHidden: boolean
}) {
  useDisplayClock()
  const picked = React.useSyncExternalStore(subscribe, () => getStore().selected.has(account.id))
  React.useSyncExternalStore(subscribe, () => usageRowOf(account.id))
  React.useSyncExternalStore(subscribe, () => getStore().usageTasks.get(account.id))
  React.useSyncExternalStore(subscribe, () => getStore().usageStarting.has(account.id) || getStore().usageStarting.has('*'))
  React.useSyncExternalStore(subscribe, () => getStore().usageInflight.has(account.id))
  React.useSyncExternalStore(subscribe, () => JSON.stringify(statesOf(account)))
  React.useSyncExternalStore(subscribe, () => JSON.stringify(tokenUsageOf(account)))
  React.useSyncExternalStore(subscribe, () => JSON.stringify(groupSortRules(providerOf(account))))
  React.useSyncExternalStore(subscribe, () => JSON.stringify(snapshot()?.accountFeatures?.[providerOf(account)]))
  const cell = (key: string) => {
    switch (key) {
      case 'pick': return <Checkbox checked={picked} data-pick={account.id} title={t('勾选后可批量操作')}
        aria-label={t('勾选后可批量操作')} onCheckedChange={next => togglePick(account.id, next)} />
      case 'order': return <OrderCell account={account} rank={rank}
        sortActive={activeGroupSortRules(providerOf(account), groupSortRules(providerOf(account))).length > 0} />
      case 'account': return <AccountCell account={account} namesHidden={namesHidden} />
      case 'proxy': return <ProxyCell account={account} />
      case 'connections': return <ConnectionsCell account={account} />
      case 'status': return <StatusCell account={account} />
      case 'limits': return <LimitsCell account={account} onClear={model => void clearLimits(account.id, model)} />
      case 'expiry': return <ExpiryCell account={account} />
      case 'usage': return <UsageCell account={account} />
      case 'actions': return <ActionsCell account={account} atFront={position <= 1} />
      default: return null
    }
  }
  return <TableRow ref={measure} data-index={index} aria-rowindex={displayRank} aria-busy={account.detailsLoaded === false}
    className={cn('acct-row', !isEnabled(account) && 'disabled', picked && 'selected')}
    data-id={account.id} data-provider={providerOf(account)} data-desktop={isDesktopAccount(account) ? '1' : undefined}>
    {columns.map(column => <TableCell key={column.key} className={`cell-${column.key} ta-${column.align}`}>
      {account.detailsLoaded === false && column.key !== 'account'
        ? column.key === 'pick' ? <Spinner className='size-3.5' /> : null : cell(column.key)}
    </TableCell>)}
  </TableRow>
}, (previous, next) => previous.account === next.account && previous.rank === next.rank
  && previous.position === next.position && previous.columnsKey === next.columnsKey && previous.index === next.index
  && previous.displayRank === next.displayRank && previous.measure === next.measure && previous.namesHidden === next.namesHidden)
