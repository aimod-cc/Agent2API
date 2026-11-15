import * as React from 'react'
import { Checkbox, TableCell, TableRow, cn } from '@ui'
import { t } from '../i18n'
import type { UsageQueryTask } from '../../../ui-shared/usage-query-task'
import type { AccountRecord } from './accounts-shared'
import { isDesktopAccount, isEnabled, providerOf } from './accounts-domain'
import { clearLimits, groupSortRules, togglePick, type AccountsStore } from './accounts-data'
import { activeGroupSortRules } from './accounts-group-settings'
import {
  AccountCell, ActionsCell, ConnectionsCell, ExpiryCell, LimitsCell, OrderCell,
  ProxyCell, StatusCell, UsageCell,
} from './accounts-panels'

type AccountRowProps = {
  account: AccountRecord
  columns: ReadonlyArray<{ key: string; align: 'left' | 'center' | 'right' }>
  rank: number
  atFront: boolean
  picked: boolean
  namesHidden: boolean
  dataVersion: number
  displayMinute: number
  connections: number
  usageInflight: boolean
  usageStarting: boolean
  usageTask: UsageQueryTask | undefined
  sorting: AccountsStore['sorting']
  groupSort: AccountsStore['groupSort']
}

// [喵喵喵]: 共享读数、排序和时间也是单元格的依赖，作为 memo 键传入，不能只比较 account。
export const AccountRow = React.memo(function AccountRow({
  account, columns, rank, atFront, picked, namesHidden,
}: AccountRowProps) {
  const provider = providerOf(account)

  function cell(key: string) {
    switch (key) {
      case 'pick':
        return (
          <Checkbox checked={picked} data-pick={account.id} title={t('勾选后可批量操作')}
            aria-label={t('勾选后可批量操作')}
            onCheckedChange={next => togglePick(account.id, next)} />
        )
      case 'order':
        return <OrderCell account={account} rank={rank}
          sortActive={activeGroupSortRules(provider, groupSortRules(provider)).length > 0} />
      case 'account':
        return <AccountCell account={account} namesHidden={namesHidden} />
      case 'proxy':
        return <ProxyCell account={account} />
      case 'connections':
        return <ConnectionsCell account={account} />
      case 'status':
        return <StatusCell account={account} />
      case 'limits':
        return <LimitsCell account={account} onClear={model => void clearLimits(account.id, model)} />
      case 'expiry':
        return <ExpiryCell account={account} />
      case 'usage':
        return <UsageCell account={account} />
      case 'actions':
        return <ActionsCell account={account} atFront={atFront} />
      default:
        return null
    }
  }

  return (
    <TableRow className={cn('acct-row', !isEnabled(account) && 'disabled', picked && 'selected')}
      data-id={account.id} data-provider={provider}
      data-desktop={isDesktopAccount(account) ? '1' : undefined}>
      {columns.map(column => (
        <TableCell key={column.key} className={`cell-${column.key} ta-${column.align}`}>
          {cell(column.key)}
        </TableCell>
      ))}
    </TableRow>
  )
})
