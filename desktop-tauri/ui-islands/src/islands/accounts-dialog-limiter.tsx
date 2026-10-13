/**
 * 账号设置弹窗的「限制器」段：**每账号**的限制规则列表（余额 / Token 两类）。
 *
 * 独立成文件的理由与 accounts-dialog-usage.tsx 相同 —— dialogs.tsx 已超过单文件
 * 行数约定，这是完整的功能块（草稿 + 校验 + 渲染）。
 *
 * ── 与旧「余额不足处理」的关系 ────────────────────────────────
 * 旧的「余额不足时 + 余额阈值」两行是这里的一条**余额规则**；「不处理」档取消
 * —— 没有规则（或全部停用）即不限制。Token 规则按**重置方式**二选一：固定周期
 * （对齐自然时间的固定窗口，30 分钟 ~ 24 小时）或自然日（每天本地时区 0 点重置
 * —— 有的账号过了 0 点额度就回来）。保存走 PATCH /api/accounts 的 `limiters`
 * 键（整块替换，后端 `apply_patch` 归一化校验并顺手同步旧 `lowBalance` 供降级兼容）。
 *
 * ── 添加 / 编辑是二级弹层 ────────────────────────────────────
 * 编辑器用组件库的 Dialog 嵌在账号设置弹窗**之上**打开（添加 / 编辑复用一张卡），
 * 不把表单平铺在规则列表下面 —— 列表只承载「看与开关」，「填」交给弹层，
 * 焦点圈定与 Esc 关闭都是 Dialog 现成的。
 *
 * 规则列表的**实时状态**（未触发 / 已跳过 / 已触发）读的是本文件的余额缓存
 * 与 Token 周期读数（`usageEntryOf` / `tokenUsageOf`），与账号列表徽章同源。
 */

import * as React from 'react'
import {
  Badge, Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle,
  Input, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Switch,
} from '@ui'
import { t } from '../i18n'
import {
  balanceBlockedOf, formatIntervalSeconds, formatTokenCount, limitersOf,
  TOKEN_PERIOD_MAX_SECONDS, TOKEN_PERIOD_MIN_SECONDS, tokenCountdownText,
  tokenReadingForRule, tokenWindowInfo,
} from './accounts-domain'
import { tokenUsageOf, usageEntryOf } from './accounts-data'
import { type AccountRecord, type LimiterRule, type TokenReading } from './accounts-shared'

/** 一条规则在列表里的实时状态（徽章的文案与色调） */
type RuleState = {
  text: string
  /** ok=绿（未触发） warn=黄（跳过 / 压线） danger=红（禁用已执行） off=灰（已停用） */
  kind: 'ok' | 'warn' | 'danger' | 'off'
  title?: string
}

/** 规则的**触发条件**一句话（列表与编辑器共用；Token 阈值带「万」的紧凑读法） */
export function describeRuleCondition(rule: LimiterRule): string {
  if (rule.type === 'balance') return t('余额 < {threshold}', { threshold: rule.threshold })
  if (rule.reset === 'daily') return t('每日消耗 ≥ {count} Token（每天 0 点重置）', { count: formatTokenCount(rule.threshold) })
  return t('每 {period}消耗 ≥ {count} Token', { period: formatIntervalSeconds(rule.period ?? 0), count: formatTokenCount(rule.threshold) })
}

/** 规则的**触发动作**一句话（与 Select 选项的文案一致） */
export function describeRuleAction(rule: LimiterRule): string {
  return rule.action === 'disable' ? t('禁用该账号（需手动重新启用）') : t('跳过该账号（自动恢复）')
}

/** Token 阈值的完整数字（title 用；千分位） */
function exactTokenText(rule: LimiterRule): string {
  return `${Math.round(rule.threshold).toLocaleString('en-US')} Token`
}

/**
 * 一条规则的实时状态：读余额缓存 / Token 窗口读数，与列表徽章同判据
 * （balanceBlockedOf / 后端选路过滤）。禁用档命中且账号已被禁用时是红的 ——
 * 那是「硬动作已执行」与「软跳过」在观感上的分界。
 */
