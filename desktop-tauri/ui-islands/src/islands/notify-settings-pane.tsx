/**
 * 设置页 · 「通知与告警」分类的面板。
 *
 * ── 为什么放在设置页而不是一级菜单 ──────────────────────────
 * 它是**低频配置**：配一次、之后几个月不动。一级菜单是稀缺资源（用户每次
 * 切页都要扫过它们），低频配置项不该占位 —— 与「网关跨域开关」这类同处一页。
 * 出问题的**结果**（谁失效了、哪个任务失败了）在「账号」与「定时任务」页看，
 * 这一页只负责「怎么通知我」。
 *
 * ── 独立加载 / 保存（与 assistant-settings-pane 同款）────────
 * 走 `/api/notify` 那三条端点，与 settings-state 的 `/api/config` 快照是两条
 * 数据流，所以单独成文件、自带加载保存，不塞进 settings-page 的骨架里。
 *
 * ── 保存语义：本地草稿 + 显式保存 ───────────────────────────
 * 与设置页其它开关（改一下立刻存）**不同**，这里必须显式点「保存」。原因是
 * 渠道地址是**整条**才有意义的输入（打一半的 URL 存下去只会让渠道失效并报错），
 * 且用户常在「粘贴前先删掉旧值」的中间态停留 —— 即时保存会在那一刻把配置写坏。
 *
 * ── URL 的读写口径（与后端的脱敏对应）─────────────────────
 * 读回来的是**遮过 token** 的地址（后端 public_config 只回 `****1234` 那种）。
 * 界面上：
 *   · 没改过的渠道 → 原样回传遮过的值，后端**保留**原 URL；
 *   · 想改地址 → 整体重填（与密码同款：不提供「只改后半段」）。
 * 这条约定靠 `dirtyUrls` 集合实现：只有用户**动过**那一栏才把它算作「新值」。
 */

import * as React from 'react'
import { Button, Input, Label, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Switch } from '@ui'
import { t } from '../i18n'

/* ─── 类型 ─────────────────────────────────────────────── */

/** 一个渠道（界面上的一行）。`url` 是从后端读回的（可能被遮过） */
type Channel = {
  /** 本地稳定 key（后端不认；React 列表与 dirty 追踪用） */
  key: number
  name: string
  type: string
  url: string
  /** 用户是否手动改过 URL —— 改过才把它当新值提交（见文件头） */
  dirty: boolean
}

/** 后端回的可选事件 */
type NotifyKind = { id: string; label: string; description: string }

type NotifyConfig = {
  enabled: boolean
  events: string[]
  channels: Array<{ name: string; type: string; url: string; hasUrl: boolean }>
}

type Payload = { config: NotifyConfig; kinds: NotifyKind[] }

/* ─── 渠道类型选项（顺序即下拉顺序）─── */
const CHANNEL_TYPES = [
  { value: 'feishu', label: '飞书' },
  { value: 'wecom', label: '企业微信' },
  { value: 'dingtalk', label: '钉钉' },
  { value: 'generic', label: '通用 Webhook（纯文本 POST）' },
] as const

/** 各家的「怎么拿地址」提示（放在输入框下方；不知道去哪拿就配不出来） */
const TYPE_HINTS: Record<string, string> = {
  feishu: '飞书群 → 设置 → 群机器人 → 添加「自定义机器人」，复制 Webhook 地址。',
  wecom: '企业微信群 → 群机器人 → 添加机器人，复制 Webhook 地址。',
  dingtalk: '钉钉群 → 智能群助手 → 添加「自定义」机器人，复制 Webhook 地址。',
  generic: '任何接受 POST 纯文本的地址（自建接收端 / 别家机器人）。',
}

/* ─── 桥 ───────────────────────────────────────────────── */

type SharedWindow = {
  workbuddyDesktop?: {
    getNotify?: () => Promise<Payload | null | undefined>
    saveNotify?: (payload: unknown) => Promise<Payload | null | undefined>
    testNotify?: (payload: unknown) => Promise<{ results?: string[] } | null | undefined>
  }
  wbApp?: { toast?: (message: string, kind?: 'err' | 'ok') => void }
}

function shared(): SharedWindow {
  return window as unknown as SharedWindow
}

