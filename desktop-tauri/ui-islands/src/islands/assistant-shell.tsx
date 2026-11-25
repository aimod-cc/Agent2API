import * as React from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import {
  Alert,
  AlertDialog,
  AlertDialogBanner,
  AlertDialogBody,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  Button,
  Empty,
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
  Popover,
  PopoverContent,
  PopoverTrigger,
  ResponsiveSheet,
  Tooltip,
  TooltipContent,
  TooltipTrigger,
  cn,
} from '@ui'
import { t } from '../i18n'
import { modelApiKeyLabel, type ModelApiKey } from '../../../ui-shared/model-api-key'
import {
  CheckIcon,
  CloseIcon,
  GearIcon,
  KeyIcon,
  PlusIcon,
  SearchIcon,
  SidebarToggleIcon,
} from './assistant-icons'
import type { SessionSummary } from './assistant-state'

type SessionGroupKey = 'today' | 'yesterday' | 'week' | 'earlier'
type FlatRow =
  | { type: 'group'; key: string; label: string }
  | { type: 'session'; key: string; item: SessionSummary }

const GROUP_ORDER: SessionGroupKey[] = ['today', 'yesterday', 'week', 'earlier']

function normalizeMs(ts: number): number {
  if (!Number.isFinite(ts) || ts <= 0) return 0
  return ts > 1e12 ? ts : ts * 1000
}

function sessionGroupKey(updatedAt: number, createdAt: number, startOfTodayMs: number): SessionGroupKey {
  const ms = normalizeMs(updatedAt) || normalizeMs(createdAt)
  if (!ms) return 'earlier'
  const dayMs = 86_400_000
  if (ms >= startOfTodayMs) return 'today'
  if (ms >= startOfTodayMs - dayMs) return 'yesterday'
  if (ms >= startOfTodayMs - 6 * dayMs) return 'week'
  return 'earlier'
}

function sessionGroupLabel(key: SessionGroupKey): string {
  switch (key) {
    case 'today': return t('今天')
    case 'yesterday': return t('昨天')
    case 'week': return t('近 7 天')
    case 'earlier': return t('更早')
  }
}

function buildGroupedRows(sessions: SessionSummary[]): FlatRow[] {
  if (!sessions.length) return []
  const now = new Date()
  const startOfTodayMs = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime()
  const buckets: Record<SessionGroupKey, SessionSummary[]> = {
    today: [], yesterday: [], week: [], earlier: [],
  }
  for (const item of sessions) {
    buckets[sessionGroupKey(item.updatedAt, item.createdAt, startOfTodayMs)].push(item)
  }
  const rows: FlatRow[] = []
  for (const group of GROUP_ORDER) {
    const items = buckets[group]
    if (!items.length) continue
    rows.push({ type: 'group', key: `group-${group}`, label: sessionGroupLabel(group) })
    for (const item of items) rows.push({ type: 'session', key: item.id, item })
  }
  return rows
}

function SessionRow({ item, active, disabled, onOpen, onRemove }: {
  item: SessionSummary
  active: boolean
  disabled: boolean
  onOpen: (id: string) => void
  onRemove: (id: string) => void
}) {
  const title = item.title || t('未命名对话')
  return (
    <div className={cn('asst-side-row', active && 'active')}>
      <button type='button' className='asst-side-btn' title={title} onClick={() => onOpen(item.id)}>
        <span className='title'>{title}</span>
      </button>
      <Button
        size='icon-2xs'
        variant='ghost'
        type='button'
        className='asst-side-del'
        title={t('删除这条对话')}
        aria-label={t('删除这条对话')}
        disabled={disabled}
        onClick={() => onRemove(item.id)}
      >
        <CloseIcon size={10} />
      </Button>
    </div>
  )
}

