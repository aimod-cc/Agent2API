import * as React from 'react'
import {
  Alert, Badge, Button, Card, CardContent, CardHeader, CardTitle, Input, Label, Switch, Textarea,
} from '@ui'
import { errorMessage, shared, toast } from './settings-model'
import { t } from '../i18n'

export type NotificationSettingsData = {
  enabled: boolean
  events: {
    loginSuccess: boolean
    loginFailure: boolean
    checkinFailure: boolean
  }
  channels: {
    webhook: {
      enabled: boolean
      url: string
      headersSet: boolean
      headerNames: string[]
    }
    telegram: {
      enabled: boolean
      botTokenSet: boolean
      chatId: string
      apiBase: string
      proxyUrl?: string
    }
    bark: {
      enabled: boolean
      deviceKeySet: boolean
      serverUrl: string
      group: string
      sound: string
    }
  }
}

export function NotificationSettingsPanel() {
  const [saved, setSaved] = React.useState<NotificationSettingsData | null>(null)
  const [busy, setBusy] = React.useState(false)
  const [testingChannel, setTestingChannel] = React.useState<string | null>(null)
  const [error, setError] = React.useState('')

  // Webhook 表单草稿
  const [webhookEnabled, setWebhookEnabled] = React.useState(false)
  const [webhookUrl, setWebhookUrl] = React.useState('')
  const [webhookHeadersText, setWebhookHeadersText] = React.useState('')

  // Telegram 表单草稿
  const [telegramEnabled, setTelegramEnabled] = React.useState(false)
  const [telegramToken, setTelegramToken] = React.useState('')
  const [telegramChatId, setTelegramChatId] = React.useState('')
  const [telegramApiBase, setTelegramApiBase] = React.useState('https://api.telegram.org')
  const [telegramProxyUrl, setTelegramProxyUrl] = React.useState('')

  // Bark 表单草稿
  const [barkEnabled, setBarkEnabled] = React.useState(false)
  const [barkDeviceKey, setBarkDeviceKey] = React.useState('')
  const [barkServerUrl, setBarkServerUrl] = React.useState('https://api.day.app')
  const [barkGroup, setBarkGroup] = React.useState('Agent2API')
  const [barkSound, setBarkSound] = React.useState('')

  const prefix = React.useId()

  const accept = React.useCallback((data: NotificationSettingsData | null | undefined, resetDrafts = true) => {
    if (!data || typeof data.enabled !== 'boolean' || !data.events || !data.channels) {
      throw new Error(t('未能读取通知设置'))
    }
    setSaved(data)
    if (resetDrafts) {
      setWebhookEnabled(data.channels.webhook.enabled)
      setWebhookUrl(data.channels.webhook.url || '')
      setWebhookHeadersText('')

      setTelegramEnabled(data.channels.telegram.enabled)
      setTelegramToken('')
      setTelegramChatId(data.channels.telegram.chatId || '')
      setTelegramApiBase(data.channels.telegram.apiBase || 'https://api.telegram.org')
      setTelegramProxyUrl(data.channels.telegram.proxyUrl || '')

      setBarkEnabled(data.channels.bark.enabled)
      setBarkDeviceKey('')
      setBarkServerUrl(data.channels.bark.serverUrl || 'https://api.day.app')
      setBarkGroup(data.channels.bark.group || 'Agent2API')
      setBarkSound(data.channels.bark.sound || '')
    }
  }, [])

  const load = React.useCallback(async () => {
    setBusy(true)
    setError('')
    try {
      const data = await shared().workbuddyDesktop?.getNotifications?.() as NotificationSettingsData | undefined
      accept(data, true)
    } catch (failure) {
      setError(errorMessage(failure))
    } finally {
      setBusy(false)
    }
  }, [accept])

  React.useEffect(() => {
    void load()
  }, [load])

  async function savePatch(patch: Record<string, unknown>, successMsg: string, resetDrafts = false) {
    setBusy(true)
    setError('')
    try {
      const result = await shared().workbuddyDesktop?.saveNotifications?.(patch) as NotificationSettingsData | undefined
      accept(result, resetDrafts)
      toast(successMsg, 'ok')
    } catch (failure) {
      const msg = errorMessage(failure)
      setError(msg)
      toast(msg, 'err')
    } finally {
      setBusy(false)
    }
  }

  function buildChannelDraft(channel: 'webhook' | 'telegram' | 'bark'): Record<string, unknown> {
    if (channel === 'webhook') {
      return {
        channels: {
          webhook: {
            enabled: webhookEnabled,
            url: webhookUrl.trim(),
            ...(webhookHeadersText.trim() ? { headers: webhookHeadersText.trim() } : {}),
          },
        },
      }
    }
    if (channel === 'telegram') {
      return {
        channels: {
          telegram: {
            enabled: telegramEnabled,
            chatId: telegramChatId.trim(),
            apiBase: telegramApiBase.trim() || 'https://api.telegram.org',
            proxyUrl: telegramProxyUrl.trim(),
            ...(telegramToken.trim() ? { botToken: telegramToken.trim() } : {}),
          },
        },
      }
    }
    return {
      channels: {
        bark: {
          enabled: barkEnabled,
          serverUrl: barkServerUrl.trim() || 'https://api.day.app',
          group: barkGroup.trim() || 'Agent2API',
          sound: barkSound.trim(),
          ...(barkDeviceKey.trim() ? { deviceKey: barkDeviceKey.trim() } : {}),
        },
      },
    }
  }

  async function testChannel(channel: 'webhook' | 'telegram' | 'bark') {
    setTestingChannel(channel)
    setError('')
    try {
      const draft = buildChannelDraft(channel)
      await shared().workbuddyDesktop?.testNotification?.({
        channel,
        ...draft,
      })
      toast(t('测试通知已发送'), 'ok')
    } catch (failure) {
      const msg = errorMessage(failure)
      setError(msg)
      toast(msg, 'err')
    } finally {
      setTestingChannel(null)
    }
  }

  const locked = busy || !saved

  return (
    <>
      <Card render={<section />} className='panel'>
        <CardHeader className='flex-row items-center justify-between gap-3'>
          <div className='flex items-center gap-2'>
            <CardTitle>{t('通知事件')}</CardTitle>
            {saved ? (
              <Badge variant={saved.enabled ? 'success' : 'outline'}>
                {saved.enabled ? t('已启用') : t('已关闭')}
              </Badge>
            ) : null}
          </div>
          <Button variant='ghost' size='sm' disabled={busy} onClick={() => void load()}>
            {t('刷新')}
          </Button>
        </CardHeader>
        <CardContent className='panel-body flex flex-col gap-3'>
          <div className='settings-switches'>
            <Label className='flex items-center justify-between gap-3' htmlFor={`${prefix}-master`}>
              <span>{t('启用系统通知')}</span>
              <Switch
                id={`${prefix}-master`}
                checked={saved?.enabled === true}
                disabled={locked}
                onCheckedChange={next => void savePatch({ enabled: next }, next ? t('系统通知已开启') : t('系统通知已关闭'))}
              />
            </Label>
            <Label className='flex items-center justify-between gap-3' htmlFor={`${prefix}-login-ok`}>
              <span>{t('登录成功通知')}</span>
              <Switch
                id={`${prefix}-login-ok`}
                checked={saved?.events.loginSuccess === true}
                disabled={locked}
                onCheckedChange={next =>
                  void savePatch({ events: { loginSuccess: next } }, t('通知事件已更新'))
                }
              />
            </Label>
            <Label className='flex items-center justify-between gap-3' htmlFor={`${prefix}-login-fail`}>
              <span>{t('登录失败与锁定告警')}</span>
              <Switch
                id={`${prefix}-login-fail`}
                checked={saved?.events.loginFailure === true}
                disabled={locked}
                onCheckedChange={next =>
                  void savePatch({ events: { loginFailure: next } }, t('通知事件已更新'))
                }
              />
            </Label>
            <Label className='flex items-center justify-between gap-3' htmlFor={`${prefix}-checkin-fail`}>
              <span>{t('自动签到失败通知')}</span>
              <Switch
                id={`${prefix}-checkin-fail`}
                checked={saved?.events.checkinFailure === true}
                disabled={locked}
                onCheckedChange={next =>
                  void savePatch({ events: { checkinFailure: next } }, t('通知事件已更新'))
                }
              />
            </Label>
          </div>
          <p className='m-0 text-xs text-muted-foreground'>
            {t('自动签到失败仅在定时轮次或自动补签出现失败账号时触发，手动点击签到不会发送通知。可同时开启多个下方通知源。')}
          </p>
          {error ? (
            <Alert variant='destructive' className='whitespace-pre-wrap break-words'>
              {error}
            </Alert>
          ) : null}
        </CardContent>
      </Card>

      {/* Webhook (通用 JSON) */}
      <Card render={<section />} className='panel'>
        <CardHeader className='flex-row items-center justify-between gap-3'>
          <div className='flex items-center gap-2'>
            <CardTitle>Webhook (JSON)</CardTitle>
            <Badge variant={saved?.channels.webhook.enabled ? 'success' : 'outline'}>
              {saved?.channels.webhook.enabled ? t('已启用') : t('未启用')}
            </Badge>
          </div>
          <div className='flex items-center gap-2'>
            <Switch
              id={`${prefix}-wh-enable`}
              checked={webhookEnabled}
              disabled={locked}
              onCheckedChange={setWebhookEnabled}
            />
          </div>
        </CardHeader>
        <CardContent className='panel-body flex flex-col gap-3'>
          <div className='space-y-1.5'>
            <Label htmlFor={`${prefix}-wh-url`}>{t('Webhook 地址')}</Label>
            <Input
              id={`${prefix}-wh-url`}
              value={webhookUrl}
              disabled={locked}
              placeholder='https://example.com/webhook'
              onChange={e => setWebhookUrl(e.target.value)}
            />
          </div>
          <div className='space-y-1.5'>
            <div className='flex items-center justify-between'>
              <Label htmlFor={`${prefix}-wh-headers`}>{t('自定义请求头（可选，每行 Header: Value）')}</Label>
              {saved?.channels.webhook.headersSet ? (
                <Button
                  variant='ghost'
                  size='sm'
                  disabled={locked}
                  onClick={() =>
                    void savePatch(
                      { channels: { webhook: { clearHeaders: true } } },
                      t('已清除自定义请求头'),
                      true,
                    )
                  }
                >
                  {t('清除已保存请求头')}
                </Button>
              ) : null}
            </div>
            <Textarea
              id={`${prefix}-wh-headers`}
              rows={2}
              value={webhookHeadersText}
              disabled={locked}
              placeholder={
                saved?.channels.webhook.headersSet
                  ? t('已保存（{names}），留空则不修改', {
                      names: saved.channels.webhook.headerNames.join(', ') || '…',
                    })
                  : 'Authorization: Bearer your-secret'
              }
              onChange={e => setWebhookHeadersText(e.target.value)}
            />
          </div>
          <div className='flex items-center justify-end gap-2'>
            <Button
              variant='outline'
              size='sm'
              disabled={locked || testingChannel !== null || !webhookUrl.trim()}
              onClick={() => void testChannel('webhook')}
            >
              {testingChannel === 'webhook' ? t('发送中…') : t('测试发送')}
            </Button>
            <Button
              size='sm'
              disabled={locked}
              onClick={() => void savePatch(buildChannelDraft('webhook'), t('Webhook 设置已保存'), true)}
            >
              {t('保存')}
            </Button>
          </div>
        </CardContent>
      </Card>

      {/* Telegram */}
      <Card render={<section />} className='panel'>
        <CardHeader className='flex-row items-center justify-between gap-3'>
          <div className='flex items-center gap-2'>
            <CardTitle>Telegram</CardTitle>
            <Badge variant={saved?.channels.telegram.enabled ? 'success' : 'outline'}>
              {saved?.channels.telegram.enabled ? t('已启用') : t('未启用')}
            </Badge>
          </div>
          <Switch
            id={`${prefix}-tg-enable`}
            checked={telegramEnabled}
            disabled={locked}
            onCheckedChange={setTelegramEnabled}
          />
        </CardHeader>
        <CardContent className='panel-body flex flex-col gap-3'>
          <div className='grid grid-cols-1 gap-3 sm:grid-cols-2'>
            <div className='space-y-1.5'>
              <Label htmlFor={`${prefix}-tg-token`}>Bot Token</Label>
              <Input
                id={`${prefix}-tg-token`}
                type='password'
                autoComplete='new-password'
                value={telegramToken}
                disabled={locked}
                placeholder={
                  saved?.channels.telegram.botTokenSet
                    ? t('已保存，留空则不修改')
                    : '123456:ABC-DEF...'
                }
                onChange={e => setTelegramToken(e.target.value)}
              />
            </div>
            <div className='space-y-1.5'>
              <Label htmlFor={`${prefix}-tg-chat`}>Chat ID</Label>
              <Input
                id={`${prefix}-tg-chat`}
                value={telegramChatId}
                disabled={locked}
                placeholder='123456789 或 -1001234567890'
                onChange={e => setTelegramChatId(e.target.value)}
              />
            </div>
          </div>
          <div className='grid grid-cols-1 gap-3 sm:grid-cols-2'>
            <div className='space-y-1.5'>
              <Label htmlFor={`${prefix}-tg-base`}>{t('接口地址')}</Label>
              <Input
                id={`${prefix}-tg-base`}
                value={telegramApiBase}
                disabled={locked}
                placeholder='https://api.telegram.org'
                onChange={e => setTelegramApiBase(e.target.value)}
              />
            </div>
            <div className='space-y-1.5'>
              <Label htmlFor={`${prefix}-tg-proxy`}>{t('代理地址（可选，留空跟随系统代理）')}</Label>
              <Input
                id={`${prefix}-tg-proxy`}
                value={telegramProxyUrl}
                disabled={locked}
                placeholder='http://127.0.0.1:7890 或 socks5://127.0.0.1:1080'
                onChange={e => setTelegramProxyUrl(e.target.value)}
              />
            </div>
          </div>
          <div className='flex items-center justify-end gap-2'>
            <Button
              variant='outline'
              size='sm'
              disabled={
                locked
                || testingChannel !== null
                || !telegramChatId.trim()
                || (!telegramToken.trim() && !saved?.channels.telegram.botTokenSet)
              }
              onClick={() => void testChannel('telegram')}
            >
              {testingChannel === 'telegram' ? t('发送中…') : t('测试发送')}
            </Button>
            <Button
              size='sm'
              disabled={locked}
              onClick={() => void savePatch(buildChannelDraft('telegram'), t('Telegram 设置已保存'), true)}
            >
              {t('保存')}
            </Button>
          </div>
        </CardContent>
      </Card>

      {/* Bark */}
      <Card render={<section />} className='panel'>
        <CardHeader className='flex-row items-center justify-between gap-3'>
          <div className='flex items-center gap-2'>
            <CardTitle>Bark</CardTitle>
            <Badge variant={saved?.channels.bark.enabled ? 'success' : 'outline'}>
              {saved?.channels.bark.enabled ? t('已启用') : t('未启用')}
            </Badge>
          </div>
          <Switch
            id={`${prefix}-bark-enable`}
            checked={barkEnabled}
            disabled={locked}
            onCheckedChange={setBarkEnabled}
          />
        </CardHeader>
        <CardContent className='panel-body flex flex-col gap-3'>
          <div className='grid grid-cols-1 gap-3 sm:grid-cols-2'>
            <div className='space-y-1.5'>
              <Label htmlFor={`${prefix}-bark-key`}>Device Key</Label>
              <Input
                id={`${prefix}-bark-key`}
                type='password'
                autoComplete='new-password'
                value={barkDeviceKey}
                disabled={locked}
                placeholder={
                  saved?.channels.bark.deviceKeySet
                    ? t('已保存，留空则不修改')
                    : t('输入 Bark 设备 Key')
                }
                onChange={e => setBarkDeviceKey(e.target.value)}
              />
            </div>
            <div className='space-y-1.5'>
              <Label htmlFor={`${prefix}-bark-server`}>{t('服务地址')}</Label>
              <Input
                id={`${prefix}-bark-server`}
                value={barkServerUrl}
                disabled={locked}
                placeholder='https://api.day.app'
                onChange={e => setBarkServerUrl(e.target.value)}
              />
            </div>
          </div>
          <div className='grid grid-cols-1 gap-3 sm:grid-cols-2'>
            <div className='space-y-1.5'>
              <Label htmlFor={`${prefix}-bark-group`}>{t('通知分组')}</Label>
              <Input
                id={`${prefix}-bark-group`}
                value={barkGroup}
                disabled={locked}
                placeholder='Agent2API'
                onChange={e => setBarkGroup(e.target.value)}
              />
            </div>
            <div className='space-y-1.5'>
              <Label htmlFor={`${prefix}-bark-sound`}>{t('提示音（可选）')}</Label>
              <Input
                id={`${prefix}-bark-sound`}
                value={barkSound}
                disabled={locked}
                placeholder={t('默认')}
                onChange={e => setBarkSound(e.target.value)}
              />
            </div>
          </div>
          <div className='flex items-center justify-end gap-2'>
            <Button
              variant='outline'
              size='sm'
              disabled={
                locked
                || testingChannel !== null
                || (!barkDeviceKey.trim() && !saved?.channels.bark.deviceKeySet)
              }
              onClick={() => void testChannel('bark')}
            >
              {testingChannel === 'bark' ? t('发送中…') : t('测试发送')}
            </Button>
            <Button
              size='sm'
              disabled={locked}
              onClick={() => void savePatch(buildChannelDraft('bark'), t('Bark 设置已保存'), true)}
            >
              {t('保存')}
            </Button>
          </div>
        </CardContent>
      </Card>
    </>
  )
}
