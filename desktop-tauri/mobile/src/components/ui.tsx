import { Icon, type IconName } from './Icon'
import { t } from '../i18n'
import { store } from '../state/store'

/** 通用页头（一级页）：大标题 + 副标题 + 右侧动作 */
export function PageHead({
  title,
  subtitle,
  actions,
}: {
  title: string
  subtitle?: string
  actions?: React.ReactNode
}) {
  return (
    <div className='m-head'>
      <div style={{ minWidth: 0 }}>
        <h1>{title}</h1>
        {subtitle ? <p className='m-sub'>{subtitle}</p> : null}
      </div>
      {actions ? <div className='m-head-actions'>{actions}</div> : null}
    </div>
  )
}

/**
 * 二级页的页内说明条。
 *
 * 二级页的标题已经由顶部返回栏（NavBar）承载，页内再写一遍大标题就是重复。
 * 但副标题（统计读数 / 时间范围）有信息量，保留成一行小字。右侧动作放这里，
 * 因为它属于页面内容而不是导航。
 */
export function PageIntro({
  subtitle,
  actions,
}: {
  subtitle?: string
  actions?: React.ReactNode
}) {
  if (!subtitle && !actions) return null
  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'space-between',
        gap: 10,
        padding: '2px 18px 12px',
      }}
    >
      <span style={{ fontSize: 12.5, color: 'var(--m-text-3)', minWidth: 0 }}>{subtitle}</span>
      {actions ? <div style={{ display: 'flex', gap: 8, flex: 'none' }}>{actions}</div> : null}
    </div>
  )
}

/** 圆形图标按钮（页头 / 返回栏右侧都用它） */
export function IconButton({
  name,
  label,
  onClick,
  primary,
}: {
  name: IconName
  label: string
  onClick?: () => void
  primary?: boolean
}) {
  return (
    <button
      type='button'
      className={primary ? 'm-icon-btn m-primary' : 'm-icon-btn'}
      aria-label={label}
      title={label}
      onClick={onClick}
    >
      <Icon name={name} />
    </button>
  )
}

/** 区块标题：左侧标题 + 右侧可选动作 */
export function SectionHead({
  title,
  action,
}: {
  title: string
  action?: { label: string; onClick: () => void }
}) {
  return (
    <div className='m-section-head'>
      <h2>{title}</h2>
      {action ? (
        <button type='button' className='m-link' onClick={action.onClick}>
          {action.label}
        </button>
      ) : null}
    </div>
  )
}

/** 徽章：语义色与桌面端一致（绿=正常、黄=告警、红=失效、灰=中性、蓝=待办） */
export function Badge({
  tone,
  dot,
  children,
}: {
  tone: 'ok' | 'warn' | 'bad' | 'mute' | 'brand'
  dot?: boolean
  children: React.ReactNode
}) {
  return (
    <span className={`m-badge m-badge-${tone}`}>
      {dot ? <i /> : null}
      {children}
    </span>
  )
}

/** 空态 */
export function Empty({ text }: { text: string }) {
  return <div className='m-empty'>{text}</div>
}

/** 错误条（页内，不弹窗） */
export function ErrorNote({ text }: { text: string }) {
  return <div className='m-error'>{text}</div>
}

/** 加载骨架：给列表页首屏用，避免「空白 → 突然出现内容」的跳动 */
export function SkeletonList({ rows = 4, height = 64 }: { rows?: number; height?: number }) {
  return (
    <div className='m-section'>
      {Array.from({ length: rows }).map((_, index) => (
        <div
          key={index}
          className='m-skeleton'
          style={{ height, marginTop: index ? 8 : 0 }}
        />
      ))}
    </div>
  )
}

/** 复制到剪贴板（失败时退回 execCommand，老 WebView 也能用） */
export async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text)
  } catch {
    const area = document.createElement('textarea')
    area.value = text
    area.style.position = 'fixed'
    area.style.opacity = '0'
    document.body.appendChild(area)
    area.select()
    try {
      document.execCommand('copy')
    } catch {
      /* 复制失败：提示由调用方给 */
    }
    area.remove()
  }
}

/** 复制 + 提示的常用组合 */
export async function copyWithToast(text: string, label?: string): Promise<void> {
  await copyText(text)
  store.toast(t('已复制{label}', { label: label ? `：${label}` : '' }))
}

/** 数值卡片（报表 / 首页共用） */
export function StatCard({
  label,
  value,
  sub,
  tone,
}: {
  label: string
  value: string
  sub?: string
  tone?: 'up' | 'ok'
}) {
  return (
    <div className='m-card m-card-pad' style={{ padding: '12px 13px' }}>
      <div style={{ fontSize: 11.5, color: 'var(--m-text-3)' }}>{label}</div>
      <div
        style={{
          fontSize: 21,
          fontWeight: 650,
          letterSpacing: '-0.03em',
          marginTop: 3,
          fontVariantNumeric: 'tabular-nums',
        }}
      >
        {value}
      </div>
      {sub ? (
        <div
          style={{
            fontSize: 11,
            marginTop: 2,
            color: tone === 'up' ? 'var(--m-ok)' : tone === 'ok' ? 'var(--m-ok)' : 'var(--m-text-3)',
          }}
        >
          {sub}
        </div>
      ) : null}
    </div>
  )
}
