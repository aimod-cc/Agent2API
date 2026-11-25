import { useEffect, useState } from 'react'
import {
  changePanelPassword,
  clearLogs,
  clearRequests,
  fetchDebugMode,
  fetchForwardingBundle,
  fetchLogs,
  fetchRequestDetail,
  fetchRequests,
  saveAutoContinue,
  saveCompatSwitch,
  saveDebugMode,
  saveRetrySettings,
  saveTimeoutSettings,
  type ForwardingBundle,
  type LogEntry,
  type RequestRow,
} from '../core/api'
import { probe } from '../core/client'
import {
  formatCompact,
  formatDateTime,
  formatDuration,
  providerDisplayName,
} from '../core/format'
import { formatCost, pricingSourceLabel, syncPricingDisplay, usePricingDisplay } from '../core/pricing'
import { baseLabel, normalizeBase } from '../core/servers'
import { store, useStore } from '../core/store'
import {
  BottomSheet,
  CommonCard,
  CommonSwitch,
  ConnectedButtonGroup,
  EmptyBlock,
  ListGroup,
  ListItem,
  RecordCard,
  SectionHeader,
  TonalChip,
} from '../ui/components'
import { GlyphIcon } from '../ui/icons'

/* ─── 1. 服务器管理抽屉 ──────────────────────────────────── */

export function ServersSheet({ onClose }: { onClose: () => void }) {
  const { server, servers } = useStore()
  const [editId, setEditId] = useState<string | undefined>(undefined)
  const [name, setName] = useState('')
  const [address, setAddress] = useState('')
  const [busy, setBusy] = useState(false)

  const handleSave = async () => {
    const normalized = normalizeBase(address)
    if (!normalized) {
      store.toast('请输入合法的 IP 或域名地址', 'danger')
      return
    }
    setBusy(true)
    try {
      const check = await probe(normalized)
      store.upsertServer({ id: editId, name: name.trim(), base: normalized }, true)
      if (check.ok) {
        store.toast(
          `已连接网关${check.version ? ` v${check.version}` : ''} (${check.latencyMs ?? 1} ms)`,
          'success',
        )
        onClose()
      } else {
        store.toast('已保存服务器，但暂未探测到响应', 'info')
      }
      setEditId(undefined)
      setName('')
      setAddress('')
    } catch (err) {
      store.toast(err instanceof Error ? err.message : '保存失败', 'danger')
    } finally {
      setBusy(false)
    }
  }

  return (
    <BottomSheet open title='网关服务器' onClose={onClose}>
      {servers.length > 0 ? (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
          <SectionHeader title='已保存的服务器' />
          {servers.map(item => {
            const active = server?.id === item.id
            return (
              <RecordCard
                key={item.id}
                onClick={() => {
                  store.selectServer(item.id)
                  store.toast(`已切换到 ${item.name || baseLabel(item.base)}`, 'success')
                  onClose()
                }}
              >
                <div
                  style={{
                    display: 'flex',
                    alignItems: 'center',
                    justifyContent: 'space-between',
                    gap: 10,
                  }}
                >
                  <div style={{ minWidth: 0, flex: 1 }}>
                    <div
                      style={{
                        fontSize: 15,
                        fontWeight: 700,
                        overflow: 'hidden',
                        textOverflow: 'ellipsis',
                        whiteSpace: 'nowrap',
                      }}
                    >
                      {item.name || baseLabel(item.base)}
                    </div>
                    <div
                      className='fc-mono'
                      style={{ fontSize: 12, color: 'var(--fc-text-secondary)', marginTop: 2 }}
                    >
                      {item.base}
                    </div>
                  </div>
                  <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                    {active ? <TonalChip tone='primary'>当前使用</TonalChip> : null}
                    <button
                      type='button'
                      className='fc-btn is-sm is-pill'
                      onClick={e => {
                        e.stopPropagation()
                        setEditId(item.id)
                        setName(item.name)
                        setAddress(item.base)
                      }}
                    >
                      编辑
                    </button>
                    {servers.length > 1 ? (
                      <button
                        type='button'
                        className='fc-btn is-danger is-sm is-pill'
                        onClick={e => {
                          e.stopPropagation()
                          store.deleteServer(item.id)
                          store.toast('已移除服务器', 'info')
                        }}
                      >
                        <GlyphIcon name='trash' size={13} />
                      </button>
                    ) : null}
                  </div>
                </div>
              </RecordCard>
            )
          })}
        </div>
      ) : null}

      <CommonCard>
        <div style={{ fontSize: 14, fontWeight: 700 }}>
          {editId ? '修改服务器地址' : '添加网关服务器'}
        </div>
        <input
          type='text'
          className='fc-input'
          value={name}
          onChange={e => setName(e.target.value)}
          placeholder='备注名称（如 家里电脑、云服务器，选填）'
        />
        <input
          type='text'
          className='fc-input fc-mono'
          value={address}
          onChange={e => setAddress(e.target.value)}
          placeholder='例如 192.168.1.10:3065 或 https://gw.example.com'
        />

        <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6 }}>
          <button
            type='button'
            className='fc-btn is-sm is-pill'
            onClick={() => {
              setName('本地调试网关')
              setAddress('http://127.0.0.1:3065')
            }}
          >
            填入 127.0.0.1:3065
          </button>
        </div>

        <div style={{ display: 'flex', gap: 8 }}>
          {editId ? (
            <button
              type='button'
              className='fc-btn'
              onClick={() => {
                setEditId(undefined)
                setName('')
                setAddress('')
              }}
            >
              取消编辑
            </button>
          ) : null}
          <button
            type='button'
            className='fc-btn is-primary'
            style={{ flex: 1 }}
            disabled={!address.trim() || busy}
            onClick={handleSave}
          >
            {busy ? '正在连接探测…' : editId ? '保存并重连' : '连接并保存'}
          </button>
        </div>
      </CommonCard>
    </BottomSheet>
  )
}

