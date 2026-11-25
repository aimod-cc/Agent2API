import * as React from 'react'
import { Badge, Button, Checkbox, Spinner, Switch } from '@ui'
import { t } from '../i18n'
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

export function LocalDirectSection() {
  const [status, setStatus] = React.useState<LocalDirectStatusPayload | null>(null)
  const [loading, setLoading] = React.useState(true)
  const [busy, setBusy] = React.useState(false)
  const [editingAgent, setEditingAgent] = React.useState<LocalDirectAgentStatus | null>(null)

  const refresh = React.useCallback(async () => {
    const get = bridge()?.getLocalDirectStatus
    if (!get) {
      setLoading(false)
      return
    }
    try {
      const next = await get()
      if (next && typeof next === 'object') setStatus(next)
    } catch {
      // 不阻塞主客户端配置页面
    } finally {
      setLoading(false)
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
      const next = await save({
        agents: { [agent.id]: { enabled } },
      })
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
    const next = await save({
      agents: { [agentId]: { customPaths } },
    })
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

  if (loading && !status) return null
  if (!status) return null

  const agents = status.agents || []
  const isWorking = busy || status.syncing
  const lastSync = formatSyncTime(status.lastSyncAt)

  return (
    <div className='flex shrink-0 flex-wrap items-center justify-between gap-3 border-b border-border bg-surface-2/60 px-4 py-2.5'>
      <div className='flex flex-wrap items-center gap-3'>
        <label className='inline-flex cursor-pointer items-center gap-2 text-[12.5px] font-medium text-foreground'>
          <Switch
            checked={status.enabled}
            disabled={isWorking}
            aria-label={t('本机直连采集')}
            onCheckedChange={checked => void saveGlobalEnabled(checked)}
          />
          <span>{t('本机直连采集')}</span>
        </label>

        <div className='flex flex-wrap items-center gap-2'>
          {agents.map(agent => {
            const rootCount = (agent.detectedRoots || []).filter(r => r.exists).length
            return (
              <div
                key={agent.id}
                className='inline-flex items-center gap-2 rounded-md border border-border bg-surface px-2.5 py-1 text-xs'
              >
                <label className='inline-flex cursor-pointer items-center gap-1.5 font-medium text-foreground'>
                  <Checkbox
                    checked={agent.enabled}
                    disabled={isWorking || !status.enabled}
                    onCheckedChange={checked => void toggleAgentEnabled(agent, Boolean(checked))}
                  />
                  <img
                    src='assets/providers/antigravity.png'
                    alt=''
                    className='size-4 rounded object-contain'
                    onError={event => { event.currentTarget.style.display = 'none' }}
                  />
                  <span>{agent.name}</span>
                </label>
                {agent.requests > 0 ? (
                  <Badge variant='info' shape='tag'>{t('{n} 条', { n: agent.requests })}</Badge>
                ) : (
                  <Badge variant={agent.detected || rootCount > 0 ? 'outline' : 'secondary'} shape='tag'>
                    {agent.detected || rootCount > 0 ? t('已检测到') : t('未检测到')}
                  </Badge>
                )}
                <Button
                  type='button'
                  variant='ghost'
                  size='xs'
                  className='h-5 px-1.5 text-[11px] text-subtle'
                  disabled={isWorking}
                  onClick={() => setEditingAgent(agent)}
                >
                  {agent.customPaths.length > 0
                    ? t('自定义路径 ({n})', { n: agent.customPaths.length })
                    : t('自定义路径')}
                </Button>
              </div>
            )
          })}
        </div>
      </div>

      <div className='ml-auto flex flex-wrap items-center gap-2'>
        {lastSync ? (
          <span className='text-[11px] text-muted-foreground'>
            {t('上次同步 {time}', { time: lastSync })}
          </span>
        ) : null}
        <Button
          type='button'
          variant='outline'
          size='xs'
          disabled={isWorking || !status.enabled}
          onClick={() => void runSync(false)}
        >
          {isWorking ? <Spinner className='size-3' /> : null}
          {t('立即同步')}
        </Button>
        <Button
          type='button'
          variant='ghost'
          size='xs'
          disabled={isWorking || !status.enabled}
          onClick={() => void runSync(true)}
          title={t('忽略文件缓存标记，重新扫描全部历史并补导')}
        >
          {t('重扫全部历史')}
        </Button>
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
