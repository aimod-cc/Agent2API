import type { ReactNode } from 'react'
import { DialogSection } from '@ui'
import type { PluginTranslate } from './plugins-store'

export function PluginPermissions({ children, t }: { children: ReactNode; t: PluginTranslate }) {
  return <DialogSection>
    <h3 className='m-0'>{t('权限')}</h3>
    {children}
  </DialogSection>
}
