import * as React from 'react'
import { Badge, Card, CardAction, CardContent, CardFooter, CardHeader, CardTitle, Collapsible, CollapsibleContent, CollapsibleTrigger } from '@ui'
import { ErrorChoice, ErrorIcon, ErrorNumber, ErrorText, ErrorToggle } from './routing-errors-controls'
import { duration, type ErrorSettings, type ErrorTranslate } from './routing-errors-types'
import { errorScenarios, simulateError, type ErrorPreviewInput } from './routing-errors-simulation'

export function ErrorPreviewPanel({ settings, providers, t, input, onChange }: {
  settings: ErrorSettings; providers: { id: string; label: string }[]; t: ErrorTranslate; input: ErrorPreviewInput; onChange(input: ErrorPreviewInput): void
}) {
  const [more, setMore] = React.useState(false)
  const patch = (next: Partial<ErrorPreviewInput>) => onChange({ ...input, ...next })
  const result = simulateError(settings, input, t)
  return <Card className='re-preview'><CardHeader><ErrorIcon name='route' /><CardTitle>{t('处理预览')}</CardTitle><CardAction><Badge variant='outline' shape='tag'>{t('模拟')}</Badge></CardAction></CardHeader>
    <CardContent className='re-preview-content'>
      <ErrorChoice label={t('选择模拟错误')} value={input.scenario} options={errorScenarios(t).map(item => ({ value: item.id, label: item.label }))} onChange={scenario => patch({ scenario })} />
      <div className={`re-result${result.stopped ? ' is-stopped' : ''}`}><strong>{result.title}</strong></div>
      {result.cooldown ? <div className='re-reset'><ErrorIcon name='timer' /><div><strong>{t('{time}后恢复参与', { time: duration(result.cooldown, t) })}</strong><p>{result.source}</p></div></div> : null}
      <ol className='re-preview-steps'>{result.steps.map((step, index) => <li key={index}><span>{index + 1}</span><div><strong>{step.title}</strong><p>{step.detail}</p></div></li>)}</ol>
      <div className='re-preview-budget'><span>{t('原账号可重发 {n} 次', { n: result.resendsLeft })}</span><span>{t('还可换 {n} 个账号', { n: result.switchesLeft })}</span></div>
      <ErrorToggle label={t('已经开始输出')} checked={input.started} onChange={started => patch({ started })} />
      <ErrorToggle label={t('任务结果不确定')} checked={input.uncertain} onChange={uncertain => patch({ uncertain })} />
      <Collapsible open={more} onOpenChange={setMore}><CollapsibleTrigger className='re-preview-fold'><ErrorIcon name='settings' size={14} />{t('更多模拟条件')}<ErrorIcon name='chevron' size={13} /></CollapsibleTrigger>
        <CollapsibleContent><div className='re-more-conditions'>
          <ErrorChoice label={t('模拟提供商')} value={input.provider} options={providers.map(item => ({ value: item.id, label: item.label }))} onChange={provider => patch({ provider })} />
          <ErrorText label={t('模拟模型')} value={input.model} onChange={model => patch({ model })} />
          <ErrorToggle label={t('明确指定账号')} checked={input.pinned} onChange={pinned => patch({ pinned })} />
          <div className='re-form-grid'><ErrorNumber label={t('已经换过的账号数')} unit={t('个')} value={input.switchesUsed} max={10} onChange={switchesUsed => patch({ switchesUsed })} />
            <ErrorNumber label={t('已经重发的次数')} unit={t('次')} value={input.resendsUsed} max={10} onChange={resendsUsed => patch({ resendsUsed })} />
            <ErrorNumber label={t('连续失败次数')} unit={t('次')} value={input.strikes} min={1} max={10} onChange={strikes => patch({ strikes })} /></div>
        </div></CollapsibleContent>
      </Collapsible>
    </CardContent><CardFooter>{t('只演示当前规则，不发送请求，也不读取真实账号。')}</CardFooter>
  </Card>
}
