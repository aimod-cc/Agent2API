import * as React from 'react'
import {
  Badge,
  BadgeDot,
  Button,
  CardHeader,
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
  Popover,
  PopoverContent,
  PopoverHeader,
  PopoverTitle,
  PopoverTrigger,
  SegmentedControl,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Spinner,
  Switch,
  Tooltip,
  TooltipContent,
  TooltipTrigger,
  cn,
} from '@ui'
import { t } from '../i18n'
import { DateRangeFields } from './date-range-fields'
import {
  LEVEL_OPTIONS,
  RANGE_OPTIONS,
  VIEW_OPTIONS,
  logCategory,
  type Filters,
} from './logs-model'

function SearchSvg() {
  return (
    <svg width='13' height='13' viewBox='0 0 16 16' fill='none' stroke='currentColor' strokeWidth='1.5' strokeLinecap='round' aria-hidden='true'>
      <circle cx='7.2' cy='7.2' r='4.6' />
      <path d='m10.8 10.8 2.8 2.8' />
    </svg>
  )
}

function CalendarSvg() {
  return (
    <svg width='13' height='13' viewBox='0 0 16 16' fill='none' stroke='currentColor' strokeWidth='1.5' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
      <rect x='2.2' y='3.2' width='11.6' height='10.6' rx='1.8' />
      <path d='M5 2v2.4M11 2v2.4M2.2 6.4h11.6' />
    </svg>
  )
}

function FilterSvg() {
  return (
    <svg width='13' height='13' viewBox='0 0 16 16' fill='none' stroke='currentColor' strokeWidth='1.5' strokeLinecap='round' aria-hidden='true'>
      <path d='M2.5 4h11M4.5 8h7M6.5 12h3' />
    </svg>
  )
}

function ExportSvg() {
  return (
    <svg width='13' height='13' viewBox='0 0 16 16' fill='none' stroke='currentColor' strokeWidth='1.5' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
      <path d='M8 2.5v7.5M5.2 7.2 8 10l2.8-2.8' />
      <path d='M3 11.5v1.3a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1v-1.3' />
    </svg>
  )
}

function TrashSvg() {
  return (
    <svg width='13' height='13' viewBox='0 0 16 16' fill='none' stroke='currentColor' strokeWidth='1.5' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
      <path d='M2.8 4.4h10.4M6 4.4V3a.8.8 0 0 1 .8-.8h2.4a.8.8 0 0 1 .8.8v1.4' />
      <path d='M4.2 4.4 4.8 13a1 1 0 0 0 1 .9h4.4a1 1 0 0 0 1-.9l.6-8.6' />
    </svg>
  )
}

