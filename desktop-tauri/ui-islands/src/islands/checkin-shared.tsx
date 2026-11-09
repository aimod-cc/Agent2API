import * as React from 'react'
import { Badge, cn } from '@ui'
import { t } from '../i18n'
import { checkinStatusLabel, checkinStatusTone, type AccountCheckinStatus } from '../../../ui-shared/checkin-result'

/**
 * 签到中心各档位共用的小件：状态徽标、明细行、状态筛选。
 *
 * 三个档位（每日签到 / 新手福利 / 成长任务）的**结构**是同一套，只是动作词不同
 * （签到 / 领取 / 完成），所以筛选器与明细行都在这里写一份，各档位只传措辞。
 */

export type StatusFilter = 'all' | 'done' | 'pending'

/** 一条筛选器的三个档位：全部 / 已完成 / 未完成（计数一起算）。 */
export function filterOptions(labels: readonly [string, string, string], total: number, done: number) {
  return [
    { value: 'all' as const, label: t(labels[0]), count: total },
    { value: 'done' as const, label: t(labels[1]), count: done },
    { value: 'pending' as const, label: t(labels[2]), count: total - done },
  ]
}

/** 按状态过滤一个列表（`doneOf` 由各档位自己给判据）。 */
export function byStatus<T>(rows: T[], filter: StatusFilter, doneOf: (row: T) => boolean): T[] {
  if (filter === 'all') return rows
  return rows.filter(row => filter === 'done' ? doneOf(row) : !doneOf(row))
}

export function StatusBadge({ status }: { status: AccountCheckinStatus }): React.ReactElement {
  const variants = { ok: 'success', warn: 'warning', bad: 'destructive', mute: 'outline', brand: 'brand' } as const
  return <Badge variant={variants[checkinStatusTone(status)]} shape='tag'>{t(checkinStatusLabel(status))}</Badge>
}

/** 展开区里的一行明细（账号 / 任务 / 成长三处共用同一档字号与节奏）。 */
export function DetailLine({ children }: { children: React.ReactNode }): React.ReactElement {
  return <div className='flex items-center gap-3 py-1.5 text-[12.5px]'>{children}</div>
}

/** 明细行的左侧「名 + 连登」；连登 0 = 还没读到过，不显示成「连登 0 天」 */
export function DetailName({ name, streakDays }: { name: string; streakDays: number }): React.ReactElement {
  return (
    <span className='min-w-0 flex-1 truncate'>
      {name}
      {streakDays > 0 ? <span className='text-muted-foreground'> · {t('连登 {days} 天', { days: streakDays })}</span> : null}
    </span>
  )
}

/** 任务行的对勾：已完成是绿底勾，未完成是一个空心点 */
export function TaskTick({ done }: { done: boolean }): React.ReactElement {
  return (
    <span className={cn('grid size-4 shrink-0 place-items-center rounded-full text-[10px]',
      done ? 'bg-success-soft text-success' : 'bg-control text-muted-foreground')}>
      {done ? '✓' : '·'}
    </span>
  )
}
