import * as React from 'react'
import {
  Badge, Button, Card, CardAction, CardFooter, CardHeader, CardTitle, Empty,
  Item, ItemActions, ItemContent, ItemDescription, ItemGroup, ItemMedia, ItemTitle,
  SegmentedControl, cn,
} from '@ui'
import {
  CampaignRewardDetails, claimableCount, claimedCount, pickCampaign, useCampaignRewards,
} from '../../../ui-shared/checkin-campaign-rewards'
import type { CampaignApi, CampaignEntry } from '../../../ui-shared/checkin-campaign-types'
import { CheckinRow, ProviderLogo } from './checkin-row'
import { formatShort, type PlanEntry } from './checkin-types'
import { loadCenter } from './checkin-store'
import { t } from '../i18n'

/**
 * 「签到中心 → 活动福利」这一档（桌面端）。
 *
 * ── 它改了什么 ───────────────────────────────────────────────
 * 旧版每个账号是一张卡片：头像一行、两行**固定文案**（「奖励与权益 · 资格、额度和
 * 有效期以官方返回为准」／「先查询活动，再选择要领取的奖励；不会购买或订阅套餐」）、
 * 右侧一枚按钮 —— 四五个账号连排时同一段话被复读四五遍，一屏塞不下三个账号。
 *
 * 现在收成行：行上只放**这个账号自己的差异**（额度、能不能领、截止时间），
 * 全组共用的说明只在卡片底部出现一次，活动明细收进行的展开区；卡片标题行右侧
 * 是「全部 / 未领取 / 已领取」筛选（各带计数）。
 *
 * ── 筛选为什么需要行上报 ─────────────────────────────────────
 * 「今天领没领到」只有查过才知道，而查询状态在每一行内部（`useCampaignRewards`）。
 * 所以行把自己那份结论上报给卡片，卡片据此汇总计数与过滤 —— 这也是「未领取」
 * 能把**还没查询**的账号一起算进来的原因。
 */

type CampaignBridge = {
  getCampaignRewards?: (id: string) => import('../../../ui-shared/checkin-campaign-types').CampaignPreview | Promise<import('../../../ui-shared/checkin-campaign-types').CampaignPreview>
  claimCampaignReward?: (id: string, campaignId: string) => Promise<import('../../../ui-shared/checkin-campaign-types').CampaignClaimResult>
  redeemCampaignBenefit?: (id: string, code: string) => Promise<import('../../../ui-shared/checkin-campaign-types').BenefitRedemptionResult>
}

const api: CampaignApi = {
  query(id) {
    const bridge = (window as unknown as { workbuddyDesktop?: CampaignBridge }).workbuddyDesktop
    if (!bridge?.getCampaignRewards) return Promise.reject(new Error(t('活动福利接口不可用，请更新网关后重试')))
    return Promise.resolve(bridge.getCampaignRewards(id))
  },
  claim(id, campaignId) {
    const bridge = (window as unknown as { workbuddyDesktop?: CampaignBridge }).workbuddyDesktop
    if (!bridge?.claimCampaignReward) return Promise.reject(new Error(t('活动福利接口不可用，请更新网关后重试')))
    return bridge.claimCampaignReward(id, campaignId)
  },
  redeem(id, code) {
    const bridge = (window as unknown as { workbuddyDesktop?: CampaignBridge }).workbuddyDesktop
    if (!bridge?.redeemCampaignBenefit) return Promise.reject(new Error(t('活动福利接口不可用，请更新网关后重试')))
    return bridge.redeemCampaignBenefit(id, code)
  },
}

/** 今天（北京时间）在本地台账里领过的套餐 id（上游按北京判「今天」，日界同一口径） */
function claimedPlansToday(claimPlans: Record<string, number> | null | undefined): string[] {
  if (!claimPlans || typeof claimPlans !== 'object') return []
  const today = new Date(Date.now() + 8 * 3600 * 1000).toISOString().slice(0, 10)
  return Object.entries(claimPlans)
    .filter(([, at]) => {
      const stamp = Number(at)
      if (!Number.isFinite(stamp) || stamp <= 0) return false
      return new Date(stamp + 8 * 3600 * 1000).toISOString().slice(0, 10) === today
    })
    .map(([planId]) => planId)
}

/* ─── 限时活动的一行 ─────────────────────────────── */

