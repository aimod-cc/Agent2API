import * as React from 'react'
import { Badge, Button, Card, CardContent, CardHeader, CardTitle, Input, Label } from '@ui'
import type { Translate } from './assistant-read-types'
import { SearchableSelect } from './searchable-select'
import { configPayload, modeLabel, type PromptApi, type PromptConfigView, type PromptPreview, type PromptSample, type PromptView } from './settings-prompt-types'

export function PromptRuleTester({ api, config, view, t, onLocate }: {
  api: PromptApi; config: PromptConfigView; view: PromptView; t: Translate; onLocate(id: string): void
}) {
  const [sample, setSample] = React.useState<PromptSample>(() => ({
    provider: view.providers[0]?.id || '', client: view.clients[0]?.id || 'other', model: view.models[0] || '',
  }))
  const [result, setResult] = React.useState<PromptPreview | null>(null)
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const sequence = React.useRef(0)
  const configKey = JSON.stringify(configPayload(config))

  React.useEffect(() => {
    const current = ++sequence.current
    if (!sample.provider) { setResult(null); return }
    setBusy(true)
    const timer = window.setTimeout(() => {
      void api.preview({ config: configPayload(config), sample }).then(next => {
        if (current === sequence.current) { setResult(next); setError('') }
      }).catch(failure => {
        if (current === sequence.current) { setResult(null); setError(failure instanceof Error ? failure.message : String(failure)) }
      }).finally(() => { if (current === sequence.current) setBusy(false) })
    }, 300)
    return () => { window.clearTimeout(timer); ++sequence.current }
  }, [api, configKey, sample.provider, sample.client, sample.model])

  return <Card className='sp-tester'>
    <CardHeader className='sp-tester-head'><CardTitle>{t('命中测试')}</CardTitle></CardHeader>
    <CardContent className='sp-tester-body'>
      <div className='sp-tester-inputs'>
        <div><Label htmlFor='sp-sample-provider'>{t('提供商')}</Label>
          <SearchableSelect id='sp-sample-provider' className='w-full' value={sample.provider}
            options={view.providers.map(option => ({ value: option.id, label: option.label }))}
            placeholder={t('选择提供商')} searchPlaceholder={t('搜索提供商')} onValueChange={provider => setSample(current => ({ ...current, provider }))} />
        </div>
        <div><Label htmlFor='sp-sample-client'>{t('客户端')}</Label>
          <SearchableSelect id='sp-sample-client' className='w-full' value={sample.client}
            options={view.clients.map(option => ({ value: option.id, label: option.label }))}
            placeholder={t('选择客户端')} searchPlaceholder={t('搜索客户端')} onValueChange={client => setSample(current => ({ ...current, client }))} />
        </div>
        <div><Label htmlFor='sp-sample-model'>{t('模型')}</Label>
          <Input id='sp-sample-model' value={sample.model} list='sp-model-choices' spellCheck={false}
            placeholder={t('客户端请求的模型名')} onChange={event => {
              const model = event.currentTarget.value
              setSample(current => ({ ...current, model }))
            }} />
        </div>
      </div>
      <div className='sp-tester-result' aria-live='polite'>
        {error ? <span className='sp-error'>{error}</span> : busy ? <span className='sp-muted'>{t('匹配中…')}</span> : result ? <>
          <span className={result.ruleId ? 'sp-match-dot' : 'sp-match-dot is-fallback'} />
          <span className='sp-match-name'>{result.ruleId ? t('规则 {index} · {name}', { index: (result.index ?? 0) + 1, name: result.name || t('未命名规则') }) : t('兜底 · 未命中任何规则')}</span>
          <Badge variant={result.ruleId ? 'brand' : 'outline'} shape='tag'>{modeLabel(result.mode, t)}</Badge>
          {result.ruleId ? <Button variant='ghost' size='xs' className='ml-auto' onClick={() => onLocate(result.ruleId!)}>{t('定位')}</Button> : null}
        </> : <span className='sp-muted'>{t('请选择提供商')}</span>}
      </div>
    </CardContent>
  </Card>
}
