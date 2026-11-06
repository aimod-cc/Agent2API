import { CampaignRewards } from '../../../ui-shared/checkin-campaign-rewards'
import type { BenefitRedemptionResult, CampaignApi, CampaignClaimResult, CampaignEntry, CampaignPreview } from '../../../ui-shared/checkin-campaign-types'
import { ProviderLogo } from './checkin-row'
import { formatTime } from './checkin-types'
import { loadCenter } from './checkin-store'
import { t } from '../i18n'

type CampaignBridge = {
  getCampaignRewards?: (id: string) => Promise<CampaignPreview>
  claimCampaignReward?: (id: string, campaignId: string) => Promise<CampaignClaimResult>
  redeemCampaignBenefit?: (id: string, code: string) => Promise<BenefitRedemptionResult>
}

const api: CampaignApi = {
  query(id) {
    const bridge = (window as unknown as { workbuddyDesktop?: CampaignBridge }).workbuddyDesktop
    if (!bridge?.getCampaignRewards) return Promise.reject(new Error(t('活动福利接口不可用，请更新网关后重试')))
    return bridge.getCampaignRewards(id)
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

export function CampaignRow({ row }: { row: CampaignEntry }) {
  return <div className='border-t px-4 py-4 first:border-t-0'>
    <div className='mb-3 flex items-center gap-2'>
      <ProviderLogo id={row.provider} label={row.provider} />
      <span className='font-medium'>{row.name || row.id}</span>
    </div>
    <CampaignRewards entry={row} api={api} t={t} locale={window.wbI18n?.locale() || 'zh-Hans'}
      formatTime={formatTime} onChange={() => void loadCenter()} />
  </div>
}
