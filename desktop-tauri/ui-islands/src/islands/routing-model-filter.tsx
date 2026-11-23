import * as React from 'react'
import { Combobox, ComboboxContent, ComboboxEmpty, ComboboxInput, ComboboxItem, ComboboxList, ComboboxTrigger, ComboboxValue, Label } from '@ui'
import { t } from '../i18n'
import { sortRoutingModels } from './routing-model-order'
import { bridge, errorMessage, type RoutingWorkspace } from './routing-types'

type ModelFilterResult = { source: string[]; model: string; providers: string[]; error: string }

// 筛选结果独立于路由草稿，不能用模型工作区替换正在编辑的配置。
export function useRoutingModelFilter(workspace: RoutingWorkspace | undefined, scope: string) {
  const [selected, setSelected] = React.useState('')
  const [result, setResult] = React.useState<ModelFilterResult | null>(null)
  const [attempt, retry] = React.useReducer(value => value + 1, 0)
  const models = workspace?.models
  const model = scope || models?.find(model => model.toLowerCase() === selected.toLowerCase()) || ''
  const local = !model || workspace?.model?.toLowerCase() === model.toLowerCase()
  const current = result?.source === models && result?.model === model ? result : null

  React.useEffect(() => {
    if (!models || local) return
    let alive = true
    setResult(null)
    const load = async () => {
      try {
        const next = await bridge().getRoutingWorkspace(model)
        if (next?.model?.toLowerCase() !== model.toLowerCase() || !Array.isArray(next.providers)) {
          throw new Error(t('路由接口返回了不完整的数据'))
        }
        if (alive) setResult({ source: models, model, providers: next.providers.filter(provider => provider.available).map(provider => provider.id), error: '' })
      } catch (failure) {
        if (alive) setResult({ source: models, model, providers: [], error: errorMessage(failure) })
      }
    }
    void load()
    return () => { alive = false }
  }, [models, model, local, attempt])

  const providers = !model ? null : new Set(local
    ? workspace?.providers.filter(provider => provider.available).map(provider => provider.id)
    : current?.providers || [])
  return { model, setModel: setSelected, providers, loading: Boolean(workspace && !local && !current), error: current?.error || '',
    retry: () => { setResult(null); retry() } }
}

export function RoutingModelFilter({ value, models, lastUsed, disabled, onChange }: {
  value: string; models: string[]; lastUsed?: Record<string, number>; disabled: boolean; onChange(model: string): void
}) {
  const [search, setSearch] = React.useState('')
  const [openedModels, setOpenedModels] = React.useState<string[] | null>(null)
  const orderedModels = React.useMemo(() => sortRoutingModels(models, model => model, lastUsed), [models, lastUsed])
  const id = React.useId()
  const activeModels = openedModels ?? orderedModels
  const options = React.useMemo(
    () => [{ value: '', label: t('全部') }, ...activeModels.map(model => ({ value: model, label: model }))],
    [activeModels]
  )
  const selected = options.find(option => option.value === value) ?? options[0]
  const text = value || t('全部')

  return <div className='routing-model-filter'>
    <Label htmlFor={id}>{t('筛选模型')}</Label>
    <Combobox items={options} value={selected} disabled={disabled}
      inputValue={search} onInputValueChange={setSearch}
      onOpenChange={open => {
        setSearch('')
        // 展开期间固定顺序，后台刷新不能挪动用户正准备点击的选项。
        setOpenedModels(open ? orderedModels : null)
      }}
      itemToStringLabel={item => item.label} itemToStringValue={item => item.value}
      isItemEqualToValue={(item, current) => item.value === current.value}
      onValueChange={item => { if (item) onChange(item.value) }}>
      <ComboboxTrigger id={id} className='w-full' aria-label={t('筛选模型')} title={text}>
        <ComboboxValue>{text}</ComboboxValue>
      </ComboboxTrigger>
      <ComboboxContent>
        <ComboboxInput placeholder={t('搜索模型')} aria-label={t('搜索模型')} autoComplete='off' />
        <ComboboxEmpty>{t('没有匹配的模型')}</ComboboxEmpty>
        <ComboboxList>
          {(item: (typeof options)[number]) => <ComboboxItem key={item.value || '__all'} value={item} title={item.label}>
            {item.label}
          </ComboboxItem>}
        </ComboboxList>
      </ComboboxContent>
    </Combobox>
    <p className='routing-config-note'>{t('仅筛选右侧账号列表，不改变路由配置。')}</p>
  </div>
}
