/**
 * AI 助手的输入区：附件行 / 文本域 / 无框纯平底栏控件。
 *
 * ── 性能优化：隔离输入草稿 ───────────────────────────────────
 * 输入框内容完全由本组件内部管理，通过 ref 暴露 `clear()` / `focus()`。
 * 这样每次敲键盘只重绘输入框本身，不会波及上方的长消息流、侧栏与抽屉。
 */

import * as React from 'react'
import { Button, Empty, Input, InputGroup, InputGroupAddon, InputGroupInput, Popover, PopoverContent, PopoverTrigger, RadioGroup, RadioGroupItem, StatusDot, Textarea, cn } from '@ui'
import { t } from '../i18n'
import { selectedModel, type ModelOption } from './assistant-models'
import type { PermissionMode } from './assistant-state'
import {
  BoltIcon,
  CheckIcon,
  ChevronIcon,
  CloseIcon,
  FileIcon,
  ImageIcon,
  LayersIcon,
  MiniChevronIcon,
  PlusIcon,
  SearchIcon,
  SendIcon,
  ShieldIcon,
  SlidersIcon,
  StopIcon,
} from './assistant-icons'
import { formatFileSize, type AssistantTextFile } from '../../../ui-shared/assistant-files'

/** 思考等级候选（与后端 `model_rules::REASONING_LEVELS` 逐字一致，弱 → 强）。 */
export const REASONING_LEVELS = ['off', 'none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'] as const

export const MAX_IMAGES = 6
export const MAX_IMAGE_BYTES = 10 * 1024 * 1024

export type AssistantComposerHandle = {
  clear: () => void
  focus: () => void
}

function modeLabel(mode: PermissionMode): string {
  return mode === 'full' ? t('完全访问') : t('变更前确认')
}

export function levelLabel(level: string | null): string {
  if (!level) return t('跟随模型')
  return level === 'xhigh' ? 'xHigh' : level.charAt(0).toUpperCase() + level.slice(1)
}

type Props = {
  images: string[]
  files: AssistantTextFile[]
  onAddFiles: (files: FileList | File[]) => void
  onRemoveImage: (index: number) => void
  onRemoveFile: (index: number) => void
  modelSupportsImages: boolean
  imageToolsEnabled: boolean
  modelAvailable: boolean
  modelsReady: boolean
  modelsLoading: boolean
  modelWarning: string
  onRefreshModels: () => void
  models: ModelOption[]
  model: string | null
  onPickModel: (name: string) => void
  reasoning: string | null
  onPickReasoning: (level: string | null) => void
  mode: PermissionMode
  onModeChange: (mode: PermissionMode) => void
  busy: boolean
  awaiting: boolean
  onSend: (text: string) => void
  onStop: () => void
}

