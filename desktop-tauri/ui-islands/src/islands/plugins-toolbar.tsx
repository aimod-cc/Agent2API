import * as React from 'react'
import { Button, InputGroup, InputGroupAddon, InputGroupInput, SegmentedControl } from '@ui'
import { PluginPageTools } from './plugins-page-tools'
import { t } from '../i18n'
import type { PluginStatusFilter } from '../../../ui-shared/plugins-filters'

export type PluginSourceFilter = 'all' | 'agent2' | 'magpie'

function FilterGroup<T extends string>({ label, options, value, onChange }: {
  label: string; options: ReadonlyArray<{ value: T; label: string; count: number }>; value: T; onChange(value: T): void
}) {
  return <div className='plugins-filter-group'><span className='plugins-filter-label'>{label}</span><SegmentedControl options={options} value={value} onValueChange={onChange} aria-label={label} /></div>
}

export function PluginPageToolbar({ source, status, search, count, selecting, locked, checking, scriptsAvailable, scriptActive,
  onSource, onStatus, onSearch, onSelect, onFile, onMissing, onRefresh, onCheck, onSettings, onScript, onDiagnostics }: {
  source: PluginSourceFilter; status: PluginStatusFilter; search: string
  count(source: PluginSourceFilter, status: PluginStatusFilter): number
  selecting: boolean; locked: boolean; checking: boolean; scriptsAvailable: boolean; scriptActive: boolean
  onSource(value: PluginSourceFilter): void; onStatus(value: PluginStatusFilter): void
  onSearch(value: string): void; onSelect(): void; onFile(): void; onMissing(): void; onRefresh(): void; onCheck(): void; onSettings(): void; onScript(): void; onDiagnostics(): void
}) {
  return <div className='plugins-toolbar' role='region' aria-label={t('插件筛选和操作')}>
    <div className='plugins-toolbar-filters'>
      <FilterGroup label={t('来源')} value={source} onChange={onSource} options={[
        { value: 'all', label: t('全部'), count: count('all', status) },
        { value: 'agent2', label: t('Agent2API 社区'), count: count('agent2', status) },
        { value: 'magpie', label: t('Magpie 插件'), count: count('magpie', status) },
      ]} />
      <FilterGroup label={t('状态')} value={status} onChange={onStatus} options={[
        { value: 'all', label: t('全部'), count: count(source, 'all') },
        { value: 'updatable', label: t('可更新'), count: count(source, 'updatable') },
        { value: 'installed', label: t('已安装'), count: count(source, 'installed') },
        { value: 'missing', label: t('未安装'), count: count(source, 'missing') },
      ]} />
    </div>
    <div className='plugins-toolbar-actions'>
      <InputGroup className='plugins-search rounded-sm border-hairline bg-surface'>
        <InputGroupInput type='search' value={search} placeholder={t('搜索插件')} aria-label={t('搜索插件')} onChange={event => onSearch(event.currentTarget.value)} />
        <InputGroupAddon><svg viewBox='0 0 16 16' className='size-3.5' fill='none' stroke='currentColor' strokeWidth='1.7' strokeLinecap='round' aria-hidden='true'><circle cx='6.8' cy='6.8' r='4.6' /><path d='M10.3 10.3 14 14' /></svg></InputGroupAddon>
      </InputGroup>
      <Button variant={selecting ? 'secondary' : 'outline'} size='sm' disabled={locked} onClick={onSelect}>{selecting ? t('退出批量管理') : t('批量管理')}</Button>
      <Button size='sm' disabled={locked} onClick={onFile}>{t('本地安装')}</Button>
      <PluginPageTools disabled={locked} fileDisabled={locked} scriptDisabled={locked || scriptActive}
        scriptsAvailable={scriptsAvailable} checking={checking} onRefresh={onRefresh} onCheck={onCheck} onSettings={onSettings}
        onFile={onFile} onScript={onScript} onDiagnostics={onDiagnostics} onMissing={onMissing} />
    </div>
  </div>
}
