import type { SVGProps } from 'react'

/**
 * 手机端图标集。
 *
 * 与 `ui/icons.js` 同风格（24×24 画布、currentColor、描边 1.8），但只挑手机端
 * 真正用到的那批：桌面端那套有 60 多个图标，移动端菜单只有 5+9 个入口，
 * 全量搬过来只会让 bundle 白胖一圈。
 *
 * 用法：`<Icon name='home' />`，尺寸由外层控制（默认 24）。
 */

const PATHS: Record<string, string> = {
  home: '<path d="M4 10.5 12 4l8 6.5V19a1.5 1.5 0 0 1-1.5 1.5h-4V15h-5v5.5h-4A1.5 1.5 0 0 1 4 19z"/>',
  chart: '<path d="M4 20V10M10 20V4M16 20v-7M22 20H2"/>',
  users:
    '<circle cx="9" cy="8" r="3.2"/><path d="M2.8 19.5c0-3.2 2.8-5.2 6.2-5.2s6.2 2 6.2 5.2"/><path d="M16.2 5.4a3 3 0 0 1 0 5.4M17.5 14.6c2.3.4 3.9 2.1 3.9 4.9"/>',
  grid:
    '<rect x="3.5" y="3.5" width="7" height="7" rx="2"/><rect x="13.5" y="3.5" width="7" height="7" rx="2"/><rect x="3.5" y="13.5" width="7" height="7" rx="2"/><rect x="13.5" y="13.5" width="7" height="7" rx="2"/>',
  back: '<path d="M14.5 5.5 8 12l6.5 6.5"/>',
  chev: '<path d="M9.5 5.5 16 12l-6.5 6.5"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  search: '<circle cx="11" cy="11" r="6.2"/><path d="m16 16 4.2 4.2"/>',
  close: '<path d="m6 6 12 12M18 6 6 18"/>',
  check: '<path d="m4.5 12.5 5 5 10-11"/>',
  copy:
    '<rect x="9" y="9" width="11" height="11" rx="2.6"/><path d="M15 5.6A2.6 2.6 0 0 0 12.4 3H6.6A2.6 2.6 0 0 0 4 5.6v5.8A2.6 2.6 0 0 0 6.6 14"/>',
  refresh: '<path d="M20 12a8 8 0 1 1-2.6-5.9"/><path d="M20 4v4.5h-4.5"/>',
  play: '<path d="M8 5.2c0-.9 1-1.5 1.8-1l9.4 6.1c.8.5.8 1.7 0 2.2L9.8 18.6c-.8.5-1.8-.1-1.8-1z"/>',
  bell: '<path d="M18 15.5V10a6 6 0 1 0-12 0v5.5L4.2 18h15.6z"/><path d="M10 21h4"/>',
  settings:
    '<circle cx="12" cy="12" r="3.1"/><path d="M19.4 14.4a1.6 1.6 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.6 1.6 0 0 0-2.7 1.1v.3a2 2 0 1 1-4 0v-.2a1.6 1.6 0 0 0-2.8-1.1l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.6 1.6 0 0 0-1.1-2.7H3a2 2 0 1 1 0-4h.2A1.6 1.6 0 0 0 4.3 7l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.6 1.6 0 0 0 2.7-1.1V3a2 2 0 1 1 4 0v.2a1.6 1.6 0 0 0 2.7 1.1l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.6 1.6 0 0 0 1.1 2.7h.3a2 2 0 1 1 0 4h-.2a1.6 1.6 0 0 0-1.5 1z"/>',
  spark:
    '<path d="M12 3.2 13.8 9l5.8 1.8-5.8 1.8L12 18.4l-1.8-5.8L4.4 10.8 10.2 9z"/><path d="M18.5 16.5l.7 2.2 2.2.7-2.2.7-.7 2.2-.7-2.2-2.2-.7 2.2-.7z"/>',
  checkin:
    '<rect x="3.4" y="5" width="17.2" height="15.6" rx="3"/><path d="M8 3v4M16 3v4M3.4 10h17.2"/><path d="m8.8 14.6 2.2 2.2 4-4.4"/>',
  model:
    '<path d="M12 3.4 20 7.7v8.6L12 20.6 4 16.3V7.7z"/><path d="m4 7.7 8 4.3 8-4.3M12 12v8.6"/>',
  proxy:
    '<circle cx="12" cy="12" r="8.4"/><path d="M3.6 12h16.8M12 3.6c2.2 2.3 3.4 5.3 3.4 8.4S14.2 18.1 12 20.4C9.8 18.1 8.6 15.1 8.6 12S9.8 5.9 12 3.6z"/>',
  key: '<circle cx="8" cy="12" r="4"/><path d="M12 12h8.5M17 12v3.2M20 12v2.4"/>',
  doc: '<path d="M6 3.6h7.4L19 9.2v11.2H6z"/><path d="M13 3.6v5.8h5.6M9 13h6M9 16.6h4"/>',
  log: '<path d="M4 6h16M4 12h11M4 18h7"/>',
  request:
    '<path d="M4 8.4h13.6M14.6 5.4 18 8.4l-3.4 3"/><path d="M20 15.6H6.4M9.4 12.6 6 15.6l3.4 3"/>',
  task: '<circle cx="12" cy="12" r="8.4"/><path d="M12 7.4V12l3.2 2"/>',
  agent:
    '<rect x="4" y="7.4" width="16" height="12" rx="3.4"/><path d="M12 3.4v4M8.6 12.6v1.6M15.4 12.6v1.6"/>',
  wallet:
    '<rect x="3.4" y="6" width="17.2" height="13" rx="3.2"/><path d="M3.4 10h17.2M16.6 14.6h1.6"/>',
  shield:
    '<path d="M12 3.4 19.4 6v6.2c0 4.2-3 7.3-7.4 8.4-4.4-1.1-7.4-4.2-7.4-8.4V6z"/><path d="m9 12 2.2 2.2 4-4.2"/>',
  download: '<path d="M12 4v10.4M8 11l4 4 4-4"/><path d="M4.5 18.4v1.2h15v-1.2"/>',
  trash:
    '<path d="M5 7h14M9.5 7V5.2h5V7M7 7l.9 12.4h8.2L17 7"/><path d="M10.5 10.5v5.5M13.5 10.5v5.5"/>',
  edit: '<path d="M15.6 4.6 19.4 8.4 9 18.8l-4.6.8.8-4.6z"/><path d="m13.8 6.4 3.8 3.8"/>',
  filter: '<path d="M4 6h16M7 12h10M10 18h4"/>',
  link:
    '<path d="M10.5 13.5a3.6 3.6 0 0 0 5.1 0l2.6-2.6a3.6 3.6 0 0 0-5.1-5.1l-1.4 1.4"/><path d="M13.5 10.5a3.6 3.6 0 0 0-5.1 0l-2.6 2.6a3.6 3.6 0 0 0 5.1 5.1l1.4-1.4"/>',
  qr:
    '<rect x="4" y="4" width="6" height="6" rx="1.4"/><rect x="14" y="4" width="6" height="6" rx="1.4"/><rect x="4" y="14" width="6" height="6" rx="1.4"/><path d="M14 14h2.5v2.5H14zM19.5 14v6M14 19.5h2.5"/>',
  moon: '<path d="M20 14.6A8.4 8.4 0 0 1 9.4 4 8.4 8.4 0 1 0 20 14.6z"/>',
  info: '<circle cx="12" cy="12" r="8.4"/><path d="M12 11v5.2M12 7.8v.2"/>',
  clock: '<circle cx="12" cy="12" r="8.4"/><path d="M12 7.6V12l3 1.8"/>',
  up: '<path d="M12 19V5M6.5 10.5 12 5l5.5 5.5"/>',
  bolt: '<path d="M13.4 3 5.6 13.4h5.2L10.2 21l8.2-10.6h-5.4z"/>',
  globe: '<circle cx="12" cy="12" r="8.4"/><path d="M3.6 12h16.8"/>',
  eye:
    '<path d="M2.6 12S6 5.8 12 5.8 21.4 12 21.4 12 18 18.2 12 18.2 2.6 12 2.6 12z"/><circle cx="12" cy="12" r="2.8"/>',
  heart:
    '<path d="M12 20s-7.4-4.4-7.4-9.4A4.2 4.2 0 0 1 12 7.8a4.2 4.2 0 0 1 7.4 2.8c0 5-7.4 9.4-7.4 9.4z"/>',
  gift:
    '<rect x="3.6" y="8.4" width="16.8" height="4" rx="1.4"/><path d="M5.4 12.4v7.2h13.2v-7.2M12 8.4v11.2"/><path d="M12 8.4C10.4 8.4 8 7.6 8 5.8A2 2 0 0 1 12 4.6a2 2 0 0 1 4 1.2c0 1.8-2.4 2.6-4 2.6z"/>',
  alert: '<path d="M12 4.2 21 19.6H3z"/><path d="M12 10v4M12 17v.2"/>',
  logout:
    '<path d="M15 8V5.6A1.6 1.6 0 0 0 13.4 4H5.6A1.6 1.6 0 0 0 4 5.6v12.8A1.6 1.6 0 0 0 5.6 20h7.8a1.6 1.6 0 0 0 1.6-1.6V16"/><path d="M9.5 12H21M17.5 8.5 21 12l-3.5 3.5"/>',
  pause: '<path d="M9 5.5v13M15 5.5v13"/>',
  install:
    '<rect x="6" y="2.6" width="12" height="18.8" rx="3"/><path d="M10.5 18.4h3"/><path d="M12 7v6M9.4 10.4 12 13l2.6-2.6"/>',
  share:
    '<path d="M12 15.4V4M8.4 7.6 12 4l3.6 3.6"/><path d="M5 13v6.4A1.6 1.6 0 0 0 6.6 21h10.8A1.6 1.6 0 0 0 19 19.4V13"/>',
  server:
    '<rect x="3.4" y="4" width="17.2" height="6.4" rx="2"/><rect x="3.4" y="13.6" width="17.2" height="6.4" rx="2"/><path d="M7 7.2h.2M7 16.8h.2"/>',
}

export type IconName = keyof typeof PATHS

type IconProps = SVGProps<SVGSVGElement> & { name: IconName }

export function Icon({ name, ...props }: IconProps) {
  const inner = PATHS[name] || PATHS.info
  return (
    <svg
      viewBox='0 0 24 24'
      fill='none'
      stroke='currentColor'
      strokeWidth='1.8'
      strokeLinecap='round'
      strokeLinejoin='round'
      aria-hidden='true'
      {...props}
      dangerouslySetInnerHTML={{ __html: inner }}
    />
  )
}

/** 品牌标：与桌面端 icons.js 的 brand 同一造型（蓝底双向箭头） */
export function BrandMark({ size = 32 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox='0 0 24 24' aria-hidden='true'>
      <rect width='24' height='24' rx='5.4' fill='#007AFF' />
      <g fill='none' stroke='#fff' strokeWidth='2.2' strokeLinecap='round' strokeLinejoin='round'>
        <path d='M5.23 9.24h10.71' />
        <path d='M15.94 6.72 18.77 9.24 15.94 11.76' />
        <path d='M18.77 14.76H8.06' />
        <path d='M8.06 12.24 5.23 14.76 8.06 17.28' />
      </g>
    </svg>
  )
}
