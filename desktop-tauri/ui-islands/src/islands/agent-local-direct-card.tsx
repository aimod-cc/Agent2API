import * as React from 'react'
import { Alert, Badge, Button, NavItem, Spinner, StatusDot, Switch, cn } from '@ui'
import { t } from '../i18n'
import { AgentIcon } from './agent-actions'
import { errorMessage, toast } from './agent-state'
import { LocalDirectPathDialog, type LocalDirectAgentStatus } from './agent-local-direct-dialog'

export type LocalDirectStatusPayload = {
  enabled: boolean
  pollIntervalSecs: number
  syncing: boolean
  lastSyncAt: number | null
  lastInserted: number
  lastDedupedExisting: number
  lastDedupedGateway: number
  lastError: string | null
  agents: LocalDirectAgentStatus[]
}

export type LocalDirectController = {
  status: LocalDirectStatusPayload | null
  busy: boolean
  saveGlobalEnabled: (enabled: boolean) => Promise<void>
  toggleAgentEnabled: (agent: LocalDirectAgentStatus, enabled: boolean) => Promise<void>
  saveCustomPaths: (agentId: string, customPaths: string[]) => Promise<void>
  runSync: (forceRescan: boolean) => Promise<void>
}

type LocalDirectBridge = {
  workbuddyDesktop?: {
    getLocalDirectStatus?: () => Promise<LocalDirectStatusPayload>
    saveLocalDirectSettings?: (payload: Record<string, unknown>) => Promise<LocalDirectStatusPayload>
    syncLocalDirect?: (payload?: Record<string, unknown>) => Promise<{
      success?: boolean
      report?: { inserted?: number; dedupedGateway?: number }
      status?: LocalDirectStatusPayload
    }>
  }
}

function bridge() {
  return (window as unknown as LocalDirectBridge).workbuddyDesktop
}

function formatSyncTime(ts: number | null): string {
  if (!ts) return ''
  try {
    return new Date(ts).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
  } catch {
    return ''
  }
}

