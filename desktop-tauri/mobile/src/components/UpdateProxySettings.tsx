import { useEffect, useState } from 'react'
import { Alert, Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@ui'
import { apiCall } from '../api/client'
import { fetchProxyPool, type PoolItem } from '../api/gateway'
import { serverIdentity } from '../lib/servers'
import { store } from '../state/store'
import { t } from '../i18n'

type ProxyChoice = {
  label?: string
  source?: string
  error?: string
  config?: { source?: string; proxyId?: string; listenerUid?: string } | null
} | null

type ProxyResponse = { proxy: ProxyChoice; saved?: boolean }
const OTHER = '__current__'

export function UpdateProxySettings({ plugin = false }: { plugin?: boolean }) {
  const [choice, setChoice] = useState<ProxyChoice>(null)
  const [pool, setPool] = useState<PoolItem[]>([])
  const [loaded, setLoaded] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const identity = serverIdentity()

  useEffect(() => {
    const controller = new AbortController()
    setLoaded(false)
    setError('')
    void Promise.all([
      apiCall<ProxyResponse>('/api/update/proxy', { signal: controller.signal }),
      fetchProxyPool(controller.signal),
    ]).then(([response, options]) => {
      if (controller.signal.aborted || serverIdentity() !== identity) return
      if (!response || !Object.prototype.hasOwnProperty.call(response, 'proxy') || !Array.isArray(options.items)) {
        throw new Error(t('更新代理设置不可用'))
      }
      setChoice(response.proxy)
      setPool(options.items)
      setLoaded(true)
    }).catch(cause => {
      if (!controller.signal.aborted && serverIdentity() === identity) {
        setError(cause instanceof Error ? cause.message : String(cause))
      }
    })
    return () => controller.abort()
  }, [identity])

  const source = choice?.config?.source || choice?.source
  const current = source === 'pool' && choice?.config?.proxyId ? choice.config.proxyId : choice ? OTHER : ''
  const selected = pool.find(item => item.id === current)
  const label = choice ? selected?.name || choice.label || t('指定代理') : t('直连')

  async function save(value: string) {
    if (busy || value === OTHER) return
    setBusy(true)
    setError('')
    try {
      const response = await apiCall<ProxyResponse>('/api/update/proxy', {
        method: 'POST', body: { proxy: value ? { source: 'pool', proxyId: value } : null },
      })
      if (serverIdentity() !== identity) return
      if (!response || !Object.prototype.hasOwnProperty.call(response, 'proxy')) {
        throw new Error(t('更新代理设置不可用'))
      }
      setChoice(response.proxy)
      store.toast(response.saved === false
        ? t('出网线路已切换，但写入磁盘失败（重启后会恢复原线路）')
        : t('出网代理已保存'), response.saved === false ? 'err' : 'ok')
    } catch (cause) {
      if (serverIdentity() === identity) setError(cause instanceof Error ? cause.message : String(cause))
    } finally { setBusy(false) }
  }

  return <div className='flex flex-col gap-2 px-3 py-3' style={{ borderTop: '1px solid var(--m-line)' }}>
    <div className='m-t1'>{t('更新出网代理')}</div>
    {plugin ? <p className='m-t2'>{t('与软件更新共用出网代理，修改任意一处都会同步生效。')}</p> : null}
    <Select value={current} disabled={busy || !loaded} onValueChange={value => void save(String(value))}>
      <SelectTrigger className='w-full' aria-label={t('更新出网代理')}>
        <SelectValue>{loaded ? label : t('读取中')}</SelectValue>
      </SelectTrigger>
      <SelectContent>
        <SelectItem value=''>{t('直连')}</SelectItem>
        {pool.map(item => <SelectItem key={item.id} value={item.id} disabled={!item.enabled}>
          {item.name || `${item.protocol} ${item.host}:${item.port || ''}`}
        </SelectItem>)}
        {choice && !selected ? <SelectItem value={current}>{label}</SelectItem> : null}
      </SelectContent>
    </Select>
    <p className='m-t2'>{t('此代理用于网关访问官网；手机下载安装包使用本机网络。')}</p>
    {choice?.error ? <Alert variant='destructive' className='m-t2'>{choice.error}</Alert> : null}
    {error ? <Alert variant='destructive' className='m-t2'>{error}</Alert> : null}
  </div>
}
