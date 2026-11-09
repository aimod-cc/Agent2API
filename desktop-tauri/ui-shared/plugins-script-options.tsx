import * as React from 'react'
import { Alert, Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogSection,
  DialogTitle, Label, Switch, Textarea } from '@ui'
import type { ScriptPackage, ScriptProviderOption } from './plugins'
import type { PluginTranslate } from './plugins-store'

type Values = { options: Record<string, unknown>; providerOptions: Record<string, Record<string, boolean>> }

function words(option: ScriptProviderOption, key: 'label' | 'description' | 'confirm'): string {
  const locale = document.documentElement.lang || 'zh-CN'
  const language = locale.startsWith('zh') ? locale === 'zh-Hant' || locale === 'zh-TW' ? 'zh-TW' : 'zh' : locale
  const translations = option[key === 'label' ? 'labels' : key === 'description' ? 'descriptions' : 'confirms']
  return translations?.[language] || translations?.[language.split('-')[0]] || option[key]
}

export function ScriptPluginOptions({ pack, disabled, request, refresh, t }: {
  pack: ScriptPackage
  disabled: boolean
  request: (path: string, body: Record<string, unknown>) => Promise<unknown>
  refresh: () => Promise<void>
  t: PluginTranslate
}) {
  const [values, setValues] = React.useState<Values | null>(null)
  const [draft, setDraft] = React.useState('')
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const [confirmation, setConfirmation] = React.useState<{ provider: string; option: ScriptProviderOption } | null>(null)
  const prefix = React.useId()
  React.useEffect(() => {
    let current = true
    void request('/api/plugins/scripts/settings', { id: pack.id }).then(raw => {
      const value = raw as Values
      if (current) { setValues(value); setDraft(JSON.stringify(value.options || {}, null, 2)) }
    }).catch(error => { if (current) setError(error instanceof Error ? error.message : String(error)) })
    return () => { current = false }
  }, [pack.id])
  const setOption = (provider: string, key: string, on: boolean) => setValues(previous => previous && ({ ...previous,
    providerOptions: { ...previous.providerOptions, [provider]: { ...previous.providerOptions?.[provider], [key]: on } } }))
  async function save() {
    if (!values || busy || disabled) return
    let options: unknown
    try { options = JSON.parse(draft) } catch { setError(t('JSON 格式不正确')); return }
    if (!options || typeof options !== 'object' || Array.isArray(options)) { setError(t('插件参数必须是 JSON 对象')); return }
    setBusy(true); setError('')
    try {
      await request('/api/plugins/scripts/settings', { id: pack.id, options, providerOptions: values.providerOptions || {} })
      await refresh()
    } catch (error) { setError(error instanceof Error ? error.message : String(error)) }
    finally { setBusy(false) }
  }
  return <>
    <DialogSection>
      <h3 className='m-0'>{t('插件选项')}</h3>
      {Object.entries(pack.settingsSchema?.providerOptions || {}).map(([provider, options]) => <div key={provider} className='space-y-3'>
        <p className='m-0 font-medium'>{pack.providers.find(item => item.id === provider)?.label || provider}</p>
        {options.map(option => <div key={option.key} className='space-y-1'>
          <Label htmlFor={`${prefix}-${provider}-${option.key}`} className='justify-between'>
            {words(option, 'label')}
            <Switch id={`${prefix}-${provider}-${option.key}`} disabled={disabled || busy || !values}
              checked={values?.providerOptions?.[provider]?.[option.key] ?? option.default}
              onCheckedChange={on => {
                if (on && option.confirm) setConfirmation({ provider, option })
                else setOption(provider, option.key, on)
              }} />
          </Label>
          {option.description ? <p className='m-0 whitespace-pre-wrap text-xs text-muted-foreground'>{words(option, 'description')}</p> : null}
        </div>)}
      </div>)}
      <Label htmlFor={`${prefix}-json`}>{t('高级参数（JSON）')}</Label>
      <Textarea id={`${prefix}-json`} value={draft} disabled={disabled || busy || !values} rows={6}
        spellCheck={false} autoComplete='off' onChange={event => setDraft(event.currentTarget.value)} />
      <p className='m-0 text-xs'>{t('修改高级参数会等待在途任务结束后重新加载插件；专属开关实时生效。')}</p>
      {error ? <Alert variant='destructive'>{error}</Alert> : null}
      <Button size='sm' disabled={disabled || busy || !values} onClick={() => void save()}>{busy ? t('保存中…') : t('保存')}</Button>
    </DialogSection>
    <Dialog open={confirmation !== null} onOpenChange={open => { if (!open) setConfirmation(null) }}>
      <DialogContent><DialogHeader><DialogTitle>{confirmation ? words(confirmation.option, 'label') : ''}</DialogTitle></DialogHeader>
        <DialogBody><p className='whitespace-pre-wrap'>{confirmation ? words(confirmation.option, 'confirm') : ''}</p></DialogBody>
        <DialogFooter><Button variant='outline' onClick={() => setConfirmation(null)}>{t('取消')}</Button>
          <Button onClick={() => { if (confirmation) setOption(confirmation.provider, confirmation.option.key, true); setConfirmation(null) }}>{t('确认')}</Button></DialogFooter>
      </DialogContent>
    </Dialog>
  </>
}
