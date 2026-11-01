import * as React from 'react'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@ui'

export function RoutingSelect({ value, options, label, disabled, onChange }: {
  value: string; options: Array<[string, string]>; label: string; disabled?: boolean; onChange(value: string): void
}) {
  const text = options.find(option => option[0] === value)?.[1] || value
  return <Select value={value} disabled={disabled} onValueChange={value => onChange(String(value))}>
    <SelectTrigger aria-label={label} title={text}><SelectValue>{text}</SelectValue></SelectTrigger>
    <SelectContent>{options.map(([key, label]) => <SelectItem key={key} value={key}>{label}</SelectItem>)}</SelectContent>
  </Select>
}
