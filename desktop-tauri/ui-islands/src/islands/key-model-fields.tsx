import * as React from 'react'
import { Label, MultiSelect, type MultiSelectOption } from '@ui'
import { t } from '../i18n'
import { type KeyRouteMode, type KeyRoutingValue } from './key-routing-fields'

/**
 * 按提供商集合取对外名并集，铺成模型多选选项。
 * 默认池传入 Key 的 provider 白名单；组模式传入所选组公开预览里的 provider id
 * 列表，因为组路由不使用 provider 白名单。默认池保留已选的历史模型，组模式只展示
 * 当前账号组真正可见的模型，避免切组后把旧组模型继续显示为可选项。
 */
function modelOptions(
  table: Record<string, string[]>,
  selectedProviders: string[],
  selected: string[],
): MultiSelectOption[] {
  const names = new Map<string, string>() // 小写 → 原始名（先到先得，保住后端给的大小写）
  const put = (value: unknown) => {
    const text = String(value ?? '').trim()
    if (!text) return
    const key = text.toLowerCase()
    if (!names.has(key)) names.set(key, text)
  }
  selectedProviders.forEach(id => {
    const list = table[id] ?? table[String(id).toLowerCase()]
    if (Array.isArray(list)) list.forEach(put)
  })
  // 默认池需要保留历史模型；组模式传入空数组，只允许当前组的候选进入列表。
  selected.forEach(put)
  // 选项**值**是模型名（提交给后端的身份）不进词典；展示 label 走 t()
  const out = [...names.values()].map(value => ({ value, label: t(value) }))
  out.sort((a, b) => (a.value.toLowerCase() < b.value.toLowerCase() ? -1 : 1))
  return out
}

export function modelCandidatesFor(
  table: Record<string, string[]>,
  routeMode: KeyRouteMode,
  scopeProviders: string[] | null,
  allowedProviders: string[],
  selectedModels: string[],
): MultiSelectOption[] | null {
  if (routeMode === 'account_group') {
    if (scopeProviders === null) return null
    return modelOptions(table, scopeProviders, [])
  }
  return modelOptions(table, allowedProviders, selectedModels)
}

function sameStringList(left: readonly string[], right: readonly string[]): boolean {
  return left.length === right.length && left.every((value, index) => value === right[index])
}

type KeyModelFieldsProps = {
  current: KeyRoutingValue
  scopeProviders: string[] | null
  providerOptions: MultiSelectOption[]
  allowedProviders: string[]
  setPickedProviders: React.Dispatch<React.SetStateAction<string[]>>
  pickedModels: string[]
  setPickedModels: React.Dispatch<React.SetStateAction<string[]>>
  modelsByProvider: Record<string, string[]>
  providerLabel: (id: string) => string
  onScopeReady: (ready: boolean) => void
}

