import * as React from 'react'
import { Button, Empty, Input, NavItem, NavList, Sidebar, SidebarContent, SidebarFooter, SidebarGroupLabel, SidebarHeader, StatusDot } from '@ui'
import { t } from '../i18n'
import { groupDirty, type RouteGroupEditor } from './routing-group-state'
import { draftDirty, type RoutingEditorState } from './routing-editor-state'
import type { RoutingWorkspace } from './routing-types'
import { RouteIcon } from './routing-ui'

export function RoutingRail({ editor, groups, workspace, onModel, onGroup, onAdd }: {
  editor: RoutingEditorState; groups: RouteGroupEditor; workspace?: RoutingWorkspace | null
  onModel(model: string): void; onGroup(id: string): void; onAdd(): void
}) {
  const [search, setSearch] = React.useState('')
  const configured = editor.draft?.workspace.configuredModels || workspace?.configuredModels || []
  const modelKeys = new Map(configured.map(model => [model.toLowerCase(), model]))
  for (const model of Object.keys(editor.drafts).filter(model => model && draftDirty(editor.drafts[model]))) modelKeys.set(model.toLowerCase(), model)
  if (editor.scope && !modelKeys.has(editor.scope.toLowerCase())) modelKeys.set(editor.scope.toLowerCase(), editor.scope)
  const models = [...modelKeys.values()]
    .filter(model => !Object.values(groups.drafts).some(draft => draft.group.model.toLowerCase() === model.toLowerCase()))
  const disabled = editor.busy || groups.busy || groups.loading
  const addDisabled = disabled || editor.loading || !(editor.draft || workspace) || !groups.data
  const query = search.trim().toLowerCase()
  const shownModels = models.filter(model => model.toLowerCase().includes(query))
  const shownGroups = Object.values(groups.drafts).filter(({ group }) => `${group.name} ${group.model}`.toLowerCase().includes(query))
  const showGlobal = !query || t('全局默认').toLowerCase().includes(query)
  const dirtyDot = (dirty: boolean) => dirty ? <span className='routing-rail-dirty' title={t('有未保存的修改')} aria-label={t('有未保存的修改')}><StatusDot variant='warning' /></span> : undefined
  return <Sidebar className='routing-config-rail'>
    <SidebarHeader className='routing-config-rail-head'><b>{t('路由配置')}</b><span>{1 + models.length + Object.keys(groups.drafts).length}</span>
      <Button variant='ghost' size='icon-sm' disabled={addDisabled} onClick={onAdd}
        title={t('新增路由配置')} aria-label={t('新增路由配置')}><RouteIcon name='plus' size={14} /></Button>
    </SidebarHeader>
    <div className='routing-rail-search'><Input value={search} onChange={event => setSearch(event.currentTarget.value)}
      placeholder={t('搜索路由配置')} aria-label={t('搜索路由配置')} /></div>
    <NavList render={<SidebarContent />} className='routing-config-rail-list' aria-label={t('路由配置列表')}>
      {showGlobal ? <><SidebarGroupLabel>{t('默认规则')}</SidebarGroupLabel>
        <NavItem className='routing-config-scope-item' active={!groups.selected && !editor.scope} disabled={disabled}
          trailing={dirtyDot(Boolean(editor.drafts[''] && draftDirty(editor.drafts[''])))} onClick={() => onModel('')}>
          <b>{t('全局默认')}</b><small>{t('账号启停与默认规则')}</small>
        </NavItem></> : null}
      {shownModels.length ? <SidebarGroupLabel>{t('模型路由')}</SidebarGroupLabel> : null}
      {shownModels.map(model => <NavItem key={model} className='routing-config-scope-item' active={!groups.selected && model.toLowerCase() === editor.scope.toLowerCase()}
        disabled={disabled} trailing={dirtyDot(Boolean(editor.drafts[model.toLowerCase()] && draftDirty(editor.drafts[model.toLowerCase()])))} onClick={() => onModel(model)} title={model}>
        <b>{model}</b><small>{editor.drafts[model.toLowerCase()]?.mode === 'inherit' ? t('继承全局') : t('独立配置')}</small>
      </NavItem>)}
      {shownGroups.length ? <SidebarGroupLabel>{t('自定义路由组')}</SidebarGroupLabel> : null}
      {shownGroups.map(draft => <NavItem key={draft.group.id} disabled={disabled} className='routing-config-scope-item'
        active={draft.group.id === groups.selected} trailing={dirtyDot(groupDirty(draft))} onClick={() => onGroup(draft.group.id)} title={draft.group.model}>
        <b>{draft.group.name}</b><small>{draft.group.model}</small>
      </NavItem>)}
      {!showGlobal && !shownModels.length && !shownGroups.length ? <Empty>{t('没有匹配的路由配置')}</Empty> : null}
      {groups.error ? <p className='routing-config-error' role='alert'>{groups.error}<Button variant='link' disabled={groups.loading} onClick={() => void groups.load()}>{t('重新读取')}</Button></p> : null}
    </NavList>
    <SidebarFooter className='routing-config-rail-foot'>
      <Button variant='dashed' disabled={addDisabled} onClick={onAdd}><RouteIcon name='plus' size={14} />{t('新增路由配置')}</Button>
    </SidebarFooter>
  </Sidebar>
}
