import { useState } from 'react'
import { resolveUrl } from '../core/client'
import { providerDisplayName } from '../core/format'
import { useStore } from '../core/store'

export type IconName =
  | 'dashboard'
  | 'accounts'
  | 'forwarding'
  | 'chat'
  | 'settings'
  | 'server'
  | 'refresh'
  | 'plus'
  | 'check'
  | 'close'
  | 'search'
  | 'copy'
  | 'chevron-right'
  | 'chevron-down'
  | 'spark'
  | 'key'
  | 'route'
  | 'cube'
  | 'calendar'
  | 'shield'
  | 'trash'
  | 'edit'
  | 'send'
  | 'stop'
  | 'history'
  | 'terminal'
  | 'activity'
  | 'sliders'
  | 'globe'
  | 'moon'
  | 'logout'
  | 'bolt'
  | 'alert'
  | 'eye'
  | 'eye-off'
  | 'link'
  | 'database'
  | 'arrow-left'
  | 'arrow-up'
  | 'arrow-down'
  | 'filter'
  | 'gift'
  | 'clock'
  | 'repeat'
  | 'bell'
  | 'dollar'

export function GlyphIcon({
  name,
  size = 20,
  className,
}: {
  name: IconName
  size?: number
  className?: string
}) {
  const s = { width: size, height: size, flexShrink: 0 as const }
  const common = {
    viewBox: '0 0 24 24',
    fill: 'none',
    stroke: 'currentColor',
    strokeWidth: 2,
    strokeLinecap: 'round' as const,
    strokeLinejoin: 'round' as const,
    style: s,
    className,
  }

  switch (name) {
    case 'dashboard':
      return (
        <svg {...common}>
          <rect x='3' y='3' width='7' height='9' rx='2' />
          <rect x='14' y='3' width='7' height='5' rx='2' />
          <rect x='14' y='12' width='7' height='9' rx='2' />
          <rect x='3' y='16' width='7' height='5' rx='2' />
        </svg>
      )
    case 'accounts':
      return (
        <svg {...common}>
          <path d='M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2' />
          <circle cx='9' cy='7' r='4' />
          <path d='M22 21v-2a4 4 0 0 0-3-3.87' />
          <path d='M16 3.13a4 4 0 0 1 0 7.75' />
        </svg>
      )
    case 'forwarding':
      return (
        <svg {...common}>
          <circle cx='6' cy='6' r='3' />
          <circle cx='18' cy='6' r='3' />
          <circle cx='12' cy='18' r='3' />
          <path d='M8.5 7.5 10.5 15.5' />
          <path d='M15.5 7.5 13.5 15.5' />
          <path d='M9 6h6' />
        </svg>
      )
    case 'chat':
      return (
        <svg {...common}>
          <path d='M21 11.5a8.38 8.38 0 0 1-.9 3.8 8.5 8.5 0 0 1-7.6 4.7 8.38 8.38 0 0 1-3.8-.9L3 21l1.9-5.7a8.38 8.38 0 0 1-.9-3.8 8.5 8.5 0 0 1 4.7-7.6 8.38 8.38 0 0 1 3.8-.9h.5a8.48 8.48 0 0 1 8 8v.5z' />
        </svg>
      )
    case 'settings':
      return (
        <svg {...common}>
          <circle cx='12' cy='12' r='3' />
          <path d='M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z' />
        </svg>
      )
    case 'server':
      return (
        <svg {...common}>
          <rect x='2' y='3' width='20' height='7' rx='2' />
          <rect x='2' y='14' width='20' height='7' rx='2' />
          <line x1='6' y1='6.5' x2='6.01' y2='6.5' />
          <line x1='6' y1='17.5' x2='6.01' y2='17.5' />
        </svg>
      )
    case 'refresh':
      return (
        <svg {...common}>
          <path d='M21 12a9 9 0 1 1-2.64-6.36' />
          <polyline points='21 3 21 9 15 9' />
        </svg>
      )
    case 'plus':
      return (
        <svg {...common}>
          <line x1='12' y1='5' x2='12' y2='19' />
          <line x1='5' y1='12' x2='19' y2='12' />
        </svg>
      )
    case 'check':
      return (
        <svg {...common}>
          <polyline points='20 6 9 17 4 12' />
        </svg>
      )
    case 'close':
      return (
        <svg {...common}>
          <line x1='18' y1='6' x2='6' y2='18' />
          <line x1='6' y1='6' x2='18' y2='18' />
        </svg>
      )
    case 'search':
      return (
        <svg {...common}>
          <circle cx='11' cy='11' r='7' />
          <line x1='21' y1='21' x2='16.65' y2='16.65' />
        </svg>
      )
    case 'copy':
      return (
        <svg {...common}>
          <rect x='9' y='9' width='13' height='13' rx='2' />
          <path d='M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1' />
        </svg>
      )
    case 'chevron-right':
      return (
        <svg {...common}>
          <polyline points='9 18 15 12 9 6' />
        </svg>
      )
    case 'chevron-down':
      return (
        <svg {...common}>
          <polyline points='6 9 12 15 18 9' />
        </svg>
      )
    case 'spark':
      return (
        <svg {...common}>
          <path d='M12 3l1.9 5.8a2 2 0 0 0 1.3 1.3L21 12l-5.8 1.9a2 2 0 0 0-1.3 1.3L12 21l-1.9-5.8a2 2 0 0 0-1.3-1.3L3 12l5.8-1.9a2 2 0 0 0 1.3-1.3L12 3z' />
        </svg>
      )
    case 'key':
      return (
        <svg {...common}>
          <circle cx='7.5' cy='15.5' r='5.5' />
          <path d='m21 2-9.6 9.6' />
          <path d='m15.5 7.5 3 3L22 7l-3-3' />
        </svg>
      )
    case 'route':
      return (
        <svg {...common}>
          <circle cx='6' cy='19' r='3' />
          <path d='M9 19h8.5a3.5 3.5 0 0 0 0-7h-11a3.5 3.5 0 0 1 0-7H15' />
          <circle cx='18' cy='5' r='3' />
        </svg>
      )
    case 'cube':
      return (
        <svg {...common}>
          <path d='M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16z' />
          <polyline points='3.27 6.96 12 12.01 20.73 6.96' />
          <line x1='12' y1='22.08' x2='12' y2='12' />
        </svg>
      )
    case 'calendar':
      return (
        <svg {...common}>
          <rect x='3' y='4' width='18' height='18' rx='2' />
          <line x1='16' y1='2' x2='16' y2='6' />
          <line x1='8' y1='2' x2='8' y2='6' />
          <line x1='3' y1='10' x2='21' y2='10' />
          <path d='m9 16 2 2 4-4' />
        </svg>
      )
    case 'shield':
      return (
        <svg {...common}>
          <path d='M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z' />
        </svg>
      )
    case 'trash':
      return (
        <svg {...common}>
          <polyline points='3 6 5 6 21 6' />
          <path d='M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2' />
        </svg>
      )
    case 'edit':
      return (
        <svg {...common}>
          <path d='M12 20h9' />
          <path d='M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z' />
        </svg>
      )
    case 'send':
      return (
        <svg {...common}>
          <line x1='22' y1='2' x2='11' y2='13' />
          <polygon points='22 2 15 22 11 13 2 9 22 2' />
        </svg>
      )
    case 'stop':
      return (
        <svg {...common}>
          <rect x='6' y='6' width='12' height='12' rx='2' fill='currentColor' />
        </svg>
      )
    case 'history':
      return (
        <svg {...common}>
          <path d='M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8' />
          <path d='M3 3v5h5' />
          <path d='M12 7v5l4 2' />
        </svg>
      )
    case 'terminal':
      return (
        <svg {...common}>
          <polyline points='4 17 10 11 4 5' />
          <line x1='12' y1='19' x2='20' y2='19' />
        </svg>
      )
    case 'activity':
      return (
        <svg {...common}>
          <polyline points='22 12 18 12 15 21 9 3 6 12 2 12' />
        </svg>
      )
    case 'sliders':
      return (
        <svg {...common}>
          <line x1='4' y1='21' x2='4' y2='14' />
          <line x1='4' y1='10' x2='4' y2='3' />
          <line x1='12' y1='21' x2='12' y2='12' />
          <line x1='12' y1='8' x2='12' y2='3' />
          <line x1='20' y1='21' x2='20' y2='16' />
          <line x1='20' y1='12' x2='20' y2='3' />
          <line x1='1' y1='14' x2='7' y2='14' />
          <line x1='9' y1='8' x2='15' y2='8' />
          <line x1='17' y1='16' x2='23' y2='16' />
        </svg>
      )
    case 'globe':
      return (
        <svg {...common}>
          <circle cx='12' cy='12' r='10' />
          <line x1='2' y1='12' x2='22' y2='12' />
          <path d='M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z' />
        </svg>
      )
    case 'moon':
      return (
        <svg {...common}>
          <path d='M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z' />
        </svg>
      )
    case 'logout':
      return (
        <svg {...common}>
          <path d='M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4' />
          <polyline points='16 17 21 12 16 7' />
          <line x1='21' y1='12' x2='9' y2='12' />
        </svg>
      )
    case 'bolt':
      return (
        <svg {...common}>
          <polygon points='13 2 3 14 12 14 11 22 21 10 12 10 13 2' />
        </svg>
      )
    case 'alert':
      return (
        <svg {...common}>
          <circle cx='12' cy='12' r='10' />
          <line x1='12' y1='8' x2='12' y2='12' />
          <line x1='12' y1='16' x2='12.01' y2='16' />
        </svg>
      )
    case 'eye':
      return (
        <svg {...common}>
          <path d='M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z' />
          <circle cx='12' cy='12' r='3' />
        </svg>
      )
    case 'eye-off':
      return (
        <svg {...common}>
          <path d='M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24' />
          <line x1='1' y1='1' x2='23' y2='23' />
        </svg>
      )
    case 'link':
      return (
        <svg {...common}>
          <path d='M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71' />
          <path d='M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71' />
        </svg>
      )
    case 'database':
      return (
        <svg {...common}>
          <ellipse cx='12' cy='5' rx='9' ry='3' />
          <path d='M21 12c0 1.66-4 3-9 3s-9-1.34-9-3' />
          <path d='M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5' />
        </svg>
      )
    case 'arrow-left':
      return (
        <svg {...common}>
          <path d='M19 12H5' />
          <path d='M11 6l-6 6 6 6' />
        </svg>
      )
    case 'arrow-up':
      return (
        <svg {...common}>
          <path d='M12 19V5' />
          <path d='M6 11l6-6 6 6' />
        </svg>
      )
    case 'arrow-down':
      return (
        <svg {...common}>
          <path d='M12 5v14' />
          <path d='M6 13l6 6 6-6' />
        </svg>
      )
    case 'filter':
      return (
        <svg {...common}>
          <path d='M3.5 5.5h17l-6.5 7.6v5.2l-4 1.6v-6.8z' />
        </svg>
      )
    case 'gift':
      return (
        <svg {...common}>
          <rect x='3.5' y='9' width='17' height='4' rx='1' />
          <path d='M5 13v7a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-7M12 9v12' />
          <path d='M12 9H8.5a2.3 2.3 0 1 1 0-4.6C10.8 4.4 12 9 12 9zm0 0h3.5a2.3 2.3 0 1 0 0-4.6C13.2 4.4 12 9 12 9z' />
        </svg>
      )
    case 'clock':
      return (
        <svg {...common}>
          <circle cx='12' cy='12' r='8.5' />
          <path d='M12 7.5V12l3.2 2' />
        </svg>
      )
    case 'repeat':
      return (
        <svg {...common}>
          <path d='m17 2 3.5 3.5L17 9' />
          <path d='M4 11V9a3.5 3.5 0 0 1 3.5-3.5h13' />
          <path d='m7 22-3.5-3.5L7 15' />
          <path d='M20 13v2a3.5 3.5 0 0 1-3.5 3.5h-13' />
        </svg>
      )
    case 'bell':
      return (
        <svg {...common}>
          <path d='M18 8A6 6 0 0 0 6 8c0 7-3 9-3 9h18s-3-2-3-9' />
          <path d='M13.73 21a2 2 0 0 1-3.46 0' />
        </svg>
      )
    case 'dollar':
      return (
        <svg {...common}>
          <line x1='12' y1='1' x2='12' y2='23' />
          <path d='M17 5H9.5a3.5 3.5 0 0 0 0 7h5a3.5 3.5 0 0 1 0 7H6' />
        </svg>
      )
  }
}

