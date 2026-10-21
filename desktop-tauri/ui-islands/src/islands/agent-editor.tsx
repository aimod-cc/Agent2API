import * as React from 'react'
import { Badge, Button, Input, InputGroup, InputGroupAddon, InputGroupInput, SegmentedControl, Switch } from '@ui'
import { t } from '../i18n'
import {
  applyProfile, editingProfile, getAgentStore, importFromClient, refreshPreview, saveProfiles,
  startEditing, subscribeAgentStore, toast, type AgentClient, type AgentProfile,
} from './agent-state'
import { kindLabel, kindVariant, protocolLabel } from './agent-page'

/**
 * 配置列表 + 编辑器（「Agent」页的主体）。
 *
 * ── 开关的语义按 configMode 分叉（这一页最容易做错的一处）────────
 *   single  单选：打开一套就关掉其它 —— 因为文件里只能有一套生效。
 *           关掉当前那套会被拒绝（至少留一套），要换就打开另一套。
 *   multi   各自独立：ZCode 这类客户端自己就支持多条并存，三条同时开是
 *           正常用法（同一个网关的 Chat / Responses / Claude 三个入口）。
 *           启用中的非默认项多一个「设为默认」。
 *
 * ── 保存 vs 写入（两个动作，不合并）─────────────────────────
 * 「保存配置」只改我们的列表，「写入客户端」才动磁盘。合并会让「点一下
 * 开关就改了别人的配置文件」变得不可预期；分开后用户可以先配好几套、
 * 再决定写哪套。
 */

const PROTOCOLS = [
  { value: 'anthropic', label: 'Anthropic Messages' },
  { value: 'responses', label: 'OpenAI Responses' },
  { value: 'openai_chat', label: 'OpenAI Chat' },
  { value: 'gemini', label: 'Gemini' },
]

const EFFORTS = [
  { value: 'default', label: t('默认') },
  { value: 'low', label: t('低') },
  { value: 'medium', label: t('中') },
  { value: 'high', label: t('高') },
]

/** 新建一套时的默认值：指向本机网关（地址由后端按实际端口给出），最常用的那一档 */
function blankProfile(existing: AgentProfile[], gateway: string): AgentProfile {
  return {
    id: `p${Date.now().toString(36)}`,
    name: t('新配置'),
    kind: 'gateway',
    protocol: 'openai_chat',
    baseUrl: gateway ? `${gateway}/v1` : '',
    model: '',
    provider: '',
    effort: 'default',
    enabled: false,
    default: false,
    note: '',
  }
}

