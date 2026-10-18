import { Icon } from './Icon'
import { t } from '../i18n'

/**
 * 二级页顶部返回栏。
 *
 * 结构与桌面端的移动版设置页返回栏同形（返回按钮 + 居中标题 + 右侧动作位），
 * 但这里做成通用组件，所有二级页共用，避免每页各写一遍。
 *
 * 右侧 `actions` 由页面自己给（刷新、新建…），没有就不占位。
 */
export function NavBar({
  title,
  onBack,
  actions,
}: {
  title: string
  onBack: () => void
  actions?: React.ReactNode
}) {
  return (
    <div className='m-navbar'>
      <button type='button' className='m-icon-btn' onClick={onBack} aria-label={t('返回')}>
        <Icon name='back' />
      </button>
      <span className='m-navbar-title'>{title}</span>
      <span className='m-navbar-slot'>{actions}</span>
    </div>
  )
}
