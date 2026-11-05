import * as React from 'react'
import { Button, Checkbox, Dialog, DialogBody, DialogContent, DialogHeader, DialogTitle, Label, Spinner } from '@ui'
import { t } from '../i18n'
import { bridge, errorMessage, type RouteGroup, type RoutePreview, type RouteProfile } from './routing-types'
import { RoutingLivePanel } from './routing-live-panel'
import { RoutingSelect } from './routing-select'
import { RouteIcon } from './routing-ui'

/** 草稿也在服务端做纯预演；不触发真实请求、轮转或会话续期。 */
export function RoutingPreviewPanel({ model, group, profile, globalScope = false, onOpenScope }: {
  model: string; group?: RouteGroup; profile?: RouteProfile; globalScope?: boolean; onOpenScope(model: string): void
}) {
  const [needs, setNeeds] = React.useState('text')
  const [failFirst, setFailFirst] = React.useState(false)
  const [details, setDetails] = React.useState(false)
  const [result, setResult] = React.useState<RoutePreview | null>(null)
  const [error, setError] = React.useState('')
  const [loading, setLoading] = React.useState(false)
  const [revision, setRevision] = React.useState(0)
  const effectiveNeeds = group ? needs : 'text'
  const input = JSON.stringify({ model, group, profile, global: globalScope, needs: { tools: effectiveNeeds === 'tools', images: effectiveNeeds === 'images', video: effectiveNeeds === 'video' }, failFirst })
  const [resolvedInput, setResolvedInput] = React.useState('')
  React.useEffect(() => {
    let alive = true
    setResult(null); setError('')
    if (!model) { setLoading(false); return }
    setLoading(true)
    const timer = window.setTimeout(() => {
      bridge().previewRouting(JSON.parse(input)).then(next => { if (alive) { setResult(next); setResolvedInput(input) } })
        .catch(failure => { if (alive) { setError(errorMessage(failure)); setResolvedInput(input) } })
        .finally(() => { if (alive) setLoading(false) })
    }, 200)
    return () => { alive = false; window.clearTimeout(timer) }
  }, [input, revision, model])
  const refresh = () => setRevision(value => value + 1)
  const current = resolvedInput === input ? result : null
  const pending = Boolean(model) && (loading || resolvedInput !== input)
  const failure = resolvedInput === input ? error : ''
  return <div className='routing-preview' data-route-anchor>
    <div className='routing-preview-head'><b>{t('本轮选路预演')}</b>
      <Button variant='link' size='sm' onClick={() => setDetails(true)}>{t('全部候选')}</Button>
      <Button variant='ghost' size='icon-sm' disabled={!model || pending} onClick={refresh} title={t('刷新预演')} aria-label={t('刷新预演')}>
        {pending ? <Spinner /> : <RouteIcon name='refresh' size={13} />}
      </Button>
    </div>
    <RoutingLivePanel compact workspace={current} model={model} loading={pending} error={failure} dirty={false} globalScope={globalScope}
      modelsAvailable={Boolean(model)} onOpenScope={onOpenScope} onRefresh={refresh} preview />
    {failFirst || effectiveNeeds !== 'text' ? <p className='routing-preview-simulation'>{t('已启用模拟条件')}</p> : null}
    <p className='routing-preview-foot'>{t('按当前草稿预演，不发送请求，不消耗额度。')}</p>
    <Dialog open={details} onOpenChange={setDetails}><DialogContent className='routing-preview-dialog'>
      <DialogHeader><DialogTitle>{t('选路预演')} · {model || t('默认模型')}</DialogTitle></DialogHeader>
      <DialogBody>
        <div className='routing-preview-options'>
          {group ? <div className='routing-rule-field'><Label>{t('请求需要')}</Label>
            <RoutingSelect value={needs} label={t('请求需要')} options={[
              ['text', t('文本对话')], ['tools', t('工具调用')], ['images', t('图片输入')], ['video', t('视频输入')],
            ]} onChange={setNeeds} /></div> : null}
          <label className='routing-preview-toggle'><Checkbox checked={failFirst} onCheckedChange={value => setFailFirst(value === true)} />{t('模拟首选失败')}</label>
        </div>
        <RoutingLivePanel workspace={current} model={model} loading={pending} error={failure} dirty={false} globalScope={globalScope}
          modelsAvailable={Boolean(model)} onOpenScope={value => { setDetails(false); onOpenScope(value) }} onRefresh={refresh} preview />
      </DialogBody>
    </DialogContent></Dialog>
  </div>
}