export function LogsToolbar({
  filters,
  categories,
  counts,
  matched,
  busy,
  loading,
  hasError,
  onChange,
  onExport,
  onClear,
}: {
  filters: Filters
  categories: string[]
  counts: Record<string, number>
  matched: number
  busy: 'clear' | 'export' | null
  loading: boolean
  hasError: boolean
  onChange: (patch: Partial<Filters>, debounce?: boolean) => void
  onExport: () => void
  onClear: () => void
}) {
  const rangeLabel = RANGE_OPTIONS.find(o => o.value === filters.range)?.label || t('今天')
  const levelLabel = LEVEL_OPTIONS.find(o => o.value === filters.level)?.label || t('全部级别')
  const categoryLabel = filters.category ? logCategory(filters.category, t) : t('全部分类')

  const errorCount = counts.error || 0
  const warnCount = counts.warn || 0
  const debugCount = counts.debug || 0
  const errActive = filters.view !== 'debug' && filters.level === 'error'
  const warnActive = filters.view !== 'debug' && filters.level === 'warn'

  const popoverCount =
    (filters.view !== 'debug' && filters.level ? 1 : 0) +
    (filters.category ? 1 : 0) +
    (!filters.aggregate ? 1 : 0)
  const hasResettable = popoverCount > 0 || filters.range === 'custom'

  const resetPopover = () => {
    onChange({
      level: '',
      category: '',
      aggregate: true,
      ...(filters.range === 'custom' ? { range: 'today' } : {}),
    })
  }

  const chips: { key: string; label: string; clear: () => void }[] = []
  if (filters.view !== 'debug' && filters.level) {
    chips.push({ key: 'level', label: `${t('级别')}：${levelLabel}`, clear: () => onChange({ level: '' }) })
  }
  if (filters.category) {
    chips.push({ key: 'category', label: `${t('分类')}：${categoryLabel}`, clear: () => onChange({ category: '' }) })
  }
  if (!filters.aggregate) {
    chips.push({ key: 'aggregate', label: `${t('折叠重复日志')}：${t('已关闭')}`, clear: () => onChange({ aggregate: true }) })
  }
  const showBar = filters.range === 'custom' || chips.length > 0

  return (
    <>
      <CardHeader className='event-log-bar'>
        <SegmentedControl
          options={VIEW_OPTIONS}
          value={filters.view}
          onValueChange={view => onChange({ view })}
          aria-label={t('日志视图')}
          className='shrink-0'
        />

        <Select value={filters.range} onValueChange={value => onChange({ range: String(value ?? 'today') })}>
          <SelectTrigger className='w-auto min-w-[98px]' aria-label={t('事件日志时间范围')}>
            <span className='flex flex-none items-center text-muted-foreground' aria-hidden='true'>
              <CalendarSvg />
            </span>
            <SelectValue>{rangeLabel}</SelectValue>
          </SelectTrigger>
          <SelectContent>
            {RANGE_OPTIONS.map(option => (
              <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>
            ))}
          </SelectContent>
        </Select>

        <InputGroup className='event-log-search w-auto min-w-[155px] flex-1'>
          <InputGroupInput
            type='search'
            placeholder={t('搜索插件、消息、提供商或账号…')}
            aria-label={t('搜索插件、消息、提供商或账号')}
            autoComplete='off'
            value={filters.keyword}
            onChange={event => onChange({ keyword: event.currentTarget.value }, true)}
          />
          <InputGroupAddon align='inline-start' aria-hidden='true'>
            <SearchSvg />
          </InputGroupAddon>
          {filters.keyword ? (
            <InputGroupAddon align='inline-end'>
              <InputGroupButton size='icon-xs' aria-label={t('清空')} onClick={() => onChange({ keyword: '' })}>
                ×
              </InputGroupButton>
            </InputGroupAddon>
          ) : null}
        </InputGroup>

        <Popover>
          <PopoverTrigger
            render={
              <Button
                variant='outline'
                type='button'
                className={cn(popoverCount > 0 && 'border-primary-bd bg-primary-soft text-primary-fg')}
                aria-label={t('筛选与显示')}
              >
                <FilterSvg />
                <span>{popoverCount > 0 ? t('筛选 ({n})', { n: popoverCount }) : t('筛选')}</span>
              </Button>
            }
          />
          <PopoverContent align='end' side='bottom' className='w-[300px] p-0'>
            <PopoverHeader>
              <PopoverTitle>{t('筛选与显示')}</PopoverTitle>
              {hasResettable ? (
                <Button variant='ghost' size='xs' type='button' className='ml-auto' onClick={resetPopover}>
                  {t('重置')}
                </Button>
              ) : null}
            </PopoverHeader>
            <div className='flex flex-col gap-3 p-3'>
              {filters.view !== 'debug' ? (
                <div className='flex flex-col gap-1'>
                  <span className='text-[11px] text-muted-foreground'>{t('最低日志级别')}</span>
                  <Select value={filters.level} onValueChange={value => onChange({ level: String(value ?? '') })}>
                    <SelectTrigger className='w-full' aria-label={t('按最低级别筛选')}>
                      <SelectValue>{levelLabel}</SelectValue>
                    </SelectTrigger>
                    <SelectContent>
                      {LEVEL_OPTIONS.map(option => (
                        <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
              ) : null}
              <div className='flex flex-col gap-1'>
                <span className='text-[11px] text-muted-foreground'>{t('事件分类')}</span>
                <Select value={filters.category} onValueChange={value => onChange({ category: String(value ?? '') })}>
                  <SelectTrigger className='w-full' aria-label={t('按分类筛选')}>
                    <SelectValue>{categoryLabel}</SelectValue>
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value=''>{t('全部分类')}</SelectItem>
                    {categories.map(category => (
                      <SelectItem key={category} value={category}>{logCategory(category, t)}</SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <div className='flex items-center justify-between border-t border-hairline pt-2.5'>
                <span className='text-[12px] text-foreground'>{t('折叠重复日志')}</span>
                <Switch
                  checked={filters.aggregate}
                  onCheckedChange={aggregate => onChange({ aggregate })}
                  aria-label={t('折叠重复日志')}
                />
              </div>
            </div>
          </PopoverContent>
        </Popover>

        <span className='event-log-sep' aria-hidden='true' />

        <div className='flex flex-wrap items-center gap-1.5'>
          <Tooltip>
            <TooltipTrigger
              render={
                <Badge
                  render={<button type='button' />}
                  variant='destructive'
                  shape='pill'
                  className='event-log-stat-badge tabular-nums'
                  data-active={errActive ? 'true' : undefined}
                  data-zero={errorCount === 0 ? 'true' : undefined}
                  onClick={() =>
                    onChange({
                      view: filters.view === 'debug' ? 'important' : filters.view,
                      level: errActive ? '' : 'error',
                    })
                  }
                >
                  <BadgeDot />
                  <span>{t('错误 {n}', { n: errorCount })}</span>
                </Badge>
              }
            />
            <TooltipContent>{errActive ? t('点击取消「仅错误」筛选') : t('点击快捷筛选：仅看错误')}</TooltipContent>
          </Tooltip>

          <Tooltip>
            <TooltipTrigger
              render={
                <Badge
                  render={<button type='button' />}
                  variant='warning'
                  shape='pill'
                  className='event-log-stat-badge tabular-nums'
                  data-active={warnActive ? 'true' : undefined}
                  data-zero={warnCount === 0 ? 'true' : undefined}
                  onClick={() =>
                    onChange({
                      view: filters.view === 'debug' ? 'important' : filters.view,
                      level: warnActive ? '' : 'warn',
                    })
                  }
                >
                  <BadgeDot />
                  <span>{t('警告 {n}', { n: warnCount })}</span>
                </Badge>
              }
            />
            <TooltipContent>{warnActive ? t('点击取消「警告及以上」筛选') : t('点击快捷筛选：警告及以上')}</TooltipContent>
          </Tooltip>

          {filters.view !== 'important' ? (
            <Tooltip>
              <TooltipTrigger
                render={
                  <Badge
                    render={<button type='button' />}
                    variant='outline'
                    shape='pill'
                    className='event-log-stat-badge tabular-nums'
                    data-active={filters.view === 'debug' ? 'true' : undefined}
                    data-zero={debugCount === 0 ? 'true' : undefined}
                    onClick={() => onChange({ view: filters.view === 'debug' ? 'all' : 'debug' })}
                  >
                    <span>{t('调试 {n}', { n: debugCount })}</span>
                  </Badge>
                }
              />
              <TooltipContent>{filters.view === 'debug' ? t('点击切回「全部事件」') : t('点击切至「调试详情」')}</TooltipContent>
            </Tooltip>
          ) : null}
        </div>

        <div className='flex items-center gap-1.5'>
          <Tooltip>
            <TooltipTrigger
              render={
                <Button
                  id='btn-logs-export'
                  variant='outline'
                  size='icon'
                  type='button'
                  disabled={busy !== null || loading || hasError}
                  aria-label={busy === 'export' ? t('导出中…') : t('导出当前筛选')}
                  onClick={onExport}
                >
                  {busy === 'export' ? <Spinner className='size-3.5' /> : <ExportSvg />}
                </Button>
              }
            />
            <TooltipContent>{busy === 'export' ? t('导出中…') : t('导出当前筛选')}</TooltipContent>
          </Tooltip>

          <Tooltip>
            <TooltipTrigger
              render={
                <Button
                  id='btn-logs-clear'
                  variant='destructive'
                  size='icon'
                  type='button'
                  disabled={busy !== null || loading || hasError || !matched}
                  aria-label={busy === 'clear' ? t('清空中…') : t('清空当前筛选')}
                  onClick={onClear}
                >
                  {busy === 'clear' ? <Spinner className='size-3.5' /> : <TrashSvg />}
                </Button>
              }
            />
            <TooltipContent>{busy === 'clear' ? t('清空中…') : t('清空当前筛选')}</TooltipContent>
          </Tooltip>
        </div>
      </CardHeader>

      {showBar ? (
        <div className='event-log-chips'>
          {filters.range === 'custom' ? (
            <DateRangeFields
              dates={filters.dates}
              onChange={dates => onChange({ dates })}
              onClear={() => onChange({ range: 'today' })}
            />
          ) : null}
          {chips.map(chip => (
            <span key={chip.key} className='event-log-chip'>
              <span>{chip.label}</span>
              <button type='button' onClick={chip.clear} aria-label={t('清除筛选')}>×</button>
            </span>
          ))}
          {hasResettable ? (
            <Button variant='ghost' size='xs' type='button' className='ml-auto' onClick={resetPopover}>
              {t('清除筛选')}
            </Button>
          ) : null}
        </div>
      ) : null}
    </>
  )
}
