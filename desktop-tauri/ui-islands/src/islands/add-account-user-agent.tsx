/**
 * 「添加账号」与「账号设置」共用的自定义 User-Agent 输入件。
 *
 * ── 预置从哪来 ──────────────────────────────────────────
 * 设置 → 网关 → 客户端识别 的那 20 个内置客户端（后端 `client::catalog`），
 * 每一条带一份代表性的 UA。取的是同一个接口（GET /api/client-identification），
 * 不在前端另抄一份清单 —— 否则两边新增一家就会漂移。
 * 顺序由后端决定：Claude Code / Codex / ZCode 固定在前三个。
 *
 * ── 为什么输入框 + 下拉而不是二选一 ─────────────────────
 * 「选预置」只是把一份常用的值填进输入框，填完仍可改版本号；「手动填写」也
 * 不必先切模式。下拉里当前值如果正好等于某个预置，就回显那个预置名，否则
 * 显示「选择预置客户端」—— 不给「当前是自定义」单独做状态，少一处要同步的东西。
 */

import * as React from 'react'
import { Input } from '@ui'
import { SearchableSelect } from '../../../ui-shared/searchable-select'
import { t } from '../i18n'

export type UserAgentPreset = { id: string; name: string; ua: string }

type IdentificationBridge = {
  getClientIdentification?: () => Promise<{
    builtins?: Array<{ id?: unknown; name?: unknown; ua?: unknown }>
  }>
}

/** 模块级缓存：同一个弹窗里每家一段表单，不该各发一次请求；失败也不反复重试 */
let presetsCache: UserAgentPreset[] | null = null
let presetsPending: Promise<UserAgentPreset[]> | null = null

function loadPresets(): Promise<UserAgentPreset[]> {
  if (presetsCache) return Promise.resolve(presetsCache)
  if (presetsPending) return presetsPending
  const bridge = (window as unknown as { workbuddyDesktop?: IdentificationBridge }).workbuddyDesktop
  const request = bridge?.getClientIdentification?.()
  if (!request) {
    presetsCache = []
    return Promise.resolve(presetsCache)
  }
  presetsPending = request
    .then(view => {
      const list: UserAgentPreset[] = []
      for (const item of view?.builtins || []) {
        const id = typeof item.id === 'string' ? item.id : ''
        const name = typeof item.name === 'string' ? item.name : ''
        const ua = typeof item.ua === 'string' ? item.ua.trim() : ''
        if (id && name && ua) list.push({ id, name, ua })
      }
      presetsCache = list
      return list
    })
    .catch(() => {
      // 旧版后端没有这个桥 / 接口暂时不可用时，只保留手动输入
      presetsCache = []
      return presetsCache
    })
    .finally(() => { presetsPending = null })
  return presetsPending
}

/** 预置清单（异步；未就绪时返回空数组，输入框照常可手填） */
export function useUserAgentPresets(): UserAgentPreset[] {
  const [presets, setPresets] = React.useState<UserAgentPreset[]>(() => presetsCache || [])
  React.useEffect(() => {
    if (presetsCache) return
    let alive = true
    void loadPresets().then(list => { if (alive) setPresets(list) })
    return () => { alive = false }
  }, [])
  return presets
}

/**
 * 这家提供商的 UA 能不能被改写（读后端摘要的 `userAgentOverridable`）。
 *
 * 未返回能力位时保留旧版界面的行为；新版后端按执行体实际能力判断，
 * 只有全部上游请求支持账号 UA 覆盖时才显示输入框。
 */
export function userAgentEditable(provider: string): boolean {
  const bridge = window as unknown as {
    wbProviders?: { all?: () => Array<{ id?: string; userAgentOverridable?: boolean }> }
    wbApp?: { getState?: () => { accounts?: { providers?: Array<{ id?: string; userAgentOverridable?: boolean }> } } | null | undefined }
  }
  const list = bridge.wbProviders?.all?.()
    || bridge.wbApp?.getState?.()?.accounts?.providers
  if (!Array.isArray(list)) return true
  const found = list.find(item => item?.id === provider)
  return found?.userAgentOverridable !== false
}

/**
 * 「网页登录」这类**由共享登录桥建号**的路径用不了请求体里的 `userAgent`
 * （账号是桥那边建的，前端只收到结果）。所以建号成功后补一条 PATCH ——
 * 与「账号设置」走同一个入口，校验与清除语义都只有一处。
 *
 * 失败只提示、不回滚账号：账号已经建好了，UA 只是它的一个属性，
 * 用户可以到账号设置里再改一次。
 */
export async function applyUserAgentAfterCreate(
  accountId: string | undefined,
  provider: string,
  userAgent: string,
): Promise<void> {
  const value = userAgent.trim()
  if (!value || !accountId) return
  if (!userAgentEditable(provider)) return
  const internals = (window as unknown as {
    __TAURI_INTERNALS__?: { invoke?: (cmd: string, args: unknown) => Promise<unknown> }
  }).__TAURI_INTERNALS__
  if (!internals?.invoke) return
  try {
    await internals.invoke('api_request', {
      request: {
        method: 'PATCH',
        path: `/api/accounts/${encodeURIComponent(accountId)}`,
        body: { userAgent: value },
      },
    })
  } catch {
    // 静默：调用方随后的 toast 已经说了「账号已添加」，再叠一条失败会让人以为没加成
  }
}

/**
 * 网页登录 / OAuth 那条路建号成功后，账号 id 可能只在 `session` 里
 * （`accountUid` / `accountId`，见 `normalizeLoginResult`）。
 * 这里按「account.id → session.accountId → session.accountUid」的顺序取，
 * 与 `saveAccountProxySelection` 同一口径 —— 两条收尾补写不能各写一套。
 */
export function createdAccountIdOf(result: unknown): string | undefined {
  const record = result as { account?: { id?: unknown }; session?: Record<string, unknown> } | null
  const direct = record?.account?.id
  if (typeof direct === 'string' && direct) return direct
  for (const key of ['accountId', 'accountUid']) {
    const value = record?.session?.[key]
    if (typeof value === 'string' && value) return value
  }
  return undefined
}

/**
 * 输入框 + 预置下拉。`id` 给输入框（Label 的 htmlFor 指它，与其它字段同一手法）。
 * 布局交给调用方：这里只管一行「输入框吃剩余、下拉定宽」。
 */
export function UserAgentInput({
  id,
  value,
  onChange,
  presets,
  disabled,
  className,
}: {
  id: string
  value: string
  onChange: (value: string) => void
  presets: UserAgentPreset[]
  disabled?: boolean
  className?: string
}): React.ReactElement {
  const matched = presets.find(preset => preset.ua === value)
  return (
    <div className={`flex min-w-0 items-center gap-2 ${className || ''}`}>
      <Input
        id={id}
        className='min-w-0 flex-1'
        type='text'
        maxLength={1024}
        spellCheck={false}
        autoComplete='off'
        disabled={disabled}
        placeholder={t('留空使用该提供商默认值')}
        value={value}
        onChange={event => onChange(event.currentTarget.value)}
      />
      <SearchableSelect value={matched?.id ?? ''} disabled={disabled || !presets.length}
        className='w-[172px] shrink-0' aria-label={t('选择预置客户端')}
        displayValue={matched?.name || t('选择预置客户端')}
        options={[{ value: '', label: t('不使用预置') }, ...presets.map(preset => ({
          value: preset.id, label: preset.name, searchText: preset.ua,
        }))]}
        onValueChange={next => {
          if (!next) { onChange(''); return }
          const preset = presets.find(item => item.id === next)
          if (preset) onChange(preset.ua)
        }} />
    </div>
  )
}
