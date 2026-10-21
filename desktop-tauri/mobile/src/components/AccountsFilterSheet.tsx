import { Sheet, SheetContent, SheetHeader, SheetBody, SheetFooter, SheetTitle } from '@ui'
import { Icon } from './Icon'
import { ProviderLogo } from './ProviderLogo'
import { providerLabel } from '../lib/providers'
import { t } from '../i18n'

/**
 * 账号筛选面板（账号页右上角那颗「筛选」按钮打开）。
 *
 * ── 为什么从横排 chip 换成侧边面板 ────────────────────────────
 * 原来只有「全部 / 启用 / 限流 / 禁用」四颗横排 chip。四颗还放得下，但账号
 * 页真正要筛的维度不止这一个：提供商、有效期、余额状态各自都是一组。全都
 * 横排会变成一条要左右滑动的长条 —— 手机上滑条本身比内容还难操作，而且
 * 用户看不出「现在到底筛了什么」。
 *
 * 面板里每个维度占一行、各自单选（提供商多选），底部常驻「重置」与「完成」。
 * 条件**即时生效**：关掉面板就能看到结果，不需要先点应用再看 —— 手机上多
 * 一步确认只会让人怀疑自己点没点对。
 *
 * ── 为什么是右侧而不是底部 ────────────────────────────────────
 * 底部抽屉会盖住列表的下半部分，而筛选恰恰是「边看结果边调条件」的动作。
 * 右侧抽屉只占 340px（窄屏 86vw），左边还能看到列表在变。
 */

export type StatusFilter = 'all' | 'enabled' | 'disabled' | 'limited' | 'relogin' | 'expired'
export type ExpiryFilter = 'all' | 'soon' | 'expired' | 'longlived'
export type UsageFilter = 'all' | 'has' | 'unqueried' | 'failed'

export type AccountFilters = {
  status: StatusFilter
  /** 选中的提供商 id；空数组 = 不限制 */
  providers: string[]
  expiry: ExpiryFilter
  usage: UsageFilter
}

export const DEFAULT_FILTERS: AccountFilters = {
  status: 'all',
  providers: [],
  expiry: 'all',
  usage: 'all',
}

/** 生效中的筛选维度个数（按钮上的角标；提供商按一个维度算） */
export function activeFilterCount(filters: AccountFilters): number {
  let count = 0
  if (filters.status !== 'all') count++
  if (filters.providers.length) count++
  if (filters.expiry !== 'all') count++
  if (filters.usage !== 'all') count++
  return count
}

