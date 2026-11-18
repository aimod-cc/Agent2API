import * as React from 'react'
import { advertisementCopy, type Advertisement } from './advertisements'
import { SponsorCard } from './sponsor-card'
import { useImageFallback } from './use-image-fallback'

function AdvertisementLogo({ ad }: { ad: Advertisement }) {
  const light = useImageFallback(ad.imageUrl)
  const dark = useImageFallback(ad.imageDarkUrl, ad.imageUrl)
  if (!light.src && !dark.src) return <span aria-hidden='true'>{advertisementCopy(ad).title.trim().slice(0, 1)}</span>
  return <picture>
    {light.src ? <img className='sponsor-wordmark sponsor-logo-light' src={light.src} alt='' referrerPolicy='no-referrer' onError={light.onError} /> : null}
    {dark.src ? <img className='sponsor-wordmark sponsor-logo-dark' src={dark.src} alt='' referrerPolicy='no-referrer' onError={dark.onError} /> : null}
  </picture>
}

export function RemoteSponsorCard({ ad, t, openExternal, compact = false, action, className = '' }: {
  ad: Advertisement
  t: (key: string) => string
  openExternal: (url: string) => void | Promise<void>
  compact?: boolean
  action?: { label: string; onClick: () => void }
  className?: string
}) {
  const copy = advertisementCopy(ad)
  return <SponsorCard t={t} title={copy.title} description={copy.description} note={copy.note}
    logo={<AdvertisementLogo ad={ad} />} tone={ad.tone} tags={compact ? [] : copy.tags}
    url={ad.url} linkLabel={copy.linkLabel} openExternal={openExternal} action={action} className={className} />
}
