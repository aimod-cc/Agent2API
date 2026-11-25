import * as React from 'react'
import { Button, Collapsible, CollapsibleContent, CollapsibleTrigger, NavItem, NavList } from '@ui'
import { ChevronIcon, ProviderLogo } from './checkin-row'
import { t } from '../i18n'

/**
 * 签到中心的**提供商分类栏**（左栏）。
 *
 * ── 它为什么在这里 ──────────────────────────────────────────
 * 「每日签到 / 福利与任务」是**功能档位**，提供商是同一档位下的
 * **分类**。所以左栏不是固定的：切到某个档位时，只列这个档位里真正有账号或任务的提供商
 * —— 否则用户会在列表里看到一堆点进去空空如也的家。
 *
 * ── 计数口径 ────────────────────────────────────────────────
 * 每家后面是 `已完成 / 总数`（或纯总数）。计数由调用方按**当前档位**算好传进来（各档位的
 * 「完成」判据不同：签到看今日已签、福利与任务按账号数或任务展示），这里只负责
 * 画。没完成的用警示色，不额外挂徽标 —— 徽标会把提供商名字挤断。
 */

export type ProviderOption = {
  id: string
  label: string
  /** 已完成的账号数；`null` = 这一档没有统一的「完成」口径（福利与任务按各自区块展示） */
  done: number | null
  /** 这一家在**当前档位**里的账号总数 */
  total: number
  /** 仅由插件自动化任务挂载（无逐账号列表），不归入「暂无账号」折叠组 */
  isTask?: boolean
}

export function ProviderRail({ options, selected, total, onSelect }: {
  options: ProviderOption[]
  selected: string
  total: { done: number | null; all: number }
  onSelect: (id: string) => void
}): React.ReactElement {
  const isEmptyOption = (option: ProviderOption) => !option.isTask && option.total === 0
  const activeOptions = options.filter(option => !isEmptyOption(option))
  const emptyOptions = options.filter(isEmptyOption)
  const [emptyExpanded, setEmptyExpanded] = React.useState(false)
  const hasSelectedEmpty = emptyOptions.some(option => option.id === selected)

  React.useEffect(() => {
    if (hasSelectedEmpty) setEmptyExpanded(true)
  }, [hasSelectedEmpty])

  const renderOption = (option: ProviderOption) => (
    <NavItem
      key={option.id}
      active={selected === option.id}
      icon={<ProviderLogo id={option.id} label={option.label} />}
      trailing={<Count done={option.done} total={option.total} taskOnly={option.isTask} />}
      onClick={() => onSelect(option.id)}
    >
      {t(option.label)}
    </NavItem>
  )

  return (
    <aside className='ck-provider-rail'>
      <div className='ck-provider-rail-head'>
        <h3>{t('提供商')}</h3>
        <span>{t('{n} 家', { n: options.length })}</span>
      </div>
      <NavList className='ck-provider-nav'>
        <NavItem
          active={selected === ''}
          icon={<AllIcon />}
          trailing={<Count done={total.done} total={total.all} />}
          onClick={() => onSelect('')}
        >
          {t('全部提供商')}
        </NavItem>
        {activeOptions.map(renderOption)}
        {emptyOptions.length > 0 ? (
          <Collapsible open={emptyExpanded} onOpenChange={setEmptyExpanded} className='ck-provider-empty-group'>
            <CollapsibleTrigger
              render={<Button variant='ghost' size='sm' className='ck-provider-empty-trigger w-full justify-start rounded-md' />}
            >
              <ChevronIcon open={emptyExpanded} />
              <span className='min-w-0 flex-1 truncate text-left'>{t('暂无账号')}</span>
              <span className='ck-provider-count'>{emptyOptions.length}</span>
            </CollapsibleTrigger>
            <CollapsibleContent>
              <div className='ck-provider-empty-list'>
                {emptyOptions.map(renderOption)}
              </div>
            </CollapsibleContent>
          </Collapsible>
        ) : null}
      </NavList>
      {emptyOptions.length === 0 ? (
        <p className='ck-provider-note'>{t('只列当前任务里有账号的提供商。')}</p>
      ) : null}
    </aside>
  )
}

function Count({ done, total, taskOnly }: { done: number | null; total: number; taskOnly?: boolean }): React.ReactElement {
  return <span className={`ck-provider-count${done !== null && done < total ? ' pending' : ''}`}>
    {done !== null ? `${done}/${total}` : taskOnly && total === 0 ? t('任务') : total}
  </span>
}

function AllIcon(): React.ReactElement {
  return (
    <span className='ck-provider-all-icon' aria-hidden='true'>
      <svg viewBox='0 0 24 24' width='15' height='15' fill='none' stroke='currentColor'
        strokeWidth='1.7' strokeLinecap='round' strokeLinejoin='round'>
        <rect x='4' y='4' width='7' height='7' rx='1' />
        <rect x='13' y='4' width='7' height='7' rx='1' />
        <rect x='4' y='13' width='7' height='7' rx='1' />
        <rect x='13' y='13' width='7' height='7' rx='1' />
      </svg>
    </span>
  )
}
