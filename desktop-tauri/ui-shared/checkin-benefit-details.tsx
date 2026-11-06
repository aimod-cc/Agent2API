import { useId, useRef, useState } from 'react'
import { Badge, Button, Input, Label } from '@ui'
import type { CampaignApi, CampaignEntry, CampaignPreview, MonthlyGift } from './checkin-campaign-types'

type Translate = (key: string, params?: Record<string, string | number>) => string

function giftStatus(status: MonthlyGift['status'], t: Translate): string {
  switch (status) {
    case 'pending': return t('待发放')
    case 'granted': return t('已发放')
    case 'failed': return t('发放失败')
    case 'cancelled': return t('已取消')
    case 'processing': return t('发放中')
    default: return t('状态未知')
  }
}

export function CampaignBenefitDetails({ entry, data, api, t, formatTime, onChange }: {
  entry: CampaignEntry
  data: CampaignPreview
  api: CampaignApi
  t: Translate
  formatTime: (value: number) => string
  onChange?: () => void
}) {
  const inputId = useId()
  const [code, setCode] = useState('')
  const [confirming, setConfirming] = useState(false)
  const [busy, setBusy] = useState(false)
  const [message, setMessage] = useState('')
  const [error, setError] = useState('')
  const lock = useRef(false)

  const redeem = async () => {
    if (lock.current || !api.redeem || !code.trim()) return
    lock.current = true
    setBusy(true)
    setError('')
    setMessage('')
    try {
      const result = await api.redeem(entry.id, code.trim())
      if (!result?.outcome) throw new Error(t('后端未返回权益兑换结果'))
      setMessage(result.outcome === 'claimed' ? t('权益兑换成功，重新查询可查看最新记录')
        : result.outcome === 'already_claimed' ? t('该权益已兑换，无需重复提交')
          : result.outcome === 'confirming' ? t('兑换结果待确认，请到官方客户端核对；不会重复提交')
            : result.message || t('权益兑换未成功'))
      onChange?.()
    } catch {
      setError(t('兑换请求未得到确认，请到官方客户端核对；不要重复提交'))
    } finally {
      setCode('')
      setConfirming(false)
      setBusy(false)
      lock.current = false
    }
  }

  return <div className='space-y-4'>
    {data.notice ? <p className='text-sm text-muted-foreground break-words'>{data.notice}</p> : null}
    {data.redemption?.supported && api.redeem ? <div className='space-y-2 border-t pt-3'>
      <Label htmlFor={inputId}>{t('权益兑换码 / 订单号')}</Label>
      <Input id={inputId} type='password' autoComplete='new-password' spellCheck={false} maxLength={128}
        value={code} disabled={busy} placeholder={t('填写你已有的兑换码或订单号')}
        onChange={event => { setCode(event.currentTarget.value); setConfirming(false) }} />
      <p className='text-xs text-muted-foreground'>{t('仅提交你填写的权益，不购买套餐；兑换码激活可能需要原客户端的设备标识。')}</p>
      {confirming ? <div className='space-y-2'>
        <p className='text-sm'>{t('确认把这份权益兑换到当前账号？兑换后可能无法撤销。')}</p>
        <div className='flex gap-2'>
          <Button size='sm' disabled={busy || !entry.available} onClick={() => void redeem()}>
            {busy ? t('兑换中…') : t('确认兑换')}
          </Button>
          <Button size='sm' variant='outline' disabled={busy} onClick={() => setConfirming(false)}>{t('取消')}</Button>
        </div>
      </div> : <Button size='sm' variant='outline' disabled={!code.trim() || !entry.available}
        onClick={() => setConfirming(true)}>{t('兑换权益')}</Button>}
      {message ? <p role='status' className='text-sm break-words'>{message}</p> : null}
      {error ? <p role='alert' className='text-sm text-destructive'>{error}</p> : null}
    </div> : null}
    {Array.isArray(data.monthlyGifts) ? <div className='space-y-2 border-t pt-3'>
      <h4 className='text-sm font-medium'>{t('月度赠送')}</h4>
      <p className='text-xs text-muted-foreground'>{t('由官方按期自动发放，这里只展示记录，不重复领取。')}</p>
      {data.monthlyGifts.length === 0 ? <p className='text-sm text-muted-foreground'>{t('暂无月度赠送记录')}</p> : null}
      {data.monthlyGifts.map(gift => <div key={gift.id} className='flex flex-wrap items-center gap-x-3 gap-y-1 text-sm'>
        <span className='min-w-0 flex-1 break-words'>{t('订单 {order} · 第 {period} 期', { order: gift.orderId, period: gift.period })}</span>
        {gift.amount !== null ? <span className='tabular-nums'>{t('{points} 积分', { points: gift.amount })}</span> : null}
        <Badge variant={gift.status === 'granted' ? 'success' : 'outline'} shape='tag'>{giftStatus(gift.status, t)}</Badge>
        {gift.scheduledAt ? <span className='w-full text-xs text-muted-foreground'>
          {t('预计发放：{time}', { time: formatTime(gift.scheduledAt) })}
        </span> : null}
        {gift.grantedAt ? <span className='w-full text-xs text-muted-foreground'>
          {t('实际发放：{time}', { time: formatTime(gift.grantedAt) })}
        </span> : null}
      </div>)}
    </div> : null}
  </div>
}
