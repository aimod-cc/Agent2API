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

export function CampaignRewards({ entry, api, t, locale, formatTime, onChange }: {
  entry: CampaignEntry
  api: CampaignApi
  t: Translate
  locale: string
  formatTime: (value: number) => string
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
    if (lock.current || currentStatus(campaign) !== 'claimable') return
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
        return result.campaign || { ...row, status: 'inactive' }
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

  return <div className='min-w-0 space-y-3'>
    <div className='flex flex-wrap items-center gap-2'>
      <span className='text-sm text-muted-foreground'>{t('奖励与权益 · 资格、额度和有效期以官方返回为准')}</span>
      <Button size='sm' variant='outline' className='ml-auto' disabled={Boolean(busy) || !entry.available} onClick={() => void query()}>
        {busy === 'query' ? t('查询中…') : t('查询活动')}
      </Button>
    </div>
    {!entry.available ? <p className='text-sm text-muted-foreground'>{t('凭证不可用')}</p> : null}
    {!data && !error ? <p className='text-sm text-muted-foreground'>{t('先查询活动，再选择要领取的奖励；不会购买或订阅套餐')}</p> : null}
    {error ? <p role='alert' className='text-sm text-destructive break-words'>{error}</p> : null}
    {message ? <p role='status' className='text-sm break-words'>{message}</p> : null}
    {data?.riskNote ? <p className='text-sm text-muted-foreground break-words'>
      {t('设备校验未就绪，部分活动可能不显示：{reason}', { reason: data.riskNote })}
    </p> : null}
    {data?.campaigns.length === 0 ? <p className='text-sm text-muted-foreground'>{t('该账号当前没有可领取的限时活动')}</p> : null}
    {data?.campaigns.map(campaign => {
      const status = currentStatus(campaign, now)
      const description = locale.startsWith('zh') ? campaign.description : campaign.descriptionEn || campaign.description
      const variant = status === 'claimed' ? 'success' : status === 'claimable' ? 'brand' : status === 'confirming' ? 'warning' : 'outline'
      return <div key={campaign.campaignId} className='border-t pt-3 space-y-2'>
        <div className='flex flex-wrap items-start gap-2'>
          <div className='min-w-0 flex-1 space-y-1'>
            <div className='flex flex-wrap items-center gap-2'>
              <span className='font-medium break-words'>{titleOf(campaign, locale)}</span>
              <Badge variant={variant} shape='tag'>{statusLabel(status, t)}</Badge>
            </div>
            {description ? <p className='text-sm text-muted-foreground break-words'>{description}</p> : null}
          </div>
          {typeof campaign.amount === 'number' && Number.isFinite(campaign.amount) ? <span className='text-sm font-medium tabular-nums'>+{campaign.amount.toLocaleString()} {campaign.unit ? t(campaign.unit) : 'Credits'}</span> : null}
          <Button size='sm' variant='outline' disabled={Boolean(busy) || !entry.available || status !== 'claimable'} onClick={() => void claim(campaign)}>
            {busy === campaign.campaignId ? t('领取中…') : t('领取')}
          </Button>
        </div>
        <div className='flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground'>
          {campaign.startsAt ? <span>{t('活动开始：{time}', { time: formatTime(campaign.startsAt) })}</span> : null}
          {campaign.endsAt ? <span>{t('领取截止：{time}', { time: formatTime(campaign.endsAt) })}</span> : null}
          {campaign.expiresAt ? <span>{t('额度到期：{time}', { time: formatTime(campaign.expiresAt) })}</span>
            : campaign.validDays ? <span>{t('领取后 {days} 天有效', { days: campaign.validDays })}</span> : null}
          {campaign.claimedAt ? <span>{t('领取时间：{time}', { time: formatTime(campaign.claimedAt) })}</span> : null}
        </div>
        {status === 'confirming' ? <p className='text-sm text-muted-foreground'>{t('领取结果待确认，请稍后查询；不会重复提交领取')}</p> : null}
      </div>
    })}
    {data ? <CampaignBenefitDetails key={entry.id} entry={entry} data={data} api={api} t={t}
      formatTime={formatTime} onChange={onChange} /> : null}
  </div>
}
