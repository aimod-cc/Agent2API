/** Maintenance tasks have their own egress; account/model proxy settings stay untouched. */
import * as React from 'react'
import { Button, Card, CardHeader } from '@ui'
import { t } from '../i18n'
import type { ProxyPoolItem } from './proxies-state'

type Settings = {
  enabled: boolean; mode: 'direct' | 'auto' | 'fixed'; proxyId: string; providers: string[]
  balance: boolean; checkin: boolean; rewards: boolean; maxConcurrent: number
  perProxyConcurrent: number; minIntervalMs: number; maxIntervalMs: number; maxPending: number
  queueTimeoutSeconds: number; usageCacheSeconds: number; failureCooldownSeconds: number
}
type Payload = { settings: Settings; scheduler: { active?: number; pending?: number } }
type Bridge = {
  getMaintenancePolicy?: () => Promise<Payload>
  saveMaintenancePolicy?: (settings: Settings) => Promise<Payload>
}
const bridge = () => (window as unknown as { workbuddyDesktop?: Bridge }).workbuddyDesktop
const numbers: Array<[keyof Settings, string, number, number]> = [
  ['maxConcurrent', '最大并发任务', 1, 16], ['perProxyConcurrent', '每个入口最大并发', 1, 16],
  ['minIntervalMs', '随机启动间隔下限（毫秒）', 0, 60000], ['maxIntervalMs', '随机启动间隔上限（毫秒）', 0, 60000],
  ['maxPending', '最大排队任务', 1, 4096], ['queueTimeoutSeconds', '排队超时（秒）', 1, 600],
  ['usageCacheSeconds', '成功余额缓存（秒，0 为禁用）', 0, 3600], ['failureCooldownSeconds', '自动模式超时入口冷却（秒）', 0, 600],
]
export function MaintenancePolicy({ items }: { items: ProxyPoolItem[] }) {
  const [draft, setDraft] = React.useState<Settings | null>(null)
  const [providers, setProviders] = React.useState('')
  const [busy, setBusy] = React.useState(false)
  const [message, setMessage] = React.useState('')
  const [error, setError] = React.useState('')
  React.useEffect(() => {
    let active = true
    const get = bridge()?.getMaintenancePolicy
    if (!get) { setError(t('当前主程序不支持独立任务代理，需要先升级主程序')); return }
    get().then(data => { if (active) { setDraft(data.settings); setProviders(data.settings.providers.join(', ')) } })
      .catch(() => { if (active) setError(t('读取任务代理设置失败')) })
    return () => { active = false }
  }, [])
  async function save(event: React.FormEvent) {
    event.preventDefault()
    if (!draft) return
    const put = bridge()?.saveMaintenancePolicy
    if (!put) { setError(t('当前主程序不支持保存任务代理设置')); return }
    setBusy(true); setError(''); setMessage('')
    try {
      const settings = { ...draft, providers: providers.split(',').map(v => v.trim()).filter(Boolean) }
      const result = await put(settings)
      setDraft(result.settings); setProviders(result.settings.providers.join(', '))
      setMessage(t('已保存。新任务和新加入的账户沿用此设置；账户代理和模型请求不变。'))
    } catch (e) { setError(e instanceof Error ? e.message : t('保存失败')) }
    finally { setBusy(false) }
  }
  const field = 'rounded border border-border bg-transparent px-2 py-1 text-sm'
  return <Card render={<section />} className='panel mt-4'>
    <CardHeader><h2>{t('余额、签到与福利专用代理')}</h2></CardHeader>
    <div className='p-4 space-y-3'>
      <p className='text-sm text-subtle'>{t('仅影响支持此能力的插件任务，不写入账户代理。新导入或注册的账户自动继承。关闭后保持原有行为。')}</p>
      {error ? <p role='alert' className='text-sm'>{error}</p> : null}
      {message ? <p role='status' className='text-sm'>{message}</p> : null}
      {draft ? <form onSubmit={save}><fieldset disabled={busy} className='space-y-3'>
        <label className='flex gap-2'><input type='checkbox' checked={draft.enabled}
          onChange={e => setDraft({ ...draft, enabled: e.target.checked })} />{t('启用独立任务代理与错峰调度')}</label>
        <div className='grid gap-3 md:grid-cols-2'>
          <label className='flex flex-col gap-1'>{t('出口模式')}
            <select className={field} value={draft.mode} onChange={e => setDraft({ ...draft, mode: e.target.value as Settings['mode'] })}>
              <option value='direct'>{t('直连（不使用系统或账户代理）')}</option>
              <option value='auto'>{t('自动（在已启用的代理池入口间轮换）')}</option>
              <option value='fixed'>{t('固定（指定一个代理池入口）')}</option>
            </select>
          </label>
          <label className='flex flex-col gap-1'>{t('固定入口')}
            <select className={field} disabled={draft.mode !== 'fixed'} value={draft.proxyId}
              onChange={e => setDraft({ ...draft, proxyId: e.target.value })}>
              <option value=''>{t('请选择')}</option>
              {draft.proxyId && !items.some(p => p.id === draft.proxyId)
                ? <option value={draft.proxyId} disabled>{t('原入口已删除，请重新选择')}</option> : null}
              {items.map(p => <option key={p.id} value={p.id} disabled={p.enabled === false}>{p.name || p.id}{p.enabled === false ? ` (${t('已禁用')})` : ''}</option>)}
            </select>
          </label>
          <label className='flex flex-col gap-1'>{t('提供商 ID（逗号分隔；插件必须支持任务代理）')}
            <input className={field} value={providers} onChange={e => setProviders(e.target.value)} required />
          </label>
          {numbers.map(([key, label, min, max]) => <label key={key} className='flex flex-col gap-1'>{t(label)}
            <input className={field} type='number' required min={min} max={max} step={1} value={draft[key] as number}
              onChange={e => setDraft({ ...draft, [key]: e.target.valueAsNumber })} />
          </label>)}
        </div>
        <div className='flex flex-wrap gap-4'>{([['balance', '余额查询'], ['checkin', '签到'], ['rewards', '福利查询与领取']] as const).map(([key, label]) =>
          <label key={key} className='flex gap-2'><input type='checkbox' checked={draft[key]} onChange={e => setDraft({ ...draft, [key]: e.target.checked })} />{t(label)}</label>)}</div>
        <p className='text-sm text-subtle'>{t('自动或固定模式无可用入口时会报错，不回退直连。签到和领取不自动重放；超时入口只对后续自动任务冷却。固定聚合入口内部仍可自动选节点，固定入口不等于固定公网 IP。')}</p>
        <Button type='submit' disabled={busy}>{busy ? t('保存中…') : t('保存任务设置')}</Button>
      </fieldset></form> : null}
    </div>
  </Card>
}
