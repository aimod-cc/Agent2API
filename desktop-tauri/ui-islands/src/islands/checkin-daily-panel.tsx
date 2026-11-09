import * as React from 'react'
import { Badge, Button } from '@ui'
import { signSingle, store } from './checkin-store'
import { MODE_LABEL, MODE_TITLE, formatShort, modesOf, type CheckinAccount, type ProviderGroup } from './checkin-types'
import { ProviderLogo } from './checkin-row'
import { DetailLine, StatusBadge } from './checkin-shared'
import { t } from '../i18n'
import { accountCheckinStatus, checkinActionLabel, checkinPlanProgress } from '../../../ui-shared/checkin-result'

/**
 * 「签到中心 → 每日签到」的账号明细（右栏主体）。
 *
 * ── 为什么不再是「每家一行、点开展开」 ─────────────────────
 * 提供商分类挪到左栏之后，右栏直接列账号：左栏选「全部」时按家分组显示（每组
 * 一个小标题），选某一家时只列这一家的账号。少一层展开，一屏能多看几个账号。
 *
 * ── 一行只放这个账号自己的差异 ─────────────────────────────
 * 状态、上次签到、收益与操作都在行上；这一家的通用规则由右栏标题下的说明条
 * 承担（见 CenterTab）。
 */

export function AccountLine({ account, group }: {
  account: CheckinAccount
  group: ProviderGroup
}): React.ReactElement {
  const busy = (mode: string) => store.signing.has(`${account.id}:${mode}`)
  const status = accountCheckinStatus(account)
  const daily = account.dailyCheckin
  const modes = modesOf(group)
  // 分档执行由插件申报：keepalive / claim / full 各一颗按钮 —— 领取才加积分、
  // 保活只维持活跃，分开是刻意的
  const dailyActivity = modes.some(mode => mode === 'keepalive' || mode === 'claim' || mode === 'full')
  const manualPlans = group.manualPlans === true
  const progress = checkinPlanProgress(daily) && daily
    ? t('已领 {done} / {total} 份套餐', { done: daily.completedCount, total: daily.totalCount })
    : ''
  return (
    <DetailLine>
      <span className='min-w-0 flex-1 truncate'>{account.name}
        {account.streakDays > 0
          ? <span className='text-muted-foreground'> · {t('连登 {days} 天', { days: account.streakDays })}</span> : null}
      </span>
      {account.available ? null : <Badge variant='outline' shape='tag'>{t('凭据不可用')}</Badge>}
      {/* 异常信息优先于进度读数：它是「为什么还没签上」的答案 */}
      {daily?.message && status !== 'done' ? (
        <span className='max-w-[320px] flex-1 truncate text-[11.5px] text-muted-foreground'
          title={t(daily.message)}>{t(daily.message)}</span>
      ) : progress ? (
        <span className='shrink-0 text-[11.5px] text-muted-foreground'>{progress}</span>
      ) : null}
      {daily?.kind === 'rolling' && daily.until ? <span className='shrink-0 tabular-nums text-muted-foreground'>
        {t('下次检查：{time}', { time: formatShort(daily.until) })}
      </span> : null}
      <span className='shrink-0 tabular-nums text-muted-foreground'>
        {account.checkinAt ? t('上次 {time}', { time: formatShort(account.checkinAt) }) : t('从未签到')}
      </span>
      <StatusBadge status={status} />
      <span className='flex shrink-0 items-center gap-0.5'>
        {dailyActivity
          ? modes.map(mode => (
            <Button
              key={mode}
              size='xs'
              variant='ghost'
              disabled={busy(mode) || !account.available}
              title={MODE_TITLE[mode]}
              onClick={() => void signSingle(account.id, mode)}
            >
              {busy(mode) ? `${MODE_LABEL[mode] ?? mode}中…` : (MODE_LABEL[mode] ?? mode)}
            </Button>
          ))
          : (
            <Button
              size='xs'
              variant='ghost'
              disabled={busy('checkin') || !account.available}
              onClick={() => {
                if (manualPlans && window.wbZcodePlans?.open) {
                  window.wbZcodePlans.open({ id: account.id, name: account.name })
                } else {
                  void signSingle(account.id, 'checkin')
                }
              }}
            >
              {manualPlans ? t('套餐明细') : busy('checkin')
                ? t(daily?.queryOnly ? '查询中…' : '签到中…') : t(checkinActionLabel(account))}
            </Button>
          )}
      </span>
    </DetailLine>
  )
}

/** 每日签到的账号列表（右栏）：左栏选全部时按家分组，选一家时只列这一家。 */
export function DailyAccountList({ entries, showProvider }: {
  entries: Array<{ group: ProviderGroup; accounts: CheckinAccount[] }>
  showProvider: boolean
}): React.ReactElement {
  const rows = entries.filter(entry => entry.accounts.length > 0)
  if (!rows.length) return <p className='ck-list-empty'>{t('没有符合条件的账号。')}</p>
  return (
    <div className='ck-account-list'>
      {rows.map(entry => (
        <React.Fragment key={entry.group.id}>
          {showProvider ? <div className='ck-account-group'>
            <ProviderLogo id={entry.group.id} label={entry.group.label} />
            <span>{t(entry.group.label)}</span>
            <span className='text-muted-foreground'>{t('{done} / {total} 个账号已签', {
              done: entry.group.doneCount, total: entry.group.totalCount,
            })}</span>
          </div> : null}
          {entry.accounts.map(account => (
            <AccountLine key={account.id} account={account} group={entry.group} />
          ))}
        </React.Fragment>
      ))}
    </div>
  )
}