export function KeyModelFields({
  current,
  scopeProviders,
  providerOptions,
  allowedProviders,
  setPickedProviders,
  pickedModels,
  setPickedModels,
  modelsByProvider,
  providerLabel,
  onScopeReady,
}: KeyModelFieldsProps) {
  const candidateOptions = modelCandidatesFor(
    modelsByProvider,
    current.routeMode,
    scopeProviders,
    allowedProviders,
    pickedModels,
  )
  const modelCandidates = candidateOptions ?? []
  const allowedModels = pickedFromOptions(pickedModels, modelCandidates)
  const previousGroupIdRef = React.useRef<string | null>(
    current.routeMode === 'account_group' ? current.accountGroupId : null,
  )
  const scopeProviderKey = scopeProviders === null
    ? 'missing'
    : scopeProviders.map(provider => provider.toLowerCase()).join('\0')
  const modelCandidateKey = modelCandidates.map(option => option.value.toLowerCase()).join('\0')

  React.useEffect(() => {
    onScopeReady(current.routeMode !== 'account_group'
      || (Boolean(current.accountGroupId) && scopeProviders !== null))
  }, [current.routeMode, current.accountGroupId, scopeProviderKey, onScopeReady])

  React.useEffect(() => {
    const nextGroupId = current.routeMode === 'account_group' ? current.accountGroupId : null
    const groupChanged = previousGroupIdRef.current !== nextGroupId
    previousGroupIdRef.current = nextGroupId
    if (!nextGroupId || (!groupChanged && scopeProviders === null)) return
    setPickedModels(previous => {
      const next = pickedFromOptions(previous, modelCandidates)
      return sameStringList(previous, next) ? previous : next
    })
  }, [current.routeMode, current.accountGroupId, scopeProviderKey, modelCandidateKey])

  let modelEmptyHint = t('没有匹配的选项')
  if (!modelCandidates.length && current.routeMode !== 'account_group') {
    modelEmptyHint = allowedProviders.length
      ? t('这几家当前没有可用模型（账号未登录或清单为空）')
      : t('请先在上面选择可用提供商')
  }

  return <>
    {current.routeMode === 'default_pool' ? (
      <div className='flex flex-wrap items-center gap-2.5'>
        <Label className='text-[12.5px] whitespace-nowrap text-subtle'>{t('可用提供商')}</Label>
        <MultiSelect
          value={allowedProviders}
          onValueChange={setPickedProviders}
          options={providerOptions}
          placeholder={t('留空 = 不限制')}
          searchPlaceholder={t('搜索提供商…')}
          aria-label={t('可用提供商')}
          className='flex-auto min-w-[180px] max-w-[260px]'
        />
      </div>
    ) : null}
    <div className='flex flex-wrap items-center gap-2.5'>
      <Label className='text-[12.5px] whitespace-nowrap text-subtle'>{t('可用模型')}</Label>
      <MultiSelect
        value={allowedModels}
        onValueChange={setPickedModels}
        options={modelCandidates}
        placeholder={t('留空 = 不限制')}
        emptyHint={modelEmptyHint}
        searchPlaceholder={t('搜索模型…')}
        aria-label={t('可用模型')}
        className='flex-auto min-w-[180px] max-w-[260px]'
      />
    </div>
    <p>
      {current.routeMode === 'account_group'
        ? t('留空表示不限制；同时设置时请求需同时满足模型白名单与账号组路由。账号组模式下模型候选来自组可见的全部对外模型。')
        : t('留空表示不限制；同时设置时请求需同时满足两个条件（模型在白名单内且路由到允许的提供商）。 「可用模型」的候选跟着上面勾选的提供商走：没勾提供商时它是空的（还没有约束范围）， 勾了几家就列出这几家能收的全部对外名。')}
    </p>
    <p id='key-restrict-summary'>
      {restrictionSummary(
        current.routeMode === 'account_group' ? [] : allowedProviders,
        allowedModels,
        current.routeMode === 'account_group',
        providerLabel,
      )}
    </p>
  </>
}

/**
 * 弹窗里「当前选的摘要」。为什么要有它：限制是**看不见的** —— 弹窗一关，列表上只剩
 * 「限制：…」一行；而多选的触发器只显示连接后的一行文案，清单长了会被省略号收掉。
 * 用户点完「保存范围」就看不到弹窗了，勾了哪几家 / 哪些模型得在这儿给他核对一遍。
 */
function restrictionSummary(
  providers: readonly string[],
  models: readonly string[],
  accountGroupMode = false,
  providerLabel: (id: string) => string,
): string {
  if (accountGroupMode) {
    if (!models.length) return t('不限制')
    return t('限制：{text}', { text: t('模型：{names}', { names: models.join(t('、')) }) })
  }
  if (!providers.length && !models.length) {
    return t('当前不限制：这把 Key 可以用全部提供商与全部对外模型')
  }
  const parts: string[] = []
  if (providers.length) parts.push(t('提供商：{names}', { names: providers.map(providerLabel).join(t('、')) }))
  if (models.length) parts.push(t('模型：{names}', { names: models.join(t('、')) }))
  // 只限制了模型、没限制提供商（旧数据里可能存在这种组合）：模型候选此刻只剩已勾的
  // 那几个（没有提供商就没有并集可铺），要说清怎么把候选拿回来 —— 否则用户会以为
  // 「模型清单坏了，加不了新的」
  if (!providers.length) {
    parts.push(t('（模型候选需先选提供商；不选则沿用当前这几项，保存后仍按模型白名单生效）'))
  }
  return parts.join('　')
}

/**
 * 把 state 里的勾选值按**候选表**归一：只留候选里存在的项、顺序照候选表、写法以候选
 * 为准（模型名的大小写可能与清单不一致，旧实现就是按小写比对后把候选的写法写回 DOM）。
 * 这就是旧实现 `[...select.selectedOptions].map(option => option.value)` 的口径 ——
 * 那时「勾选」只存在于渲染出来的 option 上，候选里没有的 id 落不到 DOM 里，提交时自然
 * 被丢掉。**提交给后端的两个数组必须逐字保持这个口径**。
 */
export function pickedFromOptions(values: readonly string[], options: readonly MultiSelectOption[]): string[] {
  const canonical = new Map(options.map(option => [option.value.toLowerCase(), option.value]))
  const chosen = new Set<string>()
  values.forEach(value => {
    const hit = canonical.get(String(value).toLowerCase())
    if (hit) chosen.add(hit)
  })
  return options.filter(option => chosen.has(option.value)).map(option => option.value)
}