function toast(message: string, kind?: 'err' | 'ok'): void {
  shared().wbApp?.toast?.(message, kind)
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

/* ─── 小组件 ───────────────────────────────────────────── */

/** 一行的开关（与设置页其它面板同构） */
function SwitchLine({ id, label, note, checked, onCheckedChange }: {
  id: string
  label: string
  note?: string
  checked: boolean
  onCheckedChange: (value: boolean) => void
}) {
  return (
    <div className='field-row'>
      <div className='field-label'>
        <Label htmlFor={id}>{label}</Label>
        {note && <p className='field-note'>{note}</p>}
      </div>
      <Switch id={id} checked={checked} onCheckedChange={onCheckedChange} />
    </div>
  )
}

/* ─── 主面板 ───────────────────────────────────────────── */

export function NotifySettingsPane() {
  const [loaded, setLoaded] = React.useState(false)
  const [kinds, setKinds] = React.useState<NotifyKind[]>([])
  const [enabled, setEnabled] = React.useState(false)
  const [events, setEvents] = React.useState<string[]>([])
  const [channels, setChannels] = React.useState<Channel[]>([])
  const [busy, setBusy] = React.useState(false)
  const [testing, setTesting] = React.useState(false)
  const [error, setError] = React.useState('')
  const keyRef = React.useRef(1)

  /** 把后端形状摊成界面形状（每次加载都重置 dirty） */
  const applyData = React.useCallback((payload: Payload | null | undefined) => {
    if (!payload?.config) return
    setKinds(Array.isArray(payload.kinds) ? payload.kinds : [])
    setEnabled(Boolean(payload.config.enabled))
    setEvents(Array.isArray(payload.config.events) ? payload.config.events : [])
    setChannels((Array.isArray(payload.config.channels) ? payload.config.channels : []).map(item => ({
      key: keyRef.current++,
      name: String(item.name || ''),
      type: String(item.type || 'feishu'),
      url: String(item.url || ''),
      dirty: false,
    })))
  }, [])

  const load = React.useCallback(async () => {
    try {
      applyData(await shared().workbuddyDesktop?.getNotify?.())
      setError('')
    } catch (error) {
      setError(t('读取通知设置失败：{reason}', { reason: errorText(error) }))
    } finally {
      setLoaded(true)
    }
  }, [applyData])

  React.useEffect(() => { void load() }, [load])

  const patchChannel = (key: number, patch: Partial<Channel>) => {
    setChannels(prev => prev.map(item => (item.key === key ? { ...item, ...patch } : item)))
  }

  const addChannel = () => {
    setChannels(prev => [...prev, { key: keyRef.current++, name: '', type: 'feishu', url: '', dirty: true }])
  }

  const removeChannel = (key: number) => {
    setChannels(prev => prev.filter(item => item.key !== key))
  }

  /** 组装提交体：未改过的渠道回传**后端给的原值**（遮过的），
   *  这样后端能据此认出「没改」并保留真 URL */
  const buildPayload = () => ({
    enabled,
    events,
    channels: channels.map(channel => ({
      name: channel.name.trim(),
      type: channel.type,
      // dirty 为假时原样回传（可能带 ****），后端见到占位符就保留原值
      url: channel.url.trim(),
    })),
  })

  const save = async () => {
    if (busy) return
    setBusy(true)
    setError('')
    try {
      const next = await shared().workbuddyDesktop?.saveNotify?.(buildPayload())
      applyData(next)
      toast(t('✅ 通知设置已保存'))
    } catch (error) {
      const message = errorText(error)
      setError(message)
      toast(message, 'err')
    } finally {
      setBusy(false)
    }
  }

  const sendTest = async () => {
    if (testing) return
    setTesting(true)
    setError('')
    try {
      // 先保存再测：用户刚填的地址还没落盘，直接测会用到旧配置 —— 那是最容易
      // 让人困惑的一种「明明填对了却说没配」
      await shared().workbuddyDesktop?.saveNotify?.(buildPayload())
      const result = await shared().workbuddyDesktop?.testNotify?.({})
      const lines = Array.isArray(result?.results) ? result!.results! : []
      const failed = lines.filter(line => !line.includes('发送成功'))
      if (failed.length) {
        setError(failed.join('；'))
        toast(t('测试未全部成功，详见下方'), 'err')
      } else {
        toast(t('✅ 测试消息已发出，请查看对应群'))
      }
      void load()
    } catch (error) {
      const message = errorText(error)
      setError(message)
      toast(message, 'err')
    } finally {
      setTesting(false)
    }
  }

  if (!loaded) return <div className='panel'><div className='panel-body'>{t('正在加载…')}</div></div>

  return (
    <>
      <section className='panel'>
        <div className='panel-head'>
          <h2>{t('通知与告警')}</h2>
          <span className='panel-sub'>{t('出事了主动推给你（桌面端常驻托盘，不必自己盯着看）')}</span>
        </div>
        <div className='panel-body'>
          <SwitchLine id='notify-enabled' label={t('启用通知')}
            note={t('关掉时不发任何消息（配置保留）。默认关 —— 没配好就发是打扰。')}
            checked={enabled} onCheckedChange={setEnabled} />

          <div className='field-row'>
            <div className='field-label'>
              <Label>{t('订阅哪些事')}</Label>
              <p className='field-note'>{t('一个都不勾 = 全部订阅（配好渠道就能收到东西）')}</p>
            </div>
            <div className='notify-events'>
              {kinds.map(kind => (
                <label key={kind.id} className='notify-event' title={kind.description}>
                  <input type='checkbox' checked={events.includes(kind.id)}
                    onChange={event => setEvents(prev => event.target.checked
                      ? [...prev, kind.id]
                      : prev.filter(item => item !== kind.id))} />
                  {kind.label}
                </label>
              ))}
            </div>
          </div>
        </div>
      </section>

      <section className='panel'>
        <div className='panel-head'>
          <h2>{t('通知渠道')}</h2>
          <span className='panel-sub'>{t('按配置顺序逐个发送；一个失败不影响其它')}</span>
          <div className='head-actions'>
            <Button variant='outline' size='sm' onClick={addChannel}>{t('添加渠道')}</Button>
          </div>
        </div>
        <div className='panel-body'>
          {channels.length === 0
            ? <p className='field-note'>{t('还没有渠道。点右上角「添加渠道」，把群机器人的 Webhook 地址填进来。')}</p>
            : (
              <div className='notify-channels'>
                {channels.map(channel => (
                  <div key={channel.key} className='notify-channel'>
                    <div className='notify-channel-head'>
                      <Input className='notify-name' value={channel.name}
                        placeholder={t('备注名（可留空）')}
                        onChange={event => patchChannel(channel.key, { name: event.target.value })} />
                      <Select value={channel.type}
                        onValueChange={value => patchChannel(channel.key, { type: String(value) })}>
                        <SelectTrigger className='notify-type' aria-label={t('渠道类型')}>
                          <SelectValue>
                            {CHANNEL_TYPES.find(item => item.value === channel.type)?.label || channel.type}
                          </SelectValue>
                        </SelectTrigger>
                        <SelectContent>
                          {CHANNEL_TYPES.map(item => (
                            <SelectItem key={item.value} value={item.value}>{item.label}</SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                      <Button variant='ghost' size='sm' onClick={() => removeChannel(channel.key)}>{t('删除')}</Button>
                    </div>
                    <Input className='notify-url' value={channel.url}
                      placeholder={t('Webhook 地址（https://…）')}
                      onChange={event => patchChannel(channel.key, { url: event.target.value, dirty: true })} />
                    <p className='field-note'>{TYPE_HINTS[channel.type] || TYPE_HINTS.generic}</p>
                    {/* 提醒用户：读回来的是遮过的地址，要改就整体重填 */}
                    {!channel.dirty && channel.url.includes('****') && (
                      <p className='field-note'>{t('地址已遮住 token（保存时不动它）；要更换请整体重新粘贴')}</p>
                    )}
                  </div>
                ))}
              </div>
            )}

          {error && <p className='notify-error'>{error}</p>}

          <div className='notify-actions'>
            <Button size='sm' onClick={() => void save()} disabled={busy}>{busy ? t('保存中…') : t('保存')}</Button>
            <Button variant='outline' size='sm' onClick={() => void sendTest()}
              disabled={testing || channels.length === 0}>
              {testing ? t('发送中…') : t('发送测试')}
            </Button>
          </div>
        </div>
      </section>
    </>
  )
}
