import * as React from 'react'
import {
  Avatar, AvatarFallback, AvatarImage, Button, Collapsible, CollapsibleContent,
  CollapsibleTrigger, cn,
} from '@ui'

import { PROVIDER_ICONS } from './add-provider-pick'
import { t } from '../i18n'

const ICON_FALLBACK: Record<string, string> = {
  'workbuddy-intl': 'workbuddy',
  'autoclaw-intl': 'autoclaw',
  'qoder-intl': 'qoder',
  'zcode-intl': 'zcode',
  'monkeycode-intl': 'monkeycode',
  'modelscope-intl': 'modelscope',
  'cline-pass': 'cline',
  'cline-free': 'cline',
  'accio-cn': 'accio',
}

export function ProviderLogo({ id, label }: { id: string; label: string }): React.ReactElement {
  const slug = ICON_FALLBACK[id] ?? id
  const icon = PROVIDER_ICONS[id] || `assets/providers/${slug}.png`
  const initial = (label || id || '·').trim().slice(0, 1).toUpperCase() || '·'
  return (
    <Avatar className='size-[30px] rounded-sm bg-control' title={label} aria-hidden='true'>
      <AvatarFallback className='font-bold'>{initial}</AvatarFallback>
      <AvatarImage keepMounted src={icon} alt='' loading='lazy' className='object-contain' />
    </Avatar>
  )
}

type CheckinRowProps = {
  provider: string
  name: string
  description?: React.ReactNode
  summary?: React.ReactNode
  actions?: React.ReactNode
  note?: React.ReactNode
  expandable: boolean
  expanded: boolean
  onExpandedChange: (open: boolean) => void
  children: React.ReactNode
}

export function CheckinRow({
  provider, name, description, summary, actions, note,
  expandable, expanded, onExpandedChange, children,
}: CheckinRowProps): React.ReactElement {
  const open = expandable && expanded
  return (
    <Collapsible open={open} onOpenChange={onExpandedChange} disabled={!expandable}
      className={cn('ck-row', open && 'open')}>
      <div className='ck-row-header'>
        <CollapsibleTrigger
          nativeButton={false}
          render={<Button variant='ghost' disabled={!expandable} nativeButton={false} render={<div />} />}
          tabIndex={expandable ? 0 : -1}
          className='ck-row-main h-auto min-w-0 flex-1 shrink justify-start gap-3 rounded-none border-0 bg-transparent px-4 py-[11px] text-left whitespace-normal font-normal shadow-none active:translate-y-0 data-disabled:cursor-default data-disabled:hover:bg-transparent'
        >
          <ProviderLogo id={provider} label={name} />
          <div className='ck-row-info'>
            <div className='ck-row-name'>{t(name)}</div>
            {description ? <div className='ck-row-desc'>{typeof description === 'string' ? t(description) : description}</div> : null}
          </div>
          {summary ? <div className='ck-row-right'>{summary}</div> : null}
          {expandable ? (
            <span className={cn('ck-chev', open && 'open')} aria-hidden='true'>
              <svg viewBox='0 0 24 24' width='14' height='14' fill='none' stroke='currentColor'
                strokeWidth='1.8' strokeLinecap='round' strokeLinejoin='round'>
                <path d='M9.5 5.5 16 12l-6.5 6.5' />
              </svg>
            </span>
          ) : null}
        </CollapsibleTrigger>
        {actions ? <div className='ck-row-actions'>{actions}</div> : null}
      </div>
      {note ? <div className='ck-fold-note empty:hidden'>{note}</div> : null}
      <CollapsibleContent className='ck-row-detail'>{children}</CollapsibleContent>
    </Collapsible>
  )
}
