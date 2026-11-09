/** 分组与勾选账号共用的批量操作弹窗。 */

import * as React from 'react'
import {
  Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogSection,
  DialogTitle, Input, Label, RadioGroup, RadioGroupItem, Select, SelectContent,
  SelectItem, SelectTrigger, SelectValue,
  Field, FieldLabel, FieldSet,
} from '@ui'
import { t } from '../i18n'
import {
  ACCOUNT_CONCURRENCY_MAX as CONC_MAX, errorMessage, esc, shared, toast,
  type AccountRecord, type BatchAccountsPayload, type BatchAccountsResult, type BatchLimiterMode, type LimiterGroup,
} from './accounts-shared'
import {
  displayNameOf, providerFeatures, providerOf, supportsProxyRotate, supportsUsage,
} from './accounts-domain'
import { allAccounts, findAccount, syncBalancesSnapshot } from './accounts-data'
import { confirmBatchResult, refreshSavedAccounts } from './accounts-update'
import { tokenRefreshSkipReason } from './accounts-authorization'
import { refreshAccountTokens, type BatchTokenRefreshProgress } from './accounts-batch-refresh'
import { limiterDraftOf, LimiterSection } from './accounts-dialog-limiter'
import { readUsageDraft, UsageSettingsSection, usageDraftOf, type UsageDraft } from './accounts-dialog-usage'
import { draftOfProxy, ProxyForm, readProxyDraft, type ProxyDraft } from './accounts-proxy-form'

const BATCH_ACTIONS = [
  { value: 'enable', label: () => t('启用') },
  { value: 'disable', label: () => t('禁用') },
  { value: 'refresh', label: () => t('刷新 Token') },
  { value: 'proxy', label: () => t('修改代理') },
  { value: 'concurrency', label: () => t('设置并发上限') },
  { value: 'usageQuery', label: () => t('查询设置') },
  { value: 'limiters', label: () => t('限制器') },
  { value: 'remove', label: () => t('删除') },
] as const

const LIMITER_MODES: Array<{ value: BatchLimiterMode; label: () => string }> = [
  { value: 'append', label: () => t('追加规则（保留原有）') },
  { value: 'replace', label: () => t('替换全部规则') },
  { value: 'enable', label: () => t('启用全部规则') },
  { value: 'disable', label: () => t('停用全部规则') },
  { value: 'clear', label: () => t('清空全部规则') },
]

function labelOf(account: AccountRecord | null | undefined): string {
  return displayNameOf(account) || account?.id || ''
}

