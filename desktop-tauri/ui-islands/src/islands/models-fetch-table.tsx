import * as React from 'react'
import {
  Badge, Button, Checkbox, Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
  type BadgeProps,
} from '@ui'
import { t } from '../i18n'

export type RefreshItem = {
  provider?: string; providerLabel?: string; status?: string; count?: unknown
  refreshedAt?: unknown; fixed?: boolean; message?: string; accountId?: string
}

export type Row = { provider: string; label: string; item?: RefreshItem }

type Props = {
  custom: boolean
  status: 'idle' | 'loading' | 'error' | 'ready'
  rows: Row[]
  upstream: string[]
  managed: ReadonlySet<string>
  picked: ReadonlySet<string>
  keyword: string
  busy: boolean
  busyRow: string
  onPick: (id: string, checked: boolean) => void
  onFetchRow: (row: Row) => void
  sourceCell: (provider: string, reported?: unknown) => React.ReactNode
  refreshedAt: (provider: string) => number
  formatTime: (value: number) => string
}

// 列宽层按表 id / data-col 定位，卸载失败态后重新挂载时必须恢复列宽与拖动把手。
const TABLE_ID = { intl: 'fm-table-intl', custom: 'fm-table-custom' }
const TABLE_COL_ID = { intl: 'fetch-models', custom: 'fetch-models-custom' }
const COLS_INTL = ['provider', 'source', 'state', 'count', 'note', 'updated', 'op']
const COLS_CUSTOM = ['pick', 'model', 'state']
const WIDTH_INTL: Record<string, string> = {
  provider: 'w-[13%]', source: 'w-[21%]', state: 'w-[9%]',
  count: 'w-[6%]', note: 'w-[20%]', updated: 'w-[16%]', op: 'w-[15%]',
}
const WIDTH_CUSTOM: Record<string, string> = { pick: 'w-[8%]', model: 'w-[74%]', state: 'w-[18%]' }
const HEAD_LABEL: Record<string, string> = {
  pick: '', provider: t('提供商'), source: t('模型来源'), state: t('状态'), count: t('模型数'),
  note: t('说明'), updated: t('更新日期'), model: t('上游模型'), op: t('操作'),
}
const KIND: Record<string, { variant: NonNullable<BadgeProps['variant']>; label: string }> = {
  refreshed: { variant: 'brand', label: t('已刷新') },
  failed: { variant: 'destructive', label: t('失败') },
  skipped: { variant: 'outline', label: t('跳过') },
}

function norm(id: string) {
  return id.trim().toLowerCase()
}

function rateCell(text: string) {
  return <span className='font-mono text-[11.5px] tabular-nums text-muted-foreground'>{text}</span>
}

function nameCell(text: string) {
  return <div className='truncate font-mono text-[12px] font-semibold'>{t(text)}</div>
}

