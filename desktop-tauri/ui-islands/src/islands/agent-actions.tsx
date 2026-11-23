import * as React from 'react'
import { Button, Popover, PopoverContent, PopoverTrigger } from '@ui'

export function AgentIcon({ name }: { name: 'plus' | 'edit' | 'more' | 'search' | 'chevron' | 'refresh' | 'folder' | 'bolt' }) {
  const paths = {
    plus: 'M12 5v14M5 12h14',
    edit: 'm16 3 5 5M4 20l4-1L21 6a2 2 0 0 0-3-3L5 16l-1 4Z',
    more: 'M4 12h.01M12 12h.01M20 12h.01',
    search: 'm21 21-5-5M18 10a8 8 0 1 1-16 0 8 8 0 0 1 16 0',
    chevron: 'm6 9 6 6 6-6',
    refresh: 'M3 12a9 9 0 0 1 15-6.7L21 8M21 3v5h-5M21 12a9 9 0 0 1-15 6.7L3 16M8 16H3v5',
    folder: 'M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7Z',
    bolt: 'M13 2 4 14h7l-1 8 9-12h-7l1-8Z',
  }
  return <svg width='16' height='16' viewBox='0 0 24 24' fill='none' stroke='currentColor' strokeWidth={name === 'more' ? 3 : 1.5}
    strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'><path d={paths[name]} /></svg>
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