export function BatchDialog({ ids, action, onClose }: { ids: string[]; action: string; onClose: () => void }) {
  const [current, setCurrent] = React.useState(BATCH_ACTIONS.some(item => item.value === action) ? action : 'enable')
  const [proxyDraft, setProxyDraft] = React.useState<ProxyDraft>(() => draftOfProxy(null))
  const [concDraft, setConcDraft] = React.useState('')
  // 混选时没有共同的间隔；不预填，避免直接执行就覆盖整组配置。
  const [usageDraft, setUsageDraft] = React.useState<UsageDraft>(() => ({ ...usageDraftOf(null), intervalInput: '' }))
  const [limiterMode, setLimiterMode] = React.useState<BatchLimiterMode>('append')
  const [limiterDraft, setLimiterDraft] = React.useState<LimiterGroup[]>([])
  const [sourceId, setSourceId] = React.useState('')
  const [busy, setBusy] = React.useState(false)
  const [result, setResult] = React.useState<React.ReactNode>('')
  const [refreshProgress, setRefreshProgress] = React.useState<BatchTokenRefreshProgress | null>(null)
  const running = React.useRef(false)
  const accounts = ids.map(id => findAccount(id)).filter((account): account is AccountRecord => account !== null)
  const usageAccounts = accounts.filter(supportsUsage)
  const editingRules = current === 'limiters' && (limiterMode === 'append' || limiterMode === 'replace')
  const needsBalance = editingRules && limiterDraft.some(group => group.conditions.some(condition => condition.metric !== 'tokens' && condition.metric !== 'requests'))
  const targets = current === 'refresh'
    ? accounts.filter(account => !tokenRefreshSkipReason(account))
    : current === 'usageQuery' || needsBalance ? usageAccounts : accounts
  const skippedAccounts = accounts.filter(account => !targets.includes(account))
  const copyOptions = current === 'usageQuery' ? usageAccounts : accounts
  const source = copyOptions.find(account => account.id === sourceId)
  const concHardCap = accounts.reduce(
    (cap, account) => Math.max(cap, Number(providerFeatures(providerOf(account)).concurrencyDefault) || 0), 0,
  )
  const allowRotate = accounts.length > 0 && accounts.every(supportsProxyRotate)
  const destructive = current === 'remove' || (current === 'limiters' && (limiterMode === 'replace' || limiterMode === 'clear'))
  const verb = current === 'limiters'
    ? (limiterMode === 'append' ? t('追加限制规则') : LIMITER_MODES.find(item => item.value === limiterMode)!.label())
    : BATCH_ACTIONS.find(item => item.value === current)!.label()

  function copySettings(): void {
    if (!source) return
    if (current === 'usageQuery') setUsageDraft(usageDraftOf(source))
    else setLimiterDraft(limiterDraftOf(source))
    setResult('')
    toast(t('已复制到草稿，执行后才会应用到所选账号'))
  }

  async function confirmOperation(): Promise<boolean> {
    const ask = shared().wbConfirm?.ask
    if (current === 'remove') {
      const ok = await Promise.resolve(ask?.({
        title: t('删除选中的账号'),
        html: t('确定删除选中的 <strong>{count}</strong> 个账号？此操作不可恢复，账号的登录态会一并移除。', { count: ids.length }),
        okText: t('删除'), okClass: 'danger',
      }) ?? false)
      if (!ok) return false
    }
    if (current === 'disable' || current === 'remove') {
      const picked = new Set(ids)
      const survivors = allAccounts().filter(account => account.enabled !== false && !picked.has(account.id))
      if (!survivors.length) {
        const what = current === 'remove' ? t('删除') : t('禁用')
        return Promise.resolve(ask?.({
          title: t('全部启用中的账号将被{what}', { what }),
          html: t('这会<strong>{what}</strong>所有启用中的账号，转发将不可用。确定继续？', { what }),
          okText: what, okClass: current === 'remove' ? 'danger' : 'primary',
        }) ?? false)
      }
    }
    if (current === 'limiters' && limiterMode === 'replace') {
      return Promise.resolve(ask?.({
        title: t('替换选中账号的限制规则'),
        html: t('将用草稿中的 <strong>{rules}</strong> 条规则替换 <strong>{count}</strong> 个账号的全部限制规则，原有规则不会保留。确定继续？', {
          rules: limiterDraft.length, count: targets.length,
        }) + (skippedAccounts.length ? `<p>${t('以下 {count} 个账号不支持余额查询，本次会跳过：{names}', {
          count: skippedAccounts.length, names: esc(skippedAccounts.map(labelOf).join(t('、'))),
        })}</p>` : ''),
        okText: t('替换全部规则'), okClass: 'danger',
      }) ?? false)
    }
    if (current === 'limiters' && limiterMode === 'clear') {
      return Promise.resolve(ask?.({
        title: t('清空选中账号的限制规则'),
        html: t('确定清空 <strong>{count}</strong> 个账号的全部限制规则？余额与 Token 将不再设限；已禁用的账号不会自动启用。', { count: targets.length }),
        okText: t('清空全部规则'), okClass: 'danger',
      }) ?? false)
    }
    return true
  }

  async function run(): Promise<void> {
    if (running.current) return
    if (!ids.length) { toast(t('没有选中的账号'), 'err'); return }
    if (!targets.length) { toast(t('没有适用于本次操作的账号'), 'err'); return }
    const payload: BatchAccountsPayload = { action: current, ids }
    try {
      if (current === 'proxy') payload.proxy = readProxyDraft(proxyDraft)
      if (current === 'concurrency') {
        const raw = Number(concDraft.trim())
        if (!concDraft.trim() || !Number.isFinite(raw) || raw < 0) {
          throw new Error(t('并发上限必须是 0~{max} 的整数', { max: CONC_MAX }))
        }
        payload.maxConcurrent = Math.min(CONC_MAX, Math.round(raw))
      }
      if (current === 'usageQuery') {
        const changes = readUsageDraft(usageDraft)
        if ('error' in changes) throw new Error(changes.error)
        payload.usageQuery = changes.value
      }
      if (current === 'limiters') {
        payload.limiterMode = limiterMode
        if (editingRules) {
          if (!limiterDraft.length) throw new Error(t('请至少添加一条限制规则；清空请使用清空操作'))
          if (limiterDraft.some(group => group.conditions.some(condition =>
            condition.sourceId !== '' && condition.sourceId !== 'local' && condition.sourceId !== 'account'))) {
            throw new Error(t('套餐实例不能跨账号复制，请改用通用额度或本地保护条件'))
          }
          payload.limiterGroups = structuredClone(limiterDraft)
        }
      }
    } catch (error) {
      setResult(<span className='text-destructive'>{errorMessage(error)}</span>)
      toast(errorMessage(error), 'err')
      return
    }

    running.current = true
    setBusy(true)
    setResult('')
    setRefreshProgress(null)
    try {
      if (!(await confirmOperation())) return
      let data: BatchAccountsResult
      if (current === 'refresh') {
        data = await refreshAccountTokens(ids, setRefreshProgress)
      } else {
        const bridge = shared().workbuddyDesktop
        if (!bridge?.batchAccounts) throw new Error(t('批量操作接口不可用，请重新打开页面'))
        data = confirmBatchResult(await bridge.batchAccounts(payload), payload)
      }
      const accepted = data.ok || []
      const changed = current === 'remove' ? (data.removed || []).length
        : current === 'refresh' ? accepted.length : accepted.filter(item => item.changes?.length).length
      const unchanged = current === 'remove' ? 0 : accepted.length - changed
      const failed = data.failed || []
      const skipped = data.skipped || []
      const summary = current === 'refresh'
        ? t('刷新 Token 完成：成功 {ok} 个，跳过 {skip} 个，失败 {fail} 个', {
          ok: changed, skip: skipped.length, fail: failed.length,
        })
        : t('{verb}完成：成功 {ok} 个，未变更 {unchanged} 个，跳过 {skip} 个，失败 {fail} 个', {
          verb, ok: changed, unchanged, skip: skipped.length, fail: failed.length,
        })
      toast(summary, failed.length ? 'err' : 'ok')
      if (failed.length || skipped.length) {
        setResult(<div role='status' className='space-y-2'>
          <p>{summary}</p>
          {skipped.length ? <p>{t('跳过 {count} 个：', { count: skipped.length })}
            {skipped.map(item => `${labelOf(findAccount(String(item.id))) || item.name || item.id}（${item.reason}）`).join(t('；'))}
          </p> : null}
          {failed.length ? <p className='text-destructive'>{t('失败 {count} 个：', { count: failed.length })}
            {failed.map(item => `${labelOf(findAccount(String(item.id))) || item.id}（${item.error}）`).join(t('；'))}
          </p> : null}
        </div>)
      } else {
        onClose()
      }
      try {
        await refreshSavedAccounts()
        await syncBalancesSnapshot()
      } catch (error) {
        toast(t('批量操作已保存，但列表刷新失败：{error}', { error: errorMessage(error) }), 'err')
      }
    } catch (error) {
      setResult(<span className='text-destructive'>{errorMessage(error)}</span>)
      toast(t('批量操作失败：{error}', { error: errorMessage(error) }), 'err')
    } finally {
      running.current = false
      setBusy(false)
      setRefreshProgress(null)
    }
  }

  return (
    <Dialog open onOpenChange={next => { if (next || busy) return; onClose() }}>
      <DialogContent className='w-[min(900px,calc(100vw-48px))]' showCloseButton={!busy}>
        <DialogHeader><DialogTitle>{t('批量操作 · 已选 {count} 个账号', { count: ids.length })}</DialogTitle></DialogHeader>
        <DialogBody>
          <FieldSet disabled={busy} className='m-0 flex min-w-0 flex-col gap-4 border-0 p-0'>
            <DialogSection>
              <h3>{t('将作用于以下账号')}</h3>
              <p className='max-h-[84px] overflow-y-auto'>{targets.map(labelOf).join(t('、')) || t('没有适用于本次操作的账号')}</p>
              {skippedAccounts.length ? <p className='max-h-[84px] overflow-y-auto'>{current === 'refresh'
                ? <>{t('跳过 {count} 个：', { count: skippedAccounts.length })}
                  {skippedAccounts.map(account => `${labelOf(account)}（${tokenRefreshSkipReason(account)}）`).join(t('；'))}
                </>
                : t('以下 {count} 个账号不支持余额查询，本次会跳过：{names}', {
                  count: skippedAccounts.length, names: skippedAccounts.map(labelOf).join(t('、')),
                })}</p> : null}
              {current === 'refresh'
                ? <p>{t('逐个刷新所选账号的 Token，未到期也会尝试刷新；不改变启用状态、代理或转发顺序。')}</p>
                : current !== 'remove' ? <p>{t('只修改本次选择的设置，不影响备注、凭证、转发顺序或其他设置。')}</p> : null}
            </DialogSection>
            <DialogSection>
              <h3>{t('操作')}</h3>
              <RadioGroup value={current} onValueChange={value => { setCurrent(value); setResult('') }}
                disabled={busy} className='flex-row flex-wrap items-center gap-5' aria-label={t('批量动作')}>
                {BATCH_ACTIONS.map(item => <Label key={item.value} className='inline-flex cursor-pointer items-center gap-2 font-normal'>
                  <RadioGroupItem value={item.value} />{item.label()}
                </Label>)}
              </RadioGroup>
              {current === 'proxy' ? <div className='mt-2.5'>
                <ProxyForm draft={proxyDraft} onChange={setProxyDraft} idPrefix='batch-proxy' allowRotate={allowRotate} />
              </div> : null}
              {current === 'concurrency' ? <div className='mt-2.5'>
                <Field orientation='horizontal' className='field-row'>
                  <FieldLabel htmlFor='batch-conc-input'>{t('并发上限')}</FieldLabel>
                  <Input id='batch-conc-input' type='number' min={0} max={CONC_MAX} step={1}
                    className='max-w-[110px]' placeholder={t('并发上限')} value={concDraft}
                    onChange={event => setConcDraft(event.currentTarget.value)} />
                  <span className='detail'>{t('0 = 不限')}</span>
                </Field>
                <p className='detail mt-2.5'>{concHardCap > 0
                  ? t('并发上限 0~{max}；CodeArts 等有默认值的家填 0 即按本家默认；达到上限的账号会被跳过、请求转给其他账号', { max: CONC_MAX })
                  : t('并发上限 0~{max}；达到上限的账号会被跳过、请求转给其他账号', { max: CONC_MAX })}</p>
              </div> : null}
              {current === 'limiters' ? <div className='mt-2.5'>
                <Field orientation='horizontal' className='field-row'>
                  <FieldLabel htmlFor='batch-limiter-mode'>{t('规则操作')}</FieldLabel>
                  <Select value={limiterMode} onValueChange={value => { setLimiterMode(value as BatchLimiterMode); setResult('') }}>
                    <SelectTrigger id='batch-limiter-mode' className='min-w-[220px]'>
                      <SelectValue>{LIMITER_MODES.find(item => item.value === limiterMode)!.label()}</SelectValue>
                    </SelectTrigger>
                    <SelectContent>{LIMITER_MODES.map(item => <SelectItem key={item.value} value={item.value}>{item.label()}</SelectItem>)}</SelectContent>
                  </Select>
                </Field>
                <p className='detail mt-2.5'>{limiterMode === 'append'
                  ? t('保留各账号原有规则，只追加下面的规则；完全相同的规则不会重复添加。')
                  : limiterMode === 'replace'
                    ? t('用下面的规则统一替换各账号的全部规则，执行前会再次确认。')
                    : limiterMode === 'clear'
                      ? t('删除各账号的全部限制规则，执行前会再次确认。')
                      : t('仅统一修改现有规则的开关，保留阈值、动作和重置方式；不改变账号的启用状态。')}</p>
              </div> : null}
              {current === 'usageQuery' || editingRules ? <div className='mt-2.5'>
                <Field orientation='horizontal' className='field-row'>
                  <FieldLabel htmlFor='batch-settings-source'>{t('复制自账号')}</FieldLabel>
                  <Select value={source?.id || ''} disabled={!copyOptions.length} onValueChange={value => setSourceId(String(value))}>
                    <SelectTrigger id='batch-settings-source' className='min-w-0 flex-1'>
                      <SelectValue>{source ? labelOf(source) : t('选择所选账号作为模板（可选）')}</SelectValue>
                    </SelectTrigger>
                    <SelectContent>{copyOptions.map(account => <SelectItem key={account.id} value={account.id}>{labelOf(account)}</SelectItem>)}</SelectContent>
                  </Select>
                  <Button variant='outline' size='sm' disabled={!source} onClick={copySettings}>{t('复制当前设置')}</Button>
                </Field>
                {current === 'usageQuery' ? <>
                  <UsageSettingsSection draft={usageDraft} onChange={setUsageDraft} idPrefix='batch-usage' />
                  <p className='detail mt-2.5'>{t('统一设置自动查询的开关、间隔和失败重试次数；不会复制或覆盖余额查询凭证。')}</p>
                </> : <LimiterSection draft={limiterDraft} onChange={setLimiterDraft} allowBalance={usageAccounts.length > 0} />}
              </div> : null}
              {current === 'usageQuery' && !usageDraft.enabled ? <p className='detail mt-2.5'>
                {t('关闭自动查询后，余额限制器只能使用上次读数；手动查询仍可用。')}
              </p> : null}
              {current === 'limiters' ? <p className='detail mt-2.5'>
                {t('Token 规则按每个账号已有的窗口消耗判定，保存不会清零；停用或清空规则不会自动启用账号。')}
              </p> : null}
            </DialogSection>
          </FieldSet>
          {refreshProgress ? <p role='status' className='detail'>
            {t('刷新 Token：已处理 {completed}/{total} 个，当前账号：{name}', refreshProgress)}
          </p> : null}
          <div className='detail min-h-[18px]'>{result}</div>
        </DialogBody>
        <DialogFooter>
          <div className='mr-auto' />
          <Button variant='outline' disabled={busy} onClick={onClose}>{t('取消')}</Button>
          <Button variant={destructive ? 'destructive' : 'default'} disabled={busy || !targets.length} onClick={() => void run()}>
            {busy ? t('执行中…') : t('执行（{action}）', { action: verb })}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
