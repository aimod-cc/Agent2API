/**
 * 分组设置弹窗（分组带上齿轮那颗按钮打开的）。
 *
 * ── 为什么是弹窗，不是贴着齿轮的气泡 ──────────────────────────
 * 第一版只有「排序」一节，气泡装得下；但这一处以后还会有默认展开 / 收起、
 * 新账号落到组首还是组尾、本组并发上限、本组代理这些设置 —— 气泡既装不下
 * 几节内容，也没有「标题 + 可滚动正文 + 固定底栏」这套骨架可依。改成弹窗之后，
 * 加设置 = 往正文里追加一个 `GroupSettingSection`，骨架一行不用动。
 *
 * ── 排序规则的口径 ────────────────────────────────────────────
 * 规则**按数组顺序比较**，与界面上从上到下的优先级是同一个东西；`manual`
 * 不在这份数组里 —— 它是所有规则都相等时的兜底（保持拖出来的顺序），
 * 固定末位、不可勾选。见 accounts-domain 的 sortGroupMembers。
 *
 * 「积分到期时间」只对查得到逐份积分包的家开放（`supportsCreditExpiry`）：
 * 其余家那一行置灰成「不可用」而**不是隐藏** —— 藏起来会让人以为界面上少了
 * 一块，而置灰还能说明「这家确实没这个概念」。
 *
 * 改动即时生效（不设「保存」）：排序是纯展示偏好，改错了当场就能看出来；
 * 一个「保存」按钮只会让用户以为还有别的地方要一起提交。
 */

import * as React from 'react'
import {
  Button, Checkbox, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle,
  Select, SelectContent, SelectItem, SelectTrigger, SelectValue, cn,
} from '@ui'
import { t } from '../i18n'
import { shared } from './accounts-shared'
import {
  creditExpiryOf, providerOf, supportsCreditExpiry, type GroupSortRule,
} from './accounts-domain'
import {
  allAccounts, getStore, groupSortRules, hasCustomGroupSort, setGroupSortRules, subscribe,
  usageEntryOf,
} from './accounts-data'
import { PROVIDER_ICONS } from './add-provider-pick'

/**
 * 每条规则在界面上的名字与两档方向。
 *
 * 名字与说明写**字面量 t()** 而不是拼在表里：词典扫描器只认 `t('…')` 的首参是
 * 字符串字面量（变量 / 模板串一律不提取，见 i18n/scan.mjs 的说明），把它们塞进
 * 这张表就会在五种语言的词典里同时缺键、界面回落成中文。
 */
function ruleMeta(key: GroupSortRule['key']): {
  name: string
  hint: string
  options: Array<{ value: string; label: string }>
} {
  if (key === 'credit') {
    return {
      name: t('积分到期时间'),
      hint: t('按账号最早到期、且还有剩余的积分包比较'),
      options: [
        { value: 'soon', label: t('最快到期优先') },
        { value: 'late', label: t('最晚到期优先') },
      ],
    }
  }
  return {
    name: t('账号添加时间'),
    hint: t('所有提供商都有这个字段'),
    options: [
      { value: 'new', label: t('最新添加优先') },
      { value: 'old', label: t('最早添加优先') },
    ],
  }
}

/**
 * 规则摘要（分组带上那枚小标记、弹窗里的「当前规则」都用它）。
 * 只列**勾选的**那些 —— 关掉的规则不参与比较，写进摘要只会让「当前规则」说谎。
 */
export function groupSortSummary(rules: GroupSortRule[]): string {
  return rules
    .filter(rule => rule.on)
    .map(rule => {
      if (rule.key === 'credit') return rule.dir === 'soon' ? t('到期 近→远') : t('到期 远→近')
      return rule.dir === 'new' ? t('添加 新→旧') : t('添加 旧→新')
    })
    .join(' · ')
}

