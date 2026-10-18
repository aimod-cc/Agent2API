import { Icon } from './Icon'
import { providerIcon, providerLabel } from '../lib/providers'

/**
 * 提供商图标块：有图用图，没图用首字。
 *
 * 尺寸档与 `.m-logo` 一致（34px / `lg` 40px），这里只负责内容。
 */
export function ProviderLogo({
  id,
  label,
  size = 'default',
}: {
  id?: string
  label?: string
  size?: 'default' | 'lg'
}) {
  const icon = providerIcon(id)
  const text = providerLabel(id, label)
  return (
    <span className={size === 'lg' ? 'm-logo m-lg' : 'm-logo'}>
      {icon ? (
        <img src={icon} alt='' loading='lazy' />
      ) : (
        <span style={{ fontSize: size === 'lg' ? 16 : 14, fontWeight: 600 }}>{text.slice(0, 1)}</span>
      )}
    </span>
  )
}

/** 通用图标块：给「不是提供商」的入口用（签到、Key、代理…） */
export function GlyphLogo({
  name,
  size = 'default',
}: {
  name: Parameters<typeof Icon>[0]['name']
  size?: 'default' | 'lg'
}) {
  return (
    <span className={size === 'lg' ? 'm-logo m-lg' : 'm-logo'}>
      <Icon name={name} />
    </span>
  )
}
