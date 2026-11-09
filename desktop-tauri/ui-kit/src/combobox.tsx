import * as React from 'react'
import { Combobox as BaseCombobox } from '@base-ui/react/combobox'
import { createChangeEventDetails } from '@base-ui/react/internals/createBaseUIEventDetails'
import { useRender } from '@base-ui/react/use-render'
import { cn } from './lib/cn'
import { useSelectBoundary } from './lib/select-boundary'

const ComboboxContext = React.createContext<{
  open: boolean
  trigger: HTMLButtonElement | null
  setTrigger: React.Dispatch<React.SetStateAction<HTMLButtonElement | null>>
  closeOnAnchorHidden: () => void
  closedOnAnchorHidden: boolean
} | null>(null)

function Combobox<Value, Multiple extends boolean | undefined = false>({
  open: controlledOpen, defaultOpen = false, onOpenChange, children, ...props
}: BaseCombobox.Root.Props<Value, Multiple>) {
  const [internalOpen, setInternalOpen] = React.useState(defaultOpen)
  const [trigger, setTrigger] = React.useState<HTMLButtonElement | null>(null)
  const [closedOnAnchorHidden, setClosedOnAnchorHidden] = React.useState(false)
  const open = controlledOpen ?? internalOpen
  React.useLayoutEffect(() => {
    if (open) setClosedOnAnchorHidden(false)
  }, [open])

  const changeOpen = React.useCallback((next: boolean, details: BaseCombobox.Root.ChangeEventDetails) => {
    onOpenChange?.(next, details)
    if (details.isCanceled) return
    if (next) setClosedOnAnchorHidden(false)
    setInternalOpen(next)
  }, [onOpenChange])

  const closeOnAnchorHidden = React.useCallback(() => {
    if (!open || closedOnAnchorHidden) return
    const details = createChangeEventDetails('none', undefined, trigger ?? undefined)
    changeOpen(false, details)
    if (!details.isCanceled) setClosedOnAnchorHidden(true)
  }, [open, closedOnAnchorHidden, trigger, changeOpen])

  const context = React.useMemo(() => ({
    open, trigger, setTrigger, closeOnAnchorHidden, closedOnAnchorHidden,
  }), [open, trigger, closeOnAnchorHidden, closedOnAnchorHidden])

  return (
    <ComboboxContext.Provider value={context}>
      <BaseCombobox.Root {...props} open={open} onOpenChange={changeOpen}>{children}</BaseCombobox.Root>
    </ComboboxContext.Provider>
  )
}

function ComboboxPositionerElement({ anchorHidden, open, ...props }: React.ComponentProps<'div'> & {
  anchorHidden: boolean
  open: boolean
}) {
  const context = React.useContext(ComboboxContext)
  const closeOnAnchorHidden = context?.closeOnAnchorHidden
  React.useLayoutEffect(() => {
    if (open && anchorHidden) closeOnAnchorHidden?.()
  }, [open, anchorHidden, closeOnAnchorHidden])
  return <div {...props} />
}

const ComboboxValue = BaseCombobox.Value

type ComboboxTriggerProps = Omit<React.ComponentProps<typeof BaseCombobox.Trigger>, 'className'> & {
  className?: string
}

function ComboboxTrigger({ className, children, ref, ...props }: ComboboxTriggerProps) {
  const context = React.useContext(ComboboxContext)
  return useRender({
    render: <BaseCombobox.Trigger />,
    ref: [ref ?? null, context?.setTrigger ?? null],
    props: {
      ...props,
      'data-slot': 'combobox-trigger',
      className: cn(
        'inline-flex h-[30px] min-w-0 cursor-pointer items-center justify-between gap-1.5 rounded-md border border-control-border bg-control px-2.5',
        'text-[12.5px] font-normal text-foreground max-[767px]:h-10 max-[767px]:text-base',
        'transition-colors duration-150 ease-out',
        'hover:border-control-border-hover hover:bg-control-hover',
        'focus-visible:border-primary focus-visible:shadow-focus focus-visible:outline-none',
        'data-disabled:cursor-not-allowed data-disabled:bg-control data-disabled:text-muted-foreground data-disabled:opacity-60',
        className
      ),
      children: <>
        <span className='min-w-0 flex-1 truncate text-left'>{children}</span>
        <BaseCombobox.Icon className='flex flex-none text-muted-foreground transition-transform duration-[180ms] data-popup-open:rotate-180'>
          <svg viewBox='0 0 10 6' className='size-2.5' aria-hidden='true'>
            <path d='M1 1l4 4 4-4' stroke='currentColor' strokeWidth='1.4' fill='none' strokeLinecap='round' strokeLinejoin='round' />
          </svg>
        </BaseCombobox.Icon>
      </>,
    },
  })
}

type ComboboxContentProps = Omit<React.ComponentProps<typeof BaseCombobox.Popup>, 'className'> & {
  className?: string
  sideOffset?: number
}