/**
 * 一行规则：优先级号 / 勾选 / 名称与说明 / 方向 / 上下移。
 *
 * `state` 有三种，视觉与可操作性各不相同：
 *   · `ready`       —— 可用（可勾、可切方向）
 *   · `no-data`     —— 这一家的后端会产出到期，但**这一组此刻一条都没读到**
 *                      （没点过「查询余额」，或包都过期了）。仍然可勾 —— 用户先设
 *                      好规则、之后查到数据就自然生效；说明行写「暂无数据」。
 *   · `unsupported` —— 这一家的数据里根本没有到期字段。勾选框禁用、方向位换成
 *                      「不可用」，整行不压淡（压淡是「这次没勾」的语义）。
 */
function RuleRow({ rule, priority, isLast, state, onChange }: {
  rule: GroupSortRule
  priority: number
  isLast: boolean
  state: 'ready' | 'no-data' | 'unsupported'
  onChange: (patch: Partial<GroupSortRule>) => void
}) {
  const meta = ruleMeta(rule.key)
  const blocked = state === 'unsupported'
  const off = !blocked && !rule.on
  return (
    <div className={cn(
      'grid items-center gap-2 rounded-sm px-1.5 py-1.5',
      'grid-cols-[16px_15px_minmax(0,1fr)_132px_16px]',
      'hover:bg-nav-hover',
      off && 'opacity-[.58]',
    )}>
      <span className='text-center text-[10.5px] font-bold text-subtle tabular-nums'>{priority}</span>
      <Checkbox checked={rule.on && !blocked} disabled={blocked}
        aria-label={meta.name}
        onCheckedChange={next => onChange({ on: next === true })} />
      <span className='flex min-w-0 flex-col gap-px'>
        <span className={cn('text-xs font-medium', blocked && 'text-subtle', off && 'text-subtle')}>
          {meta.name}
        </span>
        <span className={cn(
          'overflow-hidden text-ellipsis whitespace-nowrap text-[10.5px]',
          blocked ? 'text-warning' : state === 'no-data' ? 'text-subtle' : 'text-subtle',
        )} title={blocked ? t('这一家查不到积分到期时间') : undefined}>
          {blocked
            ? t('这一家查不到积分到期时间')
            : state === 'no-data' ? t('暂无数据：先查询一次余额，之后会自动按到期排序') : meta.hint}
        </span>
      </span>
      {!blocked ? (
        <Select value={rule.dir} disabled={!rule.on}
          onValueChange={value => onChange({ dir: value } as Partial<GroupSortRule>)}>
          <SelectTrigger className='h-[26px] w-full text-[11px]' aria-label={t('{name}的方向', { name: meta.name })}>
            <SelectValue>{meta.options.find(item => item.value === rule.dir)?.label || ''}</SelectValue>
          </SelectTrigger>
          <SelectContent>
            {meta.options.map(item => (
              <SelectItem key={item.value} value={item.value}>{item.label}</SelectItem>
            ))}
          </SelectContent>
        </Select>
      ) : (
        <span className='rounded-xs border border-dashed border-border py-1 text-center text-[10.5px] text-subtle'>
          {t('不可用')}
        </span>
      )}
      <span className='flex flex-col gap-px'>
        <button type='button' disabled={priority <= 1}
          className='h-3 w-4 rounded-[3px] text-subtle hover:bg-nav-hover hover:text-foreground disabled:cursor-default disabled:opacity-30'
          title={t('提高优先级')} aria-label={t('提高优先级')}
          onClick={() => onChange({ __move: 'up' } as unknown as Partial<GroupSortRule>)}>
          <svg viewBox='0 0 24 24' width='10' height='10' className='mx-auto block'>
            <path fill='none' stroke='currentColor' strokeWidth='2.4' strokeLinecap='round' strokeLinejoin='round' d='m6 14 6-6 6 6' />
          </svg>
        </button>
        <button type='button' disabled={isLast}
          className='h-3 w-4 rounded-[3px] text-subtle hover:bg-nav-hover hover:text-foreground disabled:cursor-default disabled:opacity-30'
          title={t('降低优先级')} aria-label={t('降低优先级')}
          onClick={() => onChange({ __move: 'down' } as unknown as Partial<GroupSortRule>)}>
          <svg viewBox='0 0 24 24' width='10' height='10' className='mx-auto block'>
            <path fill='none' stroke='currentColor' strokeWidth='2.4' strokeLinecap='round' strokeLinejoin='round' d='m6 10 6 6 6-6' />
          </svg>
        </button>
      </span>
    </div>
  )
}