function ruleStateOf(account: AccountRecord, rule: LimiterRule): RuleState {
  if (rule.enabled === false) {
    return { text: t('已停用'), kind: 'off' }
  }
  if (rule.type === 'balance') {
    const entry = usageEntryOf(account)
    const blocked = balanceBlockedOf(account, entry)
    const data = (entry && typeof entry === 'object' ? entry : null) as Record<string, unknown> | null
    const raw = data ? data.totalLeft ?? data.available : undefined
    const current = raw === null || raw === undefined || raw === '' || data?.unlimited
      ? ''
      : t(' · 当前 {n}', { n: Number(raw) })
    return blocked
      ? { text: t('已跳过{current}', { current }), kind: 'warn', title: t('余额低于阈值，转发时会跳过该账号（余额回升自动恢复）') }
      : { text: t('未触发{current}', { current }), kind: 'ok' }
  }
  const readings = tokenUsageOf(account)
  const reading = tokenReadingForRule(rule, readings)
  const used = reading ? reading.used : 0
  const quota = formatTokenCount(rule.threshold)
  const usage = rule.reset === 'daily'
    ? t(' · 今日 {used} / {quota}', { used: formatTokenCount(used), quota })
    : t(' · 本周期 {used} / {quota}', { used: formatTokenCount(used), quota })
  const countdown = reading ? ` · ${tokenCountdownText(tokenWindowInfo(rule).remainingMs, rule.reset === 'daily')}` : ''
  const hit = reading !== null && used >= rule.threshold
  if (!hit) {
    return { text: t('未触发{usage}{countdown}', { usage, countdown }), kind: 'ok', title: exactTokenText(rule) }
  }
  if (rule.action === 'disable') {
    return account.enabled === false
      ? { text: t('已触发 · 账号已禁用'), kind: 'danger', title: t('重置也不恢复，需手动启用') }
      : { text: t('已达上限{usage}', { usage }), kind: 'warn', title: t('稍后由后台判定自动禁用') }
  }
  return {
    text: t('已跳过{usage}{countdown}', { usage, countdown }),
    kind: 'warn',
    title: t('窗口内已用 {used} / {limit} Token，转发时会跳过该账号（重置后自动恢复）', {
      used: Math.round(used).toLocaleString('en-US'),
      limit: Math.round(rule.threshold).toLocaleString('en-US'),
    }),
  }
}

/* ─── 编辑器弹层（添加 / 编辑复用一张卡）────────────────────── */

/** 编辑器的草稿：数字输入框以字符串中转（输入中途允许为空 / 非法） */
type LimiterEditorDraft = {
  type: 'balance' | 'token'
  action: 'skip' | 'disable'
  thresholdInput: string
  /** Token 阈值的输入单位（万 / 个）：界面辅助，落库统一换算成原始个数 */
  tokenUnit: 'wan' | 'unit'
  /** 重置方式：固定周期（周期输入）/ 自然日（每天 0 点重置，无周期输入） */
  reset: 'fixed' | 'daily'
  periodInput: string
  periodUnit: 'minutes' | 'hours'
}

/** 周期秒数 → 「数字 + 最大可整除的单位」（900 秒 → 15 分钟）；非法给 1 小时缺省 */
function splitPeriod(seconds: number): { value: string; unit: 'minutes' | 'hours' } {
  if (seconds > 0 && seconds % 3600 === 0) return { value: String(seconds / 3600), unit: 'hours' }
  if (seconds > 0 && seconds % 60 === 0) return { value: String(seconds / 60), unit: 'minutes' }
  return { value: '1', unit: 'hours' }
}

