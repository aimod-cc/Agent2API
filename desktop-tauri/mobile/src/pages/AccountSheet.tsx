import { useEffect, useState } from 'react'
import { Sheet, SheetContent, SheetHeader, SheetBody, SheetFooter, SheetTitle } from '@ui'
import { Icon } from '../components/Icon'
import { ProviderLogo } from '../components/ProviderLogo'
import { Badge } from '../components/ui'
import { store } from '../state/store'
import {
  clearAccountRateLimits,
  deleteAccount,
  fetchAccountUsage,
  fetchUsageSnapshot,
  promoteAccount,
  refreshAccount,
  renameAccount,
  setAccountEnabled,
  type Account,
  type UsageRow,
} from '../api/gateway'
import { accountName, activeLimits, formatExpiry, formatRelative, formatStamp, isRateLimited } from '../lib/format'
import { expiryOf, hasExpiry, isLongLivedCredential } from '../lib/providerFacts'
import { usageIndex, usageText } from '../lib/usage'
import { providerLabel } from '../lib/providers'
import { t } from '../i18n'

/**
 * 账号详情抽屉。
 *
 * 手机上不做「编辑表单」：账号的凭证、代理、限流规则都是多字段表单，窄屏填
 * 起来痛苦且容易出错。这一层只给**查看 + 几个高频动作**（启停、刷新凭证、
 * 查询余额、设为首选、清除限流、删除），编辑仍回桌面端。
 */