/* ─── 2. 请求日志抽屉 ────────────────────────────────────── */

type ReqFilter = 'all' | 'ok' | 'error' | 'running'

const REQ_FILTERS: ReadonlyArray<{ id: ReqFilter; label: string }> = [
  { id: 'all', label: '全部' },
  { id: 'ok', label: '成功' },
  { id: 'error', label: '失败' },
  { id: 'running', label: '进行中' },
]

export function RequestsSheet({ onClose }: { onClose: () => void }) {
  const pricing = usePricingDisplay()
  const [filter, setFilter] = useState<ReqFilter>('all')
  const [entries, setEntries] = useState<RequestRow[]>([])
  const [selected, setSelected] = useState<RequestRow | null>(null)
  const [busy, setBusy] = useState(false)

  const load = async (currentFilter = filter) => {
    setBusy(true)
    try {
      const page = await fetchRequests({
        limit: 50,
        status: currentFilter === 'all' ? undefined : currentFilter,
      })
      const extra = page as { currency?: 'usd' | 'cny'; exchangeRate?: number }
      if (extra.currency || extra.exchangeRate) {
        syncPricingDisplay({ currency: extra.currency, exchangeRate: extra.exchangeRate })
      }
      setEntries(page.entries || [])
    } catch {
      /* ignore */
    } finally {
      setBusy(false)
    }
  }

  useEffect(() => {
    void load(filter)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filter])

  const handleOpenDetail = async (row: RequestRow) => {
    setSelected(row)
    try {
      const full = await fetchRequestDetail({ rowId: row.rowId, id: row.id, ts: row.ts })
      setSelected(full)
    } catch {
      /* keep summary */
    }
  }

  const handleClear = async () => {
    try {
      await clearRequests('all')
      setEntries([])
      setSelected(null)
      store.toast('已清空请求日志', 'success')
    } catch (err) {
      store.toast(err instanceof Error ? err.message : '清理失败', 'danger')
    }
  }

  return (
    <BottomSheet
      open
      title='请求日志'
      onClose={onClose}
      fixedHeader={<ConnectedButtonGroup options={REQ_FILTERS} value={filter} onChange={setFilter} />}
      actions={
        <>
          <button
            type='button'
            className='fc-btn is-sm is-pill'
            disabled={busy}
            onClick={() => void load()}
          >
            刷新
          </button>
          {entries.length > 0 ? (
            <button
              type='button'
              className='fc-btn is-danger is-sm is-pill'
              onClick={handleClear}
            >
              清空
            </button>
          ) : null}
        </>
      }
    >
      {selected ? (
        <CommonCard>
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
            <span style={{ fontSize: 14, fontWeight: 700 }}>请求详情</span>
            <button
              type='button'
              className='fc-btn is-sm is-pill'
              onClick={() => setSelected(null)}
            >
              返回列表
            </button>
          </div>
          <div className='fc-inner-block' style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
            <div className='fc-mono' style={{ fontSize: 13.5, fontWeight: 700 }}>
              {selected.model || selected.clientModel || '未知模型'}
            </div>
            <div style={{ fontSize: 12.5, color: 'var(--fc-text-secondary)' }}>
              状态码：{selected.status ?? '进行中'} · 耗时：{formatDuration(selected.durationMs)}
            </div>
            <div style={{ fontSize: 12.5, color: 'var(--fc-text-secondary)' }}>
              账号：{selected.accountName || selected.accountId || '—'} (
              {providerDisplayName(selected.provider, selected.providerLabel)})
            </div>
            <div style={{ fontSize: 12.5, color: 'var(--fc-text-secondary)' }}>
              Token：输入 {selected.promptTokens ?? 0} / 输出 {selected.completionTokens ?? 0} / 总计{' '}
              {selected.totalTokens ?? 0}
            </div>
            {(selected.cacheReadTokens || selected.cacheCreationTokens) ? (
              <div style={{ fontSize: 12.5, color: 'var(--fc-text-secondary)' }}>
                缓存：读取 {selected.cacheReadTokens ?? 0} / 写入 {selected.cacheCreationTokens ?? 0}
              </div>
            ) : null}
            <div style={{ fontSize: 12.5, color: 'var(--fc-text-secondary)' }}>
              预估费用：
              {selected.costUsd !== null && selected.costUsd !== undefined
                ? formatCost(selected.costUsd, pricing.currency, pricing.exchangeRate)
                : '未定价'}
              {selected.priceSource ? `（${pricingSourceLabel(selected.priceSource)}）` : ''}
            </div>
            <div style={{ fontSize: 12, color: 'var(--fc-text-tertiary)' }}>
              时间：{formatDateTime(selected.ts)}
            </div>
          </div>
          {selected.error ? (
            <div
              className='fc-inner-block'
              style={{
                color: 'var(--fc-danger)',
                fontSize: 12.5,
                whiteSpace: 'pre-wrap',
                wordBreak: 'break-word',
              }}
            >
              {selected.error}
            </div>
          ) : null}
        </CommonCard>
      ) : entries.length === 0 ? (
        <EmptyBlock icon='activity' title='暂无请求记录' />
      ) : (
        entries.map((row, idx) => {
          const ok = row.status && row.status >= 200 && row.status < 300
          const running = !row.status
          return (
            <RecordCard
              key={row.rowId || row.id || idx}
              onClick={() => void handleOpenDetail(row)}
            >
              <div
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'space-between',
                  gap: 10,
                }}
              >
                <span
                  className='fc-mono'
                  style={{
                    fontSize: 13.5,
                    fontWeight: 700,
                    overflow: 'hidden',
                    textOverflow: 'ellipsis',
                    whiteSpace: 'nowrap',
                  }}
                >
                  {row.model || row.clientModel || '未知模型'}
                </span>
                <TonalChip tone={running ? 'primary' : ok ? 'success' : 'danger'}>
                  {running ? '进行中' : `HTTP ${row.status}`}
                </TonalChip>
              </div>
              <div
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'space-between',
                  fontSize: 12,
                  color: 'var(--fc-text-secondary)',
                }}
              >
                <span>
                  {providerDisplayName(row.provider, row.providerLabel)}
                  {row.accountName ? ` · ${row.accountName}` : ''}
                </span>
                <span className='fc-mono'>
                  {row.costUsd !== null && row.costUsd !== undefined
                    ? `${formatCost(row.costUsd, pricing.currency, pricing.exchangeRate)} · `
                    : ''}
                  {formatDuration(row.durationMs)} · {formatCompact(row.totalTokens)} T
                </span>
              </div>
              {row.error ? (
                <div
                  style={{
                    fontSize: 12,
                    color: 'var(--fc-danger)',
                    overflow: 'hidden',
                    textOverflow: 'ellipsis',
                    whiteSpace: 'nowrap',
                  }}
                >
                  {row.error}
                </div>
              ) : null}
            </RecordCard>
          )
        })
      )}
    </BottomSheet>
  )
}

