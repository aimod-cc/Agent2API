import * as React from 'react'
import { Alert, Button, Dialog, DialogBody, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, Input, Label, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Spinner } from '@ui'
import { launchMessage, launchPending, launchPhaseText, readLaunchResult, readLaunchSettings, type LaunchClient, type LaunchRequest, type LaunchResult, type LaunchSelection, type LaunchSettings, type LaunchTranslate } from './plugin-desktop-launch'

const BASE = '/api/plugins/desktop-launch'
let managementGeneration = 0
if (typeof window !== 'undefined') window.addEventListener('wb:management-session-changed', () => { managementGeneration++ })

export function PluginDesktopLaunchDialog({ provider, account, name, clients, request, pickProgram, onClose, onChanged, recoveryBackup, t }: {
  provider: string
  account: string
  name: string
  clients: LaunchClient[]
  request: LaunchRequest
  pickProgram(path?: string): Promise<{ canceled: boolean; path?: string }>
  onClose(): void
  onChanged(): Promise<unknown> | void
  recoveryBackup?: string
  t: LaunchTranslate
}) {
  const [client, setClient] = React.useState(clients[0]?.id || '')
  const [settings, setSettings] = React.useState<LaunchSettings | null>(null)
  const [path, setPath] = React.useState('')
  const [result, setResult] = React.useState<LaunchResult | null>(null)
  const [busy, setBusy] = React.useState(false)
  const [notice, setNotice] = React.useState('')
  const [unknown, setUnknown] = React.useState(false)
  const [showPath, setShowPath] = React.useState(false)
  const alive = React.useRef(true)
  const sent = React.useRef(new Set<string>())
  const generation = React.useRef(0)
  const session = React.useRef(managementGeneration)
  const resultRef = React.useRef<LaunchResult | null>(null)
  const pending = React.useRef(false)
  const [restoreConfirm, setRestoreConfirm] = React.useState(false)
  const [restartConfirm, setRestartConfirm] = React.useState(false)
  const selection: LaunchSelection = { provider, account, client }
  const [queuedClient, setQueuedClient] = React.useState('')
  const selected = clients.find(item => item.id === client)
  const processing = !!result && launchPending(result.phase)
  const locked = busy || processing
  const activeClient = result?.client.label || selected?.label
  const pathInput = React.useId()

  React.useEffect(() => {
    alive.current = true
    const changed = () => {
      generation.current++
      setUnknown(true)
      setNotice(t('本机管理会话已变化，本次确认已失效；不会自动重发操作'))
    }
    window.addEventListener('wb:management-session-changed', changed)
    return () => {
      alive.current = false
      generation.current++
      window.removeEventListener('wb:management-session-changed', changed)
      const value = resultRef.current
      if (value && (!value.committed || launchPending(value.phase)) && session.current === managementGeneration) {
        void request('POST', `${BASE}/cancel`, { intent: value.intent }, { retry: 'never' }).catch(() => {})
      }
    }
  }, [])

  async function action(work: () => Promise<void>) {
    if (pending.current || session.current !== managementGeneration) return
    pending.current = true
    setBusy(true)
    setNotice('')
    const current = generation.current
    try { await work() }
    catch (error) {
      if (alive.current && current === generation.current) setNotice(launchMessage(error instanceof Error ? error.message : String(error), t))
    } finally {
      pending.current = false
      if (alive.current) setBusy(false)
    }
  }

  function accept(raw: unknown) {
    const value = readLaunchResult(raw)
    if (value.provider !== provider || value.account !== account || value.client.id !== client
      || resultRef.current && resultRef.current.intent !== value.intent) throw new Error(t('本机启动响应不属于当前账号'))
    if (!alive.current || session.current !== managementGeneration) return
    const previous = resultRef.current
    const changed = !previous || previous.phase !== value.phase || previous.message !== value.message
    resultRef.current = value
    setResult(value)
    setUnknown(false)
    if (changed) {
      if (value.started || value.restored) void Promise.resolve(onChanged()).catch(() => {})
      window.dispatchEvent(new Event('wb:desktop-launch-changed'))
    }
  }

  async function load() {
    setSettings(null)
    const previous = resultRef.current
    if (previous?.phase === 'awaiting_confirmation') {
      await request('POST', `${BASE}/cancel`, { intent: previous.intent }, { retry: 'never' })
    }
    setResult(null)
    resultRef.current = null
    sent.current.clear()
    setUnknown(false)
    setRestoreConfirm(false)
    setRestartConfirm(false)
    setShowPath(false)
    if (recoveryBackup) {
      accept(await request('POST', `${BASE}/recovery-prepare`, { backup: recoveryBackup }, { retry: 'never' }))
      return
    }
    const value = readLaunchSettings(await request('POST', `${BASE}/settings`, selection, { retry: 'never' }), selection)
    if (!alive.current || session.current !== managementGeneration) return
    setSettings(value)
    setPath(value.path || value.detected || '')
    setShowPath(!value.detected || !!value.error)
    if (!value.detected || value.error) return
    accept(await request('POST', `${BASE}/prepare`, selection, { retry: 'never' }))
  }

  React.useEffect(() => { if (client) void action(load) }, [client])
  React.useEffect(() => {
    if (queuedClient && !pending.current && !busy && !processing) {
      setClient(queuedClient)
      setQueuedClient('')
    }
  }, [queuedClient, busy, processing])

  React.useEffect(() => {
    if (!result || !processing || unknown) return
    const intent = result.intent
    let checking = false
    const timer = window.setInterval(() => {
      if (checking || pending.current || !alive.current || session.current !== managementGeneration) return
      checking = true
      void request('GET', `${BASE}/status?intent=${encodeURIComponent(intent)}`, undefined, { retry: 'never' })
        .then(accept).catch(error => {
          if (!alive.current || session.current !== managementGeneration) return
          setUnknown(true)
          setNotice(launchMessage(error instanceof Error ? error.message : String(error), t))
        }).finally(() => { checking = false })
    }, 700)
    return () => window.clearInterval(timer)
  }, [result?.intent, processing, unknown])

  async function savePath() {
    if (!settings) return
    await action(async () => {
      await request('PATCH', `${BASE}/settings`, { selection, path: path.trim(), revision: settings.revision }, { retry: 'never' })
      await load()
    })
  }

  async function execute(kind: 'confirm' | 'restore' | 'retry-start') {
    if (!result || pending.current || session.current !== managementGeneration) return
    const key = `${kind}:${result.intent}`
    if (sent.current.has(key)) return
    sent.current.add(key)
    await action(async () => {
      try {
        const value = await request('POST', `${BASE}/${kind}`, { intent: result.intent,
          ...(kind === 'confirm' ? { confirmed: true, restartConfirmed: result.requiresRestart }
            : kind === 'restore' ? { confirmed: true }
            : result.running ? { restartConfirmed: true } : {}) }, { retry: 'never' })
        accept(value)
        if (kind !== 'confirm') sent.current.delete(key)
      } catch (error) {
        setUnknown(true)
        throw error
      }
    })
  }

  async function readStatus() {
    if (!result) return
    await action(async () => {
      accept(await request('GET', `${BASE}/status?intent=${encodeURIComponent(result.intent)}`, undefined, { retry: 'never' }))
      if (resultRef.current && !launchPending(resultRef.current.phase)) {
        sent.current.delete(`retry-start:${result.intent}`)
        sent.current.delete(`restore:${result.intent}`)
      }
    })
  }

  async function cancel() {
    if (!result || pending.current || session.current !== managementGeneration || sent.current.has(`cancel:${result.intent}`)) return
    sent.current.add(`cancel:${result.intent}`)
    await action(async () => {
      try { accept(await request('POST', `${BASE}/cancel`, { intent: result.intent }, { retry: 'never' })) }
      catch (error) { setUnknown(true); throw error }
    })
  }

  return <Dialog open onOpenChange={open => { if (!open) onClose() }}>
    <DialogContent className='desktop-launch-dialog w-[min(560px,calc(100vw-32px))]'>
      <DialogHeader className='py-4'><DialogTitle>{recoveryBackup ? t('恢复原凭证') : result?.hotSwitch ? t('切换客户端账号') : t('启动客户端')}</DialogTitle></DialogHeader>
      <DialogBody className='gap-4 py-4'>
        <DialogDescription>{recoveryBackup ? t('关闭客户端并恢复上次切换前的登录凭证') : result?.hotSwitch ? t('在线切换当前客户端的登录账号，无需重启') : t('将本机客户端切换到此账号后启动')}</DialogDescription>
        <dl className='grid grid-cols-[72px_minmax(0,1fr)] gap-x-3 gap-y-2 text-sm'>
          <dt className='text-muted-foreground'>{t('账号')}</dt><dd className='break-words'>{name}</dd>
          <dt className='text-muted-foreground'>{t('客户端')}</dt><dd>
            {clients.length > 1 && !locked && !result?.committed ? <Select value={client} onValueChange={value => {
              if (value) { if (pending.current) setQueuedClient(value); else setClient(value) }
            }}>
              <SelectTrigger className='w-full'><SelectValue>{activeClient || t('请选择')}</SelectValue></SelectTrigger>
              <SelectContent>{clients.map(item => <SelectItem key={item.id} value={item.id}>{item.label}</SelectItem>)}</SelectContent>
            </Select> : activeClient}
          </dd>
        </dl>
        {settings && (showPath || !result) ? <div className='flex flex-col gap-2'>
          <Label htmlFor={pathInput}>{t('程序位置')}</Label>
          <div className='flex items-center gap-2'><Input id={pathInput} className='min-w-0 flex-1' value={path} disabled={locked}
            placeholder={t('选择本机客户端程序')} onChange={event => setPath(event.currentTarget.value)} />
            <Button variant='outline' disabled={locked} onClick={() => void action(async () => {
              const picked = await pickProgram(path)
              if (!picked.canceled && picked.path) setPath(picked.path)
            })}>{t('选择')}</Button></div>
          <div className='flex items-center gap-2'>
            <Button size='sm' variant='outline' disabled={locked} onClick={() => void savePath()}>{t('保存并继续')}</Button>
            <Button size='sm' variant='ghost' disabled={locked} onClick={() => { setPath(''); void action(async () => {
              await request('PATCH', `${BASE}/settings`, { selection, path: '', revision: settings.revision }, { retry: 'never' }); await load()
            }) }}>{t('自动探测')}</Button>
          </div>
          {settings.error ? <p className='text-xs text-warning'>{launchMessage(settings.error, t)}</p> : null}
        </div> : null}
        {result ? <>
          {!showPath ? <div className='flex min-w-0 items-start justify-between gap-2 text-xs text-muted-foreground'>
            <span className='break-all'>{result.executable}</span>
            {!locked && !result.committed && !recoveryBackup ? <Button size='xs' variant='ghost' onClick={() => setShowPath(true)}>{t('更改')}</Button> : null}
          </div> : null}
          {result.phase === 'awaiting_confirmation' ? <>
            <div className='flex flex-col gap-2 border-y border-hairline py-3 text-xs'>
              <span className='font-medium text-foreground'>{t('本次凭证修改范围')}</span>
              {result.resources.map(resource => <div key={resource.id} className='flex flex-col gap-1'>
                <span>{t(resource.label)}</span><span className='break-all text-muted-foreground'>{launchMessage(resource.location, t)}</span>
              </div>)}
            </div>
            <p className='text-xs text-muted-foreground'>{recoveryBackup
              ? t('仅恢复备份中的登录凭证，完成后客户端保持关闭')
              : result.hotSwitch
                ? t('确认后将备份并替换上述登录凭证，仅重载后台服务，已打开的窗口与内容保持不变')
                : t('确认后将备份并替换上述登录凭证；不会改变反代优先级')}</p>
            {result.requiresRestart && !result.hotSwitch && !recoveryBackup ? <Alert variant='warning'>{t('客户端正在运行，继续会关闭并重启，请先保存未完成的内容')}</Alert> : null}
            {result.pendingBackup && !recoveryBackup ? <Alert variant='warning'>{t('上次切换未完整结束，请先恢复原凭证')}</Alert> : null}
          </> : <div role='status' className='flex flex-col gap-2'>
            <p className='flex items-center gap-2 text-sm'>{processing ? <Spinner className='size-4' /> : null}{launchPhaseText(result.phase, t)}</p>
            {result.message && launchMessage(result.message, t) !== launchPhaseText(result.phase, t)
              ? <p className={`text-xs ${result.error ? 'text-warning' : 'text-muted-foreground'}`}>{launchMessage(result.message, t)}</p> : null}
          </div>}
        </> : null}
        {busy && !processing ? <p role='status' className='flex items-center gap-2 text-xs text-muted-foreground'><Spinner />{t('正在读取客户端状态')}</p> : null}
        {restoreConfirm ? <Alert variant='warning'>{t('恢复会关闭客户端，请先保存未完成的内容；恢复完成后不会自动启动')}</Alert> : null}
        {restartConfirm ? <Alert variant='warning'>{t('重启会关闭当前客户端，请先保存未完成的内容')}</Alert> : null}
        {notice ? <Alert variant='warning'>{notice}</Alert> : null}
        {unknown && result ? <Button size='sm' variant='outline' className='self-start' disabled={busy} onClick={() => void readStatus()}>{t('查询本次状态')}</Button> : null}
      </DialogBody>
      <DialogFooter className='flex-wrap gap-2 py-4'>
        {processing && !busy ? <Button variant='outline' onClick={() => void cancel()}>{t('取消')}</Button> : null}
        {restoreConfirm && !locked ? <Button variant='outline' onClick={() => setRestoreConfirm(false)}>{t('取消')}</Button> : null}
        {restartConfirm && !locked ? <Button variant='outline' onClick={() => setRestartConfirm(false)}>{t('取消')}</Button> : null}
        <Button variant='outline' onClick={onClose}>{t('关闭')}</Button>
        {result && !locked && !unknown && result.canRestore && !restoreConfirm && !restartConfirm ? <Button variant='outline'
          onClick={() => setRestoreConfirm(true)}>{t('恢复原凭证')}</Button> : null}
        {restoreConfirm && !locked && !unknown ? <Button onClick={() => { setRestoreConfirm(false); void execute('restore') }}>
          {t('关闭客户端并恢复原凭证')}</Button> : null}
        {result?.canRetryStart && !locked && !unknown && !restoreConfirm && !restartConfirm ? (
          result.running ? <Button onClick={() => setRestartConfirm(true)}>{t('关闭并重启客户端')}</Button>
            : <Button onClick={() => void execute('retry-start')}>{t('重试启动')}</Button>
        ) : null}
        {restartConfirm && !locked && !unknown ? <Button onClick={() => { setRestartConfirm(false); void execute('retry-start') }}>
          {t('确认关闭并重启')}</Button> : null}
        {result?.phase === 'awaiting_confirmation' && !result.pendingBackup && !showPath && !restoreConfirm && !restartConfirm ? <Button
          disabled={locked || unknown || sent.current.has(`confirm:${result.intent}`) || result.remainingMs === 0}
          onClick={() => void execute('confirm')}>{result.hotSwitch ? t('立即切换') : result.requiresRestart ? t('切换并重启') : t('切换并启动')}</Button> : null}
      </DialogFooter>
    </DialogContent>
  </Dialog>
}
