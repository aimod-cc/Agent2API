import { useEffect, useState } from 'react'
import {
  fetchOauthSettings,
  pollOauthBindState,
  saveOauthSettings,
  startOauthBind,
  type OauthSettingsData,
} from '../core/api'
import { apiBase, isPackaged, openExternalUrl } from '../core/servers'
import { store } from '../core/store'
import {
  BottomSheet,
  CommonCard,
  CommonSwitch,
  SectionHeader,
  TonalChip,
} from '../ui/components'

export function OauthSheet({ onClose }: { onClose: () => void }) {
  const [saved, setSaved] = useState<OauthSettingsData | null>(null)
  const [busy, setBusy] = useState(false)
  const [binding, setBinding] = useState<'google' | 'github' | null>(null)

  const [publicBaseUrl, setPublicBaseUrl] = useState('')
  const [gEnabled, setGEnabled] = useState(false)
  const [gId, setGId] = useState('')
  const [gSecret, setGSecret] = useState('')
  const [gEmails, setGEmails] = useState('')

  const [ghEnabled, setGhEnabled] = useState(false)
  const [ghId, setGhId] = useState('')
  const [ghSecret, setGhSecret] = useState('')
  const [ghUsers, setGhUsers] = useState('')

  const applyData = (data: OauthSettingsData) => {
    setSaved(data)
    setPublicBaseUrl(data.publicBaseUrl || '')
    setGEnabled(data.google.enabled)
    setGId(data.google.clientId || '')
    setGSecret('')
    setGEmails((data.google.allowedEmails || []).join('\n'))
    setGhEnabled(data.github.enabled)
    setGhId(data.github.clientId || '')
    setGhSecret('')
    setGhUsers((data.github.allowedUsers || []).join('\n'))
  }

  const load = () => {
    setBusy(true)
    fetchOauthSettings()
      .then(applyData)
      .catch(err => store.toast(err instanceof Error ? err.message : '加载设置失败', 'danger'))
      .finally(() => setBusy(false))
  }

  useEffect(() => {
    load()
  }, [])

  const origin = publicBaseUrl.trim().replace(/\/+$/, '') || (isPackaged() ? apiBase() : window.location.origin)

  const buildPatch = () => ({
    publicBaseUrl: publicBaseUrl.trim(),
    google: {
      enabled: gEnabled,
      clientId: gId.trim(),
      allowedEmails: gEmails.split(/[\n,，;；]/).map(s => s.trim()).filter(Boolean),
      ...(gSecret.trim() ? { clientSecret: gSecret.trim() } : {}),
    },
    github: {
      enabled: ghEnabled,
      clientId: ghId.trim(),
      allowedUsers: ghUsers.split(/[\n,，;；]/).map(s => s.trim()).filter(Boolean),
      ...(ghSecret.trim() ? { clientSecret: ghSecret.trim() } : {}),
    },
  })

  const handleSave = async (patch: Record<string, unknown>, msg = '第三方登录设置已保存') => {
    setBusy(true)
    try {
      const next = await saveOauthSettings(patch)
      applyData(next)
      store.toast(msg, 'success')
    } catch (err) {
      store.toast(err instanceof Error ? err.message : '保存失败', 'danger')
    } finally {
      setBusy(false)
    }
  }

  const handleBind = async (provider: 'google' | 'github') => {
    setBinding(provider)
    try {
      const { state, authorizeUrl } = await startOauthBind({
        provider,
        origin,
        patch: buildPatch(),
      })
      void openExternalUrl(authorizeUrl)
      const startedAt = Date.now()
      while (Date.now() - startedAt < 180_000) {
        await new Promise(resolve => setTimeout(resolve, 1200))
        const poll = await pollOauthBindState(state)
        if (poll.status === 'pending') continue
        if (poll.status === 'done') {
          load()
          store.toast(`已绑定 ${poll.provider || provider}：${poll.identity || ''}`, 'success')
          return
        }
        store.toast(poll.error || '绑定失败', 'danger')
        return
      }
      store.toast('绑定超时，请重试', 'danger')
    } catch (err) {
      store.toast(err instanceof Error ? err.message : '无法发起绑定', 'danger')
    } finally {
      setBinding(null)
    }
  }

  const locked = busy || !saved

  return (
    <BottomSheet open title='第三方登录 (OAuth)' onClose={onClose}>
      <SectionHeader title='回调与访问地址' />
      <CommonCard>
        <input
          className='fc-input'
          placeholder='面板外部访问地址（可选，如 https://panel.example.com）'
          value={publicBaseUrl}
          disabled={locked}
          onChange={e => setPublicBaseUrl(e.target.value)}
        />
        <div className='fc-mono' style={{ fontSize: 11.5, color: 'var(--fc-text-secondary)', wordBreak: 'break-all' }}>
          回调地址：{origin}/api/panel/oauth/callback
        </div>
      </CommonCard>

      <SectionHeader title='Google 登录' />
      <CommonCard>
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <span style={{ fontSize: 14.5, fontWeight: 700 }}>Google</span>
            <TonalChip tone={saved?.google.enabled ? 'success' : 'neutral'}>
              {saved?.google.enabled ? '已启用' : '未启用'}
            </TonalChip>
          </div>
          <CommonSwitch checked={gEnabled} disabled={locked} onChange={setGEnabled} />
        </div>
        <input
          className='fc-input'
          placeholder='Client ID'
          value={gId}
          disabled={locked}
          onChange={e => setGId(e.target.value)}
        />
        <input
          className='fc-input'
          type='password'
          placeholder={saved?.google.clientSecretSet ? 'Client Secret 已保存，留空不修改' : 'Client Secret'}
          value={gSecret}
          disabled={locked}
          onChange={e => setGSecret(e.target.value)}
        />
        <textarea
          className='fc-input'
          rows={2}
          placeholder='允许登录的 Google 邮箱（每行一个）'
          value={gEmails}
          disabled={locked}
          onChange={e => setGEmails(e.target.value)}
        />
        {saved?.google.boundAccounts.length ? (
          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6 }}>
            {saved.google.boundAccounts.map(acc => (
              <button
                key={acc.id}
                type='button'
                className='fc-btn is-sm is-pill'
                disabled={locked}
                onClick={() => void handleSave({ google: { unbindId: acc.id } }, '已解除 Google 绑定')}
              >
                {acc.email || acc.name || acc.id} ×
              </button>
            ))}
          </div>
        ) : null}
        <button
          type='button'
          className='fc-btn is-sm is-pill'
          disabled={locked || binding !== null || !gId.trim() || (!gSecret.trim() && !saved?.google.clientSecretSet)}
          onClick={() => void handleBind('google')}
        >
          {binding === 'google' ? '等待授权绑定…' : '跳转绑定当前 Google 账号'}
        </button>
      </CommonCard>

      <SectionHeader title='GitHub 登录' />
      <CommonCard>
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <span style={{ fontSize: 14.5, fontWeight: 700 }}>GitHub</span>
            <TonalChip tone={saved?.github.enabled ? 'success' : 'neutral'}>
              {saved?.github.enabled ? '已启用' : '未启用'}
            </TonalChip>
          </div>
          <CommonSwitch checked={ghEnabled} disabled={locked} onChange={setGhEnabled} />
        </div>
        <input
          className='fc-input'
          placeholder='Client ID'
          value={ghId}
          disabled={locked}
          onChange={e => setGhId(e.target.value)}
        />
        <input
          className='fc-input'
          type='password'
          placeholder={saved?.github.clientSecretSet ? 'Client Secret 已保存，留空不修改' : 'Client Secret'}
          value={ghSecret}
          disabled={locked}
          onChange={e => setGhSecret(e.target.value)}
        />
        <textarea
          className='fc-input'
          rows={2}
          placeholder='允许登录的 GitHub 用户名（每行一个）'
          value={ghUsers}
          disabled={locked}
          onChange={e => setGhUsers(e.target.value)}
        />
        {saved?.github.boundAccounts.length ? (
          <div style={{ display: 'flex', flexWrap: 'wrap', gap: 6 }}>
            {saved.github.boundAccounts.map(acc => (
              <button
                key={acc.id}
                type='button'
                className='fc-btn is-sm is-pill'
                disabled={locked}
                onClick={() => void handleSave({ github: { unbindId: acc.id } }, '已解除 GitHub 绑定')}
              >
                {acc.login || acc.email || acc.id} ×
              </button>
            ))}
          </div>
        ) : null}
        <button
          type='button'
          className='fc-btn is-sm is-pill'
          disabled={locked || binding !== null || !ghId.trim() || (!ghSecret.trim() && !saved?.github.clientSecretSet)}
          onClick={() => void handleBind('github')}
        >
          {binding === 'github' ? '等待授权绑定…' : '跳转绑定当前 GitHub 账号'}
        </button>
      </CommonCard>

      <button
        type='button'
        className='fc-btn is-primary'
        disabled={locked}
        onClick={() => void handleSave(buildPatch())}
      >
        {busy ? '保存中…' : '保存设置'}
      </button>
    </BottomSheet>
  )
}
