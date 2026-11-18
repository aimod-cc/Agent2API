import { useAdvertisements } from '../../../ui-shared/advertisements'
import { RemoteSponsorCard } from '../../../ui-shared/remote-sponsor-card'
import { openExternal } from '../lib/community'
import { t } from '../i18n'

export function ProviderSponsorCards() {
  const ads = useAdvertisements().filter(ad => ad.category === 'provider')
  if (!ads.length) return null
  return <div className='sponsor-proxy-cards'>{ads.map(ad => <RemoteSponsorCard key={ad.id} ad={ad} t={t} openExternal={openExternal} />)}</div>
}
