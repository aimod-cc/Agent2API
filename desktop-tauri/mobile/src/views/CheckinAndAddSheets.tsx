import { useEffect, useState } from 'react'
import {
  createAccountDirect,
  fetchCheckinCenter,
  fetchLoginMethods,
  runCheckin,
  setAutoCheckin,
  startDynamicLogin,
  submitDynamicCallback,
  waitDynamicLogin,
  type CheckinCenterPayload,
  type DynamicLoginField,
  type DynamicLoginMethod,
  type DynamicLoginStart,
} from '../core/api'
import { copyText, formatRelativeTime, providerDisplayName } from '../core/format'
import { openExternalUrl } from '../core/servers'
import { store, useStore } from '../core/store'
import {
  BottomSheet,
  CommonCard,
  CommonSwitch,
  EmptyBlock,
  ListGroup,
  ListItem,
  RecordCard,
  SectionHeader,
  TonalChip,
} from '../ui/components'
import { GlyphIcon, ProviderAvatar } from '../ui/icons'
import { PickerSheet, PickerTrigger, type PickerOption } from '../ui/pickers'

/* ─── 1. 签到中心抽屉 ────────────────────────────────────── */

export function CheckinSheet({ onClose }: { onClose: () => void }) {
  const [data, setData] = useState<CheckinCenterPayload | null>(null)
  const [busyAll, setBusyAll] = useState(false)
  const [busyId, setBusyId] = useState<string | null>(null)

  const reload = async () => {
    try {
      const res = await fetchCheckinCenter()
      setData(res)
    } catch (err) {
      store.toast(err instanceof Error ? err.message : '加载签到信息失败', 'danger')
    }
  }

  useEffect(() => {
    void reload()
  }, [])

  const todayDone = data?.daily?.todayDone ?? 0
  const todayEligible = data?.daily?.todayEligible ?? 0
  const autoEnabled = Boolean(data?.auto?.enabled)

  const groups = data?.daily?.providers || []
  const flatAccounts =
    groups.flatMap(g =>
      (g.accounts || []).map(a => ({ ...a, provider: a.provider || g.provider, providerLabel: g.label })),
    ) ||
    data?.daily?.accounts ||
    []

  const handleRunAll = async () => {
    if (busyAll) return
    setBusyAll(true)
    try {
      const res = await runCheckin()
      const ok = (res.succeeded ?? 0) + (res.alreadyDone ?? 0)
      store.toast(`一键签到完成：成功 ${ok} 个`, 'success')
      await reload()
    } catch (err) {
      store.toast(err instanceof Error ? err.message : '签到失败', 'danger')
    } finally {
      setBusyAll(false)
    }
  }

  const handleRunOne = async (id: string) => {
    if (busyId) return
    setBusyId(id)
    try {
      await runCheckin(id)
      store.toast('已执行账号签到', 'success')
      await reload()
    } catch (err) {
      store.toast(err instanceof Error ? err.message : '签到失败', 'danger')
    } finally {
      setBusyId(null)
    }
  }

  return (
    <BottomSheet
      open
      title='每日签到中心'
      onClose={onClose}
      actions={
        <button
          type='button'
          className='fc-btn is-primary is-sm is-pill'
          disabled={busyAll || todayEligible === 0}
          onClick={handleRunAll}
        >
          {busyAll ? '签到中…' : '全部签到'}
        </button>
      }
    >
      <ListGroup>
        <ListItem
          icon='calendar'
          title={`今日进度 ${todayDone} / ${todayEligible}`}
          subtitle='每日定时或手动领取上游免费积分'
          trailing={
            <TonalChip tone={todayDone >= todayEligible && todayEligible > 0 ? 'success' : 'primary'}>
              {todayEligible === 0 ? '无支持账号' : todayDone >= todayEligible ? '已完成' : '待签到'}
            </TonalChip>
          }
        />
        <ListItem
          icon='bolt'
          title='自动每日签到'
          subtitle='网关在后台按计划自动执行签到'
          trailing={
            <CommonSwitch
              checked={autoEnabled}
              onChange={async next => {
                setData(prev => (prev ? { ...prev, auto: { enabled: next } } : prev))
                try {
                  await setAutoCheckin(next)
                  store.toast(next ? '已开启自动签到' : '已关闭自动签到', 'success')
                } catch (err) {
                  store.toast(err instanceof Error ? err.message : '设置失败', 'danger')
                  await reload()
                }
              }}
            />
          }
        />
      </ListGroup>

      {flatAccounts.length === 0 ? (
        <EmptyBlock
          icon='calendar'
          title='暂无支持签到的账号'
          description='添加支持每日签到的提供商账号（如 ZCode、智谱、Qoder 等）后会在此显示'
        />
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
          <SectionHeader title='账号签到状态' />
          {flatAccounts.map(acc => (
            <RecordCard key={acc.id}>
              <div
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'space-between',
                  gap: 10,
                }}
              >
                <div style={{ display: 'flex', alignItems: 'center', gap: 10, minWidth: 0 }}>
                  <ProviderAvatar provider={acc.provider} label={acc.providerLabel} size={36} />
                  <div style={{ minWidth: 0 }}>
                    <div
                      style={{
                        fontSize: 14,
                        fontWeight: 700,
                        overflow: 'hidden',
                        textOverflow: 'ellipsis',
                        whiteSpace: 'nowrap',
                      }}
                    >
                      {acc.name || acc.id}
                    </div>
                    <div style={{ fontSize: 12, color: 'var(--fc-text-secondary)' }}>
                      {providerDisplayName(acc.provider, acc.providerLabel)}
                      {acc.lastAt ? ` · ${formatRelativeTime(acc.lastAt)}` : ''}
                    </div>
                  </div>
                </div>

                {acc.todayDone ? (
                  <TonalChip tone='success'>今日已签</TonalChip>
                ) : (
                  <button
                    type='button'
                    className='fc-btn is-tonal is-sm is-pill'
                    disabled={busyId === acc.id}
                    onClick={() => void handleRunOne(acc.id)}
                  >
                    {busyId === acc.id ? '执行中…' : '签到'}
                  </button>
                )}
              </div>
              {acc.lastMessage ? (
                <div style={{ fontSize: 12, color: 'var(--fc-text-secondary)' }}>
                  {acc.lastMessage}
                </div>
              ) : null}
            </RecordCard>
          ))}
        </div>
      )}
    </BottomSheet>
  )
}

