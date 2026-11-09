import * as React from 'react'
import { Alert, Button, Input, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Switch } from '../ui-kit/src'
import { useGithubClock } from './github-failure'
import {
  DEFAULT_GITHUB_MIRRORS, githubMirrors, githubSourceLabel,
  type GithubAccessPatch, type GithubAccessSettings, type GithubMirror, type GithubSourceObservation,
} from './github-access'

type Translate = (key: string, params?: Record<string, string | number>) => string

type Props = {
  load: () => Promise<GithubAccessSettings | null | undefined>
  persist: (patch: GithubAccessPatch) => Promise<GithubAccessSettings | null | undefined>
  notify: (message: string, failed: boolean) => void
  t: Translate
  mobile?: boolean
}

export function GithubAccessEditor({ load, persist, notify, t, mobile = false }: Props) {
  const [settings, setSettings] = React.useState<GithubAccessSettings | null>(null)
  const [draft, setDraft] = React.useState('')
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const saving = React.useRef(false)
  const revision = React.useRef(0)
  const modeOptions = [
    { value: 'auto', label: t('自动选择最快可用源') },
    { value: 'official', label: t('仅官方源') },
    { value: 'accelerated', label: t('指定加速源优先') },
  ] as const

  React.useEffect(() => {
    let alive = true
    async function refresh(initial: boolean): Promise<void> {
      if (saving.current || (!initial && document.visibilityState === 'hidden')) return
      const generation = revision.current
      try {
        const value = await load()
        if (!value) throw new Error(t('请求失败'))
        if (alive && !saving.current && revision.current === generation) setSettings(value)
      } catch (failure) {
        if (alive && initial) setError(failure instanceof Error ? failure.message : String(failure))
      }
    }
    void refresh(true)
    const timer = window.setInterval(() => void refresh(false), 5_000)
    return () => { alive = false; window.clearInterval(timer) }
  }, [load, t])

  async function save(patch: GithubAccessPatch): Promise<boolean> {
    if (saving.current) return false
    saving.current = true
    revision.current += 1
    setBusy(true)
    setError('')
    try {
      const value = await persist(patch)
      if (!value) throw new Error(t('请求失败'))
      setSettings(value)
      notify(value.saved === false
        ? t('设置已生效，但写入磁盘失败（重启后会恢复原设置）') : t('GitHub 访问设置已保存'), value.saved === false)
      return true
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
      return false
    } finally {
      saving.current = false
      setBusy(false)
    }
  }

  const mirrors = settings ? githubMirrors(settings) : []
  const modern = Array.isArray(settings?.mirrors)
  const disabled = busy || !settings || settings.mode === 'official'
  const enabled = mirrors.filter(mirror => mirror.enabled)
  const missingDefaults = DEFAULT_GITHUB_MIRRORS.filter(defaultMirror => !mirrors.some(mirror => mirror.url === defaultMirror.url))

  function updateMirror(url: string, patch: Partial<GithubMirror>): void {
    void save({ mirrors: mirrors.map(mirror => mirror.url === url ? { ...mirror, ...patch } : mirror) })
  }

  async function addMirror(): Promise<void> {
    const value = draft.trim()
    if (!value) return
    if (await save(modern ? { mirrors: [...mirrors, { url: value, enabled: true, supportsApi: true }] } : { mirrorUrl: value })) {
      setDraft('')
    }
  }

  return (
    <div className='flex flex-col gap-3 text-[13px] leading-relaxed'>
      <div className='font-semibold text-foreground'>{t('GitHub 访问方式')}</div>
      <Select value={settings?.mode || 'auto'} disabled={busy || !settings}
        onValueChange={value => { if (value) void save({ mode: value as GithubAccessSettings['mode'] }) }}>
        <SelectTrigger className='w-full' aria-label={t('GitHub 访问方式')}>
          <SelectValue>{modeOptions.find(option => option.value === settings?.mode)?.label || modeOptions[0].label}</SelectValue>
        </SelectTrigger>
        <SelectContent>
          {modeOptions.map(option => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}
        </SelectContent>
      </Select>
      <p className='text-subtle'>
        {settings?.mode === 'official' ? t('只访问 GitHub 官方源，仍使用所选出网代理。')
          : settings?.mode === 'accelerated' ? t('先使用指定加速源，失败后尝试其他已启用的加速源，最后回退官方源。')
            : t('分别探测版本查询、插件索引和文件下载，选择最快返回有效内容的源；成功线路短期复用，失败自动切换。')}
      </p>
      {settings?.mode === 'accelerated' && enabled.length > 0 ? (
        <Select value={settings.mirrorUrl} disabled={busy}
          onValueChange={value => { if (value) void save({ mirrorUrl: value }) }}>
          <SelectTrigger className='w-full' aria-label={t('优先加速源')}>
            <SelectValue>{githubSourceLabel(enabled.find(mirror => mirror.url === settings.mirrorUrl)?.url || enabled[0].url)}</SelectValue>
          </SelectTrigger>
          <SelectContent>
            {enabled.map(mirror => <SelectItem key={mirror.url} value={mirror.url}>{githubSourceLabel(mirror.url)}</SelectItem>)}
          </SelectContent>
        </Select>
      ) : null}
      <div className='rounded-md border border-border p-3'>
        <div className='font-medium text-foreground'>{t('官方源')}</div>
        <SourceStatus source='official' observations={settings?.observations || []} serverTime={settings?.serverTime} t={t} />
      </div>
      <div className='flex flex-col gap-2'>
        {mirrors.map(mirror => {
          const builtin = DEFAULT_GITHUB_MIRRORS.some(item => item.url === mirror.url)
          return (
            <div key={mirror.url} className='rounded-md border border-border p-3'>
              <div className='flex items-center justify-between gap-3'>
                <span className='min-w-0 break-all font-medium text-foreground'>{githubSourceLabel(mirror.url)}</span>
                {modern ? <Switch checked={mirror.enabled} disabled={disabled}
                  aria-label={t('启用加速源 {source}', { source: githubSourceLabel(mirror.url) })}
                  onCheckedChange={enabled => updateMirror(mirror.url, { enabled })} /> : null}
              </div>
              <p className='mt-1 text-xs text-subtle'>
                {mirror.supportsApi ? t('版本查询、插件索引、文件下载') : t('仅插件索引和文件下载，不支持版本查询')}
              </p>
              {mirror.url === 'https://cdn.gh-proxy.org/' ? <p className='mt-1 text-xs text-subtle'>{t('与 gh-proxy.com 属于同一服务的备用线路。')}</p> : null}
              {modern && !builtin ? (
                <div className='mt-2 flex items-center justify-between gap-2'>
                  <label className='inline-flex items-center gap-2 text-xs text-subtle'>
                    <Switch checked={mirror.supportsApi} disabled={disabled || /https:\/\/(ghproxy\.net|ghfast\.top)\//.test(mirror.url)}
                      aria-label={t('支持版本查询')} onCheckedChange={supportsApi => updateMirror(mirror.url, { supportsApi })} />
                    {t('支持版本查询')}
                  </label>
                  <Button size='sm' variant='ghost' disabled={disabled}
                    onClick={() => void save({ mirrors: mirrors.filter(item => item.url !== mirror.url) })}>{t('移除')}</Button>
                </div>
              ) : null}
              <SourceStatus source={mirror.url} observations={settings?.observations || []} serverTime={settings?.serverTime} t={t} />
            </div>
          )
        })}
      </div>
      {settings && settings.mode !== 'official' && !enabled.length ? <p className='text-subtle'>{t('没有启用的加速源，将只使用官方源。')}</p> : null}
      <div className='flex items-center gap-2'>
        <Input value={draft} disabled={disabled || (modern && mirrors.length >= 6)} className='min-w-0 flex-1'
          aria-label={t('自定义加速前缀')} placeholder='https://example.com/' autoComplete='off' spellCheck={false}
          onChange={event => setDraft(event.currentTarget.value)}
          onKeyDown={event => { if (event.key === 'Enter') { event.preventDefault(); void addMirror() } }} />
        <Button size='sm' variant='outline' disabled={disabled || !draft.trim() || (modern && mirrors.length >= 6)}
          onClick={() => void addMirror()}>{modern ? t('添加') : t('保存')}</Button>
      </div>
      {modern && missingDefaults.length > 0 ? <Button className='self-start' size='sm' variant='ghost'
        disabled={disabled || mirrors.length + missingDefaults.length > 6}
        onClick={() => void save({ mirrors: [...mirrors, ...missingDefaults] })}>{t('补充内置备用源')}</Button> : null}
      <p className='text-xs text-subtle'>{t('最多保留 6 个 HTTPS 加速源；自定义源可关闭版本查询支持。')}</p>
      <p className='text-xs text-subtle'>{t('状态来自最近一次实际请求，并非实时测速。过期后标为待重新检查；限流只暂停对应线路，其他线路仍可使用。')}</p>
      <p className='text-xs text-subtle'>{t('软件更新与插件共用此设置；GitHub 令牌仅发送给官方 API。镜像安装包需要官方校验值，镜像插件包需要可信签名。')}</p>
      {mobile ? <p className='text-xs text-subtle'>{t('以上状态来自网关；APK 下载会由手机按本机网络重新选择通道。')}</p> : null}
      {!modern && settings ? <p className='text-xs text-subtle'>{t('当前网关仍使用旧版单源设置，升级网关后可管理多个加速源。')}</p> : null}
      {error ? <Alert variant='destructive' role='alert' className='text-xs'>{error}</Alert> : null}
    </div>
  )
}

function SourceStatus({ source, observations, serverTime, t }: { source: string; observations: GithubSourceObservation[]; serverTime?: number; t: Translate }) {
  const now = useGithubClock(serverTime)
  const items = observations.filter(item => item.source === source)
  if (!items.length) return <p className='mt-2 text-xs text-subtle'>{t('未探测')}</p>
  const states: Record<string, string> = {
    ok: t('可用'), failed: t('连接失败'), timeout: t('连接超时'), invalid: t('返回内容异常'),
    unsupported: t('不支持此类请求'), rateLimited: t('限流冷却中'), stale: t('待重新检查'),
  }
  const resources = { api: t('版本查询'), index: t('插件索引'), download: t('文件下载') }
  return <div className='mt-2 flex flex-col gap-1 text-xs text-subtle'>
    {items.map(item => {
      const cooling = Boolean(item.retryAt && item.retryAt > now)
      const stale = !cooling && (item.state === 'stale' || item.state === 'rateLimited' || Boolean(item.validUntil && item.validUntil <= now))
      const state = cooling ? 'rateLimited' : stale ? 'stale' : item.state
      const previous = item.lastState || item.state
      return <div key={item.resource}>
        <span>{resources[item.resource]} · {states[state] || t('连接失败')}</span>
        {cooling ? <>
          <span> · {t('{seconds} 秒后可重试', { seconds: Math.ceil((item.retryAt! - now) / 1000) })}</span>
          <span> · {item.retrySource === 'fallback' ? t('上游未提供恢复时间，按 GitHub 建议短暂等待')
            : item.retrySource === 'legacy' ? t('沿用已记录的恢复时间') : t('按上游返回时间')}</span>
        </> : null}
        {stale && previous !== 'stale' ? <span> · {t('上次结果：{state}', { state: states[previous] || previous })}</span> : null}
        {state === 'ok' && item.elapsedMs != null ? <span> · {t('{ms} 毫秒', { ms: item.elapsedMs })}</span> : null}
        {item.selected && state === 'ok' ? <span> · {t('最近采用')}</span> : null}
        {item.checkedAt > 0 ? <span> · {t('记录于 {time}', { time: new Date(item.checkedAt).toLocaleTimeString() })}</span> : null}
      </div>
    })}
  </div>
}
