import * as React from 'react'
import type { Branding } from './branding'

export function BrandLogo({ branding, size = 32, className = '' }: { branding: Branding; size?: number; className?: string }) {
  const [failed, setFailed] = React.useState<string | null>(null)
  if (branding.logoUrl && branding.logoUrl !== failed) {
    return <img src={branding.logoUrl} alt='' aria-hidden='true' width={size} height={size}
      className={`block shrink-0 object-contain ${className}`} style={{ width: size, height: size }}
      onError={() => setFailed(branding.logoUrl)} />
  }
  return <svg width={size} height={size} viewBox='0 0 24 24' aria-hidden='true' className={`block shrink-0 ${className}`}>
    <rect width='24' height='24' rx='5.4' fill='#007AFF' />
    <g fill='none' stroke='#fff' strokeWidth='2.2' strokeLinecap='round' strokeLinejoin='round'>
      <path d='M5.23 9.24h10.71' />
      <path d='M15.94 6.72 18.77 9.24 15.94 11.76' />
      <path d='M18.77 14.76H8.06' />
      <path d='M8.06 12.24 5.23 14.76 8.06 17.28' />
    </g>
  </svg>
}
