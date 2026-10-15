/**
 * Agent2API · 网络代理页的两个配置弹窗：代理条目表单与代理设置。
 *
 * 从 proxies-page.tsx 拆出来：单表改版后（订阅收进「订阅管理」弹窗）页面文件
 * 重新长到项目约定的体量上限附近，而这两个弹窗与页面骨架之间只靠 props 往来、
 * 不共享任何局部状态 —— 边界干净，搬走不影响任何调用点（导入名一字未改）。
 *
 * ── 两处校验的分工（与后端同一口径）────────────────────────────
 * 弹窗先把「明显写错」挡住（端口 1-65535、起≤止、必填项），后端 `proxy_pool` /
 * `proxy_settings` 再校一次，400 的错误文案原样显示在弹窗的 status 行。
 *
 * ── 数据读取时机 ──────────────────────────────────────────────
 * 代理设置是**打开时读一次**的本地快照（低频配置，不进全局 store）；
 * 代理表单「测试出口」是**未保存也能测**的直接请求（先确认通，再决定保存）。
 */

import * as React from 'react'
import {
  Button,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Input,
  Label,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Switch,
} from '@ui'
import {
  durationText, errorMessage, loadSettings, saveItem, saveSettings, toast, wb,
  type ProxyPoolItem, type ProxySettings,
} from './proxies-state'
import { t } from '../i18n'

/* ─── 新增 / 编辑弹窗 ───────────────────────── */

