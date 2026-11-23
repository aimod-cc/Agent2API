import { useMemo, useState } from 'react'
import { t } from '../core/i18n'
import {
  BottomSheet,
  EmptyBlock,
  ListGroup,
  ListItem,
  SearchField,
  TonalChip,
} from './components'
import { GlyphIcon, type IconName } from './icons'

export type PickerOption<T extends string = string> = {
  value: T
  label: string
  subtitle?: string
  icon?: IconName
  badge?: string
  disabled?: boolean
}

export function PickerTrigger({
  valueLabel,
  placeholder,
  disabled = false,
  inList = false,
  onClick,
  ariaLabel,
}: {
  valueLabel?: string
  placeholder?: string
  disabled?: boolean
  inList?: boolean
  onClick: () => void
  ariaLabel?: string
}) {
  const showPlaceholder = !valueLabel
  return (
    <button
      type='button'
      className={`fc-picker-trigger ${inList ? 'in-list' : ''}`}
      disabled={disabled}
      onClick={onClick}
      aria-label={ariaLabel}
    >
      <span className={`fc-picker-value ${showPlaceholder ? 'is-placeholder' : ''}`}>
        {valueLabel || placeholder || t('请选择')}
      </span>
      <GlyphIcon name='chevron-down' size={16} />
    </button>
  )
}

export function PickerSheet<T extends string = string>({
  open,
  title,
  options,
  value,
  searchable = false,
  searchPlaceholder,
  emptyTitle,
  onSelect,
  onClose,
}: {
  open: boolean
  title: string
  options: ReadonlyArray<PickerOption<T>>
  value: T
  searchable?: boolean
  searchPlaceholder?: string
  emptyTitle?: string
  onSelect: (value: T) => void
  onClose: () => void
}) {
  const [query, setQuery] = useState('')
  const showSearch = searchable || options.length >= 8

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase()
    if (!q) return options
    return options.filter(
      item =>
        item.label.toLowerCase().includes(q)
        || item.subtitle?.toLowerCase().includes(q)
        || item.value.toLowerCase().includes(q),
    )
  }, [options, query])

  const close = () => {
    setQuery('')
    onClose()
  }

  return (
    <BottomSheet
      open={open}
      title={title}
      onClose={close}
      fixedHeader={
        showSearch ? (
          <SearchField
            value={query}
            onChange={setQuery}
            placeholder={searchPlaceholder || t('搜索选项')}
            inSheet
          />
        ) : undefined
      }
      footer={
        <button type='button' className='fc-btn' style={{ width: '100%' }} onClick={close}>
          {t('关闭')}
        </button>
      }
    >
      {filtered.length ? (
        <ListGroup>
          {filtered.map(item => {
            const active = item.value === value
            return (
              <ListItem
                key={item.value}
                icon={item.icon}
                title={item.label}
                subtitle={item.subtitle}
                onClick={
                  item.disabled
                    ? undefined
                    : () => {
                        onSelect(item.value)
                        close()
                      }
                }
                trailing={
                  <span style={{ display: 'inline-flex', alignItems: 'center', gap: 8 }}>
                    {item.badge ? <TonalChip>{item.badge}</TonalChip> : null}
                    {active ? (
                      <span
                        style={{
                          width: 24,
                          height: 24,
                          borderRadius: 999,
                          background: 'var(--fc-primary-soft)',
                          color: 'var(--fc-primary)',
                          display: 'inline-flex',
                          alignItems: 'center',
                          justifyContent: 'center',
                        }}
                      >
                        <GlyphIcon name='check' size={15} />
                      </span>
                    ) : null}
                  </span>
                }
              />
            )
          })}
        </ListGroup>
      ) : (
        <EmptyBlock icon='search' title={emptyTitle || t('没有匹配的选项')} />
      )}
    </BottomSheet>
  )
}
