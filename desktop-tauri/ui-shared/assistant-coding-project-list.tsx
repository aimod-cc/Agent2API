import { useEffect, useState } from 'react'
import { Badge, Button, Collapsible, CollapsibleContent, CollapsibleTrigger, cn } from '@ui'
import type { CodingProject, CodingSession, CodingTranslate } from './assistant-coding-types'
import { CodingIcon } from './assistant-coding-icons'

export function CodingProjectList({ projects, sessions, activeId, selectedProjectId, search, loading, limited, t, onOpen, onSelectProject, onNewSession }: {
  projects: CodingProject[]; sessions: CodingSession[]; activeId: string; selectedProjectId: string; search: string; loading: boolean; limited: boolean
  t: CodingTranslate; onOpen: (id: string) => void; onSelectProject: (id: string) => void; onNewSession: (projectId: string) => void
}) {
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({})
  const activeProject = sessions.find(session => session.id === activeId)?.projectId
  useEffect(() => {
    if (activeProject) setCollapsed(previous => previous[activeProject] ? { ...previous, [activeProject]: false } : previous)
  }, [activeProject])
  const keyword = search.trim().toLocaleLowerCase()
  const groups = projects.map(project => {
    const rows = sessions.filter(session => session.projectId === project.id)
    const matchesProject = `${project.name} ${project.directory}`.toLocaleLowerCase().includes(keyword)
    const visible = !keyword || matchesProject ? rows : rows.filter(session => `${session.title} ${session.directory}`.toLocaleLowerCase().includes(keyword))
    return { project, rows, visible, matches: !keyword || matchesProject || visible.length > 0, updated: Math.max(project.createdAt, ...rows.map(session => session.updatedAt)) }
  }).filter(group => group.matches && (!group.project.legacy || group.rows.length || !projects.some(project => !project.legacy)))
    .sort((a, b) => b.updated - a.updated || a.project.name.localeCompare(b.project.name))
  return <div className='min-h-0 flex-1 space-y-2 overflow-auto'>
    {groups.map(({ project, rows, visible }) => {
      const open = Boolean(keyword) || !collapsed[project.id]
      return <Collapsible key={project.id} open={open} onOpenChange={value => { setCollapsed(previous => ({ ...previous, [project.id]: !value })); onSelectProject(project.id) }}>
        <div className={cn('flex items-center rounded-md', selectedProjectId === project.id && 'bg-muted/30')}>
          <CollapsibleTrigger render={<Button variant='ghost' size='sm' />} title={project.directory}
            className='h-9 min-w-0 flex-1 justify-start gap-2 px-2 font-medium max-sm:h-10'>
            <CodingIcon name='chevron' className={cn('size-3 text-muted-foreground transition-transform', !open && '-rotate-90')} />
            <CodingIcon name='folder' className='size-4 text-muted-foreground' /><span className='min-w-0 flex-1 truncate text-left'>{project.name}</span>
            {rows.length ? <span className='text-xs font-normal tabular-nums text-muted-foreground'>{rows.length}</span> : null}
          </CollapsibleTrigger>
          <Button variant='ghost' size='icon-sm' disabled={!project.accessible} title={t('在项目中新建会话')} aria-label={`${t('在项目中新建会话')} · ${project.name}`}
            onClick={() => onNewSession(project.id)}><CodingIcon name='plus' className='size-3.5' /></Button>
        </div>
        <CollapsibleContent>
          <div className='ml-4 space-y-1 border-l border-border pl-2 pt-1'>
            {visible.map(session => <Button key={session.id} variant={session.id === activeId ? 'secondary' : 'ghost'} size='sm' className='h-auto min-h-8 w-full justify-start gap-2 px-2 py-2 text-left font-normal max-sm:min-h-10'
              title={session.title || t('未命名会话')} onClick={() => onOpen(session.id)}>
              <span className='min-w-0 flex-1 truncate'>{session.title || t('未命名会话')}</span>
              {session.forkedFromId ? <span className='text-xs text-muted-foreground'>{t('分支')}</span> : null}
              {session.terminalCount ? <span className='text-xs text-muted-foreground'>{t('终端')}</span> : null}
              {session.pendingCount ? <Badge variant='outline'>{session.pendingCount}</Badge> : session.thinking ? <span className='text-xs text-muted-foreground'>{t('执行中')}</span> : null}
            </Button>)}
            {!visible.length ? <p className='px-2 py-2 text-xs text-muted-foreground'>{project.accessible ? t('还没有对话') : t('项目文件夹暂时无法访问')}</p> : null}
          </div>
        </CollapsibleContent>
      </Collapsible>
    })}
    {!groups.length && !loading ? <p className='px-2 py-4 text-xs text-muted-foreground'>{keyword ? t('没有匹配的会话') : t('还没有项目，先新建一个项目')}</p> : null}
    {limited ? <p className='p-2 text-xs text-muted-foreground'>{t('这里只显示最近的 500 条会话。')}</p> : null}
  </div>
}