function ComboboxContent({ className, sideOffset = 4, children, finalFocus, ...props }: ComboboxContentProps) {
  const context = React.useContext(ComboboxContext)
  const boundary = useSelectBoundary(context?.trigger ?? null, context?.open ?? false)

  return (
    <BaseCombobox.Portal>
      <BaseCombobox.Positioner sideOffset={sideOffset} align='start' collisionBoundary={boundary}
        className='z-[35] data-anchor-hidden:invisible data-anchor-hidden:pointer-events-none'
        render={(positionerProps, state) => <ComboboxPositionerElement {...positionerProps}
          anchorHidden={state.anchorHidden} open={state.open} />}>
        <BaseCombobox.Popup
          data-slot='combobox-content'
          className={cn(
            'flex max-h-[min(300px,var(--available-height))] w-[max(var(--anchor-width),220px)] max-w-[var(--available-width)] flex-col overflow-hidden',
            'rounded-lg border border-border-strong bg-raised shadow-3 outline-none',
            'data-open:animate-in data-open:fade-in-0',
            'data-closed:animate-out data-closed:fade-out-0',
            className
          )}
          // 触发器滚出可见范围时不归还焦点，避免页面被拉回原处。
          finalFocus={context?.closedOnAnchorHidden ? false : finalFocus}
          {...props}
        >
          {children}
        </BaseCombobox.Popup>
      </BaseCombobox.Positioner>
    </BaseCombobox.Portal>
  )
}

type ComboboxInputProps = Omit<React.ComponentProps<typeof BaseCombobox.Input>, 'className'> & {
  className?: string
}

function ComboboxInput({ className, ...props }: ComboboxInputProps) {
  return (
    <BaseCombobox.Input
      data-slot='combobox-input'
      className={cn(
        'h-[30px] w-full min-w-0 rounded-md border border-control-border bg-control px-2.5',
        'text-[12.5px] text-foreground outline-none max-[767px]:h-10 max-[767px]:text-base',
        'transition-[border-color,box-shadow] duration-150 ease-out placeholder:text-muted-foreground',
        'hover:border-control-border-hover focus:border-primary focus:shadow-focus',
        'disabled:pointer-events-none disabled:cursor-not-allowed disabled:text-muted-foreground',
        className
      )}
      {...props}
    />
  )
}

type ComboboxListProps = Omit<React.ComponentProps<typeof BaseCombobox.List>, 'className'> & {
  className?: string
}

function ComboboxList({ className, ...props }: ComboboxListProps) {
  return <BaseCombobox.List data-slot='combobox-list'
    className={cn('min-h-0 flex-1 overflow-y-auto overscroll-contain p-[5px]', className)} {...props} />
}

type ComboboxItemProps = Omit<React.ComponentProps<typeof BaseCombobox.Item>, 'className'> & {
  className?: string
}

function ComboboxItem({ className, children, ...props }: ComboboxItemProps) {
  return (
    <BaseCombobox.Item
      data-slot='combobox-item'
      className={cn(
        'flex min-h-7 cursor-pointer items-center gap-2 rounded-sm px-[9px] py-[5px] text-[12.5px] leading-[1.4] text-foreground select-none outline-none',
        'max-[767px]:min-h-10 max-[767px]:text-sm transition-colors duration-[120ms] ease-out',
        'data-[highlighted]:bg-nav-hover',
        'data-[selected]:bg-primary-soft data-[selected]:font-semibold data-[selected]:text-primary-fg',
        'data-[highlighted]:data-[selected]:bg-primary-tint',
        'data-disabled:pointer-events-none data-disabled:font-normal data-disabled:text-muted-foreground',
        className
      )}
      {...props}
    >
      <span className='min-w-0 flex-1 truncate'>{children}</span>
      <BaseCombobox.ItemIndicator className='flex flex-none text-primary'>
        <svg viewBox='0 0 12 12' className='size-3' aria-hidden='true'>
          <path d='M2.5 6.2 4.8 8.5 9.5 3.8' stroke='currentColor' strokeWidth='1.8' fill='none' strokeLinecap='round' strokeLinejoin='round' />
        </svg>
      </BaseCombobox.ItemIndicator>
    </BaseCombobox.Item>
  )
}

type ComboboxEmptyProps = Omit<React.ComponentProps<typeof BaseCombobox.Empty>, 'className'> & {
  className?: string
}

function ComboboxEmpty({ className, children, ...props }: ComboboxEmptyProps) {
  return <BaseCombobox.Empty data-slot='combobox-empty' {...props}>
    <div className={cn('px-2.5 py-3 text-[11.5px] leading-[1.5] text-muted-foreground', className)}>{children}</div>
  </BaseCombobox.Empty>
}

export {
  Combobox,
  ComboboxTrigger,
  ComboboxValue,
  ComboboxContent,
  ComboboxInput,
  ComboboxList,
  ComboboxItem,
  ComboboxEmpty,
}
