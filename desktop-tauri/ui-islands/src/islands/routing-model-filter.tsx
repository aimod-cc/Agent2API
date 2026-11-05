import * as React from 'react'
import { Button, Empty, InputGroup, InputGroupAddon, InputGroupInput, Label, Popover, PopoverContent, PopoverTrigger } from '@ui'
import { t } from '../i18n'
import { RouteIcon } from './routing-ui'
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

export function RoutingModelFilter({ value, models, disabled, onChange }: {
  value: string; models: string[]; disabled: boolean; onChange(model: string): void
}) {
  const [open, setOpen] = React.useState(false)
  const [search, setSearch] = React.useState('')
  const id = React.useId()
  const query = search.trim().toLowerCase()
  const matches = models.filter(model => model.toLowerCase().includes(query))
  const text = value || t('全部')
  const choose = (model: string) => { onChange(model); setOpen(false) }
  const option = (model: string, label: string) => <Button key={model} variant='ghost' className='routing-model-filter-option'
    aria-pressed={value === model} title={label} onClick={() => choose(model)}>
    <span>{label}</span>{value === model ? <RouteIcon name='check' size={13} /> : null}
  </Button>

  return <div className='routing-model-filter'>
    <Label htmlFor={id}>{t('筛选模型')}</Label>
    <Popover open={open} onOpenChange={open => { setOpen(open); if (open) setSearch('') }}>
      <PopoverTrigger id={id} render={<Button variant='outline' disabled={disabled} />} className='routing-model-filter-trigger'
        aria-label={t('筛选模型')} title={text}>
        <span>{text}</span><RouteIcon name='chevronDown' size={13} />
      </PopoverTrigger>
      <PopoverContent className='routing-model-filter-popover' sideOffset={4}>
        <InputGroup><InputGroupInput type='search' autoFocus autoComplete='off' value={search}
          placeholder={t('搜索模型')} aria-label={t('搜索模型')} onChange={event => setSearch(event.currentTarget.value)} />
          <InputGroupAddon><RouteIcon name='search' size={14} /></InputGroupAddon></InputGroup>
        <div className='routing-model-filter-options'>
          {option('', t('全部'))}
          {matches.map(model => option(model, model))}
          {query && !matches.length ? <Empty>{t('没有匹配的模型')}</Empty> : null}
        </div>
      </PopoverContent>
    </Popover>
    <p className='routing-config-note'>{t('仅筛选右侧账号列表，不改变路由配置。')}</p>
  </div>
}
