import * as React from 'react'
import { NavItem, NavList } from '@ui'
import { ProviderLogo } from './checkin-row'
import { t } from '../i18n'

/**
 * 签到中心的**提供商分类栏**（左栏）。
 *
 * ── 它为什么在这里 ──────────────────────────────────────────
 * 「每日签到 / 新手福利 / 活动福利 / 成长任务」是**任务**，提供商是同一任务下的
 * **分类**。所以左栏不是固定的：切到某个任务时，只列这个任务里真正有账号的提供商
 * —— 否则用户会在「成长任务」下面看到一堆点进去空空如也的家。
 *
 * ── 计数口径 ────────────────────────────────────────────────
 * 每家后面是 `已完成 / 总数`。计数由调用方按**当前任务**算好传进来（各任务的
 * 「完成」判据不同：签到看今日已签、福利看是否领完、成长看任务树），这里只负责
 * 画。没完成的用警示色，不额外挂徽标 —— 徽标会把提供商名字挤断。
 */

export type ProviderOption = {
  id: string
  label: string
  /** 已完成的账号数；`null` = 这一档没有「完成」概念（活动福利按领取算，逐行才知道） */
  done: number | null
  /** 这一家在**当前任务**里的账号总数 */
  total: number
}

export function ProviderRail({ options, selected, total, onSelect }: {
  options: ProviderOption[]
  selected: string
  total: { done: number; all: number }
  onSelect: (id: string) => void
}): React.ReactElement {
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
        {options.map(option => (
          <NavItem
            key={option.id}
            active={selected === option.id}
            icon={<ProviderLogo id={option.id} label={option.label} />}
            trailing={<Count done={option.done} total={option.total} />}
            onClick={() => onSelect(option.id)}
          >
            {t(option.label)}
          </NavItem>
        ))}
      </NavList>
      <p className='ck-provider-note'>{t('只列当前任务里有账号的提供商。')}</p>
    </aside>
  )
}

function Count({ done, total }: { done: number | null; total: number }): React.ReactElement {
  return <span className={`ck-provider-count${done !== null && done < total ? ' pending' : ''}`}>
    {done === null ? total : `${done}/${total}`}
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
