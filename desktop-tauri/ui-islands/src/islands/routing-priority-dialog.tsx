import * as React from 'react'
import { Badge, Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, cn } from '@ui'
import { t } from '../i18n'
import { RouteIcon } from './routing-ui'
import type { RoutePriority } from './routing-types'

type RuleItem = 'sticky' | 'consumption'

export function normalizeRoutePriority(value?: string): RoutePriority {
  return value === 'consumption' ? 'consumption' : 'sticky'
}

export function prioritySummaryText(priority?: string, sticky?: string): string {
  if (sticky === 'off' || sticky === 'turn') return t('仅按消耗顺序')
  return normalizeRoutePriority(priority) === 'consumption'
    ? t('消耗顺序 > 会话粘滞')
    : t('会话粘滞 > 消耗顺序')
}

export function RoutingPriorityRow({ priority, globalPriority, sticky, disabled, onOpen }: {
  priority?: string
  globalPriority?: string
  sticky?: string
  disabled?: boolean
  onOpen(): void
}) {
  const inherited = globalPriority !== undefined && !priority
  const text = prioritySummaryText(priority || globalPriority, sticky)
  const label = inherited ? t('继承全局：{value}', { value: text }) : text
  return <div className='routing-protection-summary'>
    <RouteIcon name='sort' size={14} />
    <span>{t('生效优先级')}</span>
    <Badge variant='ghost' shape='tag'>{label}</Badge>
    <Button variant='link' size='sm' disabled={disabled} onClick={onOpen}>{t('调整')}</Button>
  </div>
}

export function RoutingPriorityDialog({ title, value, globalValue, sticky, onChange, onClose }: {
  title: string
  value?: string
  globalValue?: string
  sticky?: string
  onChange(next: RoutePriority | undefined): void
  onClose(): void
}) {
  const stickyOn = sticky !== 'off' && sticky !== 'turn'
  const active = normalizeRoutePriority(value || globalValue)
  const order: RuleItem[] = active === 'consumption' ? ['consumption', 'sticky'] : ['sticky', 'consumption']

  const move = (index: number, delta: number) => {
    const to = index + delta
    if (to < 0 || to >= order.length) return
    const next = [...order]
    const [moved] = next.splice(index, 1)
    next.splice(to, 0, moved)
    onChange(next[0])
  }

  const infoOf = (key: RuleItem) => key === 'consumption'
    ? {
        title: t('消耗顺序'),
        desc: t('按临期积分、余额等排序选号；队首变化时可主动切号'),
      }
    : {
        title: t('会话粘滞'),
        desc: stickyOn ? t('同一对话继续用上一轮账号，优先保持缓存') : t('当前已停用'),
      }

  return <Dialog open onOpenChange={open => { if (!open) onClose() }}>
    <DialogContent className='w-[min(480px,calc(100vw-40px))]' showCloseButton>
      <DialogHeader>
        <RouteIcon name='sort' />
        <DialogTitle>{t('生效优先级')} · {title}</DialogTitle>
      </DialogHeader>
      <DialogBody>
        <p className='routing-config-note'>{t('在可用健康账号之间，按从上到下的顺序决定先听哪条规则。')}</p>
        <div className='routing-priority-list'>
          {order.map((key, index) => {
            const info = infoOf(key)
            const muted = key === 'sticky' && !stickyOn
            return <div key={key} className={cn('routing-priority-row', muted && 'is-muted')}>
              <span className='routing-priority-num'>{index + 1}</span>
              <div className='routing-priority-copy'>
                <b>{info.title}</b>
                <small>{info.desc}</small>
              </div>
              <div className='routing-priority-arrows'>
                <Button type='button' variant='ghost' size='icon-xs' disabled={index === 0}
                  aria-label={t('提高优先级')} onClick={() => move(index, -1)}>
                  <RouteIcon name='chevronUp' size={12} />
                </Button>
                <Button type='button' variant='ghost' size='icon-xs' disabled={index === order.length - 1}
                  aria-label={t('降低优先级')} onClick={() => move(index, 1)}>
                  <RouteIcon name='chevronDown' size={12} />
                </Button>
              </div>
            </div>
          })}
        </div>
        <p className='routing-priority-baseline'>
          {t('账号禁用、限流冷却、限制器与故障保护始终最先生效。')}
        </p>
      </DialogBody>
      <DialogFooter>
        {globalValue !== undefined && value !== undefined ? (
          <Button variant='ghost' className='mr-auto' onClick={() => onChange(undefined)}>{t('恢复跟随全局')}</Button>
        ) : null}
        <Button onClick={onClose}>{t('完成')}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}