export function ProviderAvatar({
  provider,
  label,
  size = 36,
}: {
  provider?: string | null
  label?: string | null
  size?: number
}) {
  const { session } = useStore()
  const [failed, setFailed] = useState(false)
  const key = (provider || '').trim().toLowerCase()
  const display = providerDisplayName(provider, label)
  const monogram = (display || 'A').slice(0, 1).toUpperCase()

  const map = session?.providerIcons || session?.accounts?.providerIcons
  const rawIcon = key && map?.[key] ? map[key] : ''
  let src = ''
  if (rawIcon) {
    src = rawIcon.startsWith('data:') || rawIcon.startsWith('http') ? rawIcon : resolveUrl(rawIcon)
  }

  return (
    <span
      style={{
        width: size,
        height: size,
        borderRadius: Math.round(size * 0.34),
        background: 'var(--fc-inset)',
        color: 'var(--fc-primary)',
        display: 'inline-flex',
        alignItems: 'center',
        justifyContent: 'center',
        fontWeight: 700,
        fontSize: Math.round(size * 0.42),
        flexShrink: 0,
        overflow: 'hidden',
      }}
    >
      {src && !failed ? (
        <img
          src={src}
          alt={display}
          style={{ width: Math.round(size * 0.62), height: Math.round(size * 0.62), objectFit: 'contain' }}
          onError={() => setFailed(true)}
        />
      ) : (
        monogram
      )}
    </span>
  )
}