export function ModelsFetchTable({
  custom, status, rows, upstream, managed, picked, keyword, busy, busyRow,
  onPick, onFetchRow, sourceCell, refreshedAt, formatTime,
}: Props) {
  React.useEffect(() => {
    const bridge = window as unknown as { wbTableColumns?: { repaint?: (id: string) => void } }
    bridge.wbTableColumns?.repaint?.(custom ? TABLE_COL_ID.custom : TABLE_COL_ID.intl)
  }, [custom])

  const cols = custom ? COLS_CUSTOM : COLS_INTL
  const needle = keyword.trim().toLowerCase()
  const shown = upstream.filter(id => !needle || id.toLowerCase().includes(needle))

  function stateRow(text: string) {
    return (
      <TableRow>
        <TableCell colSpan={cols.length} className='py-[26px] text-center text-muted-foreground'>
          {text}
        </TableCell>
      </TableRow>
    )
  }

  function rowPick(event: React.MouseEvent<HTMLTableRowElement>, id: string, added: boolean) {
    if (added) return
    const target = event.target as Element | null
    // 复选框已经处理过点击；拖选模型 id 时也不能改变勾选状态。
    if (target?.closest('[data-slot="checkbox"], input[type="checkbox"]')) return
    const selection = window.getSelection()
    if (selection && !selection.isCollapsed && event.currentTarget.contains(selection.anchorNode)) return
    onPick(id, !picked.has(id))
  }

  function customBody() {
    if (status === 'idle') return stateRow(t('点右侧「获取模型」从上游拉取这家的模型清单'))
    if (status === 'loading') return stateRow(t('正在获取…'))
    if (!shown.length) return stateRow(upstream.length
      ? t('没有匹配「{keyword}」的模型', { keyword: needle })
      : t('上游没有返回任何模型'))
    return shown.map(id => {
      const added = managed.has(norm(id))
      return (
        <TableRow key={id} className={added ? 'opacity-50' : 'cursor-pointer'}
          onClick={event => rowPick(event, id, added)}>
          <TableCell>
            <Checkbox checked={picked.has(id)} disabled={added} aria-label={id}
              onCheckedChange={next => onPick(id, Boolean(next))} />
          </TableCell>
          <TableCell>{nameCell(id)}</TableCell>
          <TableCell>
            {added ? (
              <Badge variant='brand' title={t('这个模型已经在这家的清单里，不重复导入')}>{t('已添加')}</Badge>
            ) : (
              <Badge variant='outline' title={t('勾上它，导入后即进入这家的清单')}>{t('未添加')}</Badge>
            )}
          </TableCell>
        </TableRow>
      )
    })
  }

  function rowFetchButton(row: Row) {
    const fetching = busy && busyRow === row.provider
    return (
      <Button variant='outline' size='sm' className='w-full' disabled={busy}
        title={t('只刷新这一家的模型清单')} onClick={() => onFetchRow(row)}>
        {fetching ? t('获取中…') : row.item ? t('重新获取') : t('获取')}
      </Button>
    )
  }

  function intlBody() {
    if (status === 'loading') return stateRow(t('正在获取…'))
    if (!rows.length) return stateRow(
      keyword.trim()
        ? t('没有匹配的提供商')
        : t('当前筛选条件下没有提供商'),
    )
    return rows.map(row => {
      const item = row.item
      if (!item) {
        const ts = refreshedAt(row.provider)
        const at = formatTime(ts)
        return (
          <TableRow key={row.provider}>
            <TableCell>{nameCell(row.label)}</TableCell>
            <TableCell>{sourceCell(row.provider)}</TableCell>
            <TableCell>
              {ts > 0 ? (
                <Badge variant='outline'>{t('已获取')}</Badge>
              ) : (
                <Badge variant='warning'>{t('未获取')}</Badge>
              )}
            </TableCell>
            <TableCell>{rateCell('—')}</TableCell>
            <TableCell>{rateCell('—')}</TableCell>
            <TableCell>{rateCell(at || '—')}</TableCell>
            <TableCell className='sticky right-0 z-[1] bg-surface shadow-[-1px_0_0_var(--hairline)]'>{rowFetchButton(row)}</TableCell>
          </TableRow>
        )
      }
      const kind = KIND[String(item.status)]
        ?? { variant: 'outline' as const, label: String(item.status || t('未知')) }
      const count = Number(item.count)
      const note = item.status === 'refreshed'
        ? t('远程目录已更新，「来源」列会显示为「远程」')
        : item.message || (item.fixed ? t('这家用固定模型清单，无可刷新') : t('本次没有取到新内容'))
      const at = formatTime(Number(item.refreshedAt) || 0)
      return (
        <TableRow key={row.provider}>
          <TableCell>{nameCell(row.label)}</TableCell>
          <TableCell>{sourceCell(row.provider, item.accountId)}</TableCell>
          <TableCell><Badge variant={kind.variant}>{kind.label}</Badge></TableCell>
          <TableCell>{rateCell(count ? t('{n} 个', { n: count }) : '—')}</TableCell>
          <TableCell>
            <span className='text-[11.5px] leading-[1.5] text-muted-foreground [overflow-wrap:anywhere]'>{t(note)}</span>
          </TableCell>
          <TableCell>{rateCell(at || '—')}</TableCell>
          <TableCell className='sticky right-0 z-[1] bg-surface shadow-[-1px_0_0_var(--hairline)]'>{rowFetchButton(row)}</TableCell>
        </TableRow>
      )
    })
  }

  return (
    <Table id={custom ? TABLE_ID.custom : TABLE_ID.intl} className='table-fixed'
      containerClassName='max-h-[min(48vh,460px)]'>
      <colgroup>
        {cols.map(key => (
          <col key={key} data-col={key} className={custom ? WIDTH_CUSTOM[key] : WIDTH_INTL[key]} />
        ))}
      </colgroup>
      <TableHeader>
        <TableRow>
          {cols.map(key => (
            <TableHead key={key} data-col={key}
              className={key === 'op' ? 'sticky right-0 top-0 z-[3] bg-[var(--ui-table-head-bg)] shadow-[-1px_0_0_var(--hairline)]' : undefined}>
              {HEAD_LABEL[key]}
            </TableHead>
          ))}
        </TableRow>
      </TableHeader>
      <TableBody className='[&_tr:last-child>td]:border-b-0'>{custom ? customBody() : intlBody()}</TableBody>
    </Table>
  )
}
