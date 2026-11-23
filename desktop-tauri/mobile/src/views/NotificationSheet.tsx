import { useEffect, useState } from 'react'
import {
  fetchNotifications,
  saveNotifications,
  testNotificationChannel,
  type NotificationSettings,
} from '../core/api'
import { store } from '../core/store'
import {
  BottomSheet,
  CommonCard,
  CommonSwitch,
  ListGroup,
  ListItem,
  SectionHeader,
  TonalChip,
} from '../ui/components'

export function NotificationSheet({ onClose }: { onClose: () => void }) {
  const [saved, setSaved] = useState<NotificationSettings | null>(null)
  const [busy, setBusy] = useState(false)
  const [testing, setTesting] = useState<string | null>(null)

  const [whEnabled, setWhEnabled] = useState(false)
  const [whUrl, setWhUrl] = useState('')
  const [whHeaders, setWhHeaders] = useState('')

  const [tgEnabled, setTgEnabled] = useState(false)
  const [tgToken, setTgToken] = useState('')
  const [tgChatId, setTgChatId] = useState('')
  const [tgApiBase, setTgApiBase] = useState('https://api.telegram.org')
  const [tgProxyUrl, setTgProxyUrl] = useState('')

  const [barkEnabled, setBarkEnabled] = useState(false)
  const [barkKey, setBarkKey] = useState('')
  const [barkServer, setBarkServer] = useState('https://api.day.app')
  const [barkGroup, setBarkGroup] = useState('Agent2API')
  const [barkSound, setBarkSound] = useState('')

  const applyData = (data: NotificationSettings, resetDraft = true) => {
    setSaved(data)
    if (resetDraft) {
      setWhEnabled(data.channels.webhook.enabled)
      setWhUrl(data.channels.webhook.url || '')
      setWhHeaders('')

      setTgEnabled(data.channels.telegram.enabled)
      setTgToken('')
      setTgChatId(data.channels.telegram.chatId || '')
      setTgApiBase(data.channels.telegram.apiBase || 'https://api.telegram.org')
      setTgProxyUrl(data.channels.telegram.proxyUrl || '')

      setBarkEnabled(data.channels.bark.enabled)
      setBarkKey('')
      setBarkServer(data.channels.bark.serverUrl || 'https://api.day.app')
      setBarkGroup(data.channels.bark.group || 'Agent2API')
      setBarkSound(data.channels.bark.sound || '')
    }
  }

  useEffect(() => {
    const controller = new AbortController()
    setBusy(true)
    fetchNotifications(controller.signal)
      .then(data => applyData(data, true))
      .catch(err => {
        if (!controller.signal.aborted) {
          store.toast(err instanceof Error ? err.message : '加载通知设置失败', 'danger')
        }
      })
      .finally(() => setBusy(false))
    return () => controller.abort()
  }, [])

  const handlePatch = async (
    patch: Record<string, unknown>,
    toastText: string,
    resetDraft = false,
  ) => {
    setBusy(true)
    try {
      const next = await saveNotifications(patch)
      applyData(next, resetDraft)
      store.toast(toastText, 'success')
    } catch (err) {
      store.toast(err instanceof Error ? err.message : '保存失败', 'danger')
    } finally {
      setBusy(false)
    }
  }

  const buildDraft = (channel: 'webhook' | 'telegram' | 'bark') => {
    if (channel === 'webhook') {
      return {
        channels: {
          webhook: {
            enabled: whEnabled,
            url: whUrl.trim(),
            ...(whHeaders.trim() ? { headers: whHeaders.trim() } : {}),
          },
        },
      }
    }
    if (channel === 'telegram') {
      return {
        channels: {
          telegram: {
            enabled: tgEnabled,
            chatId: tgChatId.trim(),
            apiBase: tgApiBase.trim() || 'https://api.telegram.org',
            proxyUrl: tgProxyUrl.trim(),
            ...(tgToken.trim() ? { botToken: tgToken.trim() } : {}),
          },
        },
      }
    }
    return {
      channels: {
        bark: {
          enabled: barkEnabled,
          serverUrl: barkServer.trim() || 'https://api.day.app',
          group: barkGroup.trim() || 'Agent2API',
          sound: barkSound.trim(),
          ...(barkKey.trim() ? { deviceKey: barkKey.trim() } : {}),
        },
      },
    }
  }

  const handleTest = async (channel: 'webhook' | 'telegram' | 'bark') => {
    setTesting(channel)
    try {
      await testNotificationChannel({ channel, ...buildDraft(channel) })
      store.toast('测试通知已发送', 'success')
    } catch (err) {
      store.toast(err instanceof Error ? err.message : '测试发送失败', 'danger')
    } finally {
      setTesting(null)
    }
  }

  const locked = busy || !saved

  return (
    <BottomSheet open title='系统通知' onClose={onClose}>
      <SectionHeader title='通知与触发事件' />
      <ListGroup>
        <ListItem
          title='启用系统通知'
          trailing={
            <CommonSwitch
              checked={saved?.enabled === true}
              disabled={locked}
              onChange={next =>
                void handlePatch({ enabled: next }, next ? '系统通知已开启' : '系统通知已关闭')
              }
            />
          }
        />
        <ListItem
          title='登录成功通知'
          trailing={
            <CommonSwitch
              checked={saved?.events.loginSuccess === true}
              disabled={locked}
              onChange={next =>
                void handlePatch({ events: { loginSuccess: next } }, '通知事件已更新')
              }
            />
          }
        />
        <ListItem
          title='登录失败与锁定告警'
          trailing={
            <CommonSwitch
              checked={saved?.events.loginFailure === true}
              disabled={locked}
              onChange={next =>
                void handlePatch({ events: { loginFailure: next } }, '通知事件已更新')
              }
            />
          }
        />
        <ListItem
          title='自动签到失败通知'
          subtitle='仅定时或自动补签失败时触发，手动签到不通知'
          trailing={
            <CommonSwitch
              checked={saved?.events.checkinFailure === true}
              disabled={locked}
              onChange={next =>
                void handlePatch({ events: { checkinFailure: next } }, '通知事件已更新')
              }
            />
          }
        />
      </ListGroup>

      <SectionHeader title='Webhook（通用 JSON）' />
      <CommonCard>
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <span style={{ fontSize: 14.5, fontWeight: 700 }}>Webhook</span>
            <TonalChip tone={saved?.channels.webhook.enabled ? 'success' : 'neutral'}>
              {saved?.channels.webhook.enabled ? '已启用' : '未启用'}
            </TonalChip>
          </div>
          <CommonSwitch checked={whEnabled} disabled={locked} onChange={setWhEnabled} />
        </div>
        <input
          className='fc-input'
          placeholder='https://example.com/webhook'
          value={whUrl}
          disabled={locked}
          onChange={e => setWhUrl(e.target.value)}
        />
        <textarea
          className='fc-input'
          rows={2}
          placeholder={
            saved?.channels.webhook.headersSet
              ? `已保存请求头（${saved.channels.webhook.headerNames.join(', ') || '…'}），留空不修改`
              : '自定义请求头（可选，每行 Header: Value）'
          }
          value={whHeaders}
          disabled={locked}
          onChange={e => setWhHeaders(e.target.value)}
        />
        <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8 }}>
          <button
            type='button'
            className='fc-btn is-sm is-pill'
            disabled={locked || testing !== null || !whUrl.trim()}
            onClick={() => void handleTest('webhook')}
          >
            {testing === 'webhook' ? '发送中…' : '测试发送'}
          </button>
          <button
            type='button'
            className='fc-btn is-primary is-sm is-pill'
            disabled={locked}
            onClick={() => void handlePatch(buildDraft('webhook'), 'Webhook 设置已保存', true)}
          >
            保存
          </button>
        </div>
      </CommonCard>

      <SectionHeader title='Telegram' />
      <CommonCard>
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <span style={{ fontSize: 14.5, fontWeight: 700 }}>Telegram</span>
            <TonalChip tone={saved?.channels.telegram.enabled ? 'success' : 'neutral'}>
              {saved?.channels.telegram.enabled ? '已启用' : '未启用'}
            </TonalChip>
          </div>
          <CommonSwitch checked={tgEnabled} disabled={locked} onChange={setTgEnabled} />
        </div>
        <input
          className='fc-input'
          type='password'
          placeholder={
            saved?.channels.telegram.botTokenSet
              ? 'Bot Token 已保存，留空不修改'
              : 'Bot Token (123456:ABC...)'
          }
          value={tgToken}
          disabled={locked}
          onChange={e => setTgToken(e.target.value)}
        />
        <input
          className='fc-input'
          placeholder='Chat ID'
          value={tgChatId}
          disabled={locked}
          onChange={e => setTgChatId(e.target.value)}
        />
        <input
          className='fc-input'
          placeholder='https://api.telegram.org'
          value={tgApiBase}
          disabled={locked}
          onChange={e => setTgApiBase(e.target.value)}
        />
        <input
          className='fc-input'
          placeholder='代理地址（可选，如 http://127.0.0.1:7890）'
          value={tgProxyUrl}
          disabled={locked}
          onChange={e => setTgProxyUrl(e.target.value)}
        />
        <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8 }}>
          <button
            type='button'
            className='fc-btn is-sm is-pill'
            disabled={
              locked
              || testing !== null
              || !tgChatId.trim()
              || (!tgToken.trim() && !saved?.channels.telegram.botTokenSet)
            }
            onClick={() => void handleTest('telegram')}
          >
            {testing === 'telegram' ? '发送中…' : '测试发送'}
          </button>
          <button
            type='button'
            className='fc-btn is-primary is-sm is-pill'
            disabled={locked}
            onClick={() => void handlePatch(buildDraft('telegram'), 'Telegram 设置已保存', true)}
          >
            保存
          </button>
        </div>
      </CommonCard>

      <SectionHeader title='Bark' />
      <CommonCard>
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <span style={{ fontSize: 14.5, fontWeight: 700 }}>Bark</span>
            <TonalChip tone={saved?.channels.bark.enabled ? 'success' : 'neutral'}>
              {saved?.channels.bark.enabled ? '已启用' : '未启用'}
            </TonalChip>
          </div>
          <CommonSwitch checked={barkEnabled} disabled={locked} onChange={setBarkEnabled} />
        </div>
        <input
          className='fc-input'
          type='password'
          placeholder={
            saved?.channels.bark.deviceKeySet
              ? 'Device Key 已保存，留空不修改'
              : 'Device Key'
          }
          value={barkKey}
          disabled={locked}
          onChange={e => setBarkKey(e.target.value)}
        />
        <input
          className='fc-input'
          placeholder='https://api.day.app'
          value={barkServer}
          disabled={locked}
          onChange={e => setBarkServer(e.target.value)}
        />
        <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 8 }}>
          <input
            className='fc-input'
            placeholder='通知分组（默认 Agent2API）'
            value={barkGroup}
            disabled={locked}
            onChange={e => setBarkGroup(e.target.value)}
          />
          <input
            className='fc-input'
            placeholder='提示音（可选）'
            value={barkSound}
            disabled={locked}
            onChange={e => setBarkSound(e.target.value)}
          />
        </div>
        <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8 }}>
          <button
            type='button'
            className='fc-btn is-sm is-pill'
            disabled={
              locked
              || testing !== null
              || (!barkKey.trim() && !saved?.channels.bark.deviceKeySet)
            }
            onClick={() => void handleTest('bark')}
          >
            {testing === 'bark' ? '发送中…' : '测试发送'}
          </button>
          <button
            type='button'
            className='fc-btn is-primary is-sm is-pill'
            disabled={locked}
            onClick={() => void handlePatch(buildDraft('bark'), 'Bark 设置已保存', true)}
          >
            保存
          </button>
        </div>
      </CommonCard>
    </BottomSheet>
  )
}
