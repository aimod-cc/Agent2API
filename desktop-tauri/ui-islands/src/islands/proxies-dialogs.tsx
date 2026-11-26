import * as React from 'react'
import {
  Button,
  Checkbox,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Input,
  Label,
  SegmentedControl,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Switch,
} from '@ui'
import {
  durationText, errorMessage, itemName, loadSettings, saveItem, saveSettings, toast, wb,
  type ProxyPoolItem,
} from './proxies-state'
import { t } from '../i18n'
import { openExternal } from './update-shared'
import { ProxySponsorCards } from '../../../ui-shared/proxy-sponsor-cards'
import { ProxyBatchEditor, type ProxyBatchHandle } from '../../../ui-shared/proxy-batch-editor'
import { proxyHost, type ProxyImportDraft, type ProxyImportReport } from '../../../ui-shared/proxy-import'
import { proxyServiceLabels } from '../../../ui-shared/proxy-usage'

/* ─── 新增 / 编辑弹窗 ───────────────────────── */

export function ProxyModal({
  item,
  onClose,
}: {
  item: ProxyPoolItem | null
  onClose: () => void
}) {
  const editing = item !== null
  const [name, setName] = React.useState(item?.name || '')
  const [protocol, setProtocol] = React.useState<'http' | 'socks5'>(item?.protocol === 'socks5' ? 'socks5' : 'http')
  const [host, setHost] = React.useState(item?.host || '')
  const [port, setPort] = React.useState(item?.port ? String(item.port) : '')
  const [username, setUsername] = React.useState(item?.username || '')
  const [password, setPassword] = React.useState(item?.password || '')
  const [enabled, setEnabled] = React.useState(item ? item.enabled : true)
  const [nonDirect, setNonDirect] = React.useState(item?.nonDirect === true)
  const [mode, setMode] = React.useState<'single' | 'batch'>('single')
  const body = React.useRef<HTMLDivElement>(null)
  const [batchFooterHost, setBatchFooterHost] = React.useState<HTMLDivElement | null>(null)
  const [batchBusy, setBatchBusy] = React.useState(false)
  const batch = React.useRef<ProxyBatchHandle>(null)
  const [useAuth, setUseAuth] = React.useState(Boolean(item?.username || item?.password))
  const [saving, setSaving] = React.useState(false)
  const [testing, setTesting] = React.useState(false)
  const [status, setStatus] = React.useState<React.ReactNode>(null)
  const busy = saving || testing || batchBusy

  React.useEffect(() => { setStatus(null) }, [name, protocol, host, port, username, password, useAuth, nonDirect])
  React.useEffect(() => { if (body.current) body.current.scrollTop = 0 }, [mode])

  async function saveBatch(items: ProxyImportDraft[]): Promise<ProxyImportReport> {
    const data = await saveItem(null, { items })
    if (!data?.imported) throw new Error(t('批量添加响应异常，请刷新代理列表确认后再试'))
    return data.imported
  }

  async function testBatch(draft: ProxyImportDraft) {
    const data = await wb().workbuddyDesktop?.testProxy?.({ proxy: { ...draft, source: 'custom' } })
    if (!data) throw new Error(t('当前环境不支持代理测试（桥接方法缺失）'))
    return data
  }

  /** 表单 → 归一化后的提交对象；形状非法时返回一句面向用户的错误 */
  function draftPayload(): { payload: Record<string, unknown> } | { error: string } {
    const trimmed = name.trim()
    if (!trimmed) return { error: t('请填写代理名称') }
    if (useAuth && ([...username.trim()].length > 200 || [...password].length > 200)) {
      return { error: t('代理账号和密码不能超过 200 个字符') }
    }
    if (useAuth && protocol === 'socks5' && (new TextEncoder().encode(username.trim()).length > 255 || new TextEncoder().encode(password).length > 255)) {
      return { error: t('SOCKS5 用户名和密码不能超过 255 字节') }
    }
    let hostText: string
    try { hostText = proxyHost(host, t) }
    catch (error) { return { error: errorMessage(error) } }
    const portNumber = Number(port)
    if (!Number.isInteger(portNumber) || portNumber < 1 || portNumber > 65535) {
      return { error: t('代理端口必须是 1-65535 的整数') }
    }
    return {
      payload: {
        name: trimmed,
        source: 'manual',
        protocol,
        host: hostText,
        port: portNumber,
        username: useAuth ? username.trim() : '',
        password: useAuth ? password : '',
        enabled,
        nonDirect,
      },
    }
  }

  /** 用当前表单内容测一次（**未保存也能测**：先确认能通，再决定保存） */
  async function testDraft(): Promise<void> {
    if (busy) return
    const draft = draftPayload()
    if ('error' in draft) {
      setStatus(<span className='text-destructive'>❌ {draft.error}</span>)
      return
    }
    const payload = draft.payload
    const proxy = {
      source: 'custom',
      protocol: payload.protocol,
      host: payload.host,
      port: payload.port,
      username: payload.username,
      password: payload.password,
      nonDirect: payload.nonDirect,
    }
    setTesting(true)
    setStatus(t('正在连接上游…'))
    try {
      const data = await wb().workbuddyDesktop?.testProxy?.({ proxy })
      if (data?.success) {
        const suffix = durationText(data.durationMs)
        setStatus(
          <span className='text-success'>
            {t('✅ 出口可用')}
            {data.ip ? t('　出口 IP {ip}', { ip: data.ip }) : ''}
            {suffix ? `　${suffix}` : ''}
          </span>,
        )
      } else {
        setStatus(<span className='text-destructive'>❌ {data?.error || t('连接失败')}</span>)
      }
    } catch (error) {
      setStatus(<span className='text-destructive'>❌ {errorMessage(error)}</span>)
    } finally {
      setTesting(false)
    }
  }

  async function save(): Promise<void> {
    if (busy) return
    const draft = draftPayload()
    if ('error' in draft) {
      setStatus(<span className='text-destructive'>❌ {draft.error}</span>)
      return
    }
    setSaving(true)
    setStatus(null)
    try {
      await saveItem(item, draft.payload)
      toast(editing ? t('已保存「{name}」', { name: name.trim() }) : t('已新增「{name}」', { name: name.trim() }))
      if (editing || !batch.current?.hasDraft()) onClose()
      else { setName(''); setHost(''); setPort(''); setUsername(''); setPassword(''); setStatus(null) }
    } catch (error) {
      setStatus(<span className='text-destructive'>❌ {errorMessage(error)}</span>)
    } finally { setSaving(false) }
  }

  return (
    <Dialog open onOpenChange={(open, details) => {
      if (open) return
      if (busy) { details.cancel(); return }
      onClose()
    }}>
      <DialogContent className={editing ? 'max-w-[560px]' : 'w-[min(760px,calc(100vw-48px))]'}>
        <DialogHeader className='shrink-0'>
          <DialogTitle>{editing ? t('编辑代理') : t('新增代理')}</DialogTitle>
        </DialogHeader>
        {!editing ? <div className='shrink-0 border-b border-hairline px-5 py-3'>
          <SegmentedControl value={mode} onValueChange={setMode} disabled={busy} aria-label={t('新增方式')}
            options={[{ value: 'single', label: t('单个添加') }, { value: 'batch', label: t('批量添加') }]} />
        </div> : null}
        <DialogBody ref={body} className='min-h-0 gap-3.5'>
          <div className={mode === 'single' ? 'flex min-w-0 flex-col gap-3.5' : 'hidden'} inert={busy || mode !== 'single'}>
          <div className='flex flex-col gap-1.5'>
            <Label htmlFor='proxy-name'>{t('名称')}</Label>
            <Input id='proxy-name' maxLength={60} autoComplete='off' spellCheck={false}
              placeholder={t('例如：香港 HTTP 出口')}
              value={name} onChange={event => setName(event.currentTarget.value)} />
          </div>

          <div className='flex flex-row items-end gap-2.5'>
            <div className='flex flex-col gap-1.5'>
              <Label htmlFor='proxy-protocol'>{t('协议')}</Label>
              {/* 协议名 HTTP / SOCKS5 是固定叫法，不进词典 */}
              <Select value={protocol} onValueChange={value => setProtocol(value as 'http' | 'socks5')}>
                <SelectTrigger id='proxy-protocol' className='min-w-[110px]'>
                  <SelectValue>{protocol === 'socks5' ? 'SOCKS5' : 'HTTP'}</SelectValue>
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value='http'>HTTP</SelectItem>
                  <SelectItem value='socks5'>SOCKS5</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div className='flex min-w-0 flex-1 flex-col gap-1.5'>
              <Label htmlFor='proxy-host'>{t('主机')}</Label>
              <Input id='proxy-host' placeholder='127.0.0.1' autoComplete='off' spellCheck={false}
                value={host} onChange={event => setHost(event.currentTarget.value)} />
            </div>
            <div className='flex flex-col gap-1.5'>
              <Label htmlFor='proxy-port'>{t('端口')}</Label>
              <Input id='proxy-port' type='number' min={1} max={65535} className='max-w-[110px]'
                placeholder='7890' value={port} onChange={event => setPort(event.currentTarget.value)} />
            </div>
          </div>
          <p className='text-xs leading-[1.8] text-subtle'>{t('地址由网关所在设备访问；本机代理需运行在该设备上。')}</p>
          <Label className='flex cursor-pointer items-center gap-2 text-xs font-normal'>
            <Checkbox checked={useAuth} onCheckedChange={value => setUseAuth(value === true)} />
            {t('需要用户名和密码（可选）')}
          </Label>
          {useAuth ? <div className='flex flex-row items-end gap-2.5'>
            <div className='flex min-w-0 flex-1 flex-col gap-1.5'>
              <Label htmlFor='proxy-user'>{t('用户名')}</Label>
              <Input id='proxy-user' placeholder={t('可选')} autoComplete='off'
                value={username} onChange={event => setUsername(event.currentTarget.value)} />
            </div>
            <div className='flex min-w-0 flex-1 flex-col gap-1.5'>
              <Label htmlFor='proxy-pass'>{t('密码')}</Label>
              <Input id='proxy-pass' type='password' placeholder={t('可选')} autoComplete='new-password'
                value={password} onChange={event => setPassword(event.currentTarget.value)} />
            </div>
          </div> : null}

          <Label className='flex cursor-pointer items-center gap-2 text-xs font-normal'>
            <Checkbox checked={nonDirect} onCheckedChange={setNonDirect} />{t('非直连代理')}
          </Label>
          <p className='text-xs leading-relaxed text-subtle'>{t('非直连会先经过网关所在设备的系统代理，再连接到此代理；系统代理不可用时不会改走直连。')}</p>

          <div className='flex flex-row items-center gap-2.5'>
            <Switch checked={enabled} onCheckedChange={setEnabled} aria-label={t('启用这条代理')} />
            <span className='text-[12.5px]'>
              {t('启用（禁用后，使用此代理的账号请求将失败；仍可测试）')}
            </span>
          </div>

          {status ? <div className='text-xs text-subtle' role='status'>{status}</div> : null}
          </div>
          {!editing ? <div className={mode === 'batch' ? 'min-w-0' : 'hidden'} inert={mode !== 'batch'}>
            <ProxyBatchEditor ref={batch} t={t} locked={saving || testing} onBusyChange={setBatchBusy}
              footerHost={batchFooterHost} active={mode === 'batch'}
              save={saveBatch} test={testBatch} onNotice={(message, failed) => toast(message, failed ? 'err' : 'ok')} />
          </div> : null}
          {!editing ? <ProxySponsorCards compact t={t} openExternal={openExternal} className='border-t border-hairline pt-3' /> : null}
        </DialogBody>
        <DialogFooter className='shrink-0 flex-wrap'>
          <Button variant='outline' className='mr-auto' disabled={busy} onClick={onClose}>{editing ? t('取消') : t('关闭')}</Button>
          {mode === 'single' ? <>
            <Button variant='outline' disabled={busy} onClick={() => void testDraft()}>
              {testing ? t('测试中…') : t('测试出口')}
            </Button>
            <Button disabled={busy} onClick={() => void save()}>
              {saving ? t('保存中…') : editing ? t('保存') : t('添加单个')}
            </Button>
          </> : null}
          {!editing ? <div ref={setBatchFooterHost} className={mode === 'batch' ? 'min-w-0' : 'hidden'} /> : null}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

/* ─── 连通性测试设置 ───────────────────────── */

export function ProxySettingsModal({ onClose }: { onClose: () => void }) {
  const [loading, setLoading] = React.useState(true)
  const [saving, setSaving] = React.useState(false)
  const [testUrl, setTestUrl] = React.useState('')
  const [defaultUrl, setDefaultUrl] = React.useState('')
  const [error, setError] = React.useState('')

  React.useEffect(() => {
    let alive = true
    void loadSettings().then(data => {
      if (!alive) return
      if (!data) throw new Error(t('代理设置响应异常'))
      setTestUrl(data.testUrl || '')
      setDefaultUrl(data.defaults?.testUrl || '')
    }).catch(cause => { if (alive) setError(errorMessage(cause)) })
      .finally(() => { if (alive) setLoading(false) })
    return () => { alive = false }
  }, [])

  async function save(): Promise<void> {
    if (saving || loading) return
    const url = testUrl.trim()
    if (url && !/^https?:\/\//i.test(url)) {
      setError(t('测试网址要以 http:// 或 https:// 开头'))
      return
    }
    setSaving(true)
    setError('')
    try {
      await saveSettings({ testUrl: url })
      toast(t('✅ 连通性测试设置已保存'))
      onClose()
    } catch (cause) {
      setError(errorMessage(cause))
    } finally { setSaving(false) }
  }

  return <Dialog open onOpenChange={open => { if (!open && !saving) onClose() }}>
    <DialogContent className='max-w-[500px]'>
      <DialogHeader><DialogTitle>{t('连通性测试设置')}</DialogTitle></DialogHeader>
      <DialogBody>
        <div className='flex flex-col gap-1.5'>
          <Label htmlFor='proxy-settings-url'>{t('测试目标网址')}</Label>
          <Input id='proxy-settings-url' value={testUrl} placeholder={defaultUrl} autoComplete='off' spellCheck={false}
            disabled={loading || saving} className='font-mono text-xs'
            onChange={event => setTestUrl(event.currentTarget.value)} />
        </div>
        <p className='text-xs leading-[1.8] text-subtle'>
          {t('通过代理访问该网址，检查是否能收到 HTTP 响应。留空沿用网关默认值。')}
        </p>
        {loading ? <p className='text-xs text-subtle'>{t('加载中…')}</p> : null}
        {error ? <p className='text-xs text-destructive'>❌ {error}</p> : null}
      </DialogBody>
      <DialogFooter>
        <Button variant='ghost' className='mr-auto' disabled={loading || saving} onClick={() => { setTestUrl(''); setError('') }}>{t('恢复默认')}</Button>
        <Button variant='outline' disabled={saving} onClick={onClose}>{t('取消')}</Button>
        <Button disabled={loading || saving} onClick={() => void save()}>{saving ? t('保存中…') : t('保存设置')}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}

/* ─── 删除确认弹窗 ───────────────────────── */

export type ProxyDeleteTarget = {
  mode: 'single' | 'batch'
  items: ProxyPoolItem[]
  skipped: number
}

export function ProxyDeleteDialog({
  target,
  onClose,
  onConfirm,
}: {
  target: ProxyDeleteTarget
  onClose: () => void
  onConfirm: (options: { resetUpdateProxy: boolean }) => Promise<void>
}) {
  const [resetUpdateProxy, setResetUpdateProxy] = React.useState(false)
  const [deleting, setDeleting] = React.useState(false)
  const isSingle = target.mode === 'single' && target.items.length === 1
  const single = isSingle ? target.items[0] : null
  const usedAccounts = single ? (single.usedBy || []) : []
  const usedProxyCount = target.items.filter(item => (item.usedBy || []).length > 0).length
  const services = [...new Set(target.items.flatMap(item => proxyServiceLabels(item, t)))]
  const hasUpdateService = target.items.some(item => (item.usedByServices || []).includes('update'))
  const previewNames = target.items.slice(0, 8).map(itemName).join(t('、'))
  const namesSummary = target.items.length > 8
    ? t('{names} 等 {n} 条', { names: previewNames, n: target.items.length })
    : previewNames

  async function submit(): Promise<void> {
    if (deleting) return
    setDeleting(true)
    try {
      await onConfirm({ resetUpdateProxy: hasUpdateService && resetUpdateProxy })
      onClose()
    } finally {
      setDeleting(false)
    }
  }

  return (
    <Dialog open onOpenChange={(open, details) => {
      if (open) return
      if (deleting) { details.cancel(); return }
      onClose()
    }}>
      <DialogContent className='max-w-[480px]'>
        <DialogHeader>
          <DialogTitle>{isSingle ? t('删除代理') : t('批量删除代理')}</DialogTitle>
        </DialogHeader>
        <DialogBody className='gap-3 text-[13px] leading-relaxed'>
          {single ? (
            <p>{t('确定删除「{name}」？', { name: itemName(single) })}</p>
          ) : (
            <>
              <p>{t('确定删除选中的 {n} 个代理？', { n: target.items.length })}</p>
              {namesSummary ? <p className='text-xs text-subtle break-words'>{namesSummary}</p> : null}
            </>
          )}
          {single && usedAccounts.length > 0 ? (
            <p className='text-xs text-subtle'>
              {t('有 {n} 个账号正在使用它（{names}）—— 删除后需重新设置这些账号的代理，不会自动直连。', {
                n: usedAccounts.length,
                names: usedAccounts.map(entry => String(entry.name || entry.id || '')).join(t('、')),
              })}
            </p>
          ) : null}
          {!single && usedProxyCount > 0 ? (
            <p className='text-xs text-subtle'>
              {t('其中 {n} 条正被账号使用 —— 删除后需重新设置相关账号的代理，不会自动直连。', { n: usedProxyCount })}
            </p>
          ) : null}
          {services.length > 0 ? (
            <p className='text-xs text-subtle'>
              {single
                ? t('此代理用于{services}；删除后这些线路将不可用，不会自动回退直连。', { services: services.join(t('、')) })
                : t('选中的代理用于{services}；删除后这些线路将不可用，不会自动回退直连。', { services: services.join(t('、')) })}
            </p>
          ) : null}
          {hasUpdateService ? (
            <Label className='flex cursor-pointer items-center gap-2 rounded-sm border border-border bg-surface-2 px-3 py-2 text-xs font-normal'>
              <Checkbox checked={resetUpdateProxy} disabled={deleting}
                onCheckedChange={value => setResetUpdateProxy(value === true)} />
              <span>{t('同时将「软件 / 插件更新」出网线路恢复为默认线路')}</span>
            </Label>
          ) : null}
          {target.skipped > 0 ? (
            <p className='text-xs text-subtle'>
              {t('另有 {n} 条正在与 EasyClash 同步，将跳过（请到 EasyClash 里删）。', { n: target.skipped })}
            </p>
          ) : null}
        </DialogBody>
        <DialogFooter>
          <Button variant='outline' disabled={deleting} onClick={onClose}>{t('取消')}</Button>
          <Button variant='destructive' disabled={deleting} onClick={() => void submit()}>
            {deleting ? t('删除中…') : t('删除')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
