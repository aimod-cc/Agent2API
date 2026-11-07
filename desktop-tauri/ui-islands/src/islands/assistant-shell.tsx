import { Alert, Button, Empty, NavItem, NavList, PageHeader, ResponsiveSheet } from '@ui'
import { t } from '../i18n'
import { PlusIcon } from './assistant-icons'
import type { SessionSummary } from './assistant-state'

export function AssistantSidebar({ open, onOpenChange, sessions, activeId, disabled, settingsDisabled, onNew, onOpen, onRemove, onClear, onSettings }: {
  open: boolean; onOpenChange: (open: boolean) => void; sessions: SessionSummary[]; activeId: string
  disabled: boolean; settingsDisabled: boolean; onNew: () => void; onOpen: (id: string) => void
  onRemove: (id: string) => void; onClear: () => void; onSettings: () => void
}) {
  return <ResponsiveSheet open={open} onOpenChange={onOpenChange} title={t('历史对话')}
    breakpoint={900} side='left' inlineAs='aside' inlineClassName='asst-side'
    contentClassName='w-[min(280px,86vw)]' bodyClassName='gap-0'>
    <div className='asst-side-head'>
      <Button variant='default' size='sm' className='asst-new' onClick={onNew} disabled={disabled}>
        <PlusIcon size={14} /> {t('新对话')}
      </Button>
    </div>
    <NavList className='asst-side-list'>
      {!sessions.length ? <Empty className='asst-side-empty'>{t('还没有历史对话')}</Empty> : null}
      {sessions.map(item => <div key={item.id} className='asst-side-item relative pr-7'>
        <NavItem active={item.id === activeId} className='h-auto px-2 py-1.5' onClick={() => onOpen(item.id)}>
          <span className='title'>{item.title || t('未命名对话')}</span>
        </NavItem>
        <Button size='icon-2xs' variant='ghost' type='button' className='del absolute right-1 top-1.5'
          title={t('删除这条对话')} aria-label={t('删除这条对话')} disabled={disabled} onClick={() => onRemove(item.id)}>✕</Button>
      </div>)}
    </NavList>
    <div className='asst-side-foot flex-col'>
      <Button size='sm' variant='ghost' disabled={settingsDisabled} onClick={onSettings}>{t('标题总结设置')}</Button>
      {sessions.length ? <Button size='sm' variant='ghost' disabled={disabled} onClick={onClear}>{t('清空全部')}</Button> : null}
    </div>
  </ResponsiveSheet>
}

export function AssistantTopbar({ title, keyLabel, onHistory, onSettings }: {
  title: string; keyLabel: string; onHistory: () => void; onSettings: () => void
}) {
  return <PageHeader className='asst-top'>
    <Button size='default' variant='ghost' type='button' className='asst-menu' title={t('历史对话')} aria-label={t('历史对话')} onClick={onHistory}>☰</Button>
    <span className='asst-title' title={title}>{title}</span>
    <span className='spacer' />
    <span className='asst-top-hint'>{keyLabel}</span>
    <Button variant='ghost' size='sm' className='asst-settings-btn' onClick={onSettings}>
      <svg viewBox='0 0 24 24' width='14' height='14' aria-hidden='true'>
        <g fill='none' stroke='currentColor' strokeWidth='1.6' strokeLinecap='round' strokeLinejoin='round'>
          <circle cx='12' cy='12' r='3.1' />
          <path d='M19.3 13.8l1.5 1.1-1.8 3-1.8-.6c-.5.4-1 .7-1.6.9l-.3 1.9h-3.5l-.3-1.9c-.6-.2-1.1-.5-1.6-.9l-1.8.6-1.8-3 1.5-1.1a7 7 0 0 1 0-1.9l-1.5-1.1 1.8-3 1.8.6c.5-.4 1-.7 1.6-.9l.3-1.9h3.5l.3 1.9c.6.2 1.1.5 1.6.9l1.8-.6 1.8 3-1.5 1.1c.05.32.07.63.07.95 0 .31-.02.62-.05.9z' />
        </g>
      </svg>
      {t('助手设置')}
    </Button>
  </PageHeader>
}

export function AssistantEmpty({ noTools, failed, status, onSend }: {
  noTools: boolean; failed: boolean; status: string; onSend: (text: string) => void
}) {
  return <Empty className='asst-empty'>
    <div className='asst-empty-logo'>
      <svg viewBox='0 0 24 24' width='26' height='26' aria-hidden='true'>
        <path fill='currentColor' d='M12 2.7l1.85 5.45 5.45 1.85-5.45 1.85L12 17.3l-1.85-5.45L4.7 10l5.45-1.85L12 2.7z' />
        <path fill='currentColor' d='M18.8 15l.85 2.35L22 18.2l-2.35.85L18.8 21.4l-.85-2.35L15.6 18.2l2.35-.85L18.8 15z' />
      </svg>
    </div>
    <h3>{t('模型对话')}</h3>
    <p>{t('用一句话让我查或改本机的模型转发配置。')}<br />{t('我会在动手前把改动列给你确认。')}</p>
    <div className='asst-suggests'>
      {[t('看看哪些模型在多家重复了'), t('测一下当前最快的模型通不通'), t('CodeArts 的账号余额还剩多少？'), t('把重复的模型加上家名前缀区分开')]
        .map(text => <Button size='default' variant='ghost' key={text} type='button' onClick={() => onSend(text)}>“{text}”</Button>)}
    </div>
    {noTools ? <Alert variant='warning' className='asst-empty-warn'>{t('当前一个工具都没勾选，助手只能纯聊天。点右上角「助手设置」勾上需要的工具。')}</Alert> : null}
    {failed ? <Alert variant='destructive' className='asst-empty-warn'>{status || t('助手设置读取失败，工具可能已过时。')}</Alert> : null}
  </Empty>
}
