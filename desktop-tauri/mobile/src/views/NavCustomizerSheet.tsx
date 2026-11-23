import { t } from '../core/i18n'
import { MAX_NAV_TABS, store, useStore, type TabId } from '../core/store'
import {
  BottomSheet,
  CommonSwitch,
  getTabMeta,
  ListGroup,
  ListItem,
  SectionHeader,
  TAB_CATALOG,
} from '../ui/components'
import { GlyphIcon } from '../ui/icons'

export function NavCustomizerSheet({ onClose }: { onClose: () => void }) {
  const { navTabs } = useStore()
  const hiddenTabs = TAB_CATALOG.filter(item => !navTabs.includes(item.id))

  const toggleTab = (id: TabId, enabled: boolean) => {
    const meta = getTabMeta(id)
    if (meta.required) return
    if (enabled) {
      if (navTabs.includes(id)) return
      if (navTabs.length >= MAX_NAV_TABS) {
        store.toast(t('底部导航栏最多固定 {n} 个页面，可先关闭其他页面', { n: MAX_NAV_TABS }), 'danger')
        return
      }
      const next = [...navTabs]
      const settingsIdx = next.indexOf('settings')
      if (settingsIdx >= 0) next.splice(settingsIdx, 0, id)
      else next.push(id)
      store.setNavTabs(next)
      return
    }
    if (navTabs.length <= 3) {
      store.toast(t('底部导航栏至少保留 3 个常用页面'), 'danger')
      return
    }
    store.setNavTabs(navTabs.filter(item => item !== id))
  }

  const moveTab = (index: number, delta: -1 | 1) => {
    const nextIdx = index + delta
    if (nextIdx < 0 || nextIdx >= navTabs.length) return
    const next = [...navTabs]
    const [picked] = next.splice(index, 1)
    next.splice(nextIdx, 0, picked)
    store.setNavTabs(next)
  }

  return (
    <BottomSheet
      open
      title={t('自定义底部导航栏')}
      onClose={onClose}
      footer={
        <div style={{ display: 'flex', gap: 10, width: '100%' }}>
          <button
            type='button'
            className='fc-btn'
            style={{ flex: 1 }}
            onClick={() => {
              store.resetNavTabs()
              store.toast(t('已恢复默认导航栏'), 'success')
            }}
          >
            {t('恢复默认')}
          </button>
          <button
            type='button'
            className='fc-btn is-primary'
            style={{ flex: 1 }}
            onClick={onClose}
          >
            {t('完成')}
          </button>
        </div>
      }
    >
      <div style={{ fontSize: 13, color: 'var(--fc-text-muted)', lineHeight: 1.5, padding: '0 2px' }}>
        {t('未固定在底部的页面仍可通过「设置 → 更多功能」随时打开。')}
      </div>

      <SectionHeader title={t('已显示在导航栏 ({n})', { n: navTabs.length })} />
      <ListGroup>
        {navTabs.map((id, idx) => {
          const meta = getTabMeta(id)
          return (
            <ListItem
              key={id}
              icon={meta.icon}
              title={t(meta.label)}
              subtitle={meta.required ? t('核心页面（始终保留）') : t(meta.subtitle)}
              trailing={
                <div style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}>
                  <button
                    type='button'
                    className='fc-tonal-icon-btn'
                    style={{ width: 30, minWidth: 30, height: 30, padding: 0, background: 'var(--fc-block)' }}
                    disabled={idx === 0}
                    onClick={() => moveTab(idx, -1)}
                    aria-label={t('上移')}
                  >
                    <GlyphIcon name='arrow-up' size={14} />
                  </button>
                  <button
                    type='button'
                    className='fc-tonal-icon-btn'
                    style={{ width: 30, minWidth: 30, height: 30, padding: 0, background: 'var(--fc-block)' }}
                    disabled={idx === navTabs.length - 1}
                    onClick={() => moveTab(idx, 1)}
                    aria-label={t('下移')}
                  >
                    <GlyphIcon name='arrow-down' size={14} />
                  </button>
                  <CommonSwitch
                    checked
                    disabled={meta.required}
                    onChange={next => toggleTab(id, next)}
                  />
                </div>
              }
            />
          )
        })}
      </ListGroup>

      {hiddenTabs.length > 0 ? (
        <>
          <SectionHeader title={t('可添加到导航栏')} />
          <ListGroup>
            {hiddenTabs.map(meta => (
              <ListItem
                key={meta.id}
                icon={meta.icon}
                title={t(meta.label)}
                subtitle={t(meta.subtitle)}
                trailing={
                  <CommonSwitch
                    checked={false}
                    onChange={next => toggleTab(meta.id, next)}
                  />
                }
              />
            ))}
          </ListGroup>
        </>
      ) : null}
    </BottomSheet>
  )
}
