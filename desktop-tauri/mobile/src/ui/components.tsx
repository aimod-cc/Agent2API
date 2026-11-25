import type { CSSProperties, ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { t } from '../core/i18n'
import { store, useStore, type TabId } from '../core/store'
import { GlyphIcon, type IconName } from './icons'

/* ─── CommonScaffold & Top App Bar ───────────────────────── */

export function CommonScaffold({
  title,
  badge,
  actions,
  onBack,
  stickyHeader,
  children,
  noScrollPadding = false,
}: {
  title: ReactNode
  badge?: ReactNode
  actions?: ReactNode
  onBack?: () => void
  stickyHeader?: ReactNode
  children: ReactNode
  noScrollPadding?: boolean
}) {
  return (
    <div className='fc-app-shell'>
      <header className='fc-topbar'>
        <div className='fc-topbar-title-wrap'>
          {onBack ? (
            <button
              type='button'
              className='fc-tonal-icon-btn'
              style={{ width: 34, minWidth: 34, padding: 0, background: 'var(--fc-card)' }}
              onClick={onBack}
              aria-label={t('返回')}
            >
              <GlyphIcon name='arrow-left' size={18} />
            </button>
          ) : null}
          <h1 className='fc-page-title'>{title}</h1>
          {badge}
        </div>
        {actions ? <div className='fc-tonal-btn-group'>{actions}</div> : null}
      </header>
      {stickyHeader ? <div className='fc-sticky-header'>{stickyHeader}</div> : null}
      {noScrollPadding ? (
        <div style={{ flex: 1, display: 'flex', flexDirection: 'column', overflow: 'hidden', minHeight: 0 }}>
          {children}
        </div>
      ) : (
        <main className={`fc-scroll-body ${stickyHeader ? 'with-sticky' : ''} ${onBack ? 'no-dock' : ''}`}>
          {children}
        </main>
      )}
    </div>
  )
}

export function TonalIconButton({
  icon,
  label,
  primary = false,
  disabled = false,
  onClick,
  title,
}: {
  icon: IconName
  label?: string
  primary?: boolean
  disabled?: boolean
  onClick?: () => void
  title?: string
}) {
  return (
    <button
      type='button'
      className={`fc-tonal-icon-btn ${primary ? 'is-primary' : ''}`}
      disabled={disabled}
      onClick={onClick}
      title={title || label}
    >
      <GlyphIcon name={icon} size={18} />
      {label ? <span>{label}</span> : null}
    </button>
  )
}

/* ─── Floating NavigationDock ────────────────────────────── */

export type TabMeta = {
  id: TabId
  label: string
  subtitle: string
  icon: IconName
  required?: boolean
}

export const TAB_CATALOG: ReadonlyArray<TabMeta> = [
  { id: 'dashboard', label: '总览', subtitle: '运行状态、今日用量与排行', icon: 'dashboard', required: true },
  { id: 'checkin', label: '签到', subtitle: '每日签到、福利与定时计划', icon: 'calendar' },
  { id: 'scheduled', label: '定时任务', subtitle: '自动化脚本插件、参数配置与运行日志', icon: 'clock' },
  { id: 'accounts', label: '账号', subtitle: '号池健康、限流与额度查询', icon: 'accounts' },
  { id: 'chat', label: '对话', subtitle: '内置助手对话与工具调用', icon: 'chat' },
  { id: 'models', label: '模型', subtitle: '模型启停、在线测试与别名映射', icon: 'cube' },
  { id: 'routing', label: '路由', subtitle: '调度顺序、策略与超时参数', icon: 'route' },
  { id: 'keys', label: 'Key', subtitle: '管理客户端接入密钥与权限范围', icon: 'key' },
  { id: 'settings', label: '设置', subtitle: '外观、导航与系统工具', icon: 'settings', required: true },
]

export function getTabMeta(id: TabId): TabMeta {
  return TAB_CATALOG.find(item => item.id === id) || TAB_CATALOG[0]
}

export function NavigationDock() {
  const { tab, navTabs, packaged, mobileUpdate } = useStore()
  const items = navTabs.map(id => getTabMeta(id))
  const activeIdx = items.findIndex(item => item.id === tab)
  if (activeIdx < 0) return null
  const showSettingsDot = packaged && mobileUpdate?.hasUpdate === true

  return (
    <div className='fc-dock-wrap'>
      <nav
        className='fc-dock'
        aria-label={t('主导航')}
        style={{ '--fc-dock-cols': String(items.length) } as CSSProperties}
      >
        <div
          className='fc-dock-lens'
          style={{
            transform: `translateX(${activeIdx * 100}%)`,
          }}
        />
        {items.map(item => {
          const active = item.id === tab
          const showDot = item.id === 'settings' && showSettingsDot
          return (
            <button
              key={item.id}
              type='button'
              className={`fc-dock-item ${active ? 'is-active' : ''}`}
              onClick={() => store.setTab(item.id)}
            >
              <span className='fc-dock-icon-wrap'>
                <GlyphIcon name={item.icon} size={20} />
                {showDot ? <span className='fc-dock-dot' aria-hidden='true' /> : null}
              </span>
              <span className='fc-dock-label' style={items.length >= 6 ? { fontSize: 10.5 } : undefined}>
                {t(item.label)}
              </span>
            </button>
          )
        })}
      </nav>
    </div>
  )
}

/* ─── FlClash Cards ──────────────────────────────────────── */

export function DashboardCard({
  icon,
  title,
  subtitle,
  trailing,
  children,
}: {
  icon?: IconName | ReactNode
  title?: ReactNode
  subtitle?: ReactNode
  trailing?: ReactNode
  children: ReactNode
}) {
  return (
    <section className='fc-dashboard-card'>
      {title || icon || trailing ? (
        <div className='fc-dashboard-header'>
          <div className='fc-dashboard-header-left'>
            {icon ? (
              typeof icon === 'string' ? (
                <span className='fc-icon-badge'>
                  <GlyphIcon name={icon as IconName} size={20} />
                </span>
              ) : (
                icon
              )
            ) : null}
            <div style={{ minWidth: 0 }}>
              {title ? <h2 className='fc-card-title'>{title}</h2> : null}
              {subtitle ? <p className='fc-card-subtitle'>{subtitle}</p> : null}
            </div>
          </div>
          {trailing ? <div style={{ flexShrink: 0 }}>{trailing}</div> : null}
        </div>
      ) : null}
      {children}
    </section>
  )
}

export function CommonCard({ children, className = '' }: { children: ReactNode; className?: string }) {
  return <section className={`fc-common-card ${className}`}>{children}</section>
}

export function RecordCard({
  children,
  onClick,
}: {
  children: ReactNode
  onClick?: () => void
}) {
  return (
    <div
      className={`fc-record-card ${onClick ? 'is-clickable' : ''}`}
      onClick={onClick}
      role={onClick ? 'button' : undefined}
      tabIndex={onClick ? 0 : undefined}
    >
      {children}
    </div>
  )
}

/* ─── SectionHeader & ListGroup / ListItem ───────────────── */

export function SectionHeader({
  title,
  trailing,
}: {
  title: ReactNode
  trailing?: ReactNode
}) {
  return (
    <div className='fc-section-header'>
      <span className='fc-section-title'>{title}</span>
      {trailing}
    </div>
  )
}

export function ListGroup({ children }: { children: ReactNode }) {
  return <div className='fc-list-group'>{children}</div>
}

export function ListItem({
  icon,
  leading,
  title,
  subtitle,
  trailing,
  chevron = false,
  onClick,
}: {
  icon?: IconName
  leading?: ReactNode
  title: ReactNode
  subtitle?: ReactNode
  trailing?: ReactNode
  chevron?: boolean
  onClick?: () => void
}) {
  const hasLeading = Boolean(icon || leading)
  return (
    <div
      className={`fc-list-item ${onClick ? 'is-clickable' : ''} ${hasLeading ? '' : 'no-leading'}`}
      onClick={onClick}
      role={onClick ? 'button' : undefined}
    >
      {leading ? (
        leading
      ) : icon ? (
        <span className='fc-list-icon-well'>
          <GlyphIcon name={icon} size={18} />
        </span>
      ) : null}
      <div className='fc-list-body'>
        <div className='fc-list-title'>{title}</div>
        {subtitle ? <div className='fc-list-subtitle'>{subtitle}</div> : null}
      </div>
      {trailing || chevron ? (
        <div className='fc-list-trailing'>
          {trailing}
          {chevron ? <GlyphIcon name='chevron-right' size={16} /> : null}
        </div>
      ) : null}
    </div>
  )
}

/* ─── SegmentedControl ───────────────────────────────────── */

export function SegmentedControl<T extends string>({
  options,
  value,
  onChange,
  inCard = false,
}: {
  options: ReadonlyArray<{ id: T; label: string }>
  value: T
  onChange: (next: T) => void
  inCard?: boolean
}) {
  const count = Math.max(1, options.length)
  const matchedIdx = options.findIndex(item => item.id === value)
  const activeIdx = Math.max(0, matchedIdx)

  return (
    <div
      className={`fc-segmented ${inCard ? 'in-card' : ''}`}
      style={{ gridTemplateColumns: `repeat(${count}, 1fr)` }}
    >
      <div
        className='fc-segmented-thumb'
        style={{
          width: `calc((100% - 8px) / ${count})`,
          transform: `translateX(${activeIdx * 100}%)`,
          opacity: matchedIdx >= 0 ? 1 : 0,
        }}
      />
      {options.map(item => {
        const active = item.id === value
        return (
          <button
            key={item.id}
            type='button'
            className={`fc-segmented-btn ${active ? 'is-active' : ''}`}
            onClick={() => onChange(item.id)}
          >
            {item.label}
          </button>
        )
      })}
    </div>
  )
}

/* ─── ConnectedButtonGroup (M3 Expressive Morphing Row) ──── */

export function ConnectedButtonGroup<T extends string>({
  options,
  value,
  onChange,
  inCard = false,
}: {
  options: ReadonlyArray<{ id: T; label: string }>
  value: T
  onChange: (next: T) => void
  inCard?: boolean
}) {
  return (
    <div className={`fc-connected-group ${inCard ? 'in-card' : ''}`}>
      {options.map(item => {
        const active = item.id === value
        return (
          <button
            key={item.id}
            type='button'
            className={`fc-connected-btn ${active ? 'is-active' : ''}`}
            onClick={() => onChange(item.id)}
          >
            {item.label}
          </button>
        )
      })}
    </div>
  )
}

/* ─── SearchField ────────────────────────────────────────── */

export function SearchField({
  value,
  onChange,
  placeholder = '搜索…',
  inSheet = false,
}: {
  value: string
  onChange: (next: string) => void
  placeholder?: string
  inSheet?: boolean
}) {
  return (
    <div className={`fc-search-field ${inSheet ? 'in-sheet' : ''}`}>
      <span style={{ color: 'var(--fc-text-tertiary)', display: 'inline-flex' }}>
        <GlyphIcon name='search' size={17} />
      </span>
      <input
        type='text'
        className='fc-search-input'
        value={value}
        onChange={e => onChange(e.target.value)}
        placeholder={placeholder}
      />
      {value ? (
        <button
          type='button'
          className='fc-search-clear'
          onClick={() => onChange('')}
          aria-label='清空'
        >
          <GlyphIcon name='close' size={13} />
        </button>
      ) : null}
    </div>
  )
}

/* ─── CommonSwitch ───────────────────────────────────────── */

export function CommonSwitch({
  checked,
  disabled = false,
  onChange,
}: {
  checked: boolean
  disabled?: boolean
  onChange: (next: boolean) => void
}) {
  return (
    <button
      type='button'
      role='switch'
      aria-checked={checked}
      disabled={disabled}
      className={`fc-switch ${checked ? 'is-checked' : ''}`}
      onClick={e => {
        e.stopPropagation()
        if (!disabled) onChange(!checked)
      }}
    >
      <span className='fc-switch-thumb' />
    </button>
  )
}

/* ─── TonalChip ──────────────────────────────────────────── */

export function TonalChip({
  children,
  tone = 'neutral',
}: {
  children: ReactNode
  tone?: 'neutral' | 'primary' | 'success' | 'warning' | 'danger'
}) {
  return <span className={`fc-chip ${tone !== 'neutral' ? `tone-${tone}` : ''}`}>{children}</span>
}

/* ─── BottomSheet (AdaptiveSheetScaffold) ────────────────── */

export function BottomSheet({
  open,
  title,
  onClose,
  actions,
  fixedHeader,
  footer,
  children,
}: {
  open: boolean
  title: ReactNode
  onClose: () => void
  actions?: ReactNode
  fixedHeader?: ReactNode
  footer?: ReactNode
  children: ReactNode
}) {
  if (!open) return null
  const sheet = (
    <div
      className='fc-sheet-backdrop'
      onClick={e => {
        e.stopPropagation()
        onClose()
      }}
    >
      <div
        className='fc-sheet-panel fc-sheet-surface'
        onClick={e => e.stopPropagation()}
        role='dialog'
        aria-modal='true'
      >
        <div className='fc-sheet-handle-bar'>
          <span className='fc-sheet-handle' />
        </div>
        <div className='fc-sheet-header'>
          <h3 className='fc-sheet-title'>{title}</h3>
          <div style={{ display: 'inline-flex', alignItems: 'center', gap: 8 }}>
            {actions}
            <button
              type='button'
              className='fc-tonal-icon-btn'
              style={{ background: 'var(--fc-card)', width: 34, padding: 0 }}
              onClick={onClose}
              aria-label={t('关闭')}
            >
              <GlyphIcon name='close' size={17} />
            </button>
          </div>
        </div>
        {fixedHeader ? <div className='fc-sheet-fixed-header'>{fixedHeader}</div> : null}
        <div className='fc-sheet-body'>{children}</div>
        {footer ? <div className='fc-sheet-footer'>{footer}</div> : null}
      </div>
    </div>
  )
  return typeof document !== 'undefined' ? createPortal(sheet, document.body) : sheet
}

/* ─── EmptyBlock (Clean Inline State, No Gray Skeletons) ─── */

export function EmptyBlock({
  icon = 'cube',
  title,
  description,
  subtitle,
  action,
}: {
  icon?: IconName
  title: ReactNode
  description?: ReactNode
  subtitle?: ReactNode
  action?: ReactNode
}) {
  const detail = description ?? subtitle
  return (
    <div
      className='fc-common-card'
      style={{
        alignItems: 'center',
        textAlign: 'center',
        padding: '28px 20px',
        gap: 10,
      }}
    >
      <span className='fc-icon-badge' style={{ width: 46, height: 46 }}>
        <GlyphIcon name={icon} size={22} />
      </span>
      <div style={{ fontSize: 15, fontWeight: 700, color: 'var(--fc-text)' }}>{title}</div>
      {detail ? (
        <div style={{ fontSize: 12.5, color: 'var(--fc-text-secondary)', maxWidth: 280 }}>
          {detail}
        </div>
      ) : null}
      {action ? <div style={{ marginTop: 4 }}>{action}</div> : null}
    </div>
  )
}

/* ─── ToastHost ──────────────────────────────────────────── */

export function ToastHost() {
  const { toasts } = useStore()
  if (!toasts.length) return null
  return (
    <div className='fc-toast-host'>
      {toasts.map(item => (
        <div key={item.id} className={`fc-toast tone-${item.tone}`}>
          {item.text}
        </div>
      ))}
    </div>
  )
}
