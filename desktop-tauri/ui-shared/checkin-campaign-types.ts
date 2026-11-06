export type CampaignEntry = {
  id: string
  name: string
  provider: string
  available: boolean
}

export type CampaignStatus = 'claimable' | 'claimed' | 'inactive' | 'ineligible' | 'confirming'

export type CampaignReward = {
  campaignId: string
  title: string
  titleEn: string
  description: string
  descriptionEn: string
  amount: number | null
  unit?: string
  status: CampaignStatus
  startsAt: number | null
  endsAt: number | null
  claimedAt: number | null
  expiresAt: number | null
  validDays: number | null
}

export type MonthlyGift = {
  id: string
  orderId: string
  period: number
  amount: number | null
  scheduledAt: number | null
  grantedAt: number | null
  status: 'pending' | 'granted' | 'failed' | 'cancelled' | 'processing' | 'unknown'
}

export type CampaignPreview = {
  campaigns: CampaignReward[]
  riskNote: string | null
  notice?: string | null
  monthlyGifts?: MonthlyGift[] | null
  redemption?: { supported: boolean }
}

export type BenefitRedemptionResult = {
  outcome: 'claimed' | 'already_claimed' | 'confirming' | 'failed'
  message?: string
}

export type CampaignClaimResult = {
  campaignId: string
  outcome: 'claimed' | 'already_claimed' | 'inactive' | 'ineligible' | 'confirming' | 'failed'
  campaign: CampaignReward | null
}

export type CampaignApi = {
  query(id: string): Promise<CampaignPreview>
  claim(id: string, campaignId: string): Promise<CampaignClaimResult>
  redeem?(id: string, code: string): Promise<BenefitRedemptionResult>
}