/**
 * 新增 / 编辑**手动**条目。
 *
 * 为什么不在这里提供「Clash Verge 出口」一档：Clash 那类由同步整体镜像进来
 * （见 proxies-page.tsx 模块头），是只读记录 —— 手填一条「不属于同步集合」的
 * Clash 引用会让下一次同步把它摘掉，等于用户白填一次。要引用 Clash 出口就点
 * 工具条的「同步 Clash Verge」，一次把当前出口集合全部导入。
 */
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
  const [saving, setSaving] = React.useState(false)
  const [testing, setTesting] = React.useState(false)
  const [status, setStatus] = React.useState<React.ReactNode>(null)

  /** 表单 → 归一化后的提交对象；形状非法时返回一句面向用户的错误 */
  function draftPayload(): { payload: Record<string, unknown> } | { error: string } {
    const trimmed = name.trim()
    if (!trimmed) return { error: t('请填写代理名称') }
    const hostText = host.trim()
    if (!hostText) return { error: t('请填写代理主机地址') }
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
        username: username.trim(),
        password,
        enabled,
      },
    }
  }

  /** 用当前表单内容测一次（**未保存也能测**：先确认能通，再决定保存） */
  async function testDraft(): Promise<void> {
    if (testing) return
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
    if (saving) return
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
      onClose()
    } catch (error) {
      setStatus(<span className='text-destructive'>❌ {errorMessage(error)}</span>)
      setSaving(false)
    }
  }

  return (
    <Dialog open onOpenChange={open => { if (!open && !saving) onClose() }}>
      <DialogContent className='max-w-[560px]'>
        <DialogHeader>
          <DialogTitle>{editing ? t('编辑代理') : t('新增代理')}</DialogTitle>
        </DialogHeader>
        <DialogBody className='flex flex-col gap-3.5'>
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
          <div className='flex flex-row items-end gap-2.5'>
            <div className='flex min-w-0 flex-1 flex-col gap-1.5'>
              <Label htmlFor='proxy-user'>{t('用户名')}</Label>
              <Input id='proxy-user' placeholder={t('可选')} autoComplete='off'
                value={username} onChange={event => setUsername(event.currentTarget.value)} />
            </div>
            <div className='flex min-w-0 flex-1 flex-col gap-1.5'>
              <Label htmlFor='proxy-pass'>{t('密码')}</Label>
              <Input id='proxy-pass' placeholder={t('可选')} autoComplete='off'
                value={password} onChange={event => setPassword(event.currentTarget.value)} />
            </div>
          </div>

          <div className='flex flex-row items-center gap-2.5'>
            <Switch checked={enabled} onCheckedChange={setEnabled} aria-label={t('启用这条代理')} />
            <span className='text-[12.5px]'>
              {t('启用（禁用的条目不会被账号使用，被引用时账号回退直连；仍可测试）')}
            </span>
          </div>

          <p className='text-xs leading-[1.65] text-subtle'>
            {t('想要 Clash Verge 的出口？那类条目由工具条的')}<b>{t('「同步 Clash Verge」')}</b>{t('整体 镜像进来（进本页时也会自动同步一次），名称与端口跟随 Clash、因此是只读的 —— 不必（也不能）在这里手工填。')}
          </p>

          <div className='min-h-[18px] text-xs text-subtle'>{status}</div>
        </DialogBody>
        <DialogFooter>
          <Button variant='outline' disabled={testing || saving} onClick={() => void testDraft()}>
            {testing ? t('测试中…') : t('测试出口')}
          </Button>
          <div className='mr-auto' />
          <Button variant='outline' disabled={saving} onClick={onClose}>{t('取消')}</Button>
          <Button variant='default' disabled={saving} onClick={() => void save()}>
            {saving ? t('保存中…') : t('保存')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

/* ─── 设置弹窗 ─────────────────────────────── */

/**
 * 代理设置弹窗：节点监听端口范围 + 出口连通性测试网址。
 *
 * 数据是**打开时读一次**的本地快照（低频配置，不值得进全局 store）：
 * 打开 → loadSettings 回填表单 → 改 → 保存。保存成功后不自动重启内核 ——
 * 已注册的节点端口沿用注册表、新分配才受新范围约束（见
 * `core::proxy_settings` 的模块头），所以只提示「重启内核后新节点按新范围
 * 分配」，重启入口在页面顶部的内核状态条。
 */
export function ProxySettingsModal({ onClose }: { onClose: () => void }) {
  const [loading, setLoading] = React.useState(true)
  const [saving, setSaving] = React.useState(false)
  const [base, setBase] = React.useState('')
  const [end, setEnd] = React.useState('')
  const [testUrl, setTestUrl] = React.useState('')
  const [defaults, setDefaults] = React.useState<NonNullable<ProxySettings['defaults']>>({})
  const [error, setError] = React.useState('')
  /** 保存成功后的一条常驻提示（告知内核重启的口径，不算错误） */
  const [saved, setSaved] = React.useState(false)

  React.useEffect(() => {
    let alive = true
    void (async () => {
      try {
        const data = await loadSettings()
        if (!alive || data === null) return
        setBase(String(data.listenerPortBase ?? ''))
        setEnd(String(data.listenerPortEnd ?? ''))
        setTestUrl(String(data.testUrl ?? ''))
        setDefaults(data.defaults ?? {})
        setLoading(false)
      } catch (loadError) {
        if (!alive) return
        setError(errorMessage(loadError))
        setLoading(false)
      }
    })()
    return () => { alive = false }
  }, [])

  /** 表单 → 提交 payload；形状非法时返回 null（错误写进 status 行） */
  function draftPayload(): { listenerPortBase: number; listenerPortEnd: number; testUrl: string } | null {
    const baseNumber = Number(base)
    const endNumber = Number(end)
    if (!Number.isInteger(baseNumber) || baseNumber < 1 || baseNumber > 65535
      || !Number.isInteger(endNumber) || endNumber < 1 || endNumber > 65535) {
      setError(t('端口范围必须是 1-65535 的整数'))
      return null
    }
    if (baseNumber > endNumber) {
      setError(t('起始端口不能大于结束端口'))
      return null
    }
    const url = testUrl.trim()
    if (url && !url.startsWith('http://') && !url.startsWith('https://')) {
      setError(t('测试网址要以 http:// 或 https:// 开头'))
      return null
    }
    setError('')
    return { listenerPortBase: baseNumber, listenerPortEnd: endNumber, testUrl: url }
  }

  async function save(): Promise<void> {
    if (saving || loading) return
    const payload = draftPayload()
    if (!payload) return
    setSaving(true)
    try {
      await saveSettings(payload)
      setSaved(true)
    } catch (saveError) {
      setError(errorMessage(saveError))
    } finally {
      setSaving(false)
    }
  }

  /** 恢复默认：把默认值回填进表单（还不是「保存」—— 用户确认后一起提交） */
  function resetToDefaults(): void {
    setBase(String(defaults.listenerPortBase ?? ''))
    setEnd(String(defaults.listenerPortEnd ?? ''))
    setTestUrl(String(defaults.testUrl ?? ''))
    setSaved(false)
    setError('')
  }

  return (
    <Dialog open onOpenChange={open => { if (!open && !saving) onClose() }}>
      <DialogContent className='max-w-[560px]'>
        <DialogHeader>
          <DialogTitle>{t('代理设置')}</DialogTitle>
        </DialogHeader>
        <DialogBody className='flex flex-col gap-3.5'>
          {loading ? (
            <p className='text-xs text-subtle'>{t('加载中…')}</p>
          ) : (
            <>
              <div className='flex flex-row items-end gap-2.5'>
                <div className='flex min-w-0 flex-1 flex-col gap-1.5'>
                  <Label htmlFor='proxy-settings-base'>{t('节点端口范围（起始）')}</Label>
                  <Input id='proxy-settings-base' type='number' min={1} max={65535}
                    placeholder={String(defaults.listenerPortBase ?? '')} autoComplete='off'
                    value={base} onChange={event => { setBase(event.currentTarget.value); setSaved(false) }} />
                </div>
                <span aria-hidden='true' className='flex-none pb-[7px] text-subtle'>—</span>
                <div className='flex min-w-0 flex-1 flex-col gap-1.5'>
                  <Label htmlFor='proxy-settings-end'>{t('节点端口范围（结束）')}</Label>
                  <Input id='proxy-settings-end' type='number' min={1} max={65535}
                    placeholder={String(defaults.listenerPortEnd ?? '')} autoComplete='off'
                    value={end} onChange={event => { setEnd(event.currentTarget.value); setSaved(false) }} />
                </div>
              </div>
              <p className='text-xs leading-[1.65] text-subtle'>
                {t('订阅节点会在本机各占一个监听端口，从该范围里挑空闲的分配；节点多的大订阅把默认范围（{base}-{end}）用尽时会报「没有可用的节点端口」，这时把范围调大再重启内核。已分配出去的端口不会因改范围而失效。', {
                  base: defaults.listenerPortBase ?? '',
                  end: defaults.listenerPortEnd ?? '',
                })}
              </p>

              <div className='flex flex-col gap-1.5'>
                <Label htmlFor='proxy-settings-url'>{t('出口测试网址')}</Label>
                <Input id='proxy-settings-url' placeholder={defaults.testUrl || ''}
                  autoComplete='off' spellCheck={false} className='font-mono text-[12px]'
                  value={testUrl} onChange={event => { setTestUrl(event.currentTarget.value); setSaved(false) }} />
              </div>
              <p className='text-xs leading-[1.65] text-subtle'>
                {t('「测试」与「批量测试」访问的目标；留空用默认（打 WorkBuddy 上游）。判据是能否拿到 HTTP 响应，不用考虑该网址返回什么内容。')}
              </p>

              <div className='min-h-[18px] text-xs'>
                {error ? <span className='text-destructive'>❌ {error}</span> : null}
                {!error && saved ? (
                  <span className='text-success'>
                    ✅ {t('已保存。重启内核后，新分配的节点端口才会按新范围来。')}
                  </span>
                ) : null}
              </div>
            </>
          )}
        </DialogBody>
        <DialogFooter>
          <Button variant='ghost' disabled={loading || saving} onClick={resetToDefaults}>
            {t('恢复默认')}
          </Button>
          <div className='mr-auto' />
          <Button variant='outline' disabled={saving} onClick={onClose}>{t('关闭')}</Button>
          <Button variant='default' disabled={loading || saving} onClick={() => void save()}>
            {saving ? t('保存中…') : t('保存')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