/* ─── 3. 运行日志抽屉 ────────────────────────────────────── */

type LogLevelFilter = 'all' | 'info' | 'warn' | 'error'

const LOG_LEVELS: ReadonlyArray<{ id: LogLevelFilter; label: string }> = [
  { id: 'all', label: '全部' },
  { id: 'info', label: 'INFO' },
  { id: 'warn', label: 'WARN' },
  { id: 'error', label: 'ERROR' },
]

export function LogsSheet({ onClose }: { onClose: () => void }) {
  const [level, setLevel] = useState<LogLevelFilter>('all')
  const [logs, setLogs] = useState<LogEntry[]>([])
  const [debugMode, setDebugMode] = useState(false)

  const load = async (lvl = level) => {
    try {
      const page = await fetchLogs({ level: lvl, limit: 60 })
      setLogs(page.entries || [])
    } catch {
      /* ignore */
    }
  }

  useEffect(() => {
    void load(level)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [level])

  useEffect(() => {
    fetchDebugMode()
      .then(res => setDebugMode(Boolean(res.debugMode)))
      .catch(() => {})
  }, [])

  return (
    <BottomSheet
      open
      title='运行日志'
      onClose={onClose}
      fixedHeader={
        <>
          <ListGroup>
            <ListItem
              icon='terminal'
              title='调试日志模式 (Debug)'
              subtitle='输出详细的协议转换与上游交互日志'
              trailing={
                <CommonSwitch
                  checked={debugMode}
                  onChange={async next => {
                    setDebugMode(next)
                    try {
                      await saveDebugMode(next)
                      store.toast(next ? '已开启调试日志' : '已关闭调试日志', 'success')
                    } catch {
                      setDebugMode(!next)
                    }
                  }}
                />
              }
            />
          </ListGroup>
          <ConnectedButtonGroup options={LOG_LEVELS} value={level} onChange={setLevel} />
        </>
      }
      actions={
        <>
          <button
            type='button'
            className='fc-btn is-sm is-pill'
            onClick={() => void load()}
          >
            刷新
          </button>
          {logs.length > 0 ? (
            <button
              type='button'
              className='fc-btn is-danger is-sm is-pill'
              onClick={async () => {
                await clearLogs().catch(() => {})
                setLogs([])
                store.toast('已清空运行日志', 'success')
              }}
            >
              清空
            </button>
          ) : null}
        </>
      }
    >
      {logs.length === 0 ? (
        <EmptyBlock icon='terminal' title='暂无运行日志' />
      ) : (
        logs.map((entry, idx) => {
          const lvl = (entry.level || 'info').toLowerCase()
          const tone =
            lvl === 'error' ? 'danger' : lvl === 'warn' || lvl === 'warning' ? 'warning' : 'neutral'
          return (
            <RecordCard key={`${entry.ts}-${idx}`}>
              <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
                <TonalChip tone={tone}>{(entry.level || 'INFO').toUpperCase()}</TonalChip>
                <span className='fc-mono' style={{ fontSize: 11.5, color: 'var(--fc-text-tertiary)' }}>
                  {formatDateTime(entry.ts)}
                </span>
              </div>
              <div
                style={{
                  fontSize: 13,
                  color: 'var(--fc-text)',
                  wordBreak: 'break-word',
                  whiteSpace: 'pre-wrap',
                }}
              >
                {entry.message}
              </div>
            </RecordCard>
          )
        })
      )}
    </BottomSheet>
  )
}

/* ─── 4. 转发与超时参数抽屉 ──────────────────────────────── */

export function ForwardingConfigSheet({ onClose }: { onClose: () => void }) {
  const [bundle, setBundle] = useState<ForwardingBundle | null>(null)
  const [retryCount, setRetryCount] = useState('2')
  const [crossRetryCount, setCrossRetryCount] = useState('1')
  const [connectTimeout, setConnectTimeout] = useState('15')
  const [streamIdleTimeout, setStreamIdleTimeout] = useState('120')
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    fetchForwardingBundle()
      .then(b => {
        setBundle(b)
        if (b.retry.retryCount !== undefined) setRetryCount(String(b.retry.retryCount))
        if (b.retry.retryCrossProviderCount !== undefined) {
          setCrossRetryCount(String(b.retry.retryCrossProviderCount))
        }
        if (b.timeouts.connectTimeoutSeconds !== undefined) {
          setConnectTimeout(String(b.timeouts.connectTimeoutSeconds))
        }
        if (b.timeouts.streamIdleTimeoutSeconds !== undefined) {
          setStreamIdleTimeout(String(b.timeouts.streamIdleTimeoutSeconds))
        }
      })
      .catch(() => {})
  }, [])

  const handleSaveNumbers = async () => {
    if (busy) return
    setBusy(true)
    try {
      await Promise.all([
        saveRetrySettings({
          retryCount: Math.max(0, Number(retryCount) || 0),
          retryCrossProviderCount: Math.max(0, Number(crossRetryCount) || 0),
        }),
        saveTimeoutSettings({
          connectTimeoutSeconds: Math.max(1, Number(connectTimeout) || 15),
          streamIdleTimeoutSeconds: Math.max(5, Number(streamIdleTimeout) || 120),
        }),
      ])
      store.toast('已保存重试与超时设置', 'success')
    } catch (err) {
      store.toast(err instanceof Error ? err.message : '保存失败', 'danger')
    } finally {
      setBusy(false)
    }
  }

  return (
    <BottomSheet
      open
      title='转发与超时参数'
      onClose={onClose}
      footer={
        <button
          type='button'
          className='fc-btn is-primary'
          style={{ width: '100%' }}
          disabled={busy}
          onClick={handleSaveNumbers}
        >
          {busy ? '保存中…' : '保存重试与超时数值'}
        </button>
      }
    >
      <SectionHeader title='重试与超时（秒）' />
      <CommonCard>
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 10 }}>
          <div>
            <span style={{ fontSize: 12, color: 'var(--fc-text-secondary)' }}>同提供商重试次数</span>
            <input
              type='number'
              className='fc-input'
              value={retryCount}
              onChange={e => setRetryCount(e.target.value)}
            />
          </div>
          <div>
            <span style={{ fontSize: 12, color: 'var(--fc-text-secondary)' }}>跨提供商重试次数</span>
            <input
              type='number'
              className='fc-input'
              value={crossRetryCount}
              onChange={e => setCrossRetryCount(e.target.value)}
            />
          </div>
          <div>
            <span style={{ fontSize: 12, color: 'var(--fc-text-secondary)' }}>连接超时 (秒)</span>
            <input
              type='number'
              className='fc-input'
              value={connectTimeout}
              onChange={e => setConnectTimeout(e.target.value)}
            />
          </div>
          <div>
            <span style={{ fontSize: 12, color: 'var(--fc-text-secondary)' }}>流式空闲超时 (秒)</span>
            <input
              type='number'
              className='fc-input'
              value={streamIdleTimeout}
              onChange={e => setStreamIdleTimeout(e.target.value)}
            />
          </div>
        </div>
      </CommonCard>

      <SectionHeader title='协议兼容开关' />
      <ListGroup>
        <ListItem
          icon='bolt'
          title='截断自动续写'
          subtitle='上游输出因长度中断时自动发起续写拼接'
          trailing={
            <CommonSwitch
              checked={Boolean(bundle?.autoContinue?.autoContinue)}
              onChange={async next => {
                setBundle(prev =>
                  prev ? { ...prev, autoContinue: { ...prev.autoContinue, autoContinue: next } } : prev,
                )
                await saveAutoContinue({ autoContinue: next }).catch(() => {})
                store.toast('已更新自动续写', 'success')
              }}
            />
          }
        />
        <ListItem
          icon='sliders'
          title='关键词替换'
          subtitle='按词表替换或删除出站请求中的指定文本与表头'
          trailing={
            <CommonSwitch
              checked={Boolean(bundle?.sanitize)}
              onChange={async next => {
                setBundle(prev => (prev ? { ...prev, sanitize: next } : prev))
                await saveCompatSwitch('sanitize', next).catch(() => {})
                store.toast('已更新关键词替换', 'success')
              }}
            />
          }
        />
        <ListItem
          icon='shield'
          title='隐私脱敏'
          subtitle='转发前遮罩密钥、个人隐私与自定义保密词，并在响应时还原'
          trailing={
            <CommonSwitch
              checked={Boolean(bundle?.privacyEnabled)}
              onChange={async next => {
                setBundle(prev => (prev ? { ...prev, privacyEnabled: next } : prev))
                await saveCompatSwitch('privacy', next).catch(() => {})
                store.toast('已更新隐私脱敏', 'success')
              }}
            />
          }
        />
        <ListItem
          icon='sliders'
          title='Developer 角色自动降级'
          subtitle='将 developer 角色转为 system 兼容更多上游'
          trailing={
            <CommonSwitch
              checked={Boolean(bundle?.developerRoleCompat)}
              onChange={async next => {
                setBundle(prev => (prev ? { ...prev, developerRoleCompat: next } : prev))
                await saveCompatSwitch('developer-role-compat', next).catch(() => {})
                store.toast('已更新角色兼容', 'success')
              }}
            />
          }
        />
        <ListItem
          icon='globe'
          title='允许浏览器跨域 (CORS)'
          subtitle='允许外部网页前端直连网关 /v1 接口'
          trailing={
            <CommonSwitch
              checked={Boolean(bundle?.corsEnabled)}
              onChange={async next => {
                setBundle(prev => (prev ? { ...prev, corsEnabled: next } : prev))
                await saveCompatSwitch('cors', next).catch(() => {})
                store.toast('已更新 CORS 设置', 'success')
              }}
            />
          }
        />
      </ListGroup>
    </BottomSheet>
  )
}

