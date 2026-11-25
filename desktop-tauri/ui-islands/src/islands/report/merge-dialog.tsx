import * as React from 'react'
import {
  Button,
  Combobox,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxInput,
  ComboboxItem,
  ComboboxList,
  ComboboxTrigger,
  ComboboxValue,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
} from '@ui'
import { formatTokens } from '../report-charts'
import { t } from '../../i18n'
import type { MergeKind, MergeOptionItem, MergeRules } from './report-merge'

type TargetOption = { value: string; label: string }

function MergeTargetPicker({
  sourceLabel,
  kind,
  value,
  valueLabel,
  options,
  onChange,
}: {
  sourceLabel: string
  kind: MergeKind
  value: string
  valueLabel: string
  options: readonly TargetOption[]
  onChange: (nextTarget: string) => void
}) {
  const [search, setSearch] = React.useState('')
  const keepLabel = t('保持原名')
  const items = React.useMemo<TargetOption[]>(
    () => [{ value: '', label: keepLabel }, ...options],
    [keepLabel, options],
  )
  const selected = items.find(item => item.value === value)
    ?? (value ? { value, label: valueLabel || value } : items[0])
  const searchPlaceholder = kind === 'model' ? t('搜索目标模型') : t('搜索目标提供商')
  const emptyText = kind === 'model' ? t('没有可选的模型') : t('没有可选的提供商')

  return <Combobox
    items={items}
    value={selected}
    inputValue={search}
    onInputValueChange={setSearch}
    onOpenChange={() => setSearch('')}
    itemToStringLabel={item => item.label}
    itemToStringValue={item => item.value}
    isItemEqualToValue={(a, b) => a.value === b.value}
    onValueChange={item => { if (item) onChange(item.value) }}
  >
    <ComboboxTrigger
      className='report-merge-picker'
      aria-label={t('选择 {name} 的合并目标', { name: sourceLabel })}
      title={value ? valueLabel : keepLabel}
    >
      <ComboboxValue>
        {value ? <span className='font-medium text-foreground'>{valueLabel}</span> : <span data-placeholder=''>{t('合并为…')}</span>}
      </ComboboxValue>
    </ComboboxTrigger>
    <ComboboxContent>
      <ComboboxInput placeholder={searchPlaceholder} aria-label={searchPlaceholder} autoComplete='off' />
      <ComboboxEmpty>{emptyText}</ComboboxEmpty>
      <ComboboxList>
        {(item: TargetOption) => <ComboboxItem key={item.value || '__keep'} value={item} title={item.label}>
          {item.label}
        </ComboboxItem>}
      </ComboboxList>
    </ComboboxContent>
  </Combobox>
}

