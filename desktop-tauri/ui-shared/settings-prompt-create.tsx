import * as React from 'react'
import { Alert, Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, Input, Label,
  SegmentedControl, Switch, Textarea } from '@ui'
import type { Translate } from './assistant-read-types'
import { PromptRuleFields } from './settings-prompt-rule'
import { blankRule, type PromptRuleView, type PromptView } from './settings-prompt-types'

export function PromptRuleCreateDialog({ view, t, onSave, onClose }: {
  view: PromptView
  t: Translate
  onSave(rule: PromptRuleView): Promise<string | null>
  onClose(): void
}) {
  const [rule, setRule] = React.useState(() => blankRule(t))
  const [source, setSource] = React.useState<'text' | 'file'>('text')
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const saving = React.useRef(false)
  const nameId = React.useId()
  const enabledId = React.useId()
  const textId = React.useId()
  const fileId = React.useId()
  const patch = (next: Partial<PromptRuleView>) => setRule(current => ({ ...current, ...next }))

  async function save() {
    if (saving.current) return
    saving.current = true; setBusy(true); setError('')
    const next = { ...rule, name: rule.name.trim(), text: source === 'text' ? rule.text : '', file: source === 'file' ? rule.file : '' }
    try {
      const failure = await onSave(next)
      if (failure === null) onClose()
      else setError(failure)
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
    } finally { saving.current = false; setBusy(false) }
  }

  return <Dialog open onOpenChange={open => { if (!open && !saving.current) onClose() }}>
    <DialogContent className='sp-body-dialog sp-rule-dialog'>
      <DialogHeader><DialogTitle>{t('新规则')}</DialogTitle></DialogHeader>
      <DialogBody className='sp-body-main sp-rule-dialog-main'>
        <div className='sp-rule-dialog-name'>
          <div className='sp-body-field'>
            <Label htmlFor={nameId}>{t('规则名称')}</Label>
            <Input id={nameId} value={rule.name} maxLength={80} disabled={busy} placeholder={t('规则名称')}
              onChange={event => patch({ name: event.currentTarget.value })} />
          </div>
          <div className='sp-rule-dialog-enabled'>
            <Label htmlFor={enabledId}>{t('启用规则')}</Label>
            <Switch id={enabledId} checked={rule.enabled} disabled={busy} onCheckedChange={enabled => patch({ enabled })} />
          </div>
        </div>
        <PromptRuleFields rule={rule} view={view} disabled={busy} t={t} immediate onChange={setRule} />
        {rule.mode !== 'passthrough' ? <div className='sp-editor-section'>
          <div className='sp-body-source'>
            <span className='sp-editor-label'>{t('提示词')}</span>
            <SegmentedControl value={source} disabled={busy} aria-label={t('提示词来源')} onValueChange={setSource}
              options={[{ value: 'text', label: t('正文') }, { value: 'file', label: t('文件') }]} />
            <Button variant='ghost' size='sm' className='sp-rule-dialog-reset' disabled={busy}
              onClick={() => { patch({ text: '', file: '' }); setSource('text') }}>{t('恢复内置默认')}</Button>
          </div>
          {source === 'text' ? <div className='sp-body-field'>
            <Label htmlFor={textId}>{t('提示词正文')}</Label>
            <Textarea id={textId} className='sp-rule-dialog-text' rows={8} spellCheck={false} value={rule.text} disabled={busy}
              placeholder={t('留空使用内置默认提示词')} onChange={event => patch({ text: event.currentTarget.value })} />
            <span className='sp-body-count'>{rule.text.trim()
              ? t('{lines} 行 · {chars} 字符', { lines: rule.text.split('\n').length, chars: rule.text.length })
              : t('保存空正文将使用内置默认提示词')}</span>
          </div> : <div className='sp-body-field'>
            <Label htmlFor={fileId}>{t('提示词文件')}</Label>
            <Input id={fileId} value={rule.file} disabled={busy} spellCheck={false} placeholder={t('网关所在机器上的 UTF-8 文本路径')}
              onChange={event => patch({ file: event.currentTarget.value })} />
            <span className='sp-muted'>{t('留空使用内置默认提示词')}</span>
          </div>}
        </div> : null}
        {error ? <Alert variant='destructive'>{error}</Alert> : null}
      </DialogBody>
      <DialogFooter>
        <Button variant='outline' disabled={busy} onClick={onClose}>{t('取消')}</Button>
        <Button disabled={busy} onClick={() => void save()}>{busy ? t('保存中…') : t('保存规则')}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}
