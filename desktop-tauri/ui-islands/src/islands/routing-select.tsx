import * as React from 'react'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@ui'
import { SearchableSelect } from '../../../ui-shared/searchable-select'

export function RoutingSelect({ value, options, label, disabled, onChange, searchPlaceholder }: {
  value: string; options: Array<[string, string]>; label: string; disabled?: boolean; onChange(value: string): void
  searchPlaceholder?: string
}) {
  const text = options.find(option => option[0] === value)?.[1] || value
  if (searchPlaceholder) return <SearchableSelect value={value} disabled={disabled} onValueChange={onChange}
    options={options.map(([value, label]) => ({ value, label }))} displayValue={text}
    searchPlaceholder={searchPlaceholder} aria-label={label} />
  return <Select value={value} disabled={disabled} onValueChange={value => onChange(String(value))}>
    <SelectTrigger aria-label={label} title={text}><SelectValue>{text}</SelectValue></SelectTrigger>
    <SelectContent>{options.map(([key, label]) => <SelectItem key={key} value={key}>{label}</SelectItem>)}</SelectContent>
  </Select>
}
