import * as React from 'react'
import {
  Button, SegmentedControl, Sheet, SheetBody, SheetContent, SheetDescription, SheetFooter, SheetHeader, SheetTitle, useMediaQuery,
} from '@ui'
import { PluginArtwork, type PluginArtworkKind, type PluginTranslate } from './plugins-store'

export type PluginDetailsPane = 'about' | 'settings'

export function PluginDetailsDialog({ name, description, version, sourceLabel, icon, artwork, initialPane = 'about',
  settings, metadata, more, aboutActions, settingsActions, primaryAction, closeDisabled = false, onClose, t }: {
  name: string
  description: string
  version?: string | null
  sourceLabel?: string
  icon?: string | null
  artwork?: PluginArtworkKind
  initialPane?: PluginDetailsPane
  settings?: React.ReactNode
  metadata?: React.ReactNode
  more?: React.ReactNode
  aboutActions?: React.ReactNode
  settingsActions?: React.ReactNode
  primaryAction?: React.ReactNode
  closeDisabled?: boolean
  onClose: () => void
  t: PluginTranslate
}) {
  const [pane, setPane] = React.useState<PluginDetailsPane>(settings ? initialPane : 'about')
  const [settingsMounted, setSettingsMounted] = React.useState(initialPane === 'settings')
  const bodyRef = React.useRef<HTMLDivElement>(null)
  const mobile = useMediaQuery('(max-width: 767px)')

  return <Sheet open onOpenChange={(open, details) => {
    if (!open) { if (closeDisabled) details.cancel(); else onClose() }
  }}>
    <SheetContent
      side={mobile ? 'bottom' : 'right'}
      showHandle={mobile}
      showCloseButton={!closeDisabled}
      className={mobile ? 'text-foreground' : 'w-[min(540px,92vw)] text-foreground'}
    >
      <SheetHeader className='shrink-0 gap-3'>
        <PluginArtwork src={icon} kind={artwork} />
        <div className='min-w-0 flex-1'>
          <SheetTitle className='m-0 break-words'>{name}</SheetTitle>
          <div className='mt-1 flex flex-wrap items-center gap-x-2 text-xs text-muted-foreground'>
            {version ? <span>v{version}</span> : null}
            {version && sourceLabel ? <span aria-hidden='true'>·</span> : null}
            {sourceLabel ? <span>{sourceLabel}</span> : null}
          </div>
        </div>
      </SheetHeader>
      <SheetDescription className='sr-only'>{t('插件详情')}</SheetDescription>
      {settings ? <div className='flex shrink-0 items-center justify-between gap-3 border-b border-hairline px-5 py-3'>
        <SegmentedControl<PluginDetailsPane> value={pane} aria-label={t('插件详情')}
          className='max-w-full [&_[data-slot=segmented-item]]:h-7 [&_[data-slot=segmented-item]]:px-3 max-[767px]:[&_[data-slot=segmented-item]]:h-9'
          options={[{ value: 'about', label: t('插件介绍') }, { value: 'settings', label: t('设置') }]}
          onValueChange={value => {
            if (value === 'settings') setSettingsMounted(true)
            setPane(value)
            if (bodyRef.current) bodyRef.current.scrollTop = 0
          }} />
        {more}
      </div> : null}
      <SheetBody ref={bodyRef} className='min-h-0 flex-1 gap-4 [scrollbar-gutter:stable]'>
        {pane === 'about' ? <>
          <p className='m-0 whitespace-pre-wrap break-words text-sm leading-6 text-subtle'>
            {description || t('作者尚未提供插件介绍。')}
          </p>
          {metadata}
        </> : null}
        {/* 切换介绍时保留设置草稿，首次查看介绍则不提前读取设置。 */}
        {settings && settingsMounted ? <div className={pane === 'settings' ? 'contents' : 'hidden'}>{settings}</div> : null}
      </SheetBody>
      <SheetFooter className='shrink-0 flex-wrap gap-2'>
        <div className='mr-auto flex flex-wrap items-center gap-2'>{pane === 'settings' ? settingsActions : aboutActions}</div>
        <Button variant='outline' size='sm' disabled={closeDisabled} onClick={onClose}>{t('完成')}</Button>
        {primaryAction}
      </SheetFooter>
    </SheetContent>
  </Sheet>
}
