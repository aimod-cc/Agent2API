import { useRef, useState } from 'react'
import { Badge, Button } from '@ui'
import { useDisplayClock } from './use-display-clock'
import { CampaignBenefitDetails } from './checkin-benefit-details'
import type { CampaignApi, CampaignEntry, CampaignPreview, CampaignReward, CampaignStatus } from './checkin-campaign-types'

type Translate = (key: string, params?: Record<string, string | number>) => string

function titleOf(campaign: CampaignReward, locale: string): string {
  return locale.startsWith('zh') ? campaign.title : campaign.titleEn || campaign.title
}

function statusLabel(status: CampaignStatus, t: Translate): string {
  switch (status) {
    case 'claimable': return t('可领取')
    case 'claimed': return t('已领取')
    case 'inactive': return t('非活动期')
    case 'confirming': return t('领取结果待确认')
    default: return t('暂不可领（未满足条件）')
  }
}

function outcomeMessage(outcome: string, t: Translate): string {
  switch (outcome) {
    case 'claimed': return t('活动奖励领取成功')
    case 'already_claimed': return t('该活动已领取，无需重复领取')
    case 'inactive': return t('活动未开放或已结束，请重新查询')
    case 'ineligible': return t('当前账号暂不符合该活动的领取条件')
    case 'confirming': return t('领取结果待确认，请稍后查询；不会重复提交领取')
    default: return t('活动领取未成功，请重新查询后再试')
  }
}

function currentStatus(campaign: CampaignReward, now = Date.now()): CampaignStatus {
  if (campaign.status === 'claimed' || campaign.status === 'confirming') return campaign.status
  return (campaign.startsAt && campaign.startsAt > now) || (campaign.endsAt && campaign.endsAt <= now)
    ? 'inactive' : campaign.status
}

export function claimableCampaigns(data: CampaignPreview | null, now = Date.now()): CampaignReward[] {
  return data?.campaigns.filter(campaign => currentStatus(campaign, now) === 'claimable') ?? []
}

/** 这条活动「现在能不能领」—— 桌面端用它算行上的摘要 */
export function claimableCount(data: CampaignPreview | null, now = Date.now()): number {
  return claimableCampaigns(data, now).length
}

/** 这条活动里今天领到过的条数 */
export function claimedCount(data: CampaignPreview | null, now = Date.now()): number {
  if (!data) return 0
  return data.campaigns.filter(campaign => currentStatus(campaign, now) === 'claimed').length
}

/**
 * 挑一条「最有代表性」的活动给行上的摘要用：能领的优先，其次第一条。
 * 桌面上行里只有一行字的位置，说不出全部活动，所以拿第一条可领的当代表。
 */
export function pickCampaign(data: CampaignPreview | null, now = Date.now()): CampaignReward | null {
  if (!data || data.campaigns.length === 0) return null
  return data.campaigns.find(campaign => currentStatus(campaign, now) === 'claimable') ?? data.campaigns[0]
}

/**
 * 活动福利的查询与领取状态。
 *
 * 从 `CampaignRewards` 里抽出来的：手机端仍然用那个成品组件（它自己调用本 hook），
 * 桌面端则要「行上是摘要、明细收进展开区」，两处必须共用同一份状态与同一套判据，
 * 所以在 hook 里持有，而不是各自抄一遍。
 */
export function useCampaignRewards({ entry, api, t, onChange }: {
  entry: CampaignEntry
  api: CampaignApi
  t: Translate
  onChange?: () => void
}) {
  const now = useDisplayClock()
  const [data, setData] = useState<CampaignPreview | null>(null)
  const [busy, setBusy] = useState('')
  const [error, setError] = useState('')
  const [message, setMessage] = useState('')
  const lock = useRef(false)

  const query = async () => {
    if (lock.current) return
    lock.current = true
    setBusy('query')
    setError('')
    setMessage('')
    try {
      const result = await api.query(entry.id)
      if (!Array.isArray(result?.campaigns)) throw new Error(t('后端未返回活动福利数据'))
      setData(result)
    } catch (error) {
      setData(null)
      setError(error instanceof Error ? error.message : String(error))
    } finally {
      lock.current = false
      setBusy('')
    }
  }

  const claim = async (campaign: CampaignReward) => {
    if (lock.current || !entry.available || currentStatus(campaign) !== 'claimable') return
    lock.current = true
    setBusy(campaign.campaignId)
    setError('')
    setMessage('')
    try {
      const result = await api.claim(entry.id, campaign.campaignId)
      if (!result?.outcome) throw new Error(t('后端未返回活动福利数据'))
      setMessage(outcomeMessage(result.outcome, t))
      setData(previous => previous ? { ...previous, campaigns: previous.campaigns.map(row => {
        if (row.campaignId !== campaign.campaignId) return row
        const status = result.outcome === 'claimed' || result.outcome === 'already_claimed' ? 'claimed'
          : result.outcome === 'confirming' || result.outcome === 'failed' ? 'confirming'
            : result.outcome === 'ineligible' ? 'ineligible' : 'inactive'
        return result.campaign || { ...row, status }
      }) } : previous)
      onChange?.()
    } catch (error) {
      setError(error instanceof Error ? error.message : String(error))
      // 连接断开不能证明领取失败，再次操作先查询上游状态。
      setData(previous => previous ? { ...previous, campaigns: previous.campaigns.map(row =>
        row.campaignId === campaign.campaignId ? { ...row, status: 'confirming' } : row) } : previous)
    } finally {
      lock.current = false
      setBusy('')
    }
  }

  return { data, busy, error, message, now, query, claim }
}