export const AssistantComposer = React.memo(React.forwardRef<AssistantComposerHandle, Props>(function AssistantComposer(props, ref) {
  const {
    images, files, onAddFiles, onRemoveImage, onRemoveFile,
    modelSupportsImages, imageToolsEnabled, modelAvailable, modelsReady, modelsLoading, modelWarning, onRefreshModels,
    models, model, onPickModel,
    reasoning, onPickReasoning, mode, onModeChange,
    busy, awaiting, onSend, onStop,
  } = props

  const [draft, setDraft] = React.useState('')
  const taRef = React.useRef<HTMLTextAreaElement | null>(null)
  const fileRef = React.useRef<HTMLInputElement | null>(null)
  const [dragging, setDragging] = React.useState(false)
  const [pickerOpen, setPickerOpen] = React.useState(false)
  const [pane, setPane] = React.useState<'root' | 'model' | 'reason'>('root')
  const [keyword, setKeyword] = React.useState('')

  React.useImperativeHandle(ref, () => ({
    clear: () => setDraft(''),
    focus: () => taRef.current?.focus(),
  }), [])

  const disabled = busy || awaiting
  const canSend = !disabled && modelAvailable && (Boolean(draft.trim()) || images.length > 0 || files.length > 0)
  const selected = selectedModel(models, model)
  const displayModel = model || t('选择模型…')
  const displayLevel = model && modelsReady && !modelAvailable ? t('不可用') : levelLabel(reasoning)
  const imageWarning = images.length > 0 && !modelSupportsImages

  React.useLayoutEffect(() => {
    const el = taRef.current
    if (!el) return
    if (!draft) {
      el.style.height = ''
      return
    }
    el.style.height = 'auto'
    el.style.height = `${Math.min(el.scrollHeight + 2, 168)}px`
  }, [draft])

  React.useEffect(() => {
    if (!pickerOpen) {
      setPane('root')
      setKeyword('')
    }
  }, [pickerOpen])

  const onPaste = (event: React.ClipboardEvent<HTMLTextAreaElement>) => {
    const items = event.clipboardData?.items
    if (!items) return
    const pasted: File[] = []
    for (let i = 0; i < items.length; i += 1) {
      if (items[i].kind !== 'file') continue
      const file = items[i].getAsFile()
      if (file) pasted.push(file)
    }
    if (pasted.length) {
      event.preventDefault()
      onAddFiles(pasted)
    }
  }

  const filteredModels = React.useMemo(() => {
    const key = keyword.trim().toLowerCase()
    return key ? models.filter(item => item.name.toLowerCase().includes(key)) : models
  }, [models, keyword])

  return (
    <div className='asst-composer-wrap'>
      <div
        className={cn('asst-composer', dragging && 'drag')}
        onDragOver={event => {
          if (!event.dataTransfer?.types.includes('Files')) return
          event.preventDefault()
          setDragging(true)
        }}
        onDragLeave={event => {
          if (event.currentTarget.contains(event.relatedTarget as Node | null)) return
          setDragging(false)
        }}
        onDrop={event => {
          event.preventDefault()
          setDragging(false)
          if (event.dataTransfer?.files?.length) onAddFiles(event.dataTransfer.files)
        }}
      >
        {images.length > 0 ? (
          <div className='asst-atts'>
            {images.map((url, index) => (
              <div className='asst-att' key={`${index}-${url.slice(0, 32)}`}>
                <img className='asst-att-img' src={url} alt={t('图片 {n}', { n: index + 1 })} />
                <Button size='icon-2xs' variant='outline' type='button' className='asst-att-x rounded-full' title={t('移除')} aria-label={t('移除')}
                  onClick={() => onRemoveImage(index)}>
                  <CloseIcon size={9} />
                </Button>
              </div>
            ))}
          </div>
        ) : null}

        {files.length > 0 ? (
          <div className='asst-files'>
            {files.map((file, index) => (
              <span className='asst-file' key={`${file.name}-${index}`} title={file.name}>
                <FileIcon size={13} />
                <span className='asst-file-name'>{file.name}</span>
                <span className='asst-file-size'>{formatFileSize(file.size)}</span>
                <Button size='icon-2xs' variant='ghost' type='button' className='asst-file-x rounded-full' title={t('移除')} aria-label={t('移除')}
                  onClick={() => onRemoveFile(index)}>
                  <CloseIcon size={9} />
                </Button>
              </span>
            ))}
          </div>
        ) : null}

        <Textarea
          className='min-h-[28px] max-h-[168px] resize-none rounded-none border-0 bg-transparent px-1 py-0.5 font-sans text-[13px] leading-[1.65] shadow-none focus:border-0 focus:shadow-none'
          ref={taRef}
          rows={1}
          value={draft}
          disabled={disabled}
          placeholder={disabled
            ? t('等这一轮跑完…')
            : t('发消息，或粘贴图片、JSON 文件…')}
          onChange={event => setDraft(event.currentTarget.value)}
          onPaste={onPaste}
          onKeyDown={event => {
            if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
              event.preventDefault()
              if (canSend) onSend(draft)
            }
          }}
        />

        {imageWarning ? (
          <div className='asst-composer-warn'>
            <ImageIcon size={13} />
            {imageToolsEnabled ? t('参考图会交给图片编辑工具，当前对话模型不会读取图片。') : t('当前模型没有声明图片能力，图可能发不上去。')}
          </div>
        ) : null}

        {files.length > 0 ? (
          <div className='asst-composer-warn'>
            <FileIcon size={13} />
            {t('JSON 文件会以文本发送给当前对话模型，其中的凭证也会保存在这条会话里。')}
          </div>
        ) : null}

        <div className='asst-bar'>
          <Input
            ref={fileRef}
            type='file'
            accept='image/*,.json,.info,application/json'
            multiple
            hidden
            onChange={event => {
              if (event.currentTarget.files?.length) onAddFiles(event.currentTarget.files)
              event.currentTarget.value = ''
            }}
          />
          <Button size='icon-xs' variant='ghost' type='button' className='asst-bar-icon' title={t('添加图片或 JSON 文件（也可粘贴或拖入）')}
            aria-label={t('添加图片或文件')} onClick={() => fileRef.current?.click()}>
            <PlusIcon size={15} />
          </Button>

          <Popover>
            <PopoverTrigger
              render={<Button size='default' variant='ghost' type='button' className={cn('asst-bar-ghost', mode === 'full' && 'is-full')} />}
            >
              {mode === 'full' ? <BoltIcon size={12} /> : <ShieldIcon size={13} />}
              <span>{modeLabel(mode)}</span>
              <ChevronIcon size={11} className='opacity-65' />
            </PopoverTrigger>
            <PopoverContent side='top' align='start' className='asst-mode-pop w-[276px] p-1.5'>
              <RadioGroup value={mode} onValueChange={value => onModeChange(value as PermissionMode)} className='gap-0.5'>
                {(['confirm', 'full'] as const).map(value => (
                  <label key={value} className={cn('asst-mode-opt', mode === value && 'is-active', value === 'full' && 'is-danger')}>
                    <span className='asst-mo-ico'>{value === 'full' ? <BoltIcon size={14} /> : <ShieldIcon size={14} />}</span>
                    <span className='asst-mo-text'>
                      <b>{modeLabel(value)}</b>
                      <small>{value === 'full'
                        ? t('改动类工具直接执行，不再询问')
                        : t('改动类工具先列清单，等你点「执行」')}</small>
                    </span>
                    <RadioGroupItem value={value} className='mt-0.5 ml-auto shrink-0' />
                  </label>
                ))}
              </RadioGroup>
              <p className='asst-pop-note'>
                {t('只跳过确认，不扩大工具权限；每次执行仍会记录在对话里。')}
              </p>
            </PopoverContent>
          </Popover>

          <span className='spacer' />

          <Popover open={pickerOpen} onOpenChange={setPickerOpen}>
            <PopoverTrigger
              render={<Button size='default' variant='ghost' type='button' className='asst-bar-ghost' title={modelWarning || displayModel} />}
            >
              <StatusDot variant={modelAvailable ? 'success' : 'muted'} className='size-[5px]' />
              <span className='asst-pill-model'>{displayModel}</span>
              <span className='asst-pill-sep'>·</span>
              <span className='asst-pill-level'>{displayLevel}</span>
              <ChevronIcon size={11} className='opacity-65' />
            </PopoverTrigger>
            <PopoverContent side='top' align='end' className='asst-picker-pop w-[288px] p-1.5'>
              {pane === 'root' ? (
                <>
                  <Button size='default' variant='ghost' type='button' className='asst-pick-row' onClick={() => { setPane('model'); onRefreshModels() }}>
                    <span className='asst-pr-ico'><LayersIcon size={14} /></span>
                    <span className='asst-pr-k'>{t('模型')}</span>
                    <span className='asst-pr-v'><span className='mono'>{displayModel}</span><MiniChevronIcon size={11} /></span>
                  </Button>
                  <Button size='default' variant='ghost' type='button' className='asst-pick-row' onClick={() => setPane('reason')}>
                    <span className='asst-pr-ico'><SlidersIcon size={14} /></span>
                    <span className='asst-pr-k'>{t('推理等级')}</span>
                    <span className='asst-pr-v'><span>{levelLabel(reasoning)}</span><MiniChevronIcon size={11} /></span>
                  </Button>
                </>
              ) : null}

              {pane === 'model' ? (
                <>
                  <div className='asst-sub-head'>
                    <Button size='default' variant='ghost' type='button' className='asst-back' aria-label={t('返回')} onClick={() => setPane('root')}>
                      <ChevronIcon size={11} className='flip' />
                    </Button>
                    <span>{t('模型')}</span>
                    <Button size='default' variant='ghost' type='button' className='asst-back ml-auto' disabled={modelsLoading}
                      title={t('刷新列表')} aria-label={t('刷新列表')} onClick={onRefreshModels}>↻</Button>
                  </div>
                  <InputGroup className='mb-1.5'>
                    <InputGroupInput autoFocus value={keyword} placeholder={t('搜索模型…')}
                      onChange={event => setKeyword(event.currentTarget.value)} />
                    <InputGroupAddon><SearchIcon size={13} /></InputGroupAddon>
                  </InputGroup>
                  <div className='asst-sub-list'>
                    {filteredModels.length === 0 ? (
                      <Empty className='asst-sub-empty'>
                        {modelsLoading ? t('正在读取可用模型…') : keyword.trim() ? t('没有匹配的模型') : t('没有可用的模型')}
                      </Empty>
                    ) : null}
                    {filteredModels.map(item => (
                      <Button aria-pressed={item.name === selected?.name} size='default' variant='ghost'
                        key={item.name}
                        type='button'
                        disabled={!modelsReady}
                        className={cn('h-auto w-full justify-start whitespace-normal text-left asst-sub-item', item.name === selected?.name && 'selected')}
                        onClick={() => { onPickModel(item.name); setPickerOpen(false) }}
                      >
                        <span className='mono'>{item.name}</span>
                        {item.name === selected?.name ? <span className='asst-sub-tail'><CheckIcon size={12} /></span> : null}
                      </Button>
                    ))}
                  </div>
                </>
              ) : null}

              {pane === 'reason' ? (
                <>
                  <div className='asst-sub-head'>
                    <Button size='default' variant='ghost' type='button' className='asst-back' aria-label={t('返回')} onClick={() => setPane('root')}>
                      <ChevronIcon size={11} className='flip' />
                    </Button>
                    <span>{t('推理等级')}</span>
                    <small>{t('模型不支持时会自动忽略')}</small>
                  </div>
                  <div className='asst-sub-list'>
                    <Button aria-pressed={!reasoning} size='default' variant='ghost'
                      type='button'
                      className={cn('h-auto w-full justify-start whitespace-normal text-left asst-sub-item', !reasoning && 'selected')}
                      onClick={() => { onPickReasoning(null); setPickerOpen(false) }}
                    >
                      <span>{t('跟随模型')}</span>
                      <span className='asst-sub-tail'>{!reasoning ? <CheckIcon size={12} /> : null}</span>
                    </Button>
                    {REASONING_LEVELS.map(level => (
                      <Button aria-pressed={reasoning === level} size='default' variant='ghost'
                        key={level}
                        type='button'
                        className={cn('h-auto w-full justify-start whitespace-normal text-left asst-sub-item', reasoning === level && 'selected')}
                        onClick={() => { onPickReasoning(level); setPickerOpen(false) }}
                      >
                        <span className='mono'>{level}</span>
                        <span className='asst-sub-tail'>{reasoning === level ? <CheckIcon size={12} /> : null}</span>
                      </Button>
                    ))}
                  </div>
                </>
              ) : null}
            </PopoverContent>
          </Popover>

          {busy ? (
            <Button size='icon-sm' variant='destructive' type='button' className='asst-send-btn stop rounded-full' title={t('停止')} aria-label={t('停止')} onClick={onStop}>
              <StopIcon size={11} />
            </Button>
          ) : (
            <Button size='icon-sm' variant='default' type='button' className='asst-send-btn rounded-full' title={t('发送')} aria-label={t('发送')}
              disabled={!canSend} onClick={() => onSend(draft)}>
              <SendIcon size={14} />
            </Button>
          )}
        </div>
      </div>

      {modelWarning ? <p className='asst-composer-warn' role='status'>{modelWarning}</p> : null}
    </div>
  )
}))