export function ReportMergeDialog({
  open,
  onOpenChange,
  kind,
  items,
  rules,
  onRulesChange,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  kind: MergeKind
  items: readonly MergeOptionItem[]
  rules: MergeRules
  onRulesChange: (next: MergeRules) => void
}) {
  const [query, setQuery] = React.useState('')

  React.useEffect(() => {
    if (!open) setQuery('')
  }, [open, kind])

  const isModel = kind === 'model'
  const ruleCount = Object.keys(rules).length

  const labelByKey = React.useMemo(() => {
    const map = new Map<string, string>()
    for (const item of items) map.set(item.key, item.label)
    return map
  }, [items])

  const incomingCounts = React.useMemo(() => {
    const map = new Map<string, number>()
    for (const target of Object.values(rules)) {
      map.set(target, (map.get(target) || 0) + 1)
    }
    return map
  }, [rules])

  const mergedSources = React.useMemo(() => new Set(Object.keys(rules)), [rules])

  const filtered = React.useMemo(() => {
    const keyword = query.trim().toLowerCase()
    if (!keyword) return items
    return items.filter(item => {
      const targetKey = rules[item.key] || ''
      const targetLabel = targetKey ? (labelByKey.get(targetKey) || targetKey) : ''
      return item.label.toLowerCase().includes(keyword)
        || item.key.toLowerCase().includes(keyword)
        || targetLabel.toLowerCase().includes(keyword)
    })
  }, [items, query, rules, labelByKey])

  function setTarget(sourceKey: string, targetKey: string) {
    const next = { ...rules }
    if (targetKey) next[sourceKey] = targetKey
    else delete next[sourceKey]
    onRulesChange(next)
  }

  const title = isModel ? t('模型合并管理') : t('提供商合并管理')
  const hint = isModel
    ? t('选择合并目标模型后，报表中的模型用量、Top 模型与趋势图将同步合并统计。保持原名即不合并。')
    : t('选择合并目标提供商后，报表中的提供商用量与 Top 提供商将同步合并统计。保持原名即不合并。')
  const searchPlaceholder = isModel ? t('搜索模型…') : t('搜索提供商…')
  const emptyListText = items.length === 0
    ? (isModel ? t('当前范围内暂无可合并的模型') : t('当前范围内暂无可合并的提供商'))
    : (isModel ? t('没有匹配的模型') : t('没有匹配的提供商'))

  return <Dialog open={open} onOpenChange={onOpenChange}>
    <DialogContent className='report-merge-dialog'>
      <DialogHeader>
        <DialogTitle>{title}</DialogTitle>
      </DialogHeader>
      <DialogBody className='gap-3'>
        <div className='report-merge-head'>
          <p className='report-merge-hint'>{hint}</p>
          {ruleCount > 0 ? <div className='report-merge-summary'>
            <span>{t('已合并 {n} 项', { n: ruleCount })}</span>
            <Button type='button' variant='ghost' size='xs' onClick={() => onRulesChange({})}>{t('清空')}</Button>
          </div> : null}
        </div>

        <InputGroup className='h-[30px]'>
          <InputGroupAddon align='inline-start'>
            <svg viewBox='0 0 12 12' className='size-3 text-muted-foreground' aria-hidden='true'>
              <circle cx='5' cy='5' r='3.4' stroke='currentColor' strokeWidth='1.3' fill='none' />
              <path d='M7.6 7.6 10.5 10.5' stroke='currentColor' strokeWidth='1.3' strokeLinecap='round' />
            </svg>
          </InputGroupAddon>
          <InputGroupInput
            placeholder={searchPlaceholder}
            aria-label={searchPlaceholder}
            value={query}
            onChange={event => setQuery(event.currentTarget.value)}
          />
          {query ? <InputGroupAddon align='inline-end'>
            <InputGroupButton type='button' size='icon-xs' aria-label={t('清空搜索')} onClick={() => setQuery('')}>
              ×
            </InputGroupButton>
          </InputGroupAddon> : null}
        </InputGroup>

        <div className='report-merge-list'>
          {filtered.length ? filtered.map(item => {
            const currentTarget = rules[item.key] || ''
            const currentTargetLabel = currentTarget ? (labelByKey.get(currentTarget) || currentTarget) : ''
            const incoming = incomingCounts.get(item.key) || 0
            const options: TargetOption[] = items
              .filter(candidate => candidate.key !== item.key && !mergedSources.has(candidate.key))
              .map(candidate => ({ value: candidate.key, label: candidate.label }))

            return <div key={item.key} className={`report-merge-row${currentTarget ? ' is-active' : ''}`}>
              <div className='report-merge-source'>
                <span className={`report-merge-source-name${isModel ? ' is-mono' : ''}`} title={item.label}>
                  {item.label}
                </span>
                <span className='report-merge-source-meta'>
                  {item.inactive ? t('当前范围无用量') : formatTokens(item.tokens)}
                </span>
              </div>
              <span className='report-merge-arrow' aria-hidden='true'>→</span>
              {incoming > 0 ? <span
                className='report-merge-locked'
                title={isModel ? t('已有其他模型合并到此项，如需修改请先取消指向它的规则') : t('已有其他提供商合并到此项，如需修改请先取消指向它的规则')}
              >
                {t('已归入 {n} 项', { n: incoming })}
              </span> : <MergeTargetPicker
                sourceLabel={item.label}
                kind={kind}
                value={currentTarget}
                valueLabel={currentTargetLabel}
                options={options}
                onChange={nextTarget => setTarget(item.key, nextTarget)}
              />}
              <Button
                type='button'
                variant='ghost'
                size='icon-xs'
                className='report-merge-clear'
                aria-label={t('取消 {name} 的合并', { name: item.label })}
                title={t('恢复原名')}
                onClick={() => setTarget(item.key, '')}
              >
                ×
              </Button>
            </div>
          }) : <div className='report-merge-empty'>{emptyListText}</div>}
        </div>
      </DialogBody>
      <DialogFooter>
        <Button type='button' size='sm' onClick={() => onOpenChange(false)}>{t('完成')}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}
