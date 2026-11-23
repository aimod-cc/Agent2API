import type {
  AccountExpiryFilter,
  AccountQueryCounts,
  AccountQueryGroup,
  AccountStatusFilter,
  AccountUsageFilter,
} from '../core/api'
import { providerDisplayName } from '../core/format'
import { t } from '../core/i18n'
import { BottomSheet, SectionHeader } from '../ui/components'

export type AccountFilters = {
  status: AccountStatusFilter
  providers: string[]
  expiry: AccountExpiryFilter
  usage: AccountUsageFilter
}

export const DEFAULT_ACCOUNT_FILTERS: AccountFilters = {
  status: 'all',
  providers: [],
  expiry: 'all',
  usage: 'all',
}

export function activeAdvancedFilterCount(f: AccountFilters): number {
  let n = 0
  if (f.providers.length > 0) n += 1
  if (f.expiry !== 'all') n += 1
  if (f.usage !== 'all') n += 1
  if (f.status === 'disabled' || f.status === 'expired') n += 1
  return n
}

const STATUS_OPTIONS: ReadonlyArray<{ id: AccountStatusFilter; label: string }> = [
  { id: 'all', label: '全部状态' },
  { id: 'enabled', label: '已启用' },
  { id: 'disabled', label: '已停用' },
  { id: 'limited', label: '限流中' },
  { id: 'relogin', label: '需重登' },
  { id: 'expired', label: '已过期' },
]

const EXPIRY_OPTIONS: ReadonlyArray<{ id: AccountExpiryFilter; label: string }> = [
  { id: 'all', label: '不限' },
  { id: 'soon', label: '即将过期' },
  { id: 'expired', label: '已过期' },
  { id: 'longlived', label: '长期有效' },
]

const USAGE_OPTIONS: ReadonlyArray<{ id: AccountUsageFilter; label: string }> = [
  { id: 'all', label: '不限' },
  { id: 'has', label: '已有余额' },
  { id: 'unqueried', label: '未查询' },
  { id: 'failed', label: '查询失败' },
]

export function AccountsFilterSheet({
  open,
  filters,
  counts,
  groups,
  onChange,
  onReset,
  onClose,
}: {
  open: boolean
  filters: AccountFilters
  counts: AccountQueryCounts | null
  groups: AccountQueryGroup[]
  onChange: (next: AccountFilters) => void
  onReset: () => void
  onClose: () => void
}) {
  const toggleProvider = (id: string) => {
    const exists = filters.providers.includes(id)
    const next = exists ? filters.providers.filter(x => x !== id) : [...filters.providers, id]
    onChange({ ...filters, providers: next })
  }

  return (
    <BottomSheet
      open={open}
      title={t('筛选账号')}
      onClose={onClose}
      footer={
        <div style={{ display: 'flex', gap: 10, width: '100%' }}>
          <button type='button' className='fc-btn' style={{ flex: 1 }} onClick={onReset}>
            {t('重置')}
          </button>
          <button type='button' className='fc-btn is-primary' style={{ flex: 1 }} onClick={onClose}>
            {t('完成')}
          </button>
        </div>
      }
    >
      <SectionHeader title={t('账号状态')} />
      <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8 }}>
        {STATUS_OPTIONS.map(opt => {
          const active = filters.status === opt.id
          const count = counts?.status?.[opt.id]
          return (
            <button
              key={opt.id}
              type='button'
              className={`fc-filter-pill ${active ? 'is-active' : ''}`}
              onClick={() => onChange({ ...filters, status: opt.id })}
            >
              <span>{t(opt.label)}</span>
              {typeof count === 'number' ? <span style={{ opacity: 0.75 }}>{count}</span> : null}
            </button>
          )
        })}
      </div>

      {groups.length > 0 ? (
        <>
          <SectionHeader title={t('提供商（可多选）')} />
          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8 }}>
            {groups.map(g => {
              const id = g.id || g.provider || ''
              if (!id) return null
              const active = filters.providers.includes(id)
              const n = g.total ?? g.count
              return (
                <button
                  key={id}
                  type='button'
                  className={`fc-filter-pill ${active ? 'is-active' : ''}`}
                  onClick={() => toggleProvider(id)}
                >
                  <span>{providerDisplayName(id, g.label)}</span>
                  {typeof n === 'number' ? <span style={{ opacity: 0.75 }}>{n}</span> : null}
                </button>
              )
            })}
          </div>
        </>
      ) : null}

      <SectionHeader title={t('凭据有效期')} />
      <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8 }}>
        {EXPIRY_OPTIONS.map(opt => {
          const active = filters.expiry === opt.id
          const count = counts?.expiry?.[opt.id]
          return (
            <button
              key={opt.id}
              type='button'
              className={`fc-filter-pill ${active ? 'is-active' : ''}`}
              onClick={() => onChange({ ...filters, expiry: opt.id })}
            >
              <span>{t(opt.label)}</span>
              {typeof count === 'number' ? <span style={{ opacity: 0.75 }}>{count}</span> : null}
            </button>
          )
        })}
      </div>

      <SectionHeader title={t('额度与余额')} />
      <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8 }}>
        {USAGE_OPTIONS.map(opt => {
          const active = filters.usage === opt.id
          const count = counts?.usage?.[opt.id]
          return (
            <button
              key={opt.id}
              type='button'
              className={`fc-filter-pill ${active ? 'is-active' : ''}`}
              onClick={() => onChange({ ...filters, usage: opt.id })}
            >
              <span>{t(opt.label)}</span>
              {typeof count === 'number' ? <span style={{ opacity: 0.75 }}>{count}</span> : null}
            </button>
          )
        })}
      </div>
    </BottomSheet>
  )
}
