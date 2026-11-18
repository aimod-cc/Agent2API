import { useAdvertisements } from './advertisements'
import { RemoteSponsorCard } from './remote-sponsor-card'

export function ProxySponsorCards({ t, openExternal, compact = false, className = '' }: {
  t: (key: string) => string
  openExternal: (url: string) => void | Promise<void>
  compact?: boolean
  className?: string
}) {
  const items = useAdvertisements().filter(ad => ad.category === 'proxy')
  if (!items.length) return null
  return <div className={`sponsor-proxy-cards${compact ? ' sponsor-proxy-cards--compact' : ''} ${className}`}>
    {items.map(ad => <RemoteSponsorCard key={ad.id} ad={ad} t={t} compact={compact} openExternal={openExternal} />)}
  </div>
}