/** 规则 → 编辑器草稿（Token 阈值按当前单位换算成输入框里的数） */
function editorDraftOf(rule: LimiterRule | null): LimiterEditorDraft {
  const blank: LimiterEditorDraft = {
    type: 'balance', action: 'skip', thresholdInput: '', tokenUnit: 'wan',
    reset: 'fixed', periodInput: '1', periodUnit: 'hours',
  }
  if (!rule) return blank
  if (rule.type === 'balance') {
    return { ...blank, type: 'balance', action: rule.action, thresholdInput: String(rule.threshold) }
  }
  if (rule.reset === 'daily') {
    return { ...blank, type: 'token', action: rule.action, reset: 'daily', thresholdInput: String(rule.threshold % 10_000 === 0 ? rule.threshold / 10_000 : rule.threshold), tokenUnit: rule.threshold % 10_000 === 0 ? 'wan' : 'unit' }
  }
  const period = splitPeriod(rule.period ?? 0)
  return {
    ...blank,
    type: 'token',
    action: rule.action,
    reset: 'fixed',
    thresholdInput: String(rule.threshold % 10_000 === 0 ? rule.threshold / 10_000 : rule.threshold),
    tokenUnit: rule.threshold % 10_000 === 0 ? 'wan' : 'unit',
    periodInput: period.value,
    periodUnit: period.unit,
  }
}

/** 编辑器草稿 → 规则（校验不过返回错误文案；与后端 `apply_patch` 同一套规则） */
function ruleOfDraft(draft: LimiterEditorDraft): { error: string } | { rule: LimiterRule } {
  const thresholdRaw = Number(draft.thresholdInput)
  if (!Number.isFinite(thresholdRaw) || thresholdRaw <= 0) {
    return { error: t('限制阈值必须是大于 0 的数字') }
  }
  if (draft.type === 'balance') {
    return { rule: { type: 'balance', action: draft.action, threshold: thresholdRaw, enabled: true } }
  }
  const threshold = Math.round(draft.tokenUnit === 'wan' ? thresholdRaw * 10_000 : thresholdRaw)
  if (draft.reset === 'daily') {
    return { rule: { type: 'token', action: draft.action, threshold, reset: 'daily', enabled: true } }
  }
  const periodRaw = Number(draft.periodInput) * (draft.periodUnit === 'hours' ? 3600 : 60)
  if (!Number.isFinite(periodRaw) || periodRaw <= 0) {
    return { error: t('请填写重置周期') }
  }
  const period = Math.round(periodRaw)
  if (period < TOKEN_PERIOD_MIN_SECONDS || period > TOKEN_PERIOD_MAX_SECONDS) {
    return { error: t('重置周期必须是 30 分钟 ~ 24 小时') }
  }
  return { rule: { type: 'token', action: draft.action, threshold, period, reset: 'fixed', enabled: true } }
}

