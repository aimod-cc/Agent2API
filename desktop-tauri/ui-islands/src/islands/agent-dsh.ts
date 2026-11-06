import { t } from '../i18n'

export function dshTargetLabel(profile: string, fallback: string): string {
  switch (profile) {
    case 'web': return t('网页版')
    case 'desktop': return t('桌面版')
    case 'legacy': return t('旧版共享设置')
    case 'home': return t('全部配置（全局覆盖）')
    default: return fallback
  }
}