export function ProfileEditor({ client }: { client: AgentClient }) {
  const store = React.useSyncExternalStore(subscribeAgentStore, getAgentStore, getAgentStore)
  const multi = client.configMode === 'multi'
  const editing = editingProfile(client)
  // live 列表是客户端文件的只读快照：条目不在我们的 kv 里，改它只会 500。
  // 想让它们可编辑，先「从客户端导入」收进 kv。
  const readonly = client.profilesAreLive

  const save = async (profiles: AgentProfile[]) => {
    await saveProfiles(client.id, profiles)
  }

  const toggle = async (profile: AgentProfile, checked: boolean) => {
    const next = client.profiles.map(item => ({ ...item }))
    const target = next.find(item => item.id === profile.id)
    if (!target) return

    if (multi) {
      target.enabled = checked
      // 打开一条而当前没有任何默认项时，把它设为默认（客户端总得有个默认）
      if (checked && !next.some(item => item.enabled && item.default)) target.default = true
      // 关掉默认项时把默认位让给下一条启用的
      if (!checked && target.default) {
        target.default = false
        const fallback = next.find(item => item.enabled)
        if (fallback) fallback.default = true
      }
      await save(next)
      return
    }

    if (!checked) {
      // 单配置：不允许「一套都不生效」
      return
    }
    for (const item of next) {
      item.enabled = item.id === profile.id
      item.default = item.id === profile.id
    }
    await save(next)
  }

  const setDefault = async (profile: AgentProfile) => {
    const next = client.profiles.map(item => ({ ...item, default: item.id === profile.id }))
    await save(next)
  }

  const add = async () => {
    const created = blankProfile(client.profiles, store.gateway)
    // 单配置客户端新建的这套默认不生效（生效的仍是原来那套）
    const next = [...client.profiles, created]
    const ok = await saveProfiles(client.id, next)
    if (ok) startEditing(created.id)
  }

  // 把客户端文件里的现有配置收进「我的配置」。
  // 用户装好客户端后配置早就在它自己的文件里了，这一步省掉手工抄地址。
  const importOne = async () => {
    const count = await importFromClient(client.id, true)
    if (count === 0) {
      // 0 有两种可能：本来就没配过，或者条目已经都在我的配置里了。
      // 后端不区分这两种（都不算错误），这里统一给一句不误导的话。
      toast(t('没有可导入的新条目（可能还没配过，或已经在我的配置里）'), 'err')
    }
  }

  const remove = async (profile: AgentProfile) => {
    if (client.profiles.length <= 1) return
    const next = client.profiles.filter(item => item.id !== profile.id)
    // 删掉的若是唯一生效那套，把生效位让给剩下的第一条
    if (!next.some(item => item.enabled)) {
      next[0].enabled = true
      next[0].default = true
    }
    const ok = await saveProfiles(client.id, next)
    if (ok) startEditing(next[0].id)
  }

  return (
    <>
      <div className='agent-card'>
        <div className='agent-card-head'>
          <h4>{t('配置')}</h4>
          <span className='spacer' />
          <Button variant='ghost' size='sm' onClick={() => void importOne()}>
            {t('从客户端导入')}
          </Button>
          <Button variant='outline' size='sm' onClick={() => void add()}>
            ＋ {t('新建配置')}
          </Button>
        </div>

        {/* 从客户端文件读出来的：一行小字说明只读，不占整块 */}
        {client.profilesAreLive ? (
          <div className='agent-live-note'>{t('来自客户端文件，只读')}</div>
        ) : null}
        {client.readError ? (
          <div className='agent-live-error'>
            {t('读取客户端配置失败：{reason}', { reason: client.readError })}
          </div>
        ) : null}

        <div className='agent-profiles'>
          {client.profiles.length ? (
            client.profiles.map(profile => (
              <div
                key={profile.id}
                className={[
                  'agent-profile',
                  profile.enabled ? 'is-on' : '',
                  editing?.id === profile.id ? 'is-editing' : '',
                ]
                  .filter(Boolean)
                  .join(' ')}
              >
                <Switch
                  checked={profile.enabled}
                  disabled={readonly}
                  onCheckedChange={checked => void toggle(profile, checked)}
                  aria-label={t('启用 {name}', { name: profile.name })}
                />
                <div className='agent-profile-main'>
                  <div className='agent-profile-name'>
                    <span>{profile.name}</span>
                    <Badge variant={kindVariant(profile.kind)} className='agent-tag'>
                      {kindLabel(profile.kind)}
                    </Badge>
                    {multi && profile.enabled && profile.default ? (
                      <Badge variant='success' className='agent-tag'>
                        {t('默认')}
                      </Badge>
                    ) : null}
                  </div>
                  <div className='agent-profile-sub'>
                    <code>{profile.baseUrl || '—'}</code>
                    <span>·</span>
                    <span>{protocolLabel(profile.protocol)}</span>
                    {profile.model ? (
                      <>
                        <span>·</span>
                        <b>{profile.model}</b>
                      </>
                    ) : null}
                  </div>
                  {profile.note ? <div className='agent-profile-note'>{profile.note}</div> : null}
                </div>
                <div className='agent-profile-act'>
                  {multi && profile.enabled && !profile.default ? (
                    <Button variant='ghost' size='xs' onClick={() => void setDefault(profile)}>
                      {t('设为默认')}
                    </Button>
                  ) : null}
                  <Button
                    variant='ghost'
                    size='xs'
                    disabled={readonly}
                    onClick={() => startEditing(profile.id)}
                  >
                    {t('编辑')}
                  </Button>
                  <Button
                    variant='ghost'
                    size='xs'
                    disabled={readonly || client.profiles.length <= 1}
                    onClick={() => void remove(profile)}
                  >
                    {t('删除')}
                  </Button>
                </div>
              </div>
            ))
          ) : (
            <div className='empty'>{t('还没有配置')}</div>
          )}
        </div>
      </div>

      {editing && !readonly ? <ProfileForm client={client} profile={editing} /> : null}

      <div className='agent-actions'>
        <Button
          variant='default'
          size='sm'
          disabled={Boolean(store.busy) || !editing}
          onClick={() => void applyProfile(client.id, editing?.id)}
        >
          {store.busy === client.id
            ? t('写入中…')
            : t('写入客户端')}
        </Button>
        <Button
          variant='outline'
          size='sm'
          disabled={Boolean(store.busy)}
          onClick={() => void import('./agent-state').then(module => module.restoreClient(client.id))}
        >
          {t('按备份还原')}
        </Button>
        {client.needsRestart ? (
          <span className='agent-hint'>{t('需重开会话生效')}</span>
        ) : null}
      </div>
    </>
  )
}