function CampaignRow({ row, queryAllToken, onClaimedChange }: {
  row: CampaignEntry
  queryAllToken: number
  onClaimedChange: (id: string, claimedToday: boolean) => void
}): React.ReactElement {
  const [expanded, setExpanded] = React.useState(false)
  const state = useCampaignRewards({ entry: row, api, t, onChange: () => void loadCenter() })
  const { data, busy, now } = state

  const claimable = claimableCount(data, now)
  const claimed = claimedCount(data, now)

  // 把「今天领没领到」上报给卡片（筛选与计数要用）；查询结果没到之前一律算未领
  React.useEffect(() => {
    onClaimedChange(row.id, claimed > 0)
  }, [row.id, claimed, onClaimedChange])

  // 卡片头的「全部查询」：令牌一变就补一次查询（首次挂载不触发）
  const mounted = React.useRef(false)
  React.useEffect(() => {
    if (!mounted.current) { mounted.current = true; return }
    void state.query()
    // 只认令牌 —— state.query 每次渲染都是新函数，放进依赖会变成死循环
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [queryAllToken])

  const top = pickCampaign(data, now)
  const topText = top
    ? [
      typeof top.amount === 'number' && Number.isFinite(top.amount) ? `${top.amount} ${top.unit || 'Credits'}` : '',
      top.endsAt ? t('截止 {time}', { time: formatShort(top.endsAt) }) : '',
    ].filter(Boolean).join(' · ')
    : ''
  const description = !row.available
    ? t('凭证不可用')
    : !data
      ? t('尚未查询活动')
      : claimable > 0
        ? topText || t('可领取 {n} 项', { n: claimable })
        : claimed > 0
          ? topText || t('今天已领')
          : t('该账号当前没有可领取的限时活动')

  return (
    <CheckinRow
      provider={row.provider || 'qoder'}
      name={row.name || row.id}
      description={description}
      summary={claimed > 0 ? <Badge variant='success' shape='tag'>{t('今天已领')}</Badge> : null}
      actions={!data ? (
        <Button size='sm' variant='outline' disabled={Boolean(busy) || !row.available}
          onClick={() => void state.query()}>
          {busy === 'query' ? t('查询中…') : t('查询活动')}
        </Button>
      ) : claimable === 1 ? (
        <Button size='sm' disabled={Boolean(busy) || !row.available}
          onClick={() => { if (top) void state.claim(top) }}>
          {busy ? t('领取中…') : t('领取')}
        </Button>
      ) : null}
      expandable={Boolean(data)}
      expanded={expanded}
      onExpandedChange={setExpanded}
    >
      <CampaignRewardDetails entry={row} api={api} t={t}
        locale={window.wbI18n?.locale() || 'zh-Hans'} formatTime={formatShort}
        onChange={() => void loadCenter()} state={state} />
    </CheckinRow>
  )
}

/* ─── 体验套餐的一行 ─────────────────────────────── */

function PlanRow({ row }: { row: PlanEntry }): React.ReactElement {
  const count = claimedPlansToday(row.claimPlans).length
  const description = count > 0
    ? t('今天已领 {n} 份', { n: count })
    : row.claimAt
      ? t('今天未领 · 上次领取 {time}', { time: formatShort(row.claimAt) })
      : t('今天未领')
  return (
    <Item className='gap-3'>
      <ItemMedia><ProviderLogo id={row.provider || 'zcode'} label='ZCode' /></ItemMedia>
      <ItemContent>
        <ItemTitle>{row.name || row.id}</ItemTitle>
        <ItemDescription>{description}</ItemDescription>
      </ItemContent>
      <ItemActions>
        <Button size='sm' variant={count > 0 ? 'outline' : 'default'}
          onClick={() => window.wbZcodePlans?.open?.({ id: row.id, name: row.name })}>
          {count > 0 ? t('套餐明细') : t('去领取')}
        </Button>
      </ItemActions>
    </Item>
  )
}

/** 组标题：报出这一组是谁、有几个；这一组通用的规则挂右侧，行里就不必复读 */
function GroupLabel({ title, count, hint }: {
  title: string
  count: number
  hint?: string
}): React.ReactElement {
  return (
    <div className='flex items-baseline gap-2 px-3.5 pb-1 pt-3'>
      <span className='text-[11px] font-semibold tracking-[0.08em] text-muted-foreground'>{title}</span>
      <span className='text-[11.5px] text-muted-foreground'>{t('{n} 个账号', { n: count })}</span>
      {hint ? <span className='ml-auto text-[11.5px] text-muted-foreground'>{hint}</span> : null}
    </div>
  )
}

/* ─── 卡片 ─────────────────────────────────────── */

const FILTERS = [
  { value: 'all', label: '全部' },
  { value: 'pending', label: '未领取' },
  { value: 'claimed', label: '已领取' },
] as const

type Filter = (typeof FILTERS)[number]['value']

export function PlansSection({ campaigns, plans }: {
  campaigns: CampaignEntry[]
  plans: PlanEntry[]
}): React.ReactElement {
  const [filter, setFilter] = React.useState<Filter>('all')
  const [queryAllToken, setQueryAllToken] = React.useState(0)
  const [claimedMap, setClaimedMap] = React.useState<Record<string, boolean>>({})

  const markClaimed = React.useCallback((id: string, claimedToday: boolean) => {
    setClaimedMap(current => current[id] === claimedToday ? current : { ...current, [id]: claimedToday })
  }, [])

  const isPlanClaimed = (row: PlanEntry) => claimedPlansToday(row.claimPlans).length > 0
  const claimedTotal = campaigns.filter(row => claimedMap[row.id] === true).length
    + plans.filter(isPlanClaimed).length
  const total = campaigns.length + plans.length
  const keep = (claimedToday: boolean) => filter === 'all'
    || (filter === 'claimed' ? claimedToday : !claimedToday)

  const campaignRows = campaigns.filter(row => keep(claimedMap[row.id] === true))
  const planRows = plans.filter(row => keep(isPlanClaimed(row)))
  const options = FILTERS.map(item => ({
    value: item.value,
    label: t(item.label),
    count: item.value === 'all' ? total
      : item.value === 'claimed' ? claimedTotal : total - claimedTotal,
  }))

  return (
    <Card render={<section />} className='panel'>
      <CardHeader className='border-b-0 pb-1'>
        <CardTitle>{t('活动福利')}</CardTitle>
        <span className='text-[11.5px] text-muted-foreground'>
          {t('限时活动与体验套餐 · 今天已领 {n} 份', { n: claimedTotal })}
        </span>
        <CardAction>
          {/* 用 outline 而不是 ghost：它旁边就是筛选器，没有边框会被读成一段说明文字 */}
          <Button size='sm' variant='outline' disabled={campaigns.length === 0}
            onClick={() => setQueryAllToken(token => token + 1)}>{t('全部查询')}</Button>
          <SegmentedControl
            aria-label={t('按领取状态筛选')}
            options={options}
            value={filter}
            onValueChange={setFilter}
          />
        </CardAction>
      </CardHeader>

      {campaignRows.length > 0 ? (
        <div className='pb-1'>
          <GroupLabel title={t('限时活动')} count={campaignRows.length} />
          <ItemGroup>
            {campaignRows.map(row => (
              <CampaignRow key={row.id} row={row} queryAllToken={queryAllToken} onClaimedChange={markClaimed} />
            ))}
          </ItemGroup>
        </div>
      ) : null}

      {planRows.length > 0 ? (
        <div className={cn('pt-1 pb-1', campaignRows.length > 0 && 'border-t border-hairline')}>
          <GroupLabel title={t('体验套餐')} count={planRows.length}
            hint={t('活动期内每天可再领一份 · 领取需完成滑块验证')} />
          <ItemGroup>
            {planRows.map(row => <PlanRow key={row.id} row={row} />)}
          </ItemGroup>
        </div>
      ) : null}

      {campaignRows.length === 0 && planRows.length === 0 ? (
        <Empty className='py-6'>
          {filter === 'claimed'
            ? t('今天还没有账号领到过，切到「全部」看看还能领什么。')
            : filter === 'pending'
              ? t('都领到了，暂无待领的账号。')
              : t('没有支持活动福利的账号，请添加账号并更新提供商插件。')}
        </Empty>
      ) : null}

      <CardFooter>
        {t('资格、额度与有效期以官方返回为准 · 领取不会购买或订阅套餐')}
      </CardFooter>
    </Card>
  )
}