/**
 * 活动清单（两种形态共用的渲染）。
 *
 * `compact` 是桌面端在行的展开区里用的那一档：每一条压成「标题 + 状态徽章 /
 * 说明与时间」两段，字号跟展开区的其它明细对齐；不传则保持手机端的原样式。
 */
function CampaignList({ data, busy, t, locale, formatTime, onClaim, now, compact = false }: {
  data: CampaignPreview
  busy: string
  t: Translate
  locale: string
  formatTime: (value: number) => string
  onClaim: (campaign: CampaignReward) => void
  now: number
  compact?: boolean
}) {
  return <>{data.campaigns.map(campaign => {
    const status = currentStatus(campaign, now)
    const description = locale.startsWith('zh') ? campaign.description : campaign.descriptionEn || campaign.description
    const variant = status === 'claimed' ? 'success' : status === 'claimable' ? 'brand' : status === 'confirming' ? 'warning' : 'outline'
    const amount = typeof campaign.amount === 'number' && Number.isFinite(campaign.amount)
      ? `+${campaign.amount.toLocaleString()} ${campaign.unit ? t(campaign.unit) : 'Credits'}`
      : ''
    const window_ = [
      campaign.startsAt ? t('活动开始：{time}', { time: formatTime(campaign.startsAt) }) : '',
      campaign.endsAt ? t('领取截止：{time}', { time: formatTime(campaign.endsAt) }) : '',
      campaign.expiresAt ? t('额度到期：{time}', { time: formatTime(campaign.expiresAt) })
        : campaign.validDays ? t('领取后 {days} 天有效', { days: campaign.validDays }) : '',
      campaign.claimedAt ? t('领取时间：{time}', { time: formatTime(campaign.claimedAt) }) : '',
    ].filter(Boolean)

    if (compact) {
      return <div key={campaign.campaignId} className='flex items-center gap-3 py-1.5 text-[12.5px]'>
        <div className='min-w-0 flex-1'>
          <div className='flex flex-wrap items-center gap-2'>
            <span className='truncate font-medium'>{titleOf(campaign, locale)}</span>
            <Badge variant={variant} shape='tag'>{statusLabel(status, t)}</Badge>
            {amount ? <span className='text-[11.5px] tabular-nums text-muted-foreground'>{amount}</span> : null}
          </div>
          {description ? <div className='truncate text-[11.5px] text-muted-foreground'>{description}</div> : null}
          {window_.length ? (
            <div className='text-[11.5px] text-muted-foreground'>{window_.join(' · ')}</div>
          ) : null}
        </div>
        <Button size='xs' variant='outline'
          disabled={Boolean(busy) || status !== 'claimable'}
          onClick={() => onClaim(campaign)}>
          {busy === campaign.campaignId ? t('领取中…') : t('领取')}
        </Button>
      </div>
    }

    return <div key={campaign.campaignId} className='border-t pt-3 space-y-2'>
      <div className='flex flex-wrap items-start gap-2'>
        <div className='min-w-0 flex-1 space-y-1'>
          <div className='flex flex-wrap items-center gap-2'>
            <span className='font-medium break-words'>{titleOf(campaign, locale)}</span>
            <Badge variant={variant} shape='tag'>{statusLabel(status, t)}</Badge>
          </div>
          {description ? <p className='text-sm text-muted-foreground break-words'>{description}</p> : null}
        </div>
        {amount ? <span className='text-sm font-medium tabular-nums'>{amount}</span> : null}
        <Button size='sm' variant='outline' disabled={Boolean(busy) || !data || status !== 'claimable'}
          onClick={() => onClaim(campaign)}>
          {busy === campaign.campaignId ? t('领取中…') : t('领取')}
        </Button>
      </div>
      {window_.length ? (
        <div className='flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground'>
          {window_.map(text => <span key={text}>{text}</span>)}
        </div>
      ) : null}
      {status === 'confirming' ? <p className='text-sm text-muted-foreground'>{t('领取结果待确认，请稍后查询；不会重复提交领取')}</p> : null}
    </div>
  })}</>
}

