import * as React from 'react'
import { Button, Popover, PopoverContent, PopoverTrigger } from '@ui'

export type AgentIconName =
  | 'plus'
  | 'edit'
  | 'more'
  | 'search'
  | 'chevron'
  | 'refresh'
  | 'folder'
  | 'bolt'
  | 'radar'
  | 'sliders'
  | 'trash'
  | 'check'
  | 'x'

export function AgentIcon({ name, size = 16 }: { name: AgentIconName; size?: number }) {
  const paths: Record<AgentIconName, React.ReactNode> = {
    plus: <path d='M12 5v14M5 12h14' />,
    edit: <path d='m16 3 5 5M4 20l4-1L21 6a2 2 0 0 0-3-3L5 16l-1 4Z' />,
    more: <path d='M4 12h.01M12 12h.01M20 12h.01' />,
    search: <path d='m21 21-5-5M18 10a8 8 0 1 1-16 0 8 8 0 0 1 16 0' />,
    chevron: <path d='m6 9 6 6 6-6' />,
    refresh: <path d='M3 12a9 9 0 0 1 15-6.7L21 8M21 3v5h-5M21 12a9 9 0 0 1-15 6.7L3 16M8 16H3v5' />,
    folder: <path d='M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7Z' />,
    bolt: <path d='M13 2 4 14h7l-1 8 9-12h-7l1-8Z' />,
    radar: <><circle cx='12' cy='12' r='2' /><path d='M16.24 7.76a6 6 0 0 1 0 8.49m-8.48-.01a6 6 0 0 1 0-8.49m11.31-2.82a10 10 0 0 1 0 14.14m-14.14 0a10 10 0 0 1 0-14.14' /></>,
    sliders: <path d='M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M2 14h4M10 8h4M18 16h4' />,
    trash: <path d='M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6' />,
    check: <path d='M20 6 9 17l-5-5' />,
    x: <path d='M18 6 6 18M6 6l12 12' />,
  }
  return (
    <svg
      width={size}
      height={size}
      viewBox='0 0 24 24'
      fill='none'
      stroke='currentColor'
      strokeWidth={name === 'more' ? 3 : 1.7}
      strokeLinecap='round'
      strokeLinejoin='round'
      aria-hidden='true'
      className='shrink-0'
    >
      {paths[name]}
    </svg>
  )
}

export function AgentActions({ label, disabled, actions }: {
  label: string; disabled?: boolean; actions: Array<{ label: string; disabled?: boolean; destructive?: boolean; onClick: () => void }>
}) {
  const [open, setOpen] = React.useState(false)
  return <Popover open={open} onOpenChange={setOpen}>
    <PopoverTrigger disabled={disabled} render={<Button variant='ghost' size='icon-sm' aria-label={label} title={label} />}><AgentIcon name='more' /></PopoverTrigger>
    <PopoverContent align='end' className='agent-action-menu'>
      {actions.map(action => <Button key={action.label} variant='ghost' disabled={action.disabled}
        className={action.destructive ? 'text-destructive' : undefined} onClick={() => { setOpen(false); action.onClick() }}>{action.label}</Button>)}
    </PopoverContent>
  </Popover>
}