/** 编辑器弹层（受控：`target` 为 null = 关闭；index -1 = 添加）。 */
function LimiterEditorDialog({
  account, target, onClose, onSave,
}: {
  account: AccountRecord
  /** 正在编辑的规则与它在草稿里的下标；null = 关闭 */
  target: { index: number; draft: LimiterEditorDraft } | null
  onClose: () => void
  onSave: (rule: LimiterRule, index: number) => void
}) {
  const [error, setError] = React.useState('')
  // 草稿镜像成本地 state：弹层打开期间字段改动只落在本地，「保存」才上抛 ——
  // 「取消」天然丢弃改动，不需要父组件回滚
  const [draftLocal, setDraftLocal] = React.useState<LimiterEditorDraft | null>(target?.draft ?? null)
  React.useEffect(() => {
    setDraftLocal(target?.draft ?? null)
    setError('')
  }, [target])
  if (!target || !draftLocal) return null
  const update = (next: Partial<LimiterEditorDraft>): void => {
    setError('')
    setDraftLocal({ ...draftLocal, ...next })
  }
  const save = (): void => {
    const result = ruleOfDraft(draftLocal)
    if ('error' in result) {
      setError(result.error)
      return
    }
    onSave(result.rule, target.index)
  }

  /** 实况提示：余额读数 / 窗口用量与倒计时（判不出给兜底口径说明） */
  const liveHint = (): string => {
    if (draftLocal.type === 'balance') {
      const data = usageEntryOf(account)
      if (data && typeof data === 'object') {
        const fields = data as Record<string, unknown>
        if (fields.unlimited) return t('当前可用 ∞（不参与余额不足判定）')
        const raw = fields.totalLeft ?? fields.available
        if (raw !== null && raw !== undefined && raw !== '') {
          return t('当前余额 {n}；与余额列同一数字口径，严格小于才触发（等于阈值仍可用）', { n: Number(raw) })
        }
      }
      return t('读数来自自动余额查询；与余额列同一数字口径，严格小于才触发（等于阈值仍可用）')
    }
    const result = ruleOfDraft(draftLocal)
    if (!('rule' in result)) return ''
    const rule = result.rule
    const reading = tokenReadingForRule(rule, tokenUsageOf(account))
    if (!reading) {
      return rule.reset === 'daily'
        ? t('窗口 = 今天 0 点起（本地时区），0 点重置即清零恢复；按账号在 requests 表聚合')
        : t('窗口对齐自然时间（整点 / 整 N 分钟一轮回），重置即清零恢复；按账号在 requests 表聚合')
    }
    const info = tokenWindowInfo(rule)
    const used = formatTokenCount(reading.used)
    const quota = formatTokenCount(rule.threshold)
    const scope = rule.reset === 'daily' ? t('今日已用') : t('本周期已用')
    return t('{scope} {used} / {quota} Token，{countdown}重置', {
      scope, used, quota, countdown: tokenCountdownText(info.remainingMs, info.daily),
    })
  }

  return (
    <Dialog open onOpenChange={next => { if (!next) onClose() }}>
      <DialogContent className='w-[min(580px,calc(100vw-48px))]' showCloseButton>
        <DialogHeader>
          <DialogTitle>{target.index >= 0 ? t('编辑限制') : t('添加限制')}</DialogTitle>
        </DialogHeader>
        <DialogBody>
          <div className='field-row'>
            <label>{t('限制类型')}</label>
            <Select value={draftLocal.type}
              onValueChange={value => update({ type: value as LimiterEditorDraft['type'], thresholdInput: '' })}>
              <SelectTrigger className='w-[150px]' aria-label={t('限制类型')}>
                <SelectValue>{draftLocal.type === 'balance' ? t('余额') : t('Token 消耗')}</SelectValue>
              </SelectTrigger>
              <SelectContent>
                <SelectItem value='balance'>{t('余额')}</SelectItem>
                <SelectItem value='token'>{t('Token 消耗')}</SelectItem>
              </SelectContent>
            </Select>
            <span className='detail'>{t('余额沿用现有口径；Token 按重置方式内的累计消耗判定（切换类型会清空阈值）')}</span>
          </div>
          {draftLocal.type === 'balance' ? (
            <>
              <div className='field-row'>
                <label>{t('触发条件')}</label>
                <span className='detail' style={{ fontSize: 13 }}>{t('余额低于')}</span>
                <Input type='number' min={0} step={1} className='max-w-[120px]' placeholder={t('阈值')}
                  value={draftLocal.thresholdInput}
                  onChange={event => update({ thresholdInput: event.currentTarget.value })} />
                <span className='detail' style={{ fontSize: 13 }}>{t('时')}</span>
              </div>
              <div className='field-row'>
                <label>{t('触发后')}</label>
                <Select value={draftLocal.action}
                  onValueChange={value => update({ action: value as LimiterEditorDraft['action'] })}>
                  <SelectTrigger className='min-w-[280px]' aria-label={t('触发动作')}>
                    <SelectValue>{draftLocal.action === 'disable' ? t('禁用该账号（需手动重新启用）') : t('跳过该账号（余额回升自动恢复）')}</SelectValue>
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value='skip'>{t('跳过该账号（余额回升自动恢复）')}</SelectItem>
                    <SelectItem value='disable'>{t('禁用该账号（需手动重新启用）')}</SelectItem>
                  </SelectContent>
                </Select>
              </div>
            </>
          ) : (
            <>
              <div className='field-row'>
                <label>{t('重置方式')}</label>
                <Select value={draftLocal.reset}
                  onValueChange={value => update({ reset: value as LimiterEditorDraft['reset'] })}>
                  <SelectTrigger className='w-[190px]' aria-label={t('重置方式')}>
                    <SelectValue>{draftLocal.reset === 'daily' ? t('自然日（每天 0 点）') : t('固定周期')}</SelectValue>
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value='fixed'>{t('固定周期')}</SelectItem>
                    <SelectItem value='daily'>{t('自然日（每天 0 点）')}</SelectItem>
                  </SelectContent>
                </Select>
                <span className='detail'>{t('有的账号过了 0 点额度就回来 —— 选「自然日」')}</span>
              </div>
              <div className='field-row'>
                <label>{t('触发条件')}</label>
                {draftLocal.reset === 'daily' ? (
                  <>
                    <span className='detail' style={{ fontSize: 13 }}>{t('每日消耗 ≥')}</span>
                    <Input type='number' min={0} step={1} className='max-w-[110px]' placeholder={t('阈值')}
                      value={draftLocal.thresholdInput}
                      onChange={event => update({ thresholdInput: event.currentTarget.value })} />
                    <Select value={draftLocal.tokenUnit}
                      onValueChange={value => update({ tokenUnit: value as LimiterEditorDraft['tokenUnit'] })}>
                      <SelectTrigger className='w-[92px]' aria-label={t('Token 单位')}>
                        <SelectValue>{draftLocal.tokenUnit === 'wan' ? t('万') : t('个')}</SelectValue>
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value='wan'>{t('万')}</SelectItem>
                        <SelectItem value='unit'>{t('个')}</SelectItem>
                      </SelectContent>
                    </Select>
                    <span className='detail' style={{ fontSize: 13 }}>{t('Token 时')}</span>
                  </>
                ) : (
                  <>
                    <span className='detail' style={{ fontSize: 13 }}>{t('每')}</span>
                    <Input type='number' min={1} step={1} className='max-w-[90px]' placeholder={t('周期')}
                      value={draftLocal.periodInput}
                      onChange={event => update({ periodInput: event.currentTarget.value })} />
                    <Select value={draftLocal.periodUnit}
                      onValueChange={value => update({ periodUnit: value as LimiterEditorDraft['periodUnit'] })}>
                      <SelectTrigger className='w-[92px]' aria-label={t('周期单位')}>
                        <SelectValue>{draftLocal.periodUnit === 'hours' ? t('小时') : t('分钟')}</SelectValue>
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value='minutes'>{t('分钟')}</SelectItem>
                        <SelectItem value='hours'>{t('小时')}</SelectItem>
                      </SelectContent>
                    </Select>
                    <span className='detail' style={{ fontSize: 13 }}>{t('内消耗 ≥')}</span>
                    <Input type='number' min={0} step={1} className='max-w-[110px]' placeholder={t('阈值')}
                      value={draftLocal.thresholdInput}
                      onChange={event => update({ thresholdInput: event.currentTarget.value })} />
                    <Select value={draftLocal.tokenUnit}
                      onValueChange={value => update({ tokenUnit: value as LimiterEditorDraft['tokenUnit'] })}>
                      <SelectTrigger className='w-[92px]' aria-label={t('Token 单位')}>
                        <SelectValue>{draftLocal.tokenUnit === 'wan' ? t('万') : t('个')}</SelectValue>
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value='wan'>{t('万')}</SelectItem>
                        <SelectItem value='unit'>{t('个')}</SelectItem>
                      </SelectContent>
                    </Select>
                    <span className='detail' style={{ fontSize: 13 }}>{t('Token 时')}</span>
                  </>
                )}
              </div>
              <div className='field-row'>
                <label>{t('触发后')}</label>
                <Select value={draftLocal.action}
                  onValueChange={value => update({ action: value as LimiterEditorDraft['action'] })}>
                  <SelectTrigger className='min-w-[280px]' aria-label={t('触发动作')}>
                    <SelectValue>{draftLocal.action === 'disable' ? t('禁用该账号（需手动重新启用）') : t('跳过该账号（重置后自动恢复）')}</SelectValue>
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value='skip'>{t('跳过该账号（重置后自动恢复）')}</SelectItem>
                    <SelectItem value='disable'>{t('禁用该账号（需手动重新启用）')}</SelectItem>
                  </SelectContent>
                </Select>
              </div>
            </>
          )}
          <p className='detail limiter-hint'>{liveHint()}</p>
          {error ? <p className='detail text-destructive'>{error}</p> : null}
        </DialogBody>
        <DialogFooter>
          <Button variant='outline' onClick={onClose}>{t('取消')}</Button>
          <Button variant='default' onClick={save}>{target.index >= 0 ? t('保存规则') : t('添加规则')}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

/**
 * 规则列表（受控组件：草稿在弹窗手里，保存时由 readLimiterChanges 汇总；
 * 添加 / 编辑的表单在二级弹层，见 LimiterEditorDialog）。
 */
export function LimiterSection({
  account, draft, onChange,
}: {
  account: AccountRecord
  draft: LimiterRule[]
  onChange: (next: LimiterRule[]) => void
}) {
  const [editor, setEditor] = React.useState<{ index: number; draft: LimiterEditorDraft } | null>(null)

  const openAdd = (): void => setEditor({ index: -1, draft: editorDraftOf(null) })
  const openEdit = (index: number): void => setEditor({ index, draft: editorDraftOf(draft[index]) })

  const saveEditor = (rule: LimiterRule, index: number): void => {
    const next = [...draft]
    if (index >= 0) next[index] = rule
    else next.push(rule)
    onChange(next)
    setEditor(null)
  }

  const removeRule = (index: number): void => {
    onChange(draft.filter((_, i) => i !== index))
    // 编辑弹层开着时列表行序可能变化，一并收起最稳
    if (editor) setEditor(null)
  }

  const toggleRule = (index: number, enabled: boolean): void => {
    const next = [...draft]
    next[index] = { ...next[index], enabled }
    onChange(next)
  }

  return (
    <>
      {draft.length ? (
        <div className='limiter-rules mt-2.5'>
          {draft.map((rule, index) => {
            const state = ruleStateOf(account, rule)
            return (
              <div key={index} className='limiter-rule' data-off={rule.enabled === false || undefined}>
                <Switch checked={rule.enabled !== false} aria-label={t('启用该规则')}
                  onCheckedChange={next => toggleRule(index, next === true)} />
                <div className='limiter-rule-main'>
                  <span className='limiter-rule-cond'>
                    <Badge variant='secondary' shape='tag'>{rule.type === 'balance' ? t('余额') : 'Token'}</Badge>
                    {' '}{t('{cond} 时 → {action}', { cond: describeRuleCondition(rule), action: describeRuleAction(rule) })}
                  </span>
                </div>
                <Badge variant={state.kind === 'ok' ? 'success' : state.kind === 'warn' ? 'warning'
                  : state.kind === 'danger' ? 'destructive' : 'secondary'} shape='tag' title={state.title}>
                  {state.text}
                </Badge>
                <div className='limiter-rule-ops'>
                  <Button variant='ghost' onClick={() => openEdit(index)}>{t('编辑')}</Button>
                  <Button variant='ghost' onClick={() => removeRule(index)}>{t('删除')}</Button>
                </div>
              </div>
            )
          })}
        </div>
      ) : (
        <p className='detail mt-2.5'>{t('还没有限制规则：账号照常参与转发，余额与 Token 消耗都不设上限。')}</p>
      )}
      <div className='field-row mt-2.5'>
        <Button variant='outline' onClick={openAdd}>{t('＋ 添加限制')}</Button>
        <span className='detail'>{t('可添加多条；同类规则也允许并存（例如一条按余额、两条不同重置方式的 Token）')}</span>
      </div>
      <LimiterEditorDialog account={account} target={editor} onClose={() => setEditor(null)} onSave={saveEditor} />
    </>
  )
}

/**
 * 读本段的改动：与账号的**有效规则**逐字节相同 → null（不提交，与弹窗里各段的
 * 读取函数同一形态）。比较用 JSON 序列化（键序由构造顺序决定，与后端
 * 「JSON.stringify 比较」同一形态）；校验已在编辑器保存时做过，这里不再重复。
 */
export function readLimiterChanges(
  account: AccountRecord,
  draft: LimiterRule[],
): { patch: { limiters: LimiterRule[] } } | null {
  const current = limitersOf(account)
  if (JSON.stringify(draft) === JSON.stringify(current)) return null
  return { patch: { limiters: draft } }
}

/** 弹窗挂载时的初始草稿：账号的**有效规则**（未显式配置 = 后端推导的结果）。 */
export function limiterDraftOf(account: AccountRecord | null | undefined): LimiterRule[] {
  return limitersOf(account).map(rule => ({ ...rule }))
}
