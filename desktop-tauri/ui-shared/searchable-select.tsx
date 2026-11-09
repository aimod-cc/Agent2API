import * as React from 'react'
import {
  Combobox, ComboboxContent, ComboboxEmpty, ComboboxInput, ComboboxItem,
  ComboboxList, ComboboxTrigger, ComboboxValue,
  Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
} from '@ui'
import { t } from '../ui-kit/src/i18n'

export type SearchableSelectOption = {
  value: string
  label: string
  disabled?: boolean
  searchText?: string
}

type SearchableSelectProps = {
  value: string
  onValueChange: (value: string) => void
  options: readonly SearchableSelectOption[]
  placeholder?: string
  displayValue?: string
  searchPlaceholder?: string
  emptyHint?: string
  searchable?: boolean
  disabled?: boolean
  className?: string
  id?: string
  title?: string
  'aria-label'?: string
}

export function SearchableSelect({
  value, onValueChange, options, placeholder = '', displayValue,
  searchPlaceholder = t('搜索…'), emptyHint = t('没有匹配的选项'),
  disabled, className, id, title, searchable = true, ...props
}: SearchableSelectProps) {
  const [search, setSearch] = React.useState('')
  const selected = options.find(option => option.value === value) ?? null
  const label = displayValue ?? selected?.label ?? (value || placeholder)

  if (!searchable) return <Select value={value} disabled={disabled}
    onValueChange={next => { if (typeof next === 'string') onValueChange(next) }}>
    <SelectTrigger id={id} className={className} title={title ?? label} aria-label={props['aria-label']}>
      <SelectValue>{label}</SelectValue>
    </SelectTrigger>
    <SelectContent>{options.map(item => <SelectItem key={item.value} value={item.value} disabled={item.disabled}>
      {item.label}
    </SelectItem>)}</SelectContent>
  </Select>

  return <Combobox items={options} value={selected} disabled={disabled} autoHighlight
    inputValue={search} onInputValueChange={setSearch} onOpenChange={() => setSearch('')}
    itemToStringLabel={item => item.label} itemToStringValue={item => item.value}
    isItemEqualToValue={(item, current) => item.value === current.value}
    filter={(item, query) => `${item.label} ${item.value} ${item.searchText || ''}`.toLowerCase().includes(query.trim().toLowerCase())}
    onValueChange={item => { if (item && !item.disabled) onValueChange(item.value) }}>
    <ComboboxTrigger id={id} className={className} title={title ?? label} aria-label={props['aria-label']}>
      <ComboboxValue>{label}</ComboboxValue>
    </ComboboxTrigger>
    <ComboboxContent>
      <div className='shrink-0 border-b border-hairline p-2'>
        <ComboboxInput placeholder={searchPlaceholder} aria-label={searchPlaceholder} autoComplete='off' />
      </div>
      <ComboboxEmpty>{emptyHint}</ComboboxEmpty>
      <ComboboxList>
        {(item: SearchableSelectOption) => <ComboboxItem key={item.value} value={item} disabled={item.disabled} title={item.label}>
          {item.label}
        </ComboboxItem>}
      </ComboboxList>
    </ComboboxContent>
  </Combobox>
}