export function AccountSheet({
  account,
  onClose,
  onChanged,
}: {
  account: Account | null
  onClose: () => void
  onChanged: () => void
}) {
  const [busy, setBusy] = useState('')
  /** 快照里这一行的余额结论；undefined = 还没查过 */
  const [usageRow, setUsageRow] = useState<UsageRow | undefined>(undefined)
  const [editingName, setEditingName] = useState(false)
  const [nameDraft, setNameDraft] = useState('')
  const open = Boolean(account)
  const accountId = account?.id || ''

  // 换账号（或关掉抽屉）时重置：编辑态不能带到下一个账号上。
  // 余额走快照而不是逐个查询 —— 快照只读记录表，不触发上游，打开抽屉不会变慢。
  useEffect(() => {
    setEditingName(false)
    setNameDraft('')
    setUsageRow(undefined)
    if (!accountId) return
    let cancelled = false
    fetchUsageSnapshot()
      .then(snapshot => {
        if (!cancelled) setUsageRow(usageIndex(snapshot).get(accountId))
      })
      .catch(() => {
        /* 余额快照读不到是常态：保持「未查询」，不打扰用户 */
      })
    return () => {
      cancelled = true
    }
  }, [accountId])

  const run = async (kind: string, task: () => Promise<unknown>, success: string) => {
    setBusy(kind)
    try {
      await task()
      store.toast(success)
      onChanged()
    } catch (error) {
      store.toast(error instanceof Error ? error.message : String(error), 'err')
    } finally {
      setBusy('')
    }
  }

  const remove = async () => {
    if (!account) return
    const name = accountName(account)
    // 删除不可恢复：手机上用两次确认（第一次进二次确认态）
    if (!window.confirm(t('确定删除账号「{name}」？这个操作不可恢复。', { name }))) return
    await run('delete', () => deleteAccount(account.id), t('已删除账号'))
    onClose()
  }

  /** 手动查这一个账号：会真的打上游，可能慢，所以单独走 busy='usage' */
  const queryUsage = async () => {
    if (!account) return
    setBusy('usage')
    try {
      const payload = await fetchAccountUsage(account.id)
      setUsageRow(payload.results?.[0])
      store.toast(t('余额已更新'))
    } catch (error) {
      store.toast(error instanceof Error ? error.message : String(error), 'err')
    } finally {
      setBusy('')
    }
  }

  const saveName = async () => {
    if (!account) return
    const next = nameDraft.trim()
    if (!next) {
      store.toast(t('备注名不能为空'), 'err')
      return
    }
    setBusy('name')
    try {
      await renameAccount(account.id, next)
      store.toast(t('备注名已更新'))
      setEditingName(false)
      onChanged()
    } catch (error) {
      store.toast(error instanceof Error ? error.message : String(error), 'err')
    } finally {
      setBusy('')
    }
  }

  /** 清除限流：不传 model = 清掉这个账号全部模型 */
  const clearLimit = async (model?: string) => {
    if (!account) return
    const question = model
      ? t('确定清除模型「{model}」的限流标记？', { model })
      : t('确定清除全部模型的限流标记？')
    if (!window.confirm(question)) return
    await run('clear', () => clearAccountRateLimits(account.id, model), t('限流已清除'))
  }

  const promote = async () => {
    if (!account) return
    await run('promote', () => promoteAccount(account.id), t('已设为首选'))
  }

  const limits = account ? activeLimits(account) : []
  const expiresAt = account ? expiryOf(account) : 0
  const usageView = usageText(usageRow)
  const usageColor =
    usageView.kind === 'bad'
      ? 'var(--m-danger)'
      : usageView.kind === 'warn'
        ? 'var(--m-warn)'
        : 'var(--m-text)'
  const isPrimary = account?.priority === 1

  return (
    <Sheet open={open} onOpenChange={next => { if (!next) onClose() }}>
      <SheetContent>
        <SheetHeader>
          <SheetTitle>{t('账号详情')}</SheetTitle>
        </SheetHeader>
        <SheetBody>
          {account ? (
            <>
              <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
                <ProviderLogo id={account.provider} size='lg' />
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div style={{ fontSize: 16, fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                    {accountName(account)}
                  </div>
                  <div style={{ fontSize: 12, color: 'var(--m-text-3)', marginTop: 3 }}>
                    {providerLabel(account.provider, account.editionLabel)}
                    {account.priority !== undefined ? ` · #${account.priority}` : ''}
                  </div>
                </div>
                <Badge tone={statusTone(account)} dot>
                  {statusText(account)}
                </Badge>
              </div>

              {/* 备注名就地编辑：手机上没有表单页，改一个字段不该跳走 */}
              <div
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: 8,
                  minHeight: 38,
                  padding: '6px 10px',
                  borderRadius: 11,
                  background: 'var(--m-surface-2)',
                  border: '1px solid var(--m-line)',
                }}
              >
                {editingName ? (
                  <>
                    <input
                      value={nameDraft}
                      autoFocus
                      onChange={event => setNameDraft(event.target.value)}
                      placeholder={t('备注名')}
                      style={{
                        flex: 1,
                        minWidth: 0,
                        border: 0,
                        outline: 'none',
                        background: 'transparent',
                        color: 'var(--m-text)',
                        // 16px 以下 iOS Safari 聚焦时会自动放大页面
                        fontSize: 16,
                      }}
                    />
                    <button
                      type='button'
                      className='m-btn m-sm'
                      disabled={busy === 'name'}
                      onClick={() => void saveName()}
                    >
                      {t('保存')}
                    </button>
                    <button
                      type='button'
                      className='m-btn m-ghost m-sm'
                      disabled={busy === 'name'}
                      onClick={() => {
                        setEditingName(false)
                        setNameDraft('')
                      }}
                    >
                      {t('取消')}
                    </button>
                  </>
                ) : (
                  <button
                    type='button'
                    onClick={() => {
                      setNameDraft(account.name || '')
                      setEditingName(true)
                    }}
                    style={{
                      flex: 1,
                      minWidth: 0,
                      display: 'flex',
                      alignItems: 'center',
                      gap: 8,
                      padding: 0,
                      border: 0,
                      background: 'none',
                      textAlign: 'left',
                      color: account.name ? 'var(--m-text)' : 'var(--m-text-3)',
                      fontSize: 13,
                    }}
                  >
                    <span style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                      {account.name || t('未设备注名')}
                    </span>
                    <Icon name='edit' style={{ width: 15, height: 15, flex: 'none', color: 'var(--m-text-3)' }} />
                  </button>
                )}
              </div>

              <div className='m-kv'>
                {hasExpiry(account.provider) ? (
                  <div className='m-kv-cell'>
                    <div className='k'>{t('有效期')}</div>
                    <div className='v'>
                      {isLongLivedCredential(account.provider) ? t('长期有效') : formatExpiry(expiresAt)}
                    </div>
                  </div>
                ) : null}
                <div className='m-kv-cell'>
                  <div className='k'>{t('余额')}</div>
                  <div className='v' title={usageView.detail} style={{ color: usageColor }}>
                    {usageView.text}
                  </div>
                </div>
                <div className='m-kv-cell'>
                  <div className='k'>{t('凭证')}</div>
                  <div className='v'>{account.hasCredentials === false ? t('缺失') : t('完整')}</div>
                </div>
                <div className='m-kv-cell'>
                  <div className='k'>{t('限流模型')}</div>
                  <div className='v'>{limits.length}</div>
                </div>
                <div className='m-kv-cell'>
                  <div className='k'>{t('最近更新')}</div>
                  <div className='v m-mono' style={{ fontSize: 12 }}>
                    {formatStamp(account.updatedAt as number)}
                  </div>
                </div>
              </div>

              {/* 首选是队列位次，不是账号属性：给它一个整行按钮，别混进删除那一排 */}
              <button
                type='button'
                className='m-btn m-ghost'
                disabled={isPrimary || busy === 'promote'}
                onClick={() => void promote()}
                style={{ flexDirection: 'column', gap: 2, height: 'auto', padding: '10px 16px' }}
              >
                <span>{isPrimary ? t('已是首选') : t('设为首选')}</span>
                <span style={{ fontSize: 11, fontWeight: 400, color: 'var(--m-text-3)' }}>
                  {t('调整到全局队列第一位')}
                </span>
              </button>

              <div className='m-rows'>
                <div className='m-row'>
                  <span className='m-k'>{t('账号 ID')}</span>
                  <span className='m-v m-mono' style={{ fontSize: 11 }}>
                    {account.id}
                  </span>
                </div>
                {account.email ? (
                  <div className='m-row'>
                    <span className='m-k'>{t('邮箱')}</span>
                    <span className='m-v'>{account.email}</span>
                  </div>
                ) : null}
                <div className='m-row'>
                  <span className='m-k'>{t('启用')}</span>
                  <span className='m-v'>{account.enabled ? t('是') : t('否')}</span>
                </div>
                {/* 读数之外还有话要说时才占一行（部分失败、钱包明细） */}
                {usageView.detail && usageView.detail !== usageView.text ? (
                  <div className='m-row' style={{ alignItems: 'flex-start' }}>
                    <span className='m-k' style={{ flex: 'none' }}>{t('余额详情')}</span>
                    <span
                      className='m-v'
                      style={{ flex: 1, minWidth: 0, textAlign: 'right', lineHeight: 1.5, wordBreak: 'break-word' }}
                    >
                      {usageView.detail}
                    </span>
                  </div>
                ) : null}
                {account.needsRelogin ? (
                  <div className='m-row'>
                    <span className='m-k'>{t('需要重新登录')}</span>
                    <span className='m-v' style={{ color: 'var(--m-danger)' }}>
                      {account.needsReloginReason || t('凭证已被上游注销')}
                    </span>
                  </div>
                ) : null}
              </div>

              {limits.length ? (
                <>
                  <div style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '0 2px' }}>
                    <span style={{ flex: 1, fontSize: 12, color: 'var(--m-text-3)' }}>{t('限流明细')}</span>
                    <button
                      type='button'
                      className='m-btn m-ghost m-sm'
                      disabled={busy === 'clear'}
                      onClick={() => void clearLimit()}
                    >
                      {t('全部清除')}
                    </button>
                  </div>
                  <div className='m-rows'>
                    {limits.map(({ model, resetAt }) => (
                      <div key={model} className='m-row'>
                        <span className='m-k m-mono' style={{ fontSize: 11 }}>
                          {model}
                        </span>
                        <span className='m-v'>
                          {t('{time} 恢复', { time: formatRelative(resetAt) })}
                        </span>
                        <button
                          type='button'
                          className='m-btn m-ghost m-sm'
                          style={{ flex: 'none', marginLeft: 8 }}
                          disabled={busy === 'clear'}
                          onClick={() => void clearLimit(model)}
                        >
                          {t('清除')}
                        </button>
                      </div>
                    ))}
                  </div>
                </>
              ) : null}
            </>
          ) : null}
        </SheetBody>
        <SheetFooter style={{ flexDirection: 'column', alignItems: 'stretch', gap: 8 }}>
          <button
            type='button'
            className='m-btn m-ghost'
            disabled={busy === 'usage' || !account}
            onClick={() => void queryUsage()}
          >
            <Icon name='wallet' />
            {busy === 'usage' ? t('查询中…') : t('查询余额')}
          </button>
          <button
            type='button'
            className='m-btn'
            disabled={busy === 'refresh' || !account}
            onClick={() => account && void run('refresh', () => refreshAccount(account.id), t('凭证已刷新'))}
          >
            <Icon name='refresh' />
            {busy === 'refresh' ? t('刷新中…') : t('刷新凭证')}
          </button>
          <div className='m-btn-row'>
            <button
              type='button'
              className='m-btn m-ghost'
              style={{ flex: 1 }}
              disabled={busy === 'toggle' || !account}
              onClick={() =>
                account &&
                void run(
                  'toggle',
                  () => setAccountEnabled(account.id, !account.enabled),
                  account.enabled ? t('已禁用') : t('已启用'),
                )
              }
            >
              <Icon name={account?.enabled ? 'pause' : 'play'} />
              {account?.enabled ? t('禁用') : t('启用')}
            </button>
            <button
              type='button'
              className='m-btn m-danger'
              style={{ flex: 1 }}
              disabled={busy === 'delete' || !account}
              onClick={() => void remove()}
            >
              <Icon name='trash' />
              {busy === 'delete' ? t('删除中…') : t('删除')}
            </button>
          </div>
        </SheetFooter>
      </SheetContent>
    </Sheet>
  )
}

function statusTone(account: Account): 'ok' | 'warn' | 'bad' | 'mute' {
  if (!account.enabled) return 'mute'
  if (account.needsRelogin) return 'bad'
  if (isRateLimited(account)) return 'warn'
  const expiresAt = expiryOf(account)
  if (expiresAt > 0 && expiresAt <= Date.now()) return 'bad'
  return 'ok'
}

function statusText(account: Account): string {
  if (!account.enabled) return t('已禁用')
  if (account.needsRelogin) return t('需重登')
  if (isRateLimited(account)) return t('限流中')
  const expiresAt = expiryOf(account)
  if (expiresAt > 0 && expiresAt <= Date.now()) return t('已过期')
  return t('正常')
}