export function AccountsFilterSheet({
  open,
  onClose,
  filters,
  onChange,
  providers,
  counts,
}: {
  open: boolean
  onClose: () => void
  filters: AccountFilters
  onChange: (next: AccountFilters) => void
  /** 当前账号里出现过的提供商（id + 展示名） */
  providers: Array<{ id: string; label: string }>
  /** 各状态的账号数，显示在选项右侧 */
  counts: {
    status: Record<StatusFilter, number>
    expiry: Record<ExpiryFilter, number>
    usage: Record<UsageFilter, number>
  }
}) {
  const patch = (next: Partial<AccountFilters>) => onChange({ ...filters, ...next })

  const toggleProvider = (id: string) => {
    const picked = filters.providers.includes(id)
      ? filters.providers.filter(item => item !== id)
      : [...filters.providers, id]
    patch({ providers: picked })
  }

  return (
    <Sheet open={open} onOpenChange={next => { if (!next) onClose() }}>
      <SheetContent side='right'>
        <SheetHeader>
          <SheetTitle>{t('筛选')}</SheetTitle>
        </SheetHeader>
        <SheetBody>
          <Section title={t('状态')}>
            <Chip
              label={t('全部')}
              count={counts.status.all}
              on={filters.status === 'all'}
              onClick={() => patch({ status: 'all' })}
            />
            <Chip
              label={t('启用')}
              count={counts.status.enabled}
              on={filters.status === 'enabled'}
              onClick={() => patch({ status: 'enabled' })}
            />
            <Chip
              label={t('禁用')}
              count={counts.status.disabled}
              on={filters.status === 'disabled'}
              onClick={() => patch({ status: 'disabled' })}
            />
            <Chip
              label={t('限流中')}
              count={counts.status.limited}
              on={filters.status === 'limited'}
              onClick={() => patch({ status: 'limited' })}
            />
            <Chip
              label={t('需重登')}
              count={counts.status.relogin}
              on={filters.status === 'relogin'}
              onClick={() => patch({ status: 'relogin' })}
            />
            <Chip
              label={t('已过期')}
              count={counts.status.expired}
              on={filters.status === 'expired'}
              onClick={() => patch({ status: 'expired' })}
            />
          </Section>

          {providers.length > 1 ? (
            <Section title={t('提供商')}>
              {providers.map(item => (
                <button
                  key={item.id}
                  type='button'
                  className='m-chip'
                  data-on={filters.providers.includes(item.id)}
                  onClick={() => toggleProvider(item.id)}
                  style={{ gap: 6 }}
                >
                  <ProviderLogo id={item.id} label={item.label} />
                  {item.label}
                </button>
              ))}
            </Section>
          ) : null}

          <Section title={t('有效期')}>
            <Chip
              label={t('全部')}
              count={counts.expiry.all}
              on={filters.expiry === 'all'}
              onClick={() => patch({ expiry: 'all' })}
            />
            <Chip
              label={t('7 天内到期')}
              count={counts.expiry.soon}
              on={filters.expiry === 'soon'}
              onClick={() => patch({ expiry: 'soon' })}
            />
            <Chip
              label={t('已过期')}
              count={counts.expiry.expired}
              on={filters.expiry === 'expired'}
              onClick={() => patch({ expiry: 'expired' })}
            />
            <Chip
              label={t('长期有效')}
              count={counts.expiry.longlived}
              on={filters.expiry === 'longlived'}
              onClick={() => patch({ expiry: 'longlived' })}
            />
          </Section>

          <Section title={t('余额')}>
            <Chip
              label={t('全部')}
              count={counts.usage.all}
              on={filters.usage === 'all'}
              onClick={() => patch({ usage: 'all' })}
            />
            <Chip
              label={t('已查询')}
              count={counts.usage.has}
              on={filters.usage === 'has'}
              onClick={() => patch({ usage: 'has' })}
            />
            <Chip
              label={t('未查询')}
              count={counts.usage.unqueried}
              on={filters.usage === 'unqueried'}
              onClick={() => patch({ usage: 'unqueried' })}
            />
            <Chip
              label={t('查询失败')}
              count={counts.usage.failed}
              on={filters.usage === 'failed'}
              onClick={() => patch({ usage: 'failed' })}
            />
          </Section>

          <p style={{ margin: 0, fontSize: 11.5, lineHeight: 1.7, color: 'var(--m-text-3)' }}>
            {t('条件即时生效，关掉面板就能看到结果。')}
          </p>
        </SheetBody>
        <SheetFooter>
          <button
            type='button'
            className='m-btn m-ghost'
            style={{ flex: 1 }}
            onClick={() => onChange(DEFAULT_FILTERS)}
          >
            <Icon name='refresh' />
            {t('重置')}
          </button>
          <button type='button' className='m-btn' style={{ flex: 1 }} onClick={onClose}>
            {t('完成')}
          </button>
        </SheetFooter>
      </SheetContent>
    </Sheet>
  )
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div>
      <div style={{ fontSize: 12, fontWeight: 600, color: 'var(--m-text-2)', marginBottom: 8 }}>
        {title}
      </div>
      <div style={{ display: 'flex', flexWrap: 'wrap', gap: 7 }}>{children}</div>
    </div>
  )
}

function Chip({
  label,
  count,
  on,
  onClick,
}: {
  label: string
  count: number
  on: boolean
  onClick: () => void
}) {
  return (
    <button type='button' className='m-chip' data-on={on} onClick={onClick}>
      {label}
      <span className='m-n'>{count}</span>
    </button>
  )
}
