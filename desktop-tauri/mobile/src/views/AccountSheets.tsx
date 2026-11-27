import { useEffect, useState } from 'react'
import {
  clearAccountRateLimits,
  deleteAccount,
  fetchAccount,
  fetchAccountUsage,
  fetchProxyPool,
  promoteAccount,
  refreshAccount,
  setAccountEnabled,
  updateAccountPatch,
  type Account,
  type ProxyPoolItem,
  type UsageRow,
} from '../core/api'
import { formatDateTime, providerDisplayName } from '../core/format'
import { store } from '../core/store'
import { BottomSheet, CommonCard, CommonSwitch, ListGroup, ListItem } from '../ui/components'
import { PickerSheet, PickerTrigger, type PickerOption } from '../ui/pickers'
import { GlyphIcon, ProviderAvatar } from '../ui/icons'
import { accountTitle, isAccountBalancePaused, isAccountRateLimited } from './AccountsView'

export function AccountDetailSheet({
  account: initialAccount,
  onClose,
}: {
  account: Account
  onClose: () => void
}) {
  const [account, setAccount] = useState<Account>(initialAccount)
  const [nameInput, setNameInput] = useState(() => accountTitle(initialAccount))
  const [usage, setUsage] = useState<UsageRow | null>(null)
  const [usageBusy, setUsageBusy] = useState(false)
  const [proxies, setProxies] = useState<ProxyPoolItem[]>([])
  const [busyAction, setBusyAction] = useState('')
  const [confirmDelete, setConfirmDelete] = useState(false)
  const [proxyPickerOpen, setProxyPickerOpen] = useState(false)

  useEffect(() => {
    const controller = new AbortController()
    fetchAccount(initialAccount.id, controller.signal)
      .then(fresh => {
        setAccount(fresh)
        setNameInput(accountTitle(fresh))
      })
      .catch(() => {})
    fetchProxyPool(controller.signal)
      .then(pool => setProxies((pool.items || []).filter(p => p.enabled !== false)))
      .catch(() => {})
    return () => controller.abort()
  }, [initialAccount.id])

  const handleQueryUsage = async () => {
    if (usageBusy) return
    setUsageBusy(true)
    try {
      const res = await fetchAccountUsage(account.id)
      const row = res.results?.[0] || null
      setUsage(row)
      if (row?.error) {
        store.toast(row.error, 'danger')
      } else {
        store.toast('已更新余额信息', 'success')
      }
    } catch (err) {
      store.toast(err instanceof Error ? err.message : '查询余额失败', 'danger')
    } finally {
      setUsageBusy(false)
    }
  }

  const runAction = async (key: string, fn: () => Promise<void>) => {
    if (busyAction) return
    setBusyAction(key)
    try {
      await fn()
      const fresh = await fetchAccount(account.id).catch(() => null)
      if (fresh) setAccount(fresh)
      void store.refresh(true)
    } catch (err) {
      store.toast(err instanceof Error ? err.message : '操作失败', 'danger')
    } finally {
      setBusyAction('')
    }
  }

  const limited = isAccountRateLimited(account)
  const balancePaused = isAccountBalancePaused(account)
  const enabled = account.enabled !== false
  const currentProxyId =
    account.proxy && typeof account.proxy === 'object' && account.proxy.source !== 'direct'
      ? account.proxy.id || 'custom'
      : 'direct'

  return (
    <BottomSheet open title='账号详情' onClose={onClose}>
      <CommonCard>
        <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
          <ProviderAvatar provider={account.provider} size={46} />
          <div style={{ flex: 1, minWidth: 0 }}>
            <div
              style={{
                fontSize: 16,
                fontWeight: 700,
                overflow: 'hidden',
                textOverflow: 'ellipsis',
                whiteSpace: 'nowrap',
              }}
            >
              {accountTitle(account)}
            </div>
            <div style={{ fontSize: 12.5, color: 'var(--fc-text-secondary)', marginTop: 2 }}>
              {providerDisplayName(account.provider)}
              {account.email ? ` · ${account.email}` : ''}
            </div>
          </div>
          <CommonSwitch
            checked={enabled}
            disabled={Boolean(busyAction)}
            onChange={next =>
              void runAction('toggle', async () => {
                await setAccountEnabled(account.id, next)
                store.toast(next ? '已启用账号' : '已停用账号', 'success')
              })
            }
          />
        </div>

        {account.needsRelogin ? (
          <div
            className='fc-inner-block'
            style={{ color: 'var(--fc-danger)', fontSize: 12.5, fontWeight: 600 }}
          >
            需重新登录：{account.needsReloginReason || '凭证已失效'}
          </div>
        ) : null}

        {limited ? (
          <div
            className='fc-inner-block'
            style={{
              display: 'flex',
              alignItems: 'center',
              justifyContent: 'space-between',
              gap: 10,
            }}
          >
            <span style={{ fontSize: 12.5, color: 'var(--fc-warning)', fontWeight: 600 }}>
              {balancePaused ? '上游已报余额不足，已暂停派单（余额回升后自动恢复）' : '当前账号存在限流或路由冷却记录'}
            </span>
            <button
              type='button'
              className='fc-btn is-sm is-pill'
              style={{ background: 'var(--fc-warning-soft)', color: 'var(--fc-warning)' }}
              disabled={Boolean(busyAction)}
              onClick={() =>
                void runAction('clear-limit', async () => {
                  await clearAccountRateLimits(account.id)
                  store.toast('已清除限流冷却', 'success')
                })
              }
            >
              清除冷却
            </button>
          </div>
        ) : null}

        <div style={{ display: 'grid', gridTemplateColumns: 'repeat(3, 1fr)', gap: 8 }}>
          <button
            type='button'
            className='fc-btn is-tonal is-sm'
            style={{ height: 38 }}
            disabled={Boolean(busyAction)}
            onClick={() =>
              void runAction('promote', async () => {
                await promoteAccount(account.id)
                store.toast('已设为首选账号', 'success')
              })
            }
          >
            设为首选
          </button>
          <button
            type='button'
            className='fc-btn is-sm'
            style={{ height: 38 }}
            disabled={Boolean(busyAction)}
            onClick={() =>
              void runAction('refresh', async () => {
                await refreshAccount(account.id)
                store.toast('令牌刷新完成', 'success')
              })
            }
          >
            刷新令牌
          </button>
          <button
            type='button'
            className='fc-btn is-sm'
            style={{ height: 38 }}
            disabled={usageBusy}
            onClick={handleQueryUsage}
          >
            {usageBusy ? '查询中…' : '查余额'}
          </button>
        </div>
      </CommonCard>

      {usage ? (
        <CommonCard>
          <div style={{ fontSize: 13.5, fontWeight: 700 }}>额度与余额</div>
          {usage.error ? (
            <div style={{ fontSize: 12.5, color: 'var(--fc-danger)' }}>{usage.error}</div>
          ) : (
            <div className='fc-inner-block' style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
              {usage.summary ? (
                <div style={{ fontSize: 14, fontWeight: 700, color: 'var(--fc-primary)' }}>
                  {usage.summary}
                </div>
              ) : null}
              {usage.items?.map((item, idx) => (
                <div
                  key={idx}
                  style={{
                    display: 'flex',
                    justifyContent: 'space-between',
                    fontSize: 12.5,
                    color: 'var(--fc-text-secondary)',
                  }}
                >
                  <span>{item.label || '额度'}</span>
                  <span className='fc-mono'>
                    {item.remaining !== undefined && item.remaining !== null
                      ? `剩余 ${item.remaining}${item.unit || ''}`
                      : `${item.used ?? 0} / ${item.total ?? '∞'}${item.unit || ''}`}
                  </span>
                </div>
              ))}
            </div>
          )}
        </CommonCard>
      ) : null}

      <CommonCard>
        <div style={{ fontSize: 13.5, fontWeight: 700 }}>备注名与网络代理</div>
        <div style={{ display: 'flex', gap: 8 }}>
          <input
            type='text'
            className='fc-input'
            value={nameInput}
            onChange={e => setNameInput(e.target.value)}
            placeholder='输入账号备注名称'
          />
          <button
            type='button'
            className='fc-btn is-primary'
            disabled={!nameInput.trim() || Boolean(busyAction)}
            onClick={() =>
              void runAction('rename', async () => {
                await updateAccountPatch(account.id, { name: nameInput.trim() })
                store.toast('已更新账号名称', 'success')
              })
            }
          >
            保存
          </button>
        </div>

        <div style={{ display: 'flex', flexDirection: 'column', gap: 6, marginTop: 4 }}>
          <span style={{ fontSize: 12, color: 'var(--fc-text-secondary)', fontWeight: 600 }}>
            出站网络代理
          </span>
          <PickerTrigger
            valueLabel={
              currentProxyId === 'direct'
                ? '直连（不使用代理）'
                : proxies.find(p => p.id === currentProxyId)?.name
                  || (() => {
                    const hit = proxies.find(p => p.id === currentProxyId)
                    return hit ? `${hit.protocol}://${hit.host}:${hit.port}` : currentProxyId
                  })()
            }
            disabled={busyAction === 'proxy'}
            onClick={() => setProxyPickerOpen(true)}
            ariaLabel='出站网络代理'
          />
          <PickerSheet
            open={proxyPickerOpen}
            title='出站网络代理'
            value={currentProxyId}
            options={[
              { value: 'direct', label: '直连（不使用代理）', subtitle: '直接连接上游节点' },
              ...proxies.map<PickerOption>(p => ({
                value: p.id,
                label: p.name || `${p.protocol}://${p.host}:${p.port}`,
                subtitle: p.name ? `${p.protocol}://${p.host}:${p.port}` : undefined,
              })),
            ]}
            onSelect={val => {
              const proxyPayload = val === 'direct' ? { source: 'direct' } : { source: 'pool', id: val }
              void runAction('proxy', async () => {
                await updateAccountPatch(account.id, { proxy: proxyPayload })
                store.toast('已更新账号出站代理', 'success')
              })
            }}
            onClose={() => setProxyPickerOpen(false)}
          />
        </div>
      </CommonCard>

      <ListGroup>
        <ListItem
          icon='key'
          title='访问令牌到期'
          trailing={
            <span className='fc-mono' style={{ fontSize: 12 }}>
              {formatDateTime(
                account.accessTokenExpiresAt || account.tokenExpiresAt || account.expiresAt,
              )}
            </span>
          }
        />
        <ListItem
          icon='history'
          title='刷新令牌到期'
          trailing={
            <span className='fc-mono' style={{ fontSize: 12 }}>
              {account.refreshTokenLongLived
                ? '长期有效'
                : formatDateTime(account.refreshTokenExpiresAt || account.refreshExpiresAt)}
            </span>
          }
        />
      </ListGroup>

      {!confirmDelete ? (
        <button
          type='button'
          className='fc-btn is-danger'
          style={{ width: '100%' }}
          onClick={() => setConfirmDelete(true)}
        >
          <GlyphIcon name='trash' size={16} />
          <span>删除该账号</span>
        </button>
      ) : (
        <CommonCard>
          <div style={{ fontSize: 13.5, fontWeight: 700, color: 'var(--fc-danger)' }}>
            确定要永久删除「{accountTitle(account)}」吗？
          </div>
          <div style={{ display: 'flex', gap: 10 }}>
            <button
              type='button'
              className='fc-btn'
              style={{ flex: 1 }}
              onClick={() => setConfirmDelete(false)}
            >
              取消
            </button>
            <button
              type='button'
              className='fc-btn is-danger'
              style={{ flex: 1 }}
              disabled={Boolean(busyAction)}
              onClick={() =>
                void runAction('delete', async () => {
                  await deleteAccount(account.id)
                  store.toast('账号已删除', 'success')
                  onClose()
                })
              }
            >
              确认删除
            </button>
          </div>
        </CommonCard>
      )}
    </BottomSheet>
  )
}