/**
 * 活动福利的完整块（**手机端与旧形态在用**）：说明行 + 查询按钮 + 活动清单 + 权益明细。
 * 桌面端不再用它 —— 那边行上是摘要、清单收进展开区，见 `CampaignList` 的 compact 档。
 */
export function CampaignRewards({ entry, api, t, locale, formatTime, onChange }: {
  entry: CampaignEntry
  api: CampaignApi
  t: Translate
  locale: string
  formatTime: (value: number) => string
  onChange?: () => void
}) {
  const { data, busy, error, message, now, query, claim } = useCampaignRewards({ entry, api, t, onChange })
  const available = entry.available

  return <div className='min-w-0 space-y-3'>
    <div className='flex flex-wrap items-center gap-2'>
      <span className='text-sm text-muted-foreground'>{t('奖励与权益 · 资格、额度和有效期以官方返回为准')}</span>
      <Button size='sm' variant='outline' className='ml-auto' disabled={Boolean(busy) || !available} onClick={() => void query()}>
        {busy === 'query' ? t('查询中…') : t('查询活动')}
      </Button>
    </div>
    {!available ? <p className='text-sm text-muted-foreground'>{t('凭证不可用')}</p> : null}
    {!data && !error ? <p className='text-sm text-muted-foreground'>{t('先查询活动，再选择要领取的奖励；不会购买或订阅套餐')}</p> : null}
    {error ? <p role='alert' className='text-sm text-destructive break-words'>{error}</p> : null}
    {message ? <p role='status' className='text-sm break-words'>{message}</p> : null}
    {data?.riskNote ? <p className='text-sm text-muted-foreground break-words'>
      {t('设备校验未就绪，部分活动可能不显示：{reason}', { reason: data.riskNote })}
    </p> : null}
    {data?.campaigns.length === 0 ? <p className='text-sm text-muted-foreground'>{t('该账号当前没有可领取的限时活动')}</p> : null}
    {data ? <CampaignList data={data} busy={busy} t={t} locale={locale}
      formatTime={formatTime} onClaim={campaign => void claim(campaign)} now={now} /> : null}
    {data ? <CampaignBenefitDetails key={entry.id} entry={entry} data={data} api={api} t={t}
      formatTime={formatTime} onChange={onChange} /> : null}
  </div>
}

/**
 * 桌面端的活动清单与权益明细（放进行的展开区里）：清单走 compact 档。
 * 外层已经把「查询 / 领取」放在行上了，这里只负责画明细。
 */
export function CampaignRewardDetails({ entry, api, t, locale, formatTime, onChange, state }: {
  entry: CampaignEntry
  api: CampaignApi
  t: Translate
  locale: string
  formatTime: (value: number) => string
  onChange?: () => void
  state: ReturnType<typeof useCampaignRewards>
}) {
  const { data, busy, error, message, now, claim } = state
  return <>
    {error ? <p role='alert' className='m-0 text-[12px] text-destructive break-words'>{error}</p> : null}
    {message ? <p role='status' className='m-0 text-[12px] break-words'>{message}</p> : null}
    {data?.riskNote ? <p className='m-0 text-[12px] text-muted-foreground break-words'>
      {t('设备校验未就绪，部分活动可能不显示：{reason}', { reason: data.riskNote })}
    </p> : null}
    {data?.campaigns.length === 0 ? <p className='m-0 text-[12px] text-muted-foreground'>{t('该账号当前没有可领取的限时活动')}</p> : null}
    {!data && !error ? <p className='m-0 text-[12px] text-muted-foreground'>
      {t('先查询活动，再选择要领取的奖励；不会购买或订阅套餐')}
    </p> : null}
    {data ? <CampaignList data={data} busy={busy} t={t} locale={locale}
      formatTime={formatTime} onClaim={campaign => void claim(campaign)} now={now} compact /> : null}
    {data ? <CampaignBenefitDetails key={entry.id} entry={entry} data={data} api={api} t={t}
      formatTime={formatTime} onChange={onChange} /> : null}
  </>
}
