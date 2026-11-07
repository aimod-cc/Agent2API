import * as React from 'react'
import {
  Alert, Badge, Button, Checkbox, Dialog, DialogBody, DialogContent, DialogDescription,
  DialogFooter, DialogHeader, DialogTitle, Input, Label, Progress, Switch,
} from '@ui'
import type { ScriptPackage, ScriptPluginsState } from './plugins'
import { PluginGetButton, PluginSectionHeading, PluginSettingsButton, PluginStoreCard, type PluginTranslate } from './plugins-store'
import { ScriptPluginSettings } from './plugins-script-settings'
import { matchesRecommendedPlugin, matchesScriptPlugin, recommendedDescription, reinstallSource, scriptPluginDescription, scriptPluginName, scriptRecommendations } from './plugins-script-display'

type Install = { name: string; version: string; id?: string; checkinAllowed?: boolean }

export function ScriptPluginsPanel({ state, request, refresh, t, disabled = false, showInstalled = true,
  showRecommended = true, query = '', galleryVisible = true, showInstallAction = true, installRequest = 0, onBusyChange }: {
  state: ScriptPluginsState
  request: (path: string, body: Record<string, unknown>) => Promise<unknown>
  refresh: () => Promise<void>
  t: PluginTranslate
  disabled?: boolean
  showInstalled?: boolean
  showRecommended?: boolean
  query?: string
  galleryVisible?: boolean
  showInstallAction?: boolean
  installRequest?: number
  onBusyChange?: (busy: boolean) => void
}) {
  const [install, setInstall] = React.useState<Install | null>(null)
  const [remove, setRemove] = React.useState<ScriptPackage | null>(null)
  const [checkin, setCheckin] = React.useState<ScriptPackage | null>(null)
  const [settingsId, setSettingsId] = React.useState<string | null>(null)
  const [trusted, setTrusted] = React.useState(false)
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const pending = React.useRef(false)
  const lastInstallRequest = React.useRef(0)
  const prefix = React.useId()
  const locked = disabled || busy || state.operation.active
  const recommended = scriptRecommendations(state)
  const github = /^(github:|https:\/\/github\.com\/)/i.test(install?.name.trim() || '')
  const magpie = install?.name.trim().startsWith('magpie:') === true
  const checkinWarning = t('脚本签到可能包含额外任务并消耗额度。WPS 灵犀还可能发送对话、创建及解散群聊；授权后会接入现有自动签到计划。')
  const settingsPack = state.packages.find(pack => pack.id === settingsId)
  const packages = showInstalled ? state.packages.filter(pack => matchesScriptPlugin(pack, recommended, query, t)) : []
  const recommendations = showRecommended ? recommended.filter(item => !state.packages.some(pack => pack.name === item.name)
    && matchesRecommendedPlugin(item, query, t)) : []
  const count = packages.length + recommendations.length

  React.useEffect(() => { onBusyChange?.(busy) }, [busy, onBusyChange])
  React.useEffect(() => () => { onBusyChange?.(false) }, [onBusyChange])
  React.useEffect(() => {
    if (installRequest <= lastInstallRequest.current) return
    lastInstallRequest.current = installRequest
    setTrusted(false); setError(''); setInstall({ name: '', version: '' })
  }, [installRequest])
  React.useEffect(() => {
    if (settingsId && !settingsPack) setSettingsId(null)
  }, [settingsId, settingsPack])

  async function act(path: string, body: Record<string, unknown>): Promise<boolean> {
    if (pending.current || locked) return false
    pending.current = true
    setBusy(true); setError('')
    try {
      await request(path, body)
      await refresh()
      return true
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
      return false
    } finally { pending.current = false; setBusy(false) }
  }

  function openInstall(value: Install): void {
    setSettingsId(null); setTrusted(false); setError(''); setInstall(value)
  }

  const stage = state.operation.stage === 'runtime' ? t('正在下载 Bun 运行时')
    : state.operation.stage === 'package' ? t('正在下载脚本插件')
    : state.operation.stage === 'clean-runtime' ? t('正在清理 Bun 运行时')
    : state.operation.stage === 'waiting' ? t('等待脚本插件的在途请求和登录完成')
    : state.operation.stage === 'restoring' ? t('正在恢复原有脚本插件')
    : t('正在加载脚本插件')
  const failure = error || state.error || state.hostError || state.runtime.error || state.operation.error

  return <>
    {failure ? <Alert variant='destructive' className='flex flex-wrap items-center justify-between gap-2 break-words'>
      <span className='min-w-0 flex-1'>{failure}</span>
      {state.runtime.installed ? <Button variant='outline' size='xs' disabled={locked}
        onClick={() => { void act('/api/plugins/scripts/runtime/clean', {}) }}>{t('清理 Bun 运行时')}</Button> : null}
    </Alert> : null}
    {state.operation.active ? <div className='space-y-2' aria-live='polite'>
      <div className='flex items-center justify-between gap-2 text-xs'><span>{stage}</span>
        {(state.operation.received || 0) > 0 ? <span className='tabular-nums text-muted-foreground'>{((state.operation.received || 0) / 1024 / 1024).toFixed(1)} MiB</span> : null}
      </div><Progress value={null} aria-label={stage} />
    </div> : null}
    {galleryVisible && (count > 0 || showInstallAction) ? <section aria-label={t('Magpie 社区')}>
      <PluginSectionHeading t={t} title={t('Magpie 社区')} count={count}
        hint={t('来自 Magpie 社区的插件，共用一个按需下载的运行时')}
        actions={showInstallAction ? <Button variant='ghost' size='sm' disabled={locked} onClick={() => openInstall({ name: '', version: '' })}>
          {t('从 npm / GitHub / Magpie 安装')}
        </Button> : null} />
      <div className='grid grid-cols-1 gap-3.5 min-[900px]:grid-cols-2'>
        {packages.map(pack => {
          const name = scriptPluginName(pack, recommended, t)
          return <PluginStoreCard key={pack.id} t={t} name={name} description={scriptPluginDescription(pack, recommended, t)}
            version={pack.version} artwork={pack.providers.some(provider => provider.id === 'minimax-code') ? 'minimax' : 'script'}
            status={!pack.enabled ? 'disabled' : pack.running && !pack.error ? 'running' : 'unavailable'}
            providerCount={pack.providers.length} enabledCount={pack.providers.filter(provider => provider.enabled).length}
            actions={<>
              <PluginSettingsButton name={name} t={t} onClick={() => { setError(''); setSettingsId(pack.id) }} />
              <Switch checked={pack.enabled} disabled={locked} aria-label={`${t('启用')} ${name}`}
                onCheckedChange={enabled => { void act('/api/plugins/scripts/change', { id: pack.id, action: 'enable', enabled }) }} />
            </>}
            feedback={pack.error ? <p className='m-0 break-words text-xs text-destructive'>{pack.error}</p> : null} />
        })}
        {recommendations.map(item => <PluginStoreCard key={item.name} t={t} name={t(item.label)}
          description={recommendedDescription(item, t)} version={item.version}
          artwork={item.providers.includes('minimax-code') ? 'minimax' : 'script'} status='missing' providerCount={item.providers.length}
          actions={<PluginGetButton t={t} disabled={locked} onClick={() => openInstall({ name: item.source || item.name, version: item.selector ?? item.version })} />} />)}
      </div>
    </section> : null}
    {settingsPack ? <ScriptPluginSettings key={settingsPack.id} pack={settingsPack} recommended={recommended} t={t}
      disabled={locked} error={error} onClose={() => setSettingsId(null)}
      onEnabled={enabled => { void act('/api/plugins/scripts/change', { id: settingsPack.id, action: 'enable', enabled }) }}
      onProvider={(provider, enabled) => { void act('/api/plugins/scripts/change', { id: settingsPack.id, action: 'provider', provider, enabled }) }}
      onCheckin={enabled => {
        if (enabled) { setError(''); setSettingsId(null); setCheckin(settingsPack) }
        else void act('/api/plugins/scripts/change', { id: settingsPack.id, action: 'checkin', enabled: false })
      }}
      onReinstall={() => openInstall({
        id: settingsPack.id, name: reinstallSource(settingsPack),
        version: settingsPack.source?.kind === 'github' ? settingsPack.source.ref : '', checkinAllowed: settingsPack.checkinAllowed === true,
      })}
      onRemove={() => { setError(''); setSettingsId(null); setRemove(settingsPack) }} /> : null}
    <Dialog open={install !== null} onOpenChange={open => { if (!open && !busy) setInstall(null) }}>
      <DialogContent className='text-foreground'>
        <DialogHeader><DialogTitle>{t('安装脚本插件')}</DialogTitle></DialogHeader>
        <DialogBody className='gap-4'>
          <DialogDescription className='text-xs leading-relaxed text-subtle'>{t('仅安装你信任的 npm 包、GitHub 仓库或 Magpie 社区分发包；完整性校验不等于安全审计。')}</DialogDescription>
          <div className='space-y-2'><Label htmlFor={`${prefix}-package`}>{t('npm 包名、GitHub 地址或 magpie:包名')}</Label>
            <Input id={`${prefix}-package`} value={install?.name || ''} disabled={busy} autoComplete='off' placeholder='@scope/plugin / https://github.com/owner/repo'
              onChange={event => { setInstall(value => value ? { ...value, name: event.target.value, checkinAllowed: false } : value); setTrusted(false) }} /></div>
          <div className='space-y-2'><Label htmlFor={`${prefix}-version`}>{github ? t('分支、标签或提交（留空使用默认分支）') : t('插件版本（留空使用 latest）')}</Label>
            <Input id={`${prefix}-version`} value={install?.version || ''} disabled={busy} placeholder={github ? t('默认分支') : 'latest'} autoComplete='off'
              onChange={event => { setInstall(value => value ? { ...value, version: event.target.value } : value); setTrusted(false) }} /></div>
          {github ? <p className='m-0 text-xs leading-relaxed text-muted-foreground'>{t('从 GitHub 官方源下载固定提交。仓库根目录须有可直接运行的插件入口；不会执行构建脚本或安装依赖。')}</p> : null}
          {magpie ? <p className='m-0 text-xs leading-relaxed text-muted-foreground'>{t('从 Magpie 社区官方分发目录下载，核验 SHA-512；更新与重装继续使用此来源。')}</p> : null}
          <p className='m-0 text-xs leading-relaxed text-muted-foreground'>{t('npm 未发布的包会尝试 Magpie 社区分发源；不会绕过完整性校验或执行安装脚本。')}</p>
          <div className='space-y-2 text-xs leading-relaxed text-muted-foreground'>
            <Badge variant='outline'>Bun {state.runtime.version} · {state.runtime.installed ? t('已下载') : t('按需下载')}</Badge>
            <p className='m-0'>{t('运行时不内置于安装包，首次安装脚本插件时自动下载并校验。')}</p>
            <p className='m-0'>{t('当前兼容无运行时 npm 依赖的提供商插件；不加载 Magpie 界面、Matrix 工具或 Pi 扩展。')}</p>
            <p className='m-0'>{t('支持自动授权、设备码、手动授权码和插件声明的凭据表单；本机读取作用于网关电脑。')}</p>
          </div>
          <Alert variant='warning'>{t('脚本不是安全沙箱，可以访问网关所在电脑的文件和网络。不要安装来历不明的插件。')}</Alert>
          <Label className='flex items-start gap-2 text-xs leading-relaxed'><Checkbox checked={trusted} disabled={busy} onCheckedChange={value => setTrusted(value === true)} />
            {t('我信任此插件来源，并允许它在网关电脑上运行。')}</Label>
          <div className='space-y-2'>
            <Label className='flex items-start gap-2 text-xs leading-relaxed'><Checkbox checked={install?.checkinAllowed === true} disabled={busy} onCheckedChange={checked => setInstall(value => value ? { ...value, checkinAllowed: checked === true } : value)} />
              {t('允许签到及附带任务')}</Label>
            <p className='m-0 text-xs leading-relaxed text-muted-foreground'>{checkinWarning}</p>
          </div>
          {error ? <p role='alert' className='m-0 text-xs text-destructive'>{error}</p> : null}
        </DialogBody>
        <DialogFooter><Button variant='outline' disabled={busy} onClick={() => setInstall(null)}>{t('取消')}</Button>
          <Button disabled={locked || !trusted || !install?.name.trim()} onClick={async () => {
            if (install && await act('/api/plugins/scripts/install', { id: install.id, name: install.name.trim(), source: install.name.trim(), version: install.version.trim(), trusted, checkinAllowed: install.checkinAllowed === true })) setInstall(null)
          }}>{busy ? t('正在安装…') : t('确认并安装')}</Button></DialogFooter>
      </DialogContent>
    </Dialog>
    <Dialog open={checkin !== null} onOpenChange={open => { if (!open && !busy) setCheckin(null) }}>
      <DialogContent className='text-foreground'><DialogHeader><DialogTitle>{t('允许签到及附带任务')}</DialogTitle></DialogHeader>
        <DialogBody><p className='m-0 break-all text-sm font-medium'>{checkin?.name}</p><Alert variant='warning'>{checkinWarning}</Alert>
          {error ? <p role='alert' className='m-0 text-xs text-destructive'>{error}</p> : null}
        </DialogBody>
        <DialogFooter><Button variant='outline' disabled={busy} onClick={() => setCheckin(null)}>{t('取消')}</Button>
          <Button disabled={locked} onClick={async () => {
            if (checkin && await act('/api/plugins/scripts/change', { id: checkin.id, action: 'checkin', enabled: true })) setCheckin(null)
          }}>{t('确认授权')}</Button></DialogFooter>
      </DialogContent>
    </Dialog>
    <Dialog open={remove !== null} onOpenChange={open => { if (!open && !busy) setRemove(null) }}>
      <DialogContent className='text-foreground'><DialogHeader><DialogTitle>{t('卸载插件包')}</DialogTitle></DialogHeader>
        <DialogBody><p className='m-0 text-sm leading-relaxed'>{t('确定卸载「{name}」吗？对应提供商将停止转发，账号数据仍然保留，重新安装后恢复。', { name: remove?.name || '' })}</p>
          {error ? <p role='alert' className='m-0 text-xs text-destructive'>{error}</p> : null}
        </DialogBody>
        <DialogFooter><Button variant='outline' disabled={busy} onClick={() => setRemove(null)}>{t('取消')}</Button>
          <Button variant='destructive' disabled={locked} onClick={async () => {
            if (remove && await act('/api/plugins/scripts/change', { id: remove.id, action: 'remove' })) setRemove(null)
          }}>{t('卸载')}</Button></DialogFooter>
      </DialogContent>
    </Dialog>
  </>
}
