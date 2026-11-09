export type OfficeaceSubscriptionStatus =
  | 'active' | 'inactive' | 'expired' | 'frozen' | 'unknown' | 'pending'
  | 'credentials_missing' | 'credentials_expired' | 'permission_denied' | 'unsupported'

export type OfficeaceSubscriptionState = {
  status: OfficeaceSubscriptionStatus
  canActivate: boolean
  message?: string
  checkedAt?: number
  autoRenew?: boolean
}

export type OfficeaceSubscriptionResult = {
  status: 'activated' | 'already_active' | 'skipped' | 'pending'
  message?: string
  checkedAt?: number
  autoRenew?: boolean
}

export type OfficeaceSubscriptionApi = {
  query: (id: string) => Promise<OfficeaceSubscriptionState>
  activate: (id: string, confirmAgreements: boolean) => Promise<OfficeaceSubscriptionResult>
}
