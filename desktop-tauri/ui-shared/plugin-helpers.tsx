import * as React from 'react'
import { Alert, Button, Checkbox, Label } from '@ui'
import { SearchableSelect } from './searchable-select'
import { PluginPermissions } from './plugins-permissions'
import type { PackProvider } from './plugins'
import type { PluginTranslate } from './plugins-store'

type Candidate = { id: string; pack: string; provider: string; version: string; digest: string; operations: string[] }
type Grant = Candidate & { callerPack: string; callerDigest: string }
type Permission = { pack: string; provider: string; id: string; available: boolean; callerDigest: string }
type Payload = { requested: Permission[]; candidates: Candidate[]; grants: Record<string, Record<string, Grant>>; saved?: boolean }
type Request = <T>(method: 'GET' | 'PATCH', path: string, body?: unknown) => Promise<T>

function candidateKey(candidate: Candidate): string {
  return JSON.stringify([candidate.pack, candidate.provider, candidate.version, candidate.digest, candidate.operations])
}

export function PluginHelpers({ packId, providers, enabled, disabled = false, onBusyChange, request, t }: {
  packId: string
  providers: PackProvider[]
  enabled: boolean
  disabled?: boolean
  onBusyChange?: (busy: boolean) => void
  request: Request
  t: PluginTranslate
}) {
  const [data, setData] = React.useState<Payload | null>(null)
  const [selected, setSelected] = React.useState<Record<string, string>>({})
  const [confirmed, setConfirmed] = React.useState<Record<string, boolean>>({})
  const [busy, setBusy] = React.useState(false)
  const [loading, setLoading] = React.useState(true)
  const [error, setError] = React.useState('')
  const alive = React.useRef(true)
  const pending = React.useRef(false)
  const generation = React.useRef(0)
  const latest = React.useRef(request)
  latest.current = request
  const scope = JSON.stringify([packId, enabled, providers.map(provider => [provider.id, provider.enabled, provider.loaded])])
  const load = React.useCallback(async () => {
    const current = ++generation.current
    setLoading(true)
    try {
      const value = await latest.current<Payload>('GET', `/api/plugins/helpers?pack=${encodeURIComponent(packId)}`)
      if (alive.current && current === generation.current) { setData(value); setError('') }
    } catch (cause) {
      if (alive.current && current === generation.current) setError(cause instanceof Error ? cause.message : String(cause))
    } finally { if (alive.current && current === generation.current) setLoading(false) }
  }, [packId])
  React.useEffect(() => {
    alive.current = true
    setData(null); setSelected({}); setConfirmed({}); setError('')
    void load()
    return () => { alive.current = false; generation.current++ }
  }, [load, scope])
  React.useEffect(() => { onBusyChange?.(busy) }, [busy, onBusyChange])
  React.useEffect(() => () => { onBusyChange?.(false) }, [onBusyChange])

  async function save(item: Permission, target: Candidate | null) {
    if (pending.current || disabled || loading) return
    const key = `${item.provider}:${item.id}`
    if (target && (!enabled || !item.available || !confirmed[key])) return
    pending.current = true; setBusy(true); setError('')
    const current = ++generation.current
    try {
      const value = await latest.current<Payload>('PATCH', '/api/plugins/helpers', {
        pack: packId, provider: item.provider, helper: item.id, target, confirmed: confirmed[key] === true,
      })
      if (alive.current && current === generation.current) {
        setData(value); setConfirmed({}); setSelected({})
        if (value.saved === false) setError(t('设置已生效，但写入磁盘失败（重启后会恢复原设置）'))
      }
    } catch (cause) {
      if (alive.current && current === generation.current) {
        setConfirmed({}); setError(cause instanceof Error ? cause.message : String(cause))
      }
    } finally { pending.current = false; if (alive.current) setBusy(false) }
  }

  const requested = data?.requested.filter(item => item.pack === packId) || []
  if (!requested.length && !error) return null
  const locked = disabled || busy || loading
  return <PluginPermissions t={t}>
    <div className='flex items-center justify-between gap-3'>
      <h4 className='m-0 text-[12.5px] font-medium text-foreground'>{t('原生辅助授权')}</h4>
      <Button size='sm' variant='ghost' disabled={locked} onClick={() => { setConfirmed({}); void load() }}>{t('刷新')}</Button>
    </div>
    {requested.length ? <p className='m-0'>{t('原生辅助程序不受 Wasm 沙箱限制，只授权你信任的已安装插件。辅助程序更新后需要重新确认。')}</p> : null}
    {error ? <Alert variant='destructive'>{error}</Alert> : null}
    {requested.map(item => {
      const key = `${item.provider}:${item.id}`
      const options = data?.candidates.filter(candidate => candidate.id === item.id) || []
      const current = data?.grants[item.provider]?.[item.id]
      const chosen = options.find(candidate => candidateKey(candidate) === selected[key])
      const authorized = current && current.callerPack === packId && current.callerDigest === item.callerDigest
        && options.some(candidate => candidateKey(candidate) === candidateKey(current))
      const label = providers.find(provider => provider.id === item.provider)?.label || item.provider
      return <div key={key} className='flex flex-col gap-2 border-t border-hairline pt-3'>
        <Label className='text-[12.5px] text-foreground'>{t(label)} · {item.id}</Label>
        <span className='text-xs text-muted-foreground'>{current ? `${current.pack} ${current.version}` : t('未授权')}</span>
        {enabled && item.available ? <>
          {current && !authorized ? <Alert>{t('原生辅助授权已失效或暂不可用，请重新选择并确认。')}</Alert> : null}
          <SearchableSelect value={selected[key] || ''} disabled={locked || !options.length} aria-label={`${t(label)} · ${item.id}`}
            displayValue={chosen ? `${chosen.pack} ${chosen.version}` : t('请选择')}
            onValueChange={value => { setSelected(previous => ({ ...previous, [key]: value })); setConfirmed(previous => ({ ...previous, [key]: false })) }}
            options={options.map(candidate => ({ value: candidateKey(candidate),
              label: `${candidate.pack} · ${candidate.version} · ${candidate.provider}`,
            }))} />
          {!options.length ? <p className='m-0'>{t('暂无可用的原生辅助程序，请先安装并启用所需插件。')}</p> : null}
          {chosen ? <>
            <p className='m-0 break-words'>{t('允许的操作')}：{chosen.operations.join('、')}</p>
            <Label className='flex items-start gap-2 text-xs leading-relaxed'>
              <Checkbox checked={confirmed[key] === true} disabled={locked}
                onCheckedChange={value => setConfirmed(previous => ({ ...previous, [key]: value === true }))} />
              {t('我信任所选辅助程序并允许它执行声明的本机操作')}
            </Label>
          </> : null}
          <div className='flex flex-wrap gap-2'>
            <Button size='sm' disabled={locked || !confirmed[key] || !chosen} onClick={() => { if (chosen) void save(item, chosen) }}>{t('授权')}</Button>
            {current ? <Button size='sm' variant='outline' disabled={locked} onClick={() => void save(item, null)}>{t('撤销授权')}</Button> : null}
          </div>
        </> : <>
          <p className='m-0'>{t('插件未启用或已不再申请此权限；已有授权仍可撤销。')}</p>
          {current ? <Button size='sm' variant='outline' className='self-start' disabled={locked} onClick={() => void save(item, null)}>{t('撤销授权')}</Button> : null}
        </>}
      </div>
    })}
  </PluginPermissions>
}
