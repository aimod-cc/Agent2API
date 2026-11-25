import * as React from 'react'
import {
  Button, InputGroup, InputGroupAddon, InputGroupInput,
  Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
} from '@ui'
import { PluginPageTools } from './plugins-page-tools'
import { t } from '../i18n'
import type { PluginViewMode } from '../../../ui-shared/plugins-store'

export type PluginSortKey = 'default' | 'installed-first' | 'stars' | 'name'

function sortLabel(sort: PluginSortKey): string {
  switch (sort) {
    case 'default': return t('默认排序')
    case 'installed-first': return t('已安装优先')
    case 'stars': return t('按热门度')
    case 'name': return t('按名称')
  }
}

export const PluginPageToolbar = React.memo(function PluginPageToolbar({
  title,
  count,
  subtitle,
  search,
  sort,
  viewMode,
  selecting,
  locked,
  checking,
  scriptsAvailable,
  scriptActive,
  scheduledMode = false,
  updateCount,
  updateDisabled,
  onSearch,
  onSort,
  onViewMode,
  onSelect,
  onUpdateAll,
  onTaskImport,
  onTaskCreate,
  onFile,
  onMissing,
  onRefresh,
  onCheck,
  onSettings,
  onScript,
  onDiagnostics,
}: {
  title: string
  count: number
  subtitle?: string
  search: string
  sort: PluginSortKey
  viewMode: PluginViewMode
  selecting: boolean
  locked: boolean
  checking: boolean
  scriptsAvailable: boolean
  scriptActive: boolean
  scheduledMode?: boolean
  updateCount: number
  updateDisabled: boolean
  onSearch(value: string): void
  onSort(value: PluginSortKey): void
  onViewMode(value: PluginViewMode): void
  onSelect(): void
  onUpdateAll(): void
  onTaskImport?: () => void
  onTaskCreate?: () => void
  onFile(): void
  onMissing(): void
  onRefresh(): void
  onCheck(): void
  onSettings(): void
  onScript(): void
  onDiagnostics(): void
}) {
  return (
    <div className='plugins-toolbar' role='region' aria-label={t('插件筛选和操作')}>
      <div className='flex min-w-0 flex-1 items-baseline gap-2'>
        <h2 className='m-0 truncate text-[15px] font-semibold leading-5 text-foreground'>{title}</h2>
        <span className='shrink-0 text-[12px] tabular-nums text-muted-foreground'>{t('{count} 个', { count })}</span>
        {subtitle ? (
          <span className='hidden truncate text-[11.5px] text-muted-foreground xl:inline'>· {subtitle}</span>
        ) : null}
      </div>

      <div className='plugins-toolbar-actions'>
        <InputGroup className='plugins-search rounded-sm border-hairline bg-surface'>
          <InputGroupInput
            type='search'
            value={search}
            placeholder={scheduledMode ? t('搜索任务插件') : t('搜索插件')}
            aria-label={scheduledMode ? t('搜索任务插件') : t('搜索插件')}
            onChange={event => onSearch(event.currentTarget.value)}
          />
          <InputGroupAddon>
            <svg viewBox='0 0 16 16' className='size-3.5' fill='none' stroke='currentColor' strokeWidth='1.7' strokeLinecap='round' aria-hidden='true'>
              <circle cx='6.8' cy='6.8' r='4.6' />
              <path d='M10.3 10.3 14 14' />
            </svg>
          </InputGroupAddon>
        </InputGroup>

        {!scheduledMode ? <>
          <Select value={sort} onValueChange={next => { if (next) onSort(next as PluginSortKey) }}>
            <SelectTrigger className='h-[28px] w-[108px] px-2 text-xs' aria-label={t('排序方式')}>
              <SelectValue>{sortLabel(sort)}</SelectValue>
            </SelectTrigger>
            <SelectContent>
              <SelectItem value='default'>{t('默认排序')}</SelectItem>
              <SelectItem value='installed-first'>{t('已安装优先')}</SelectItem>
              <SelectItem value='stars'>{t('按热门度')}</SelectItem>
              <SelectItem value='name'>{t('按名称')}</SelectItem>
            </SelectContent>
          </Select>

          <div className='inline-flex items-center rounded-md border border-hairline bg-control p-0.5' role='group' aria-label={t('视图切换')}>
            <Button
              type='button'
              variant={viewMode === 'cards' ? 'secondary' : 'ghost'}
              size='icon-xs'
              aria-label={t('卡片视图')}
              title={t('卡片视图')}
              onClick={() => onViewMode('cards')}
            >
              <svg viewBox='0 0 24 24' className='size-3.5' fill='none' stroke='currentColor' strokeWidth='1.8' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
                <rect x='3.5' y='3.5' width='7' height='7' rx='1.5' />
                <rect x='13.5' y='3.5' width='7' height='7' rx='1.5' />
                <rect x='3.5' y='13.5' width='7' height='7' rx='1.5' />
                <rect x='13.5' y='13.5' width='7' height='7' rx='1.5' />
              </svg>
            </Button>
            <Button
              type='button'
              variant={viewMode === 'list' ? 'secondary' : 'ghost'}
              size='icon-xs'
              aria-label={t('列表视图')}
              title={t('列表视图')}
              onClick={() => onViewMode('list')}
            >
              <svg viewBox='0 0 24 24' className='size-3.5' fill='none' stroke='currentColor' strokeWidth='1.8' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
                <path d='M8 6h12M8 12h12M8 18h12' />
                <circle cx='4.2' cy='6' r='1' fill='currentColor' stroke='none' />
                <circle cx='4.2' cy='12' r='1' fill='currentColor' stroke='none' />
                <circle cx='4.2' cy='18' r='1' fill='currentColor' stroke='none' />
              </svg>
            </Button>
          </div>

          <Button variant={selecting ? 'secondary' : 'outline'} size='sm' disabled={locked} onClick={onSelect}>
            {selecting ? t('退出批量管理') : t('批量管理')}
          </Button>

          {updateCount > 0 ? (
            <Button size='sm' disabled={updateDisabled} title={t('有 {count} 个插件可更新', { count: updateCount })} onClick={onUpdateAll}>
              {t('全部更新')}
            </Button>
          ) : null}
        </> : <>
          {onTaskImport ? <Button variant='outline' size='sm' onClick={onTaskImport}>{t('导入脚本/包')}</Button> : null}
          {onTaskCreate ? <Button size='sm' onClick={onTaskCreate}>{t('新建定时任务插件')}</Button> : null}
        </>}

        <PluginPageTools
          disabled={locked}
          fileDisabled={locked}
          scriptDisabled={locked || scriptActive}
          scriptsAvailable={scriptsAvailable}
          checking={checking}
          onRefresh={onRefresh}
          onCheck={onCheck}
          onSettings={onSettings}
          onFile={onFile}
          onScript={onScript}
          onDiagnostics={onDiagnostics}
          onMissing={onMissing}
        />
      </div>
    </div>
  )
})
