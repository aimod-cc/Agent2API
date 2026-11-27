import * as React from 'react'
import {
  Badge, NavItem, NavList, Select, SelectContent, SelectGroup, SelectItem, SelectLabel,
  SelectTrigger, SelectValue, StatusDot, useMediaQuery,
} from '@ui'
import { t } from '../i18n'

export type PluginRailKey =
  | 'installed'
  | 'updatable'
  | 'error'
  | 'all'
  | 'community'
  | 'magpie-official'
  | 'magpie-recommended'
  | 'magpie-third-party'
  | 'scheduled'

export type PluginRailCounts = {
  installed: number
  updatable: number
  error: number
  all: number
  community: number
  magpieOfficial: number
  magpieRecommended: number
  magpieThirdParty: number
  scheduled: number
  scheduledUpdatable: number
}

export function pluginRailTitle(key: PluginRailKey): string {
  switch (key) {
    case 'installed': return t('已安装插件')
    case 'updatable': return t('可更新插件')
    case 'error': return t('启动异常')
    case 'all': return t('全部插件')
    case 'community': return t('Agent2API 社区')
    case 'magpie-official': return t('Magpie 官方')
    case 'magpie-recommended': return t('Magpie 推荐')
    case 'magpie-third-party': return t('Magpie 第三方')
    case 'scheduled': return t('定时任务插件')
  }
}

export function pluginRailSubtitle(key: PluginRailKey): string {
  switch (key) {
    case 'community': return t('独立插件，来自项目插件仓库')
    case 'magpie-official':
    case 'magpie-recommended':
    case 'magpie-third-party':
      return t('Magpie 官方与社区插件，共用一个按需下载的运行时')
    case 'scheduled':
      return t('沙箱轻脚本插件，按声明自动渲染表单与操作按钮')
    default:
      return ''
  }
}

export const PluginCategoryRail = React.memo(function PluginCategoryRail({
  value,
  counts,
  scriptsAvailable,
  onChange,
}: {
  value: PluginRailKey
  counts: PluginRailCounts
  scriptsAvailable: boolean
  onChange: (next: PluginRailKey) => void
}) {
  const compact = useMediaQuery('(max-width: 960px)')

  const currentCount = value === 'installed' ? counts.installed
    : value === 'updatable' ? counts.updatable
      : value === 'error' ? counts.error
        : value === 'all' ? counts.all
          : value === 'community' ? counts.community
            : value === 'magpie-official' ? counts.magpieOfficial
              : value === 'magpie-recommended' ? counts.magpieRecommended
                : value === 'scheduled' ? counts.scheduled
                  : counts.magpieThirdParty

  if (compact) {
    return (
      <div className='plugins-rail-mobile'>
        <Select value={value} onValueChange={next => { if (next) onChange(next as PluginRailKey) }}>
          <SelectTrigger className='w-full text-xs' aria-label={t('插件分类')}>
            <SelectValue>{`${pluginRailTitle(value)} (${currentCount})`}</SelectValue>
          </SelectTrigger>
          <SelectContent>
            <SelectGroup>
              <SelectLabel>{t('我的插件')}</SelectLabel>
              <SelectItem value='installed'>{`${t('已安装')} (${counts.installed})`}</SelectItem>
              <SelectItem value='updatable'>{`${t('可更新')} (${counts.updatable})`}</SelectItem>
              {counts.error > 0 ? <SelectItem value='error'>{`${t('启动异常')} (${counts.error})`}</SelectItem> : null}
            </SelectGroup>
            <SelectGroup>
              <SelectLabel>{t('插件商店')}</SelectLabel>
              <SelectItem value='all'>{`${t('全部插件')} (${counts.all})`}</SelectItem>
              <SelectItem value='community'>{`${t('Agent2API 社区')} (${counts.community})`}</SelectItem>
              {scriptsAvailable ? <>
                <SelectItem value='magpie-official'>{`${t('Magpie 官方')} (${counts.magpieOfficial})`}</SelectItem>
                <SelectItem value='magpie-recommended'>{`${t('Magpie 推荐')} (${counts.magpieRecommended})`}</SelectItem>
                <SelectItem value='magpie-third-party'>{`${t('Magpie 第三方')} (${counts.magpieThirdParty})`}</SelectItem>
              </> : null}
            </SelectGroup>
            <SelectGroup>
              <SelectLabel>{t('其他插件')}</SelectLabel>
              <SelectItem value='scheduled'>{`${t('定时任务')} (${counts.scheduled})`}</SelectItem>
            </SelectGroup>
          </SelectContent>
        </Select>
      </div>
    )
  }

  return (
    <aside className='plugins-category-rail' aria-label={t('插件分类')}>
      <div>
        <div className='plugins-category-heading'>{t('我的插件')}</div>
        <NavList>
          <NavItem active={value === 'installed'} count={counts.installed} onClick={() => onChange('installed')}>
            {t('已安装')}
          </NavItem>
          <NavItem
            active={value === 'updatable'}
            trailing={
              counts.updatable > 0 ? (
                <Badge variant='warning' shape='tag' className='px-1.5 text-[10.5px]'>
                  {counts.updatable}
                </Badge>
              ) : (
                <span className='text-[11px] tabular-nums text-muted-foreground'>0</span>
              )
            }
            onClick={() => onChange('updatable')}
          >
            {t('可更新')}
          </NavItem>
          {counts.error > 0 ? (
            <NavItem
              active={value === 'error'}
              trailing={
                <Badge variant='destructive' shape='tag' className='px-1.5 text-[10.5px]'>
                  {counts.error}
                </Badge>
              }
              onClick={() => onChange('error')}
            >
              {t('启动异常')}
            </NavItem>
          ) : null}
        </NavList>
      </div>

      <div className='border-t border-hairline pt-2.5'>
        <div className='plugins-category-heading'>{t('插件商店')}</div>
        <NavList>
          <NavItem active={value === 'all'} count={counts.all} onClick={() => onChange('all')}>
            {t('全部插件')}
          </NavItem>
          <NavItem active={value === 'community'} count={counts.community} onClick={() => onChange('community')}>
            {t('Agent2API 社区')}
          </NavItem>
          {scriptsAvailable ? <>
            <NavItem active={value === 'magpie-official'} count={counts.magpieOfficial} onClick={() => onChange('magpie-official')}>
              {t('Magpie 官方')}
            </NavItem>
            <NavItem active={value === 'magpie-recommended'} count={counts.magpieRecommended} onClick={() => onChange('magpie-recommended')}>
              {t('Magpie 推荐')}
            </NavItem>
            <NavItem active={value === 'magpie-third-party'} count={counts.magpieThirdParty} onClick={() => onChange('magpie-third-party')}>
              {t('Magpie 第三方')}
            </NavItem>
          </> : null}
        </NavList>
      </div>

      <div className='border-t border-hairline pt-2.5'>
        <div className='plugins-category-heading'>{t('其他插件')}</div>
        <NavList>
          <NavItem
            active={value === 'scheduled'}
            count={counts.scheduled}
            trailing={
              counts.scheduledUpdatable > 0 ? (
                <span
                  className='inline-flex items-center'
                  title={t('有 {count} 个插件可更新', { count: counts.scheduledUpdatable })}
                  aria-label={t('有 {count} 个插件可更新', { count: counts.scheduledUpdatable })}
                >
                  <StatusDot variant='warning' />
                </span>
              ) : undefined
            }
            onClick={() => onChange('scheduled')}
          >
            {t('定时任务')}
          </NavItem>
        </NavList>
      </div>
    </aside>
  )
})