/** 正文里的一节：标题 + 一句口径 + 内容。后续加设置就再加一个它 */
function GroupSettingSection({ title, hint, children }: {
  title: string
  hint?: string
  children: React.ReactNode
}) {
  return (
    <section className='flex flex-col gap-1 border-t border-hairline pt-3.5 first:border-t-0 first:pt-0'>
      <div className='mb-1 flex items-baseline gap-2'>
        <h4 className='m-0 text-xs font-bold'>{title}</h4>
        {hint ? <span className='text-[10.5px] text-subtle'>{hint}</span> : null}
      </div>
      {children}
    </section>
  )
}

/**
 * 弹窗主体。`provider` 由 AccountsDialogs 从 store 的 dialog 状态传入；
 * 关闭时走 `closeDialog()`（列表页与弹窗共用同一个状态位）。
 */
export function GroupSettingsDialog({ provider, onClose }: { provider: string; onClose: () => void }) {
  // 订阅 store：规则一改就要重画（本弹窗是 store 的读者，不只是写者）
  React.useSyncExternalStore(subscribe, () => getStore().version)
  const rules = groupSortRules(provider)
  const label = shared().wbProviders?.labelOf?.(provider) || provider
  const icon = PROVIDER_ICONS[provider]
  /**
   * 方案 A：静态能力名单（这一家的后端**会不会**产出逐份到期）。
   * 方案 B：运行时是否**真的**读到了 —— 名单里的家如果这一组所有账号都还没查到
   * 到期（没点过「查询余额」、或者都过期了），这一项不该假装可用。
   */
  const creditSupported = supportsCreditExpiry(provider)
  // 依赖 store 的 version 而不是空数组：查完余额（usageEntryOf 有新读数）之后
  // 这一项要从「暂无数据」翻成可用，弹窗开着时也得跟着变
  const version = getStore().version
  const creditHasData = React.useMemo(
    () => allAccounts()
      .filter(account => providerOf(account) === provider)
      .some(account => creditExpiryOf(usageEntryOf(account)) !== null),
    [provider, version],
  )
  const creditState: 'ready' | 'no-data' | 'unsupported' =
    !creditSupported ? 'unsupported' : creditHasData ? 'ready' : 'no-data'
  const summary = groupSortSummary(rules)
  const customized = hasCustomGroupSort(provider)

  /** 改一条规则：勾选 / 方向直接落库；上移下移改的是数组里的位置 */
  const changeRule = (key: GroupSortRule['key'], patch: Partial<GroupSortRule>) => {
    const op = (patch as unknown as { __move?: 'up' | 'down' }).__move
    const next = rules.map(rule => ({ ...rule }))
    const index = next.findIndex(rule => rule.key === key)
    if (index < 0) return
    if (op) {
      // 上移 / 下移改的是**优先级**（数组位置）；手动顺序是兜底、不在数组里，
      // 所以数组末尾就是它前面 —— 下移的边界是 next.length - 1
      const to = op === 'up' ? index - 1 : index + 1
      if (to < 0 || to >= next.length) return
      const [moved] = next.splice(index, 1)
      next.splice(to, 0, moved)
    } else {
      next[index] = { ...next[index], ...patch } as GroupSortRule
    }
    setGroupSortRules(provider, next)
  }

  return (
    <Dialog open onOpenChange={next => { if (!next) onClose() }}>
      <DialogContent className='w-[min(560px,calc(100vw-48px))]' showCloseButton>
        <DialogHeader>
          <span className='inline-flex size-[26px] flex-none items-center justify-center'>
            {icon
              ? <img src={icon} alt='' className='size-[26px] rounded-md object-contain' loading='lazy' />
              : <span className='inline-flex size-[26px] items-center justify-center rounded-md bg-surface-3 text-[13px] font-bold text-muted-foreground'>
                  {String(label || '?').trim().slice(0, 1).toUpperCase()}
                </span>}
          </span>
          <DialogTitle>{t('{name} · 分组设置', { name: t(label) })}</DialogTitle>
        </DialogHeader>

        <DialogBody>
          <GroupSettingSection title={t('排序')} hint={t('按优先级从上到下比较，勾选的项才参与')}>
            {rules.map((rule, index) => (
              <RuleRow
                key={rule.key}
                rule={rule}
                priority={index + 1}
                isLast={index === rules.length - 1}
                state={rule.key === 'credit' ? creditState : 'ready'}
                onChange={patch => changeRule(rule.key, patch)}
              />
            ))}
            {/* 手动顺序：固定末位、不可勾选 —— 它是兜底而不是一条规则 */}
            <div className='grid grid-cols-[16px_15px_minmax(0,1fr)] items-center gap-2 px-1.5 py-1.5'>
              <span className='text-center text-[10.5px] font-bold text-subtle tabular-nums'>{rules.length + 1}</span>
              <span className='inline-flex items-center justify-center text-subtle' title={t('固定兜底，不受开关影响')}>
                <svg viewBox='0 0 24 24' width='12' height='12'>
                  <g fill='none' stroke='currentColor' strokeWidth='2' strokeLinecap='round' strokeLinejoin='round'>
                    <rect x='4.5' y='10' width='15' height='10.5' rx='2' />
                    <path d='M8 10V7a4 4 0 0 1 8 0v3' />
                  </g>
                </svg>
              </span>
              <span className='flex min-w-0 flex-col gap-px'>
                <span className='text-xs font-medium'>{t('组内手动顺序')}</span>
                <span className='text-[10.5px] text-subtle'>{t('上面所有规则都相同时，保持你拖出来的顺序')}</span>
              </span>
            </div>
            <p className='m-0 mt-2 rounded-xs bg-surface-inset px-2.5 py-2 text-[10.5px] leading-relaxed text-subtle'>
              {t('当前规则：{rules}。自动排序生效时，组内拖动把手会锁定；关掉全部规则即回到手动顺序。', {
                rules: summary || t('仅按手动顺序'),
              })}
            </p>
            {/* 规则默认全关时，把「怎么开始用」说清楚 —— 否则用户看到两个空勾选框
                会以为功能没生效，而实际上这正是默认状态 */}
            {!summary ? (
              <p className='m-0 mt-1.5 px-2.5 text-[10.5px] leading-relaxed text-subtle'>
                {t('默认不排序：组内顺序保持你拖出来的样子。需要自动排序时，勾选上面的规则即可。')}
              </p>
            ) : null}
          </GroupSettingSection>

          {/* 后续设置的位置：先把「这里还会有东西」说清楚，加小节时直接替换这一块 */}
          <GroupSettingSection title={t('更多设置')} hint={t('后续加入')}>
            <div className='flex flex-wrap gap-1.5'>
              {[t('默认展开 / 收起'), t('新账号落到组首 / 组尾'), t('组内并发上限'), t('余额低于阈值时的提示'), t('本组代理')]
                .map(item => (
                  <span key={item}
                    className='inline-flex h-[22px] items-center rounded-pill border border-dashed border-border px-2.5 text-[10.5px] text-subtle'>
                    {item}
                  </span>
                ))}
            </div>
          </GroupSettingSection>
        </DialogBody>

        <DialogFooter>
          <Button variant='ghost' size='sm' disabled={!customized}
            title={customized ? t('恢复到「积分到期 → 添加时间」这套默认规则') : t('当前已经是默认规则')}
            onClick={() => setGroupSortRules(provider, null)}>
            {t('恢复默认')}
          </Button>
          <span className='mr-auto text-[10.5px] text-subtle'>{t('改动即时生效')}</span>
          <Button variant='default' onClick={onClose}>{t('完成')}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