/* ─── 5. 修改管理员密码抽屉 ──────────────────────────────── */

export function PasswordSheet({ onClose }: { onClose: () => void }) {
  const [oldPassword, setOldPassword] = useState('')
  const [newPassword, setNewPassword] = useState('')
  const [confirmPassword, setConfirmPassword] = useState('')
  const [busy, setBusy] = useState(false)

  const handleSubmit = async () => {
    if (!oldPassword || !newPassword) return
    if (newPassword.length < 8) {
      store.toast('新密码长度至少 8 位', 'danger')
      return
    }
    if (newPassword !== confirmPassword) {
      store.toast('两次输入的新密码不一致', 'danger')
      return
    }
    setBusy(true)
    try {
      await changePanelPassword(oldPassword, newPassword)
      store.toast('管理员密码已更新', 'success')
      onClose()
    } catch (err) {
      store.toast(err instanceof Error ? err.message : '修改密码失败', 'danger')
    } finally {
      setBusy(false)
    }
  }

  return (
    <BottomSheet
      open
      title='修改管理员密码'
      onClose={onClose}
      footer={
        <button
          type='button'
          className='fc-btn is-primary'
          style={{ width: '100%' }}
          disabled={!oldPassword || !newPassword || !confirmPassword || busy}
          onClick={handleSubmit}
        >
          {busy ? '提交中…' : '确认修改密码'}
        </button>
      }
    >
      <CommonCard>
        <input
          type='password'
          className='fc-input'
          value={oldPassword}
          onChange={e => setOldPassword(e.target.value)}
          placeholder='当前管理员密码'
        />
        <input
          type='password'
          className='fc-input'
          value={newPassword}
          onChange={e => setNewPassword(e.target.value)}
          placeholder='新密码（至少 8 位）'
        />
        <input
          type='password'
          className='fc-input'
          value={confirmPassword}
          onChange={e => setConfirmPassword(e.target.value)}
          placeholder='再次确认新密码'
        />
      </CommonCard>
    </BottomSheet>
  )
}
