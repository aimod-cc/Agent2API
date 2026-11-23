import * as React from 'react'
import { Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, Input, Label,
  SegmentedControl, Textarea } from '@ui'
import type { Translate } from './assistant-read-types'
import { contentPayload, modeHint, sourceLabel, type GatewayBlocks, type PromptContent, type PromptContentView } from './settings-prompt-types'

export function PromptBodyDialog({ title, content, t, onSave, onClose }: {
  title: string
  content: PromptContentView
  t: Translate
  onSave(content: PromptContent): Promise<boolean>
  onClose(): void
}) {
  const initial = React.useRef({ ...content })
  const initialSource = content.text.trim() || !content.file.trim() ? 'text' : 'file'
  const [source, setSource] = React.useState<'text' | 'file'>(initialSource)
  const [text, setText] = React.useState(content.text || (content.mode !== 'passthrough' ? content.effectiveText : ''))
  const [file, setFile] = React.useState(content.file)
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')

  async function save() {
    setBusy(true); setError('')
    const original = initial.current
    const originalSource = original.text.trim() || !original.file.trim() ? 'text' : 'file'
    const initialText = original.text || (original.mode !== 'passthrough' ? original.effectiveText : '')
    const changedSource = source !== originalSource
    // 未修改的内置正文保持默认；已有文件与内联覆盖共存时，不因编辑正文丢掉文件路径。
    const nextText = source === 'file' ? '' : text === initialText && !original.text && !changedSource ? '' : text
    const nextFile = source === 'file' || !changedSource ? file : ''
    try {
      if (await onSave({ ...contentPayload(original), text: nextText, file: nextFile })) onClose()
      else setError(t('保存失败，正文仍保留在这里。'))
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure))
    } finally { setBusy(false) }
  }

  return <Dialog open onOpenChange={open => { if (!open && !busy) onClose() }}>
    <DialogContent className='sp-body-dialog'>
      <DialogHeader><DialogTitle>{title}</DialogTitle></DialogHeader>
      <DialogBody className='sp-body-main'>
        <p className='sp-body-hint'>{modeHint(content.mode, t)}</p>
        <div className='sp-body-source'>
          <SegmentedControl aria-label={t('提示词来源')} value={source} onValueChange={setSource} disabled={busy}
            options={[{ value: 'text', label: t('正文') }, { value: 'file', label: t('文件') }]} />
          <span className='sp-muted'>{sourceLabel(content, t)}</span>
        </div>
        {source === 'text' ? <div className='sp-body-field'>
          <Label htmlFor='sp-prompt-text'>{t('提示词正文')}</Label>
          <Textarea id='sp-prompt-text' className='sp-body-text' rows={16} spellCheck={false} value={text} disabled={busy}
            placeholder={t('留空使用内置默认提示词')} onChange={event => setText(event.currentTarget.value)} />
          <span className='sp-body-count'>{text.trim() ? t('{lines} 行 · {chars} 字符', { lines: text.split('\n').length, chars: text.length })
            : file.trim() && initialSource === 'text' ? t('保存空正文将使用提示词文件') : t('保存空正文将使用内置默认提示词')}</span>
        </div> : <div className='sp-body-field'>
          <Label htmlFor='sp-prompt-file'>{t('提示词文件')}</Label>
          <Input id='sp-prompt-file' value={file} disabled={busy} spellCheck={false} placeholder={t('网关所在机器上的 UTF-8 文本路径')}
            onChange={event => setFile(event.currentTarget.value)} />
          <span className='sp-muted'>{t('留空使用内置默认提示词')}</span>
        </div>}
        {error ? <p role='alert' className='sp-error'>{error}</p> : null}
      </DialogBody>
      <DialogFooter>
        <Button variant='outline' className='mr-auto' disabled={busy} onClick={() => { setText(''); setFile(''); setSource('text') }}>{t('恢复内置默认')}</Button>
        <Button variant='outline' disabled={busy} onClick={onClose}>{t('取消')}</Button>
        <Button disabled={busy} onClick={() => void save()}>{busy ? t('保存中…') : t('保存')}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}

const BLOCKS = ['identity', 'stable', 'dynamic'] as const

export function PromptGatewayDialog({ title, blocks, overrides, t, onSave, onClose }: {
  title: string
  blocks: GatewayBlocks
  overrides: Partial<GatewayBlocks> | undefined
  t: Translate
  onSave(value: Partial<GatewayBlocks> | null): Promise<boolean>
  onClose(): void
}) {
  const [draft, setDraft] = React.useState<GatewayBlocks>(() => ({
    identity: overrides?.identity || blocks.identity,
    stable: overrides?.stable || blocks.stable,
    dynamic: overrides?.dynamic || blocks.dynamic,
  }))
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const labels = [t('身份'), t('稳定段'), t('动态段')]

  async function save(value: Partial<GatewayBlocks> | null) {
    setBusy(true); setError('')
    try { if (await onSave(value)) onClose(); else setError(t('保存失败，正文仍保留在这里。')) }
    catch (failure) { setError(failure instanceof Error ? failure.message : String(failure)) }
    finally { setBusy(false) }
  }

  return <Dialog open onOpenChange={open => { if (!open && !busy) onClose() }}>
    <DialogContent className='sp-body-dialog'>
      <DialogHeader><DialogTitle>{title}</DialogTitle></DialogHeader>
      <DialogBody className='sp-body-main'>
        <p className='sp-body-hint'>{t('上游要求分段发送；留空的段恢复官方原文。')}</p>
        {BLOCKS.map((field, index) => <div className='sp-body-field' key={field}>
          <Label htmlFor={`sp-gateway-${field}`}>{labels[index]}</Label>
          <Textarea id={`sp-gateway-${field}`} rows={field === 'identity' ? 2 : 8} value={draft[field]} spellCheck={false} disabled={busy}
            onChange={event => {
              const value = event.currentTarget.value
              setDraft(current => ({ ...current, [field]: value }))
            }} />
        </div>)}
        {error ? <p role='alert' className='sp-error'>{error}</p> : null}
      </DialogBody>
      <DialogFooter>
        <Button variant='outline' className='mr-auto' disabled={busy} onClick={() => void save(null)}>{t('恢复官方原文')}</Button>
        <Button variant='outline' disabled={busy} onClick={onClose}>{t('取消')}</Button>
        <Button disabled={busy} onClick={() => void save(Object.fromEntries(BLOCKS.map(field => [field, draft[field] === blocks[field] ? '' : draft[field]])))}>{busy ? t('保存中…') : t('保存')}</Button>
      </DialogFooter>
    </DialogContent>
  </Dialog>
}