function formatTokens(tokens: number): string {
  if (!tokens || tokens <= 0) return ''
  if (tokens >= 1_000_000_000) return `${(tokens / 1_000_000_000).toFixed(2)}B Tokens`
  if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(1)}M Tokens`
  if (tokens >= 1_000) return `${Math.round(tokens / 1_000)}K Tokens`
  return `${tokens} Tokens`
}

function directAgentIcon(id: string): { src: string; full: boolean } {
  const key = id.toLowerCase()
  if (key.includes('antigravity')) return { src: 'assets/providers/antigravity.png', full: false }
  if (key.includes('claude')) return { src: 'assets/agents/claude-code.svg', full: false }
  if (key.includes('codex')) return { src: 'assets/agents/codex.png', full: true }
  if (key.includes('cursor')) return { src: 'assets/providers/cursor.svg', full: true }
  if (key.includes('gemini')) return { src: 'assets/agents/gemini-cli.svg', full: true }
  if (key.includes('opencode')) return { src: 'assets/agents/opencode.svg', full: true }
  if (key.includes('cline')) return { src: 'assets/agents/cline.png', full: true }
  return { src: '', full: false }
}

function DirectAgentAvatar({ agent }: { agent: LocalDirectAgentStatus }) {
  const [broken, setBroken] = React.useState(false)
  const meta = directAgentIcon(agent.id)
  const hasImg = Boolean(meta.src) && !broken
  return (
    <span className={cn('agent-avatar', hasImg && 'has-icon', hasImg && meta.full && 'is-full')} aria-hidden='true'>
      {hasImg ? <img src={meta.src} alt='' onError={() => setBroken(true)} /> : (agent.name || agent.id).slice(0, 2).toUpperCase()}
    </span>
  )
}

export function useLocalDirect(): LocalDirectController {
  const [status, setStatus] = React.useState<LocalDirectStatusPayload | null>(null)
  const [busy, setBusy] = React.useState(false)

  const refresh = React.useCallback(async () => {
    const get = bridge()?.getLocalDirectStatus
    if (!get) return
    try {
      const next = await get()
      if (next && typeof next === 'object') setStatus(next)
    } catch {
      // 不阻塞主客户端配置页面
    }
  }, [])

  React.useEffect(() => {
    void refresh()
  }, [refresh])

  async function saveGlobalEnabled(enabled: boolean) {
    const save = bridge()?.saveLocalDirectSettings
    if (!save || busy) return
    setBusy(true)
    try {
      const next = await save({ enabled })
      setStatus(next)
    } catch (err) {
      toast(errorMessage(err), 'err')
    } finally {
      setBusy(false)
    }
  }

  async function toggleAgentEnabled(agent: LocalDirectAgentStatus, enabled: boolean) {
    const save = bridge()?.saveLocalDirectSettings
    if (!save || busy) return
    setBusy(true)
    try {
      const next = await save({ agents: { [agent.id]: { enabled } } })
      setStatus(next)
    } catch (err) {
      toast(errorMessage(err), 'err')
    } finally {
      setBusy(false)
    }
  }

  async function saveCustomPaths(agentId: string, customPaths: string[]) {
    const save = bridge()?.saveLocalDirectSettings
    if (!save) throw new Error(t('后端桥不可用'))
    const next = await save({ agents: { [agentId]: { customPaths } } })
    setStatus(next)
    toast(t('目录已保存，正在扫描历史记录'))
  }

  async function runSync(forceRescan: boolean) {
    const sync = bridge()?.syncLocalDirect
    if (!sync || busy) return
    setBusy(true)
    try {
      const result = await sync({ forceRescan })
      if (result?.status) setStatus(result.status)
      else await refresh()
      const inserted = Number(result?.report?.inserted) || 0
      const deduped = Number(result?.report?.dedupedGateway) || 0
      if (inserted > 0) {
        toast(deduped > 0
          ? t('已导入 {n} 条本机直连请求（去重网关同源 {d} 条）', { n: inserted, d: deduped })
          : t('已导入 {n} 条本机直连请求', { n: inserted }))
      } else {
        toast(forceRescan ? t('重扫完成，暂无新增记录') : t('已是最新，暂无新增记录'))
      }
    } catch (err) {
      toast(errorMessage(err), 'err')
    } finally {
      setBusy(false)
    }
  }

  return { status, busy, saveGlobalEnabled, toggleAgentEnabled, saveCustomPaths, runSync }
}

/** 左侧清单底部固定入口（方案一） */
export function LocalDirectDockItem({
  status,
  active,
  onSelect,
}: {
  status: LocalDirectStatusPayload
  active: boolean
  onSelect: () => void
}) {
  const agents = status.agents || []
  const enabledCount = agents.filter(a => a.enabled).length
  const totalRequests = agents.reduce((sum, a) => sum + (Number(a.requests) || 0), 0)
  const subtitle = !status.enabled
    ? t('已关闭')
    : totalRequests > 0
      ? t('{c} 个客户端 · {n} 条', { c: enabledCount, n: totalRequests.toLocaleString() })
      : t('已开启 · {c} 个客户端', { c: enabledCount })

  return (
    <div className='agent-rail-dock'>
      <NavItem
        active={active}
        icon={<span className='agent-direct-dock-icon' aria-hidden='true'><AgentIcon name='radar' size={15} /></span>}
        trailing={<StatusDot variant={status.enabled ? 'success' : 'muted'} className={cn(active && 'agent-status-active')} />}
        className={cn('agent-client', active && 'is-active')}
        onClick={onSelect}
      >
        <span className='agent-client-main'>
          <span className='agent-client-name'>{t('本机直连采集')}</span>
          <span className='agent-client-sub tabular-nums'>{subtitle}</span>
        </span>
      </NavItem>
    </div>
  )
}

/** 右侧完整管理视图（选中左栏底部「本机直连采集」后展示） */
export function LocalDirectPanel({ controller }: { controller: LocalDirectController }) {
  const { status, busy, saveGlobalEnabled, toggleAgentEnabled, saveCustomPaths, runSync } = controller
  const [editingAgent, setEditingAgent] = React.useState<LocalDirectAgentStatus | null>(null)

  if (!status) return null

  const agents = status.agents || []
  const isWorking = busy || status.syncing
  const lastSync = formatSyncTime(status.lastSyncAt)
  const enabledCount = agents.filter(a => a.enabled).length
  const totalRequests = agents.reduce((sum, a) => sum + (Number(a.requests) || 0), 0)

  return (
    <div className='agent-direct-view'>
      <div className='agent-dhead'>
        <span className='agent-direct-hero-icon' aria-hidden='true'>
          <AgentIcon name='radar' size={20} />
        </span>
        <div className='agent-dtitle'>
          <h3>
            {t('本机直连采集')}
            <Badge variant={status.enabled ? 'success' : 'secondary'} shape='tag'>
              {status.enabled ? t('已开启 · {c} 个客户端', { c: enabledCount }) : t('已关闭')}
            </Badge>
          </h3>
          <div className='agent-meta'>
            <span>{t('读取未走本网关的客户端本地记录，自动汇总到「报表」与「请求日志」')}</span>
          </div>
        </div>
        <div className='agent-dhead-actions'>
          <Switch
            checked={status.enabled}
            disabled={isWorking}
            aria-label={t('本机直连采集')}
            onCheckedChange={checked => void saveGlobalEnabled(checked)}
          />
        </div>
      </div>

      <div className='agent-direct-syncbar'>
        <div className='flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-subtle'>
          <span className='inline-flex items-center gap-1.5 text-foreground'>
            <StatusDot variant={status.enabled ? 'success' : 'muted'} />
            {t('已汇总 {n} 条本机直连记录', { n: totalRequests.toLocaleString() })}
          </span>
          {lastSync ? (
            <span className='tabular-nums text-muted-foreground'>{t('上次同步 {time}', { time: lastSync })}</span>
          ) : null}
        </div>
        <div className='ml-auto flex flex-wrap items-center gap-2'>
          <Button
            type='button'
            variant='outline'
            size='sm'
            disabled={isWorking || !status.enabled}
            onClick={() => void runSync(false)}
          >
            {isWorking ? <Spinner className='size-3.5' /> : <AgentIcon name='refresh' size={13} />}
            {t('立即同步')}
          </Button>
          <Button
            type='button'
            variant='ghost'
            size='sm'
            disabled={isWorking || !status.enabled}
            onClick={() => void runSync(true)}
            title={t('忽略文件缓存标记，重新扫描全部历史并补导')}
          >
            {t('重扫全部历史')}
          </Button>
        </div>
      </div>

      {status.lastError ? (
        <Alert variant='destructive' className='mt-4'>{status.lastError}</Alert>
      ) : null}

      <div className='mt-5 flex flex-col gap-2.5'>
        <div className='flex flex-wrap items-center justify-between gap-2'>
          <h4 className='m-0 text-[13px] font-semibold text-foreground'>{t('支持的客户端')}</h4>
          <span className='text-[11.5px] text-muted-foreground'>
            {t('对话原文不落库，预览时按需实时读取')}
          </span>
        </div>

        <div className='flex flex-col gap-2.5'>
          {agents.map(agent => {
            const rootCount = (agent.detectedRoots || []).filter(r => r.exists).length
            const tokenText = formatTokens(agent.totalTokens)
            const active = status.enabled && agent.enabled
            return (
              <div
                key={agent.id}
                className={cn('agent-direct-row', active && 'is-on', !status.enabled && 'is-muted')}
              >
                <DirectAgentAvatar agent={agent} />
                <div className='min-w-0 flex-1'>
                  <div className='flex flex-wrap items-center gap-2'>
                    <span className='truncate text-[13.5px] font-semibold text-foreground'>{agent.name}</span>
                    {agent.requests > 0 ? (
                      <Badge variant='info' shape='tag' className='tabular-nums'>
                        {t('{n} 条', { n: agent.requests.toLocaleString() })}
                      </Badge>
                    ) : (
                      <Badge variant={agent.detected || rootCount > 0 ? 'outline' : 'secondary'} shape='tag'>
                        {agent.detected || rootCount > 0 ? t('已检测到') : t('未检测到')}
                      </Badge>
                    )}
                  </div>
                  {agent.description ? (
                    <p className='m-0 mt-1 text-xs leading-relaxed text-muted-foreground'>{agent.description}</p>
                  ) : null}
                  <div className='mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-[11.5px] text-subtle'>
                    <span>{rootCount > 0 ? t('已识别 {n} 个目录', { n: rootCount }) : t('未检测到默认目录')}</span>
                    {tokenText ? <span>· {tokenText}</span> : null}
                    {agent.customPaths.length > 0 ? (
                      <span className='text-primary-fg'>· {t('自定义路径 ({n})', { n: agent.customPaths.length })}</span>
                    ) : null}
                  </div>
                </div>
                <div className='flex shrink-0 items-center gap-2'>
                  <Button
                    type='button'
                    variant='outline'
                    size='xs'
                    disabled={isWorking}
                    onClick={() => setEditingAgent(agent)}
                  >
                    <AgentIcon name='folder' size={12} />
                    {agent.customPaths.length > 0
                      ? t('目录 ({n})', { n: agent.customPaths.length })
                      : t('数据目录')}
                  </Button>
                  <Switch
                    checked={agent.enabled}
                    disabled={isWorking || !status.enabled}
                    aria-label={t('采集 {name}', { name: agent.name })}
                    onCheckedChange={checked => void toggleAgentEnabled(agent, Boolean(checked))}
                  />
                </div>
              </div>
            )
          })}
        </div>
      </div>

      {editingAgent ? (
        <LocalDirectPathDialog
          agent={editingAgent}
          onSave={saveCustomPaths}
          onClose={() => setEditingAgent(null)}
        />
      ) : null}
    </div>
  )
}
