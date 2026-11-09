import * as React from 'react'
import { Button, cn } from '@ui'
import { t } from '../i18n'
import { RouteIcon } from './routing-ui'
import { RoutingCanvasEdges } from './routing-canvas-edges'

/**
 * 配置画布的外壳。
 *
 * `plain` = 特殊请求路由那一版：去掉点阵底、把画布压到 --rail-bg。
 * 理由：点阵是账号路由画布的语言（连线、流转），特殊路由那边只有一份规则列表；
 * 而规则卡片是纯白无描边的，画布必须比它暗一档才看得出卡片边界。
 */
export function RoutingConfigShell({ plain = false, children }: { plain?: boolean; children: React.ReactNode }) {
  const root = React.useRef<HTMLDivElement>(null)
  return <div className={cn('routing-config-shell', plain && 'is-plain')} ref={root}>
    <RoutingCanvasEdges root={root} />
    {children}
  </div>
}

export function RoutingCanvasLayout({ title, children, groups, note, globalAccounts = false }: {
  title: string; children: React.ReactNode; groups: React.ReactNode; note: string; globalAccounts?: boolean
}) {
  return <div className='routing-canvas'>
    <section className='routing-config-column'>
      <div className='routing-column-head'><RouteIcon name='route' size={14} /><b>{t('路由配置')}</b><span>{title}</span></div>
      {children}
      <p className='routing-canvas-note'><RouteIcon name='info' size={13} /><span>{note}</span></p>
    </section>
    {groups}
    <div className='routing-canvas-legend'><span><i />{globalAccounts ? t('连线表示账号组中有已启用账号') : t('连线表示参与本路由')}</span>
      <span><svg width={22} height={8} aria-hidden='true'><path d='M 0 4 H 22' /></svg>{t('流动：有请求在途')}</span>
      <span>{globalAccounts ? t('账号启停立即生效；路由配置保存后生效') : t('保存后才会影响真实请求')}</span></div>
  </div>
}

export function RoutingRefreshError({ error, onRetry }: { error: string; onRetry(): void }) {
  return error ? <div className='routing-page-error' role='alert'>
    <RouteIcon name='info' /><span>{t('实时状态暂未更新，当前显示上次读取的结果：{error}', { error })}</span>
    <Button variant='ghost' size='sm' onClick={onRetry}>{t('重试')}</Button>
  </div> : null
}

export function RoutingDraftFooter({ dirty, inherited, busy, disabled, error, onDiscard, onSave }: {
  dirty: boolean; inherited?: boolean; busy: boolean; disabled: boolean; error?: string; onDiscard(): void; onSave(): void
}) {
  return <footer className={cn('routing-config-draft', dirty && 'is-dirty')}>
    {error ? <p className='routing-config-error' role='alert'>{error}</p> : null}
    <span className='routing-draft-state'>{dirty ? t('有未保存的修改') : inherited ? t('继承全局') : t('已保存')}</span>
    <div className='right'><Button variant='outline' size='sm' disabled={!dirty || disabled} onClick={onDiscard}>{t('撤销修改')}</Button>
      <Button size='sm' disabled={!dirty || disabled} onClick={onSave}>{busy ? t('保存中…') : t('保存路由')}</Button></div>
  </footer>
}