/* ─── 2. 添加账号抽屉（支持动态 OAuth 与凭证录入） ──────── */

export function AddAccountSheet({
  defaultProvider,
  onClose,
}: {
  defaultProvider?: string
  onClose: () => void
}) {
  const { session } = useStore()
  const providers = (session?.accounts?.providers || []).filter(p => p.id && p.loginAvailable !== false)

  const [selectedProvider, setSelectedProvider] = useState(
    () => defaultProvider || providers[0]?.id || 'codebuddy',
  )
  const [methods, setMethods] = useState<DynamicLoginMethod[]>([])
  const [methodIdx, setMethodIdx] = useState(0)
  const [loadingMethods, setLoadingMethods] = useState(false)
  const [inputs, setInputs] = useState<Record<string, string>>({})
  const [accountName, setAccountName] = useState('')
  const [loginFlow, setLoginFlow] = useState<DynamicLoginStart | null>(null)
  const [callbackText, setCallbackText] = useState('')
  const [busy, setBusy] = useState(false)
  const [statusMsg, setStatusMsg] = useState('')
  const [providerPickerOpen, setProviderPickerOpen] = useState(false)

  useEffect(() => {
    if (!selectedProvider) return
    const controller = new AbortController()
    setLoadingMethods(true)
    setLoginFlow(null)
    setStatusMsg('')
    fetchLoginMethods(selectedProvider, controller.signal)
      .then(res => {
        const list = res.methods || []
        setMethods(list)
        setMethodIdx(0)
        setInputs({})
      })
      .catch(() => {
        setMethods([])
      })
      .finally(() => setLoadingMethods(false))
    return () => controller.abort()
  }, [selectedProvider])

  useEffect(() => {
    if (!loginFlow?.state) return
    const stateToken = loginFlow.state
    const controller = new AbortController()
    const timer = setInterval(async () => {
      try {
        const res = await waitDynamicLogin(stateToken, controller.signal)
        if (res.done || res.saved) {
          clearInterval(timer)
          store.toast('账号授权成功', 'success')
          void store.refresh(true)
          onClose()
        } else if (res.error) {
          clearInterval(timer)
          setStatusMsg(res.error)
        }
      } catch {
        /* ignore transient poll error */
      }
    }, loginFlow.pollIntervalMs || 2500)
    return () => {
      clearInterval(timer)
      controller.abort()
    }
  }, [loginFlow, onClose])

  const currentMethod = methods[methodIdx]
  const fields: DynamicLoginField[] = currentMethod?.fields || currentMethod?.inputs || []
  const isOAuthMethod =
    fields.length === 0 ||
    currentMethod?.type === 'oauth' ||
    currentMethod?.kind === 'oauth' ||
    currentMethod?.type === 'browser'

  const handleStartOAuth = async () => {
    if (busy) return
    setBusy(true)
    setStatusMsg('')
    try {
      const flow = await startDynamicLogin({
        provider: selectedProvider,
        methodIndex: currentMethod?.index ?? methodIdx,
        inputs,
      })
      setLoginFlow(flow)
      const authUrl = flow.authUrl || flow.url
      if (authUrl) {
        void openExternalUrl(authUrl)
      }
    } catch (err) {
      store.toast(err instanceof Error ? err.message : '启动登录失败', 'danger')
    } finally {
      setBusy(false)
    }
  }

  const handleSubmitCallback = async () => {
    if (!loginFlow?.state || !callbackText.trim() || busy) return
    setBusy(true)
    try {
      const res = await submitDynamicCallback(loginFlow.state, callbackText.trim())
      if (res.accountId || res.accepted) {
        store.toast('回调已提交，正在完成验证…', 'success')
        setCallbackText('')
      }
    } catch (err) {
      store.toast(err instanceof Error ? err.message : '回调提交失败', 'danger')
    } finally {
      setBusy(false)
    }
  }

  const handleDirectSubmit = async () => {
    if (busy) return
    setBusy(true)
    try {
      await createAccountDirect({
        provider: selectedProvider,
        methodIndex: currentMethod?.index ?? methodIdx,
        name: accountName.trim() || undefined,
        inputs,
      })
      store.toast('账号添加成功', 'success')
      void store.refresh(true)
      onClose()
    } catch (err) {
      store.toast(err instanceof Error ? err.message : '添加账号失败', 'danger')
    } finally {
      setBusy(false)
    }
  }

  return (
    <BottomSheet open title='添加账号' onClose={onClose}>
      <CommonCard>
        <div style={{ fontSize: 13, fontWeight: 700, color: 'var(--fc-text-secondary)' }}>
          选择提供商
        </div>
        {(() => {
          const providerList: PickerOption[] =
            providers.length > 0
              ? providers.flatMap(p =>
                  p.id
                    ? [
                        {
                          value: p.id,
                          label: providerDisplayName(p.id, p.label),
                          subtitle: p.id,
                        },
                      ]
                    : [],
                )
              : [
                  { value: 'codebuddy', label: 'CodeBuddy' },
                  { value: 'qoder', label: 'Qoder' },
                  { value: 'zcode', label: 'ZCode' },
                  { value: 'zhipu', label: '智谱清言' },
                  { value: 'deepseek', label: 'DeepSeek' },
                  { value: 'kimi', label: 'Kimi' },
                ]
          const currentLabel =
            providerList.find(o => o.value === selectedProvider)?.label
            || providerDisplayName(selectedProvider)
          return (
            <>
              <PickerTrigger
                valueLabel={currentLabel}
                disabled={busy}
                onClick={() => setProviderPickerOpen(true)}
                ariaLabel='选择提供商'
              />
              <PickerSheet
                open={providerPickerOpen}
                title='选择提供商'
                searchable
                searchPlaceholder='搜索提供商名称或标识'
                options={providerList}
                value={selectedProvider}
                onSelect={setSelectedProvider}
                onClose={() => setProviderPickerOpen(false)}
              />
            </>
          )
        })()}

        {methods.length > 1 ? (
          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6, marginTop: 4 }}>
            {methods.map((m, idx) => (
              <button
                key={idx}
                type='button'
                className={`fc-btn is-sm is-pill ${idx === methodIdx ? 'is-primary' : ''}`}
                onClick={() => {
                  setMethodIdx(idx)
                  setLoginFlow(null)
                }}
              >
                {m.label || m.title || `方式 ${idx + 1}`}
              </button>
            ))}
          </div>
        ) : null}
      </CommonCard>

      <CommonCard>
        {loadingMethods ? (
          <div style={{ fontSize: 13, color: 'var(--fc-text-secondary)', padding: '12px 0' }}>
            正在读取该提供商的登录方式…
          </div>
        ) : (
          <>
            {currentMethod?.description ? (
              <div style={{ fontSize: 12.5, color: 'var(--fc-text-secondary)', lineHeight: 1.5 }}>
                {currentMethod.description}
              </div>
            ) : null}

            <input
              type='text'
              className='fc-input'
              value={accountName}
              onChange={e => setAccountName(e.target.value)}
              placeholder='备注名称（选填）'
            />

            {fields.map((field, idx) => {
              const key = field.key || field.name || `field_${idx}`
              return (
                <div key={key} style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
                  <span style={{ fontSize: 12, fontWeight: 600, color: 'var(--fc-text-secondary)' }}>
                    {field.label || key}
                  </span>
                  <input
                    type={field.type === 'password' ? 'password' : 'text'}
                    className='fc-input'
                    value={inputs[key] || ''}
                    onChange={e => setInputs(prev => ({ ...prev, [key]: e.target.value }))}
                    placeholder={field.placeholder || `请输入 ${field.label || key}`}
                  />
                </div>
              )
            })}

            {isOAuthMethod ? (
              <button
                type='button'
                className='fc-btn is-primary'
                disabled={busy}
                onClick={handleStartOAuth}
              >
                <GlyphIcon name='link' size={16} />
                <span>{busy ? '正在生成授权链接…' : '获取授权链接并登录'}</span>
              </button>
            ) : (
              <button
                type='button'
                className='fc-btn is-primary'
                disabled={busy}
                onClick={handleDirectSubmit}
              >
                <GlyphIcon name='check' size={16} />
                <span>{busy ? '提交中…' : '保存并添加账号'}</span>
              </button>
            )}
          </>
        )}
      </CommonCard>

      {loginFlow ? (
        <CommonCard>
          <div style={{ fontSize: 14, fontWeight: 700 }}>完成浏览器授权</div>
          {loginFlow.instructions ? (
            <div style={{ fontSize: 12.5, color: 'var(--fc-text-secondary)', lineHeight: 1.5 }}>
              {loginFlow.instructions}
            </div>
          ) : null}

          {loginFlow.authUrl || loginFlow.url ? (
            <div
              className='fc-inner-block'
              style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8 }}
            >
              <div
                className='fc-mono'
                style={{
                  fontSize: 12,
                  overflow: 'hidden',
                  textOverflow: 'ellipsis',
                  whiteSpace: 'nowrap',
                }}
              >
                {loginFlow.authUrl || loginFlow.url}
              </div>
              <button
                type='button'
                className='fc-btn is-tonal is-sm is-pill'
                onClick={async () => {
                  const link = loginFlow.authUrl || loginFlow.url || ''
                  await copyText(link)
                  store.toast('已复制授权链接', 'success')
                }}
              >
                复制链接
              </button>
            </div>
          ) : null}

          <input
            type='text'
            className='fc-input'
            value={callbackText}
            onChange={e => setCallbackText(e.target.value)}
            placeholder='粘贴授权完成后的回调链接或验证码…'
          />
          <button
            type='button'
            className='fc-btn is-primary'
            disabled={!callbackText.trim() || busy}
            onClick={handleSubmitCallback}
          >
            提交回调完成登录
          </button>

          {statusMsg ? (
            <div style={{ fontSize: 12.5, color: 'var(--fc-danger)' }}>{statusMsg}</div>
          ) : null}
        </CommonCard>
      ) : null}
    </BottomSheet>
  )
}
