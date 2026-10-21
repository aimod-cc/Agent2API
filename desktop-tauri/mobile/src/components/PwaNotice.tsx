import { useState } from 'react'
import { Icon } from './Icon'
import { usePwa } from '../lib/usePwa'
import { useStore } from '../state/store'
import { applyUpdate } from '../lib/pwa'
import { t } from '../i18n'

/**
 * PWA 状态条。
 *
 * 两件事，位置不同：
 *   · **离线**：常驻顶部细条。它解释的是「为什么页面上的数字不动了」，
 *     放在内容上方、一眼能看到，且不抢操作。
 *   · **有新版本**：底部浮条。它需要一个明确的动作（重启），
 *     放在拇指够得到的地方；同时给「稍后」，不打断当前操作。
 *
 * 两者都是 `.m-app` 的 flex 子项 / 浮层，不遮挡导航：离线条把内容整体
 * 往下推，更新条浮在标签栏之上。
 *
 * 设置页不弹更新浮条：那一页的「应用状态」卡片常驻同样的按钮，
 * 两处一起出现只是重复。
 */
export function PwaNotice() {
  const pwa = usePwa()
  const { route } = useStore()
  const [busy, setBusy] = useState(false)
  // 收起只对本次会话有效：新 SW 仍在等待，下次打开会再提示一次
  const [snoozed, setSnoozed] = useState(false)
  const showUpdate = pwa.updateReady && !snoozed && route.screen !== 'settings'

  return (
    <>
      {pwa.offline ? (
        <div className='m-notice-top' role='status'>
          <Icon name='alert' />
          <span>{t('已离线 · 显示的是缓存内容')}</span>
        </div>
      ) : null}

      {showUpdate ? (
        <div className='m-notice-update' role='status'>
          <span className='m-notice-text'>{t('有新版本，重启后生效')}</span>
          <button
            type='button'
            className='m-btn m-sm'
            disabled={busy}
            onClick={() => {
              setBusy(true)
              void applyUpdate()
            }}
          >
            {busy ? t('重启中…') : t('重启更新')}
          </button>
          <button type='button' className='m-notice-later' onClick={() => setSnoozed(true)}>
            {t('稍后')}
          </button>
        </div>
      ) : null}
    </>
  )
}
