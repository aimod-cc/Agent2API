import * as React from 'react'
import { Field, FieldLabel, Input, InputGroup, InputGroupAddon, InputGroupInput, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Switch } from '@ui'

export function ErrorIcon({ name, size = 16 }: { name: 'settings' | 'refresh' | 'info' | 'plus' | 'close' | 'check' | 'shield' | 'timer' | 'route' | 'chevron'; size?: number }) {
  const paths = {
    settings: <><path d='M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3' /><path d='M1 14h6M9 8h6M17 16h6' /></>,
    refresh: <><path d='M21 4v6h-6' /><path d='M19 16a8 8 0 1 1-1-10l3 4' /></>,
    info: <><circle cx='12' cy='12' r='9' /><path d='M12 11v5M12 7.5v.1' /></>,
    plus: <path d='M12 5v14M5 12h14' />, close: <path d='m6 6 12 12M18 6 6 18' />, check: <path d='m5 12 4 4L19 6' />,
    shield: <path d='M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10Z' />,
    timer: <><path d='M10 2h4M12 14l3-3' /><circle cx='12' cy='14' r='8' /></>,
    route: <><path d='M4 12h5c3 0 3-6 6-6h4M9 12c3 0 3 6 6 6h4' /><path d='m17 3 3 3-3 3m0 9 3 3-3 3' /></>,
    chevron: <path d='m6 9 6 6 6-6' />,
  }
  return <svg aria-hidden='true' className='re-icon' width={size} height={size} viewBox='0 0 24 24' fill='none' stroke='currentColor' strokeWidth='1.8' strokeLinecap='round' strokeLinejoin='round'>{paths[name]}</svg>
}

export function ErrorChoice<Value extends string>({ label, value, options, onChange, disabled = false }: {
  label: string; value: Value; options: readonly { value: Value; label: string }[]; onChange(value: Value): void; disabled?: boolean
}) {
  return <Select value={value} onValueChange={next => { if (next !== null) onChange(next) }} disabled={disabled}>
    <SelectTrigger aria-label={label} className='re-select'><SelectValue>{options.find(item => item.value === value)?.label || value}</SelectValue></SelectTrigger>
    <SelectContent>{options.map(item => <SelectItem key={item.value} value={item.value}>{item.label}</SelectItem>)}</SelectContent>
  </Select>
}

export function ErrorNumber({ label, value, onChange, unit, min = 0, max = 300, disabled = false, compact = false, hint }: {
  label: string; value: number; onChange(value: number): void; unit: string; min?: number; max?: number; disabled?: boolean; compact?: boolean; hint?: string
}) {
  return <Field className={`re-number${compact ? ' is-compact' : ''}`} orientation={compact ? 'horizontal' : 'vertical'}>
    <FieldLabel title={hint}>{label}</FieldLabel><InputGroup><InputGroupInput aria-label={label} type='number' min={min} max={max} step={1} value={value} disabled={disabled}
      onChange={event => { const next = Number(event.currentTarget.value); if (Number.isFinite(next)) onChange(next) }} /><InputGroupAddon align='inline-end'>{unit}</InputGroupAddon></InputGroup>
  </Field>
}

export function ErrorToggle({ label, description, checked, onChange, disabled = false }: {
  label: string; description?: string; checked: boolean; onChange(value: boolean): void; disabled?: boolean
}) {
  const id = React.useId()
  return <div className='re-toggle'><div><label htmlFor={id}>{label}</label>{description ? <p>{description}</p> : null}</div>
    <Switch id={id} checked={checked} disabled={disabled} onCheckedChange={onChange} /></div>
}

export function ErrorText({ label, value, onChange, placeholder = '' }: { label: string; value: string; onChange(value: string): void; placeholder?: string }) {
  return <Field><FieldLabel>{label}</FieldLabel><Input aria-label={label} value={value} placeholder={placeholder} onChange={event => onChange(event.currentTarget.value)} /></Field>
}