export const AssistantSidebar = React.memo(function AssistantSidebar({
  open, onOpenChange, collapsed, onToggleCollapse, sessions, activeId, disabled, settingsDisabled, onNew, onOpen, onRemove, onClear, onSettings,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  collapsed: boolean
  onToggleCollapse: () => void
  sessions: SessionSummary[]
  activeId: string
  disabled: boolean
  settingsDisabled: boolean
  onNew: () => void
  onOpen: (id: string) => void
  onRemove: (id: string) => void
  onClear: () => void
  onSettings: () => void
}) {
  const [searchOpen, setSearchOpen] = React.useState(false)
  const [keyword, setKeyword] = React.useState('')
  const [clearDialogOpen, setClearDialogOpen] = React.useState(false)
  const listRef = React.useRef<HTMLDivElement | null>(null)

  const filtered = React.useMemo(() => {
    const q = keyword.trim().toLowerCase()
    if (!q) return sessions
    return sessions.filter(item => (item.title || t('未命名对话')).toLowerCase().includes(q))
  }, [sessions, keyword])

  const rows = React.useMemo(() => buildGroupedRows(filtered), [filtered])
  const virtual = rows.length > 60
  const virtualizer = useVirtualizer<HTMLDivElement, HTMLDivElement>({
    count: rows.length,
    getScrollElement: () => listRef.current,
    estimateSize: index => rows[index]?.type === 'group' ? 28 : 31,
    getItemKey: index => rows[index]?.key || index,
    overscan: 8,
    enabled: virtual,
  })

  return (
    <>
      <ResponsiveSheet
        open={open}
        onOpenChange={onOpenChange}
        title={t('历史对话')}
        breakpoint={900}
        side='left'
        inlineAs='aside'
        inlineClassName={cn('asst-side', collapsed && 'is-collapsed')}
        contentClassName='w-[min(280px,86vw)]'
        bodyClassName='gap-0 p-0'
      >
        <div className='asst-side-head'>
          <Button variant='ghost' size='sm' type='button' className='asst-new' onClick={onNew} disabled={disabled}>
            <PlusIcon size={14} />
            <span>{t('新对话')}</span>
          </Button>
          <span className='spacer' />
          <Tooltip>
            <TooltipTrigger
              render={<Button type='button' variant='ghost' size='icon-xs' aria-label={t('搜索历史')} />}
              onClick={() => setSearchOpen(prev => {
                if (prev) setKeyword('')
                return !prev
              })}
            >
              <SearchIcon size={13} />
            </TooltipTrigger>
            <TooltipContent>{t('搜索历史')}</TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger
              render={<Button type='button' variant='ghost' size='icon-xs' className='asst-side-collapse' aria-label={t('收起历史')} />}
              onClick={onToggleCollapse}
            >
              <SidebarToggleIcon collapsed={false} size={14} />
            </TooltipTrigger>
            <TooltipContent>{t('收起历史')}</TooltipContent>
          </Tooltip>
        </div>

        {searchOpen ? (
          <div className='asst-side-search'>
            <InputGroup className='h-7'>
              <InputGroupInput
                autoFocus
                value={keyword}
                placeholder={t('筛选历史标题…')}
                className='text-[12px]'
                onChange={event => setKeyword(event.currentTarget.value)}
              />
              <InputGroupAddon><SearchIcon size={12} /></InputGroupAddon>
              {keyword ? (
                <InputGroupAddon align='inline-end'>
                  <InputGroupButton size='icon-xs' aria-label={t('清空搜索')} onClick={() => setKeyword('')}>
                    <CloseIcon size={10} />
                  </InputGroupButton>
                </InputGroupAddon>
              ) : null}
            </InputGroup>
          </div>
        ) : null}

        <div className='asst-side-list' ref={listRef}>
          {!rows.length ? (
            <Empty className='asst-side-empty'>
              {keyword.trim() ? t('没有匹配的历史对话') : t('还没有历史对话')}
            </Empty>
          ) : virtual ? (
            <div style={{ height: `${virtualizer.getTotalSize()}px`, position: 'relative', width: '100%' }}>
              {virtualizer.getVirtualItems().map(vRow => {
                const row = rows[vRow.index]
                if (!row) return null
                return (
                  <div
                    key={vRow.key}
                    style={{
                      position: 'absolute',
                      top: 0,
                      left: 0,
                      width: '100%',
                      transform: `translateY(${vRow.start}px)`,
                    }}
                  >
                    {row.type === 'group' ? (
                      <div className='asst-side-group'>{row.label}</div>
                    ) : (
                      <SessionRow
                        item={row.item}
                        active={row.item.id === activeId}
                        disabled={disabled}
                        onOpen={onOpen}
                        onRemove={onRemove}
                      />
                    )}
                  </div>
                )
              })}
            </div>
          ) : (
            rows.map(row => (
              row.type === 'group' ? (
                <div key={row.key} className='asst-side-group'>{row.label}</div>
              ) : (
                <SessionRow
                  key={row.key}
                  item={row.item}
                  active={row.item.id === activeId}
                  disabled={disabled}
                  onOpen={onOpen}
                  onRemove={onRemove}
                />
              )
            ))
          )}
        </div>

        <div className='asst-side-foot'>
          <Button size='xs' variant='ghost' type='button' className='asst-side-foot-btn' disabled={settingsDisabled} onClick={onSettings}>
            {t('标题设置')}
          </Button>
          {sessions.length > 0 ? (
            <Button size='xs' variant='ghost' type='button' className='asst-side-foot-btn is-danger' disabled={disabled} onClick={() => setClearDialogOpen(true)}>
              {t('清空')}
            </Button>
          ) : null}
        </div>
      </ResponsiveSheet>

      <AlertDialog open={clearDialogOpen} onOpenChange={setClearDialogOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t('清空全部历史对话')}</AlertDialogTitle>
            <AlertDialogDescription>{t('此操作不可恢复，请确认是否继续。')}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogBody className='gap-3'>
            <AlertDialogBanner tone='danger'>
              {t('将永久删除当前列表里的 {n} 条历史会话及关联图片记录。', { n: sessions.length })}
            </AlertDialogBanner>
          </AlertDialogBody>
          <AlertDialogFooter>
            <Button type='button' variant='outline' onClick={() => setClearDialogOpen(false)}>{t('取消')}</Button>
            <Button type='button' variant='destructive' onClick={() => { setClearDialogOpen(false); onClear() }}>
              {t('确认清空')}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
})

export const AssistantTopbar = React.memo(function AssistantTopbar({
  title, hasMessages, sidebarCollapsed, onExpandSidebar, onNewChat,
  keys, selectedKeyId, keyLabel, keysLoading, onSelectKey, onRefreshKeys,
  onHistory, onSettings, disableNew, disableKeySwitch,
}: {
  title: string
  hasMessages: boolean
  sidebarCollapsed: boolean
  onExpandSidebar: () => void
  onNewChat: () => void
  keys: ModelApiKey[]
  selectedKeyId: string | null
  keyLabel: string
  keysLoading: boolean
  onSelectKey: (id: string) => void
  onRefreshKeys: () => void
  onHistory: () => void
  onSettings: () => void
  disableNew?: boolean
  disableKeySwitch?: boolean
}) {
  const [keyOpen, setKeyOpen] = React.useState(false)

  return (
    <div className={cn('asst-top', hasMessages && 'has-border')}>
      <Button size='icon-xs' variant='ghost' type='button' className='asst-menu' title={t('历史对话')} aria-label={t('历史对话')} onClick={onHistory}>
        ☰
      </Button>

      {sidebarCollapsed ? (
        <div className='asst-top-collapsed-actions'>
          <Tooltip>
            <TooltipTrigger
              render={<Button type='button' variant='ghost' size='icon-xs' aria-label={t('展开历史')} />}
              onClick={onExpandSidebar}
            >
              <SidebarToggleIcon collapsed size={14} />
            </TooltipTrigger>
            <TooltipContent>{t('展开历史')}</TooltipContent>
          </Tooltip>
          <Tooltip>
            <TooltipTrigger
              render={<Button type='button' variant='ghost' size='icon-xs' disabled={disableNew} aria-label={t('新对话')} />}
              onClick={onNewChat}
            >
              <PlusIcon size={14} />
            </TooltipTrigger>
            <TooltipContent>{t('新对话')}</TooltipContent>
          </Tooltip>
        </div>
      ) : null}

      {hasMessages ? <span className='asst-title' title={title}>{title}</span> : null}
      <span className='spacer' />

      <Popover open={keyOpen} onOpenChange={setKeyOpen}>
        <PopoverTrigger
          render={<Button type='button' variant='ghost' className='asst-ghost-ctl' disabled={disableKeySwitch} title={keyLabel} />}
        >
          <KeyIcon size={12} />
          <span className='asst-key-label'>{keyLabel}</span>
        </PopoverTrigger>
        <PopoverContent side='bottom' align='end' className='w-[252px] p-1.5'>
          <div className='flex items-center justify-between px-2 py-1'>
            <span className='text-[11px] font-medium text-muted-foreground'>{t('切换网关 Key')}</span>
            <Button type='button' variant='ghost' size='icon-xs' disabled={keysLoading} title={t('刷新')} aria-label={t('刷新')} onClick={onRefreshKeys}>↻</Button>
          </div>
          {keys.length === 0 ? (
            <Empty className='py-4 text-xs'>{t('没有可用的 API Key')}</Empty>
          ) : (
            <div className='flex max-h-[240px] flex-col gap-0.5 overflow-y-auto'>
              {keys.map(item => {
                const active = item.id === selectedKeyId
                return (
                  <Button
                    key={item.id}
                    type='button'
                    variant='ghost'
                    className={cn('asst-key-pop-row', active && 'is-active')}
                    onClick={() => { onSelectKey(item.id); setKeyOpen(false) }}
                  >
                    <span className='min-w-0 flex-1'>
                      <span className='block truncate text-[12px] font-medium text-foreground'>{item.name || item.id}</span>
                      <span className='block truncate font-mono text-[11px] text-muted-foreground'>
                        {item.masked || modelApiKeyLabel(item)}
                      </span>
                    </span>
                    {active ? <span className='text-foreground'><CheckIcon size={13} /></span> : null}
                  </Button>
                )
              })}
            </div>
          )}
        </PopoverContent>
      </Popover>

      <Button variant='ghost' type='button' className='asst-ghost-ctl asst-settings-btn' onClick={onSettings}>
        <GearIcon size={13} />
        <span>{t('设置')}</span>
      </Button>
    </div>
  )
})

export const AssistantEmpty = React.memo(function AssistantEmpty({
  composer, noTools, failed, status, onSend,
}: {
  composer: React.ReactNode
  noTools: boolean
  failed: boolean
  status: string
  onSend: (text: string) => void
}) {
  const quickPrompts = React.useMemo(() => [
    { label: t('排查重复模型'), prompt: t('看看哪些模型在多家重复了') },
    { label: t('测试最快线路'), prompt: t('测一下当前最快的模型通不通') },
    { label: t('查 CodeArts 余额'), prompt: t('CodeArts 的账号余额还剩多少？') },
    { label: t('批量加前缀'), prompt: t('把重复的模型加上家名前缀区分开') },
  ], [])

  return (
    <div className='asst-empty-center'>
      <div className='asst-empty-inner'>
        <h2 className='asst-empty-heading'>{t('有什么需要处理的？')}</h2>
        {composer}
        <div className='asst-quick-row'>
          {quickPrompts.map(item => (
            <Button key={item.label} size='sm' variant='ghost' type='button' className='asst-quick-chip' onClick={() => onSend(item.prompt)}>
              {item.label}
            </Button>
          ))}
        </div>
        {noTools ? <Alert variant='warning' className='asst-empty-warn'>{t('当前一个工具都没勾选，助手只能纯聊天。点右上角「设置」勾上需要的工具。')}</Alert> : null}
        {failed ? <Alert variant='destructive' className='asst-empty-warn'>{status || t('助手设置读取失败，工具可能已过时。')}</Alert> : null}
      </div>
    </div>
  )
})
