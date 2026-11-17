import * as React from 'react'
import { Badge, Button } from '@ui'
import { t } from '../i18n'
import type { AccountRecord } from './accounts-shared'
import { groupsOf, limiterHit, statesOf } from './accounts-limiter-model'
import { LiveCountdown } from './live-countdown'

/**
 * 命中条件里**最早**的恢复时刻（毫秒时间戳）。
 *
 * 后端已经在每个读数上给了窗口终点 `resetsAt`（`limiter/evaluate.rs`）：
 * 固定周期 / 自然日周月 / 总额档都是精确时刻；滚动窗口在补上
 * `counter::rolling_release` 之后也有了「计数回落到阈值以下」的时刻。
 * 仍然拿不到的状况只有三种，此时返回 0，由调用方明说原因：
 *   · 滚动窗口里还没有事件滑出去（窗口刚满，暂时算不出来）；
 *   · `total` 档已经过了 `endsAt`；
 *   · `disable` 档（不自动恢复，`account.enabled` 已为 false，本来就不走这里）。
 */
function soonestReset(account: AccountRecord, now = Date.now()): number {
  let soonest = 0
  for (const state of statesOf(account)) {
    if (!state.hit) continue
    for (const reading of state.conditions) {
      if (!reading.hit) continue
      const at = Number(reading.resetsAt)
      if (!Number.isFinite(at) || at <= now) continue
      if (soonest === 0 || at < soonest) soonest = at
    }
  }
  return soonest
}

export function LimiterStatus({ account, onOpen }: { account: AccountRecord; onOpen: () => void }) {
  if (account.enabled === false || !limiterHit(account)) return null
  const states = statesOf(account)
  const groups = groupsOf(account)
  const all = groups.some((group, index) => group.action === 'skip' && states[index]?.hit && states[index]?.account)
  const models = [...new Set(groups.flatMap((group, index) => group.action === 'skip' && states[index]?.hit ? states[index].models : []))]
  const label = all || !models.length ? t('限制器 · 账号已跳过') : t('限制器 · {n} 个模型', { n: models.length })
  const soonest = soonestReset(account)
  return <>
    <Button variant='ghost' size='xs' className='h-auto p-0' onClick={onOpen}
      title={all ? t('账号级条件组已触发；点击查看限制器') : t('仅跳过命中条件的模型，其他模型仍可转发；点击查看限制器')}>
      <Badge variant='warning' shape='tag'>{label}</Badge>
    </Button>
    {soonest > 0
      /* 与「限流」列的冷却行同一套 `.lim-sub`（该列本来就按两行排版） */
      ? <LiveCountdown until={soonest} className='lim-sub' title={t('到恢复时间后自动解除，无需手动操作')} />
      : <span className='lim-sub' title={t('滚动窗口按请求逐条滑出，计数回落到阈值以下才会放行，此刻还算不出确切时刻')}>
        {t('待计数回落')}
      </span>}
  </>
}
