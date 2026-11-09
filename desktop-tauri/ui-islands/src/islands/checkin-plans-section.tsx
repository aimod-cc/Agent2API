import * as React from 'react'
import {
  Badge, Button, Card, CardAction, CardFooter, CardHeader, CardTitle, Empty,
  ItemGroup, SegmentedControl, cn,
} from '@ui'
import {
  CampaignRewardDetails, claimableCampaigns, claimableCount, claimedCount, pickCampaign, useCampaignRewards,
} from '../../../ui-shared/checkin-campaign-rewards'
import type { CampaignApi, CampaignEntry } from '../../../ui-shared/checkin-campaign-types'
import { CheckinRow } from './checkin-row'
import { PlanRow, claimedPlansToday } from './checkin-plan-row'
import { formatShort, type PlanEntry } from './checkin-types'
import { errorMessage, loadCenter, patch, store, toast } from './checkin-store'
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

/* ─── 限时活动的一行 ─────────────────────────────── */

function CampaignRow({ row, disabled, onReport }: {
  row: CampaignEntry
  disabled: boolean
  onReport: PlansRowReporter
}): React.ReactElement {
  const [expanded, setExpanded] = React.useState(false)
  const state = useCampaignRewards({ entry: row, api, t, onChange: () => void loadCenter() })
  const { data, busy, now } = state

  const claimable = row.available ? claimableCount(data, now) : 0
  const claimed = claimedCount(data, now)
  const key = `campaign:${row.id}`
  const mounted = React.useRef(true)
  const batchLock = React.useRef(false)
  const current = React.useRef(state)
  current.current = state

  React.useEffect(() => {
    mounted.current = true
    return () => { mounted.current = false; onReport(key, null) }
  }, [key, onReport])

  const claimAll = async () => {
    if (batchLock.current || current.current.busy || !row.available) return true
    batchLock.current = true
    setExpanded(true)
    try {
      const targets = [...new Map(claimableCampaigns(current.current.data)
        .map(campaign => [campaign.campaignId, campaign])).values()]
      for (const campaign of targets) {
        if (!mounted.current) return false
        await current.current.claim(campaign)
      }
      return true
    } finally {
      batchLock.current = false
    }
  }

  React.useLayoutEffect(() => {
    onReport(key, { claimed: claimed > 0 && claimable === 0
      && !data?.campaigns.some(campaign => campaign.status === 'confirming'), claimable,
      busy: Boolean(busy), query: row.available ? state.query : async () => {}, claimAll })
  })
  const viewState = disabled && !busy ? { ...state, busy: 'batch' } : state

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
      note={!expanded || !data ? state.error || state.message || undefined : undefined}
      actions={!data ? (
        <Button size='sm' variant='outline' disabled={disabled || Boolean(busy) || !row.available}
          onClick={() => void state.query()}>
          {busy === 'query' ? t('查询中…') : t('查询活动')}
        </Button>
      ) : claimable === 1 ? (
        <Button size='sm' disabled={disabled || Boolean(busy) || !row.available}
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
        onChange={() => void loadCenter()} state={viewState} />
    </CheckinRow>
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

export type PlansCounts = {
  claimed: number
  total: number
  claimable: number
  busy: boolean
  byProvider: Record<string, { claimed: number; total: number }>
}

type PlansRowState = {
  claimed: boolean
  claimable: number
  busy: boolean
  query(): Promise<void>
  claimAll(): Promise<boolean>
}

export type PlansRowReporter = (key: string, state: PlansRowState | null) => void

export function PlansSection({ campaigns, plans, providerId = '', filter: outerFilter, onFilterChange,
  queryAllToken: outerToken, claimAllToken = 0, onCountsChange, embedded }: {
  campaigns: CampaignEntry[]
  plans: PlanEntry[]
  providerId?: string
  /** 外层（签到中心）给的筛选值：与其它任务共用同一枚分段控件 */
  filter?: 'all' | 'done' | 'pending'
  onFilterChange?: (next: 'all' | 'done' | 'pending') => void
  /** 外层「全部查询」的令牌；变化时补查一次 */
  queryAllToken?: number
  claimAllToken?: number
  /** 把「已领 / 总数」上报给外层（外层标题栏的筛选计数要用；按提供商分开给） */
  onCountsChange?: (counts: PlansCounts) => void
  /** true = 只画列表（外层已经有卡片、标题与筛选） */
  embedded?: boolean
}): React.ReactElement {
  const [ownFilter, setOwnFilter] = React.useState<Filter>('all')
  const [ownToken, setOwnToken] = React.useState(0)
  const [ownClaimToken, setOwnClaimToken] = React.useState(0)
  const [rowStates, setRowStates] = React.useState<Record<string, Pick<PlansRowState, 'claimed' | 'claimable' | 'busy'>>>({})
  const actions = React.useRef(new Map<string, PlansRowState>())
  const alive = React.useRef(true)
  React.useEffect(() => {
    alive.current = true
    return () => { alive.current = false }
  }, [])

  const filter: Filter = embedded
    ? outerFilter === 'done' ? 'claimed' : outerFilter === 'pending' ? 'pending' : 'all'
    : ownFilter
  const queryAllToken = embedded ? outerToken ?? 0 : ownToken
  const claimToken = embedded ? claimAllToken : ownClaimToken
  const setFilter = (next: Filter) => {
    if (embedded) onFilterChange?.(next === 'claimed' ? 'done' : next === 'pending' ? 'pending' : 'all')
    else setOwnFilter(next)
  }

  const reportRow = React.useCallback<PlansRowReporter>((key, state) => {
    if (!state) { actions.current.delete(key); return }
    actions.current.set(key, state)
    setRowStates(current => {
      const before = current[key]
      return before?.claimed === state.claimed && before.claimable === state.claimable && before.busy === state.busy
        ? current : { ...current, [key]: { claimed: state.claimed, claimable: state.claimable, busy: state.busy } }
    })
  }, [])

  const isCampaignClaimed = (row: CampaignEntry) => rowStates[`campaign:${row.id}`]?.claimed === true
  const isPlanClaimed = (row: PlanEntry) => rowStates[`plan:${row.id}`]?.claimed ?? claimedPlansToday(row.claimPlans).length > 0
  const keep = (claimed: boolean) => filter === 'all' || (filter === 'claimed' ? claimed : !claimed)
  const scopedCampaigns = campaigns.filter(row => !providerId || row.provider === providerId)
  const scopedPlans = plans.filter(row => !providerId || row.provider === providerId)
  const campaignRows = scopedCampaigns.filter(row => keep(isCampaignClaimed(row)))
  const planRows = scopedPlans.filter(row => keep(isPlanClaimed(row)))
  const visibleKeys = [...campaignRows.map(row => `campaign:${row.id}`), ...planRows.map(row => `plan:${row.id}`)]
  const claimableTotal = visibleKeys.reduce((sum, key) => sum + (rowStates[key]?.claimable ?? 0), 0)
  const busy = store.plansBatch !== null || visibleKeys.some(key => rowStates[key]?.busy)
  const claimedTotal = scopedCampaigns.filter(isCampaignClaimed).length + scopedPlans.filter(isPlanClaimed).length
  const total = scopedCampaigns.length + scopedPlans.length

  React.useEffect(() => {
    const byProvider: PlansCounts['byProvider'] = {}
    const add = (provider: string, claimed: boolean) => {
      const entry = byProvider[provider] ?? { claimed: 0, total: 0 }
      entry.total += 1
      if (claimed) entry.claimed += 1
      byProvider[provider] = entry
    }
    for (const row of campaigns) add(row.provider, isCampaignClaimed(row))
    for (const row of plans) add(row.provider, isPlanClaimed(row))
    onCountsChange?.({ claimed: campaigns.filter(isCampaignClaimed).length + plans.filter(isPlanClaimed).length,
      total: campaigns.length + plans.length, claimable: claimableTotal, busy, byProvider })
  }, [campaigns, plans, rowStates, claimableTotal, busy, onCountsChange])

  const runBatch = async (mode: 'query' | 'claim') => {
    if (store.plansBatch) return
    const keys = [...visibleKeys]
    if (!keys.length) return
    patch({ plansBatch: mode })
    try {
      for (const key of keys) {
        if (!alive.current) break
        const row = actions.current.get(key)
        if (!row || row.busy) continue
        if (mode === 'query') await row.query()
        else if (row.claimable > 0 && !await row.claimAll()) break
      }
    } catch (cause) {
      toast(t('领取失败：{error}', { error: errorMessage(cause) }), 'err')
    } finally {
      patch({ plansBatch: null })
    }
  }
  const lastQuery = React.useRef(queryAllToken)
  const lastClaim = React.useRef(claimToken)
  React.useEffect(() => {
    if (lastQuery.current === queryAllToken) return
    lastQuery.current = queryAllToken
    void runBatch('query')
  }, [queryAllToken])
  React.useEffect(() => {
    if (lastClaim.current === claimToken) return
    lastClaim.current = claimToken
    void runBatch('claim')
  }, [claimToken])

  const options = FILTERS.map(item => ({
    value: item.value,
    label: t(item.label),
    count: item.value === 'all' ? total
      : item.value === 'claimed' ? claimedTotal : total - claimedTotal,
  }))

  const body = <>
    {campaigns.length > 0 ? (
      <div className='pb-1' hidden={campaignRows.length === 0}>
        <GroupLabel title={t('限时活动')} count={campaignRows.length} />
        <ItemGroup>
          {campaigns.map(row => (
            <div key={row.id} hidden={!campaignRows.some(item => item.id === row.id)}>
              <CampaignRow row={row} disabled={store.plansBatch !== null} onReport={reportRow} />
            </div>
          ))}
        </ItemGroup>
      </div>
    ) : null}

    {plans.length > 0 ? (
      <div className={cn('pt-1 pb-1', campaignRows.length > 0 && 'border-t border-hairline')} hidden={planRows.length === 0}>
        <GroupLabel title={t('体验套餐')} count={planRows.length}
          hint={t('活动期内每天可再领一份 · 领取需完成滑块验证')} />
        <ItemGroup>
          {plans.map(row => (
            <div key={row.id} hidden={!planRows.some(item => item.id === row.id)}>
              <PlanRow row={row} disabled={store.plansBatch !== null} onReport={reportRow} />
            </div>
          ))}
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
  </>

  if (embedded) return <div className='ck-plans-embedded'>{body}</div>

  return (
    <Card render={<section />} className='panel'>
      <CardHeader className='border-b-0 pb-1'>
        <CardTitle>{t('活动福利')}</CardTitle>
        <span className='text-[11.5px] text-muted-foreground'>
          {t('限时活动与体验套餐 · 今天已领 {n} 份', { n: claimedTotal })}
        </span>
        <CardAction>
          {/* 用 outline 而不是 ghost：它旁边就是筛选器，没有边框会被读成一段说明文字 */}
          <Button size='sm' variant='outline' disabled={busy || visibleKeys.length === 0}
            onClick={() => setOwnToken(token => token + 1)}>{store.plansBatch === 'query' ? t('查询中…') : t('全部查询')}</Button>
          <Button size='sm' variant='outline' disabled={busy || claimableTotal === 0}
            onClick={() => setOwnClaimToken(token => token + 1)}>{store.plansBatch === 'claim' ? t('领取中…') : t('全部领取')}</Button>
          <SegmentedControl
            aria-label={t('按领取状态筛选')}
            options={options}
            value={filter}
            onValueChange={value => setFilter(value as Filter)}
          />
        </CardAction>
      </CardHeader>

      {body}

      <CardFooter>
        {t('资格、额度与有效期以官方返回为准 · 领取不会购买或订阅套餐')}
      </CardFooter>
    </Card>
  )
}