/* ─── 编辑表单 ─────────────────────────────── */

function ProfileForm({ client, profile }: { client: AgentClient; profile: AgentProfile }) {
  /** 本地草稿：输入时不逐字保存（每次按键都发请求会很吵），失焦或点保存才落 */
  const [draft, setDraft] = React.useState<AgentProfile>(profile)
  const [dirty, setDirty] = React.useState(false)

  React.useEffect(() => {
    setDraft(profile)
    setDirty(false)
  }, [profile.id, profile.name, profile.baseUrl, profile.model, profile.protocol, profile.effort, profile.kind])

  const update = (patch: Partial<AgentProfile>) => {
    setDraft(current => ({ ...current, ...patch }))
    setDirty(true)
  }

  const commit = async () => {
    const next = client.profiles.map(item => (item.id === profile.id ? { ...draft } : item))
    const ok = await saveProfiles(client.id, next)
    if (ok) {
      setDirty(false)
      void refreshPreview(client.id, profile.id)
    }
  }

  return (
    <div className='agent-form'>
      <div className='agent-form-head'>
        <b>{t('编辑：{name}', { name: draft.name })}</b>
        <Badge variant={kindVariant(draft.kind)} className='agent-tag'>
          {kindLabel(draft.kind)}
        </Badge>
        <span className='spacer' />
        {dirty ? (
          <Button variant='default' size='xs' onClick={() => void commit()}>
            {t('保存修改')}
          </Button>
        ) : (
          <span className='agent-hint'>{t('已保存')}</span>
        )}
      </div>

      <Field label={t('名称')}>
        <Input
          value={draft.name}
          onChange={event => update({ name: event.currentTarget.value })}
          onBlur={() => void commit()}
        />
      </Field>

      <Field label={t('地址')}>
        <InputGroup className='agent-url-group'>
          <InputGroupInput
            value={draft.baseUrl}
            spellCheck={false}
            className='mono'
            onChange={event => update({ baseUrl: event.currentTarget.value })}
            onBlur={() => void commit()}
          />
          {draft.kind === 'gateway' ? <InputGroupAddon>{t('本机')}</InputGroupAddon> : null}
        </InputGroup>
      </Field>

      <Field label={t('来源')}>
        <SegmentedControl
          options={[
            { value: 'gateway', label: t('本机网关') },
            { value: 'official', label: t('官方直连') },
            { value: 'relay', label: t('中转') },
            { value: 'local', label: t('本机服务') },
          ]}
          value={draft.kind}
          onValueChange={value => update({ kind: value })}
          aria-label={t('配置来源')}
          className='shrink-0'
        />
      </Field>

      <Field label={t('对话协议')}>
        <SegmentedControl
          options={PROTOCOLS}
          value={draft.protocol}
          onValueChange={value => update({ protocol: value })}
          aria-label={t('对话协议')}
          className='shrink-0'
        />
      </Field>

      <Field label={t('模型')}>
        <Input
          value={draft.model}
          placeholder={t('模型 id')}
          spellCheck={false}
          className='mono'
          onChange={event => update({ model: event.currentTarget.value })}
          onBlur={() => void commit()}
        />
      </Field>

      <Field label={t('思考等级')}>
        <SegmentedControl
          options={EFFORTS}
          value={draft.effort || 'default'}
          onValueChange={value => update({ effort: value })}
          aria-label={t('思考等级')}
          className='shrink-0'
        />
      </Field>

      <Field label={t('备注')}>
        <Input
          value={draft.note}
          placeholder={t('备注')}
          onChange={event => update({ note: event.currentTarget.value })}
          onBlur={() => void commit()}
        />
      </Field>
    </div>
  )
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className='agent-field'>
      <label>{label}</label>
      <div className='agent-field-body'>{children}</div>
    </div>
  )
}
