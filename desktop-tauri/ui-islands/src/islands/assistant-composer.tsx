/**
 * AI 助手的输入区：附件行 / 文本域 / 底部工具条。
 *
 * ── 为什么不再用两个并排的 Select ───────────────────────────
 * 模型与思考等级是**同一次请求的两个参数**，各占一个下拉时，用户既看不出
 * 它们是一组，也看不出当前组合。这里合成一个胶囊（`模型 · 等级`），点开是
 * 两行菜单，再点哪一行才展开对应的候选 —— 与用户截图里的形态一致。
 *
 * ── 图片的三个入口 ──────────────────────────────────────────
 *   · 左下角「+」：`<input type=file>`（多选）
 *   · 粘贴：输入框上的 `paste` 事件里取剪贴板文件
 *   · 拖入：composer 的 dragover / drop
 * 三条路都落到同一个 `onAddImages(files)`，由调用方转 data URL 并去重限额。
 *
 * ── 弹层为什么用 Popover ────────────────────────────────────
 * 位置翻转、点外部关闭、Esc、焦点归位都是组件库（Base UI）内建的；
 * 自绘绝对定位在「输入框贴底、弹层要往上翻」这个场景最容易做漏。
 */

import * as React from 'react'
import { Popover, PopoverContent, PopoverTrigger, cn, Button, Textarea, Input, InputGroup, InputGroupAddon, InputGroupInput, StatusDot, RadioGroup, RadioGroupItem } from '@ui'
import { t } from '../i18n'
import { selectedModel, type ModelOption } from './assistant-models'
import type { PermissionMode } from './assistant-state'
import {
  BoltIcon,
  CheckIcon,
  ChevronIcon,
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
import { Empty } from '@ui'
import { formatFileSize, type AssistantTextFile } from '../../../ui-shared/assistant-files'

/** 思考等级候选（与后端 `model_rules::REASONING_LEVELS` 逐字一致，弱 → 强）。
 *  **只在这一处写死**，加档位时与后端一起改（见 assistant-page 的旧注释）。 */
export const REASONING_LEVELS = ['off', 'none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'] as const

/** 附件上限：张数与单张字节。超了给明确提示，不做静默丢弃。 */
export const MAX_IMAGES = 6
export const MAX_IMAGE_BYTES = 10 * 1024 * 1024

/** 模式的展示文案。**写成字面量**（不放进常量表再 t(常量)）：
 *  i18n 扫描器只认 t 的字面量首参，间接调用提不到键。 */
function modeLabel(mode: PermissionMode): string {
  return mode === 'full' ? t('完全访问') : t('变更前确认')
}

/** 等级展示：内部存的是小写，界面统一显示成首字母大写（Max / High…） */
function levelLabel(level: string | null): string {
  if (!level) return t('跟随模型')
  return level === 'xhigh' ? 'xHigh' : level.charAt(0).toUpperCase() + level.slice(1)
}

type Props = {
  draft: string
  onDraftChange: (value: string) => void
  images: string[]
  files: AssistantTextFile[]
  onAddFiles: (files: FileList | File[]) => void
  onRemoveImage: (index: number) => void
  onRemoveFile: (index: number) => void
  /** 当前模型是否收图（未声明 = 不收）。附件不全时发送前给提示。 */
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
  onSend: () => void
  onStop: () => void
}

export function AssistantComposer(props: Props) {
  const {
    draft, onDraftChange, images, files, onAddFiles, onRemoveImage, onRemoveFile,
    modelSupportsImages, imageToolsEnabled, modelAvailable, modelsReady, modelsLoading, modelWarning, onRefreshModels,
    models, model, onPickModel,
    reasoning, onPickReasoning, mode, onModeChange,
    busy, awaiting, onSend, onStop,
  } = props

  const taRef = React.useRef<HTMLTextAreaElement | null>(null)
  const fileRef = React.useRef<HTMLInputElement | null>(null)
  const [dragging, setDragging] = React.useState(false)
  const [pickerOpen, setPickerOpen] = React.useState(false)
  /** 模型弹层当前停在哪一屏：两行菜单 → 子列表 */
  const [pane, setPane] = React.useState<'root' | 'model' | 'reason'>('root')
  const [keyword, setKeyword] = React.useState('')

  const disabled = busy || awaiting
  const canSend = !disabled && modelAvailable && (Boolean(draft.trim()) || images.length > 0 || files.length > 0)
  const selected = selectedModel(models, model)
  const displayModel = model || t('选择模型…')
  const displayLevel = model && modelsReady && !modelAvailable ? t('不可用') : levelLabel(reasoning)
  /** 有图但当前模型不收 —— 只提示，不拦（用户可能就是想试试，网关会回明确错误） */
  const imageWarning = images.length > 0 && !modelSupportsImages

  /* 自增高（上限 168px ≈ 8 行）。
     +2px 是给亚像素差的余量：scrollHeight 取整后常比实际内容矮半像素，
     不留这点余量浏览器会误判「还差一行」，于是一行字也挂一条滚动条。 */
  const autoGrow = React.useCallback(() => {
    const el = taRef.current
    if (!el) return
    el.style.height = 'auto'
    el.style.height = `${Math.min(el.scrollHeight + 2, 168)}px`
  }, [])
  React.useEffect(autoGrow, [draft, autoGrow])

  /* 弹层关闭时回到第一屏，下次打开看到的是两行菜单 */
  React.useEffect(() => {
    if (!pickerOpen) {
      setPane('root')
      setKeyword('')
    }
  }, [pickerOpen])

  const onPaste = (event: React.ClipboardEvent<HTMLTextAreaElement>) => {
    const items = event.clipboardData?.items
    if (!items) return
    const files: File[] = []
    for (let i = 0; i < items.length; i += 1) {
      if (items[i].kind !== 'file') continue
      const file = items[i].getAsFile()
      if (file) files.push(file)
    }
    if (files.length) {
      event.preventDefault()
      onAddFiles(files)
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
        {images.length > 0 && (
          <div className='asst-atts'>
            {images.map((url, index) => (
              <div className='asst-att' key={index}>
                <img className='asst-att-img' src={url} alt={t('图片 {n}', { n: index + 1 })} />
                <Button size='icon-2xs' variant='outline' type='button' className='asst-att-x rounded-full' title={t('移除')} aria-label={t('移除')}
                  onClick={() => onRemoveImage(index)}>✕</Button>
              </div>
            ))}
          </div>
        )}

        {files.length > 0 && (
          <div className='asst-files'>
            {files.map((file, index) => (
              <div className='asst-file' key={`${file.name}-${index}`} title={file.name}>
                <FileIcon size={15} />
                <span className='asst-file-name'>{file.name}</span>
                <span className='asst-file-size'>{formatFileSize(file.size)}</span>
                <Button size='icon-2xs' variant='ghost' type='button' className='asst-file-x rounded-full' title={t('移除')} aria-label={t('移除')}
                  onClick={() => onRemoveFile(index)}>✕</Button>
              </div>
            ))}
          </div>
        )}

        <Textarea
          className='min-h-[26px] max-h-[168px] resize-none rounded-none border-0 bg-transparent px-1 py-[3px] font-sans text-[13px] leading-[1.7] shadow-none focus:border-0 focus:shadow-none'
          ref={taRef}
          rows={1}
          value={draft}
          disabled={disabled}
          placeholder={disabled
            ? t('等这一轮跑完…')
            : t('发消息或做任务…（Enter 发送，Shift+Enter 换行，可粘贴图片或 JSON 文件）')}
          onChange={event => onDraftChange(event.currentTarget.value)}
          onPaste={onPaste}
          onKeyDown={event => {
            if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
              event.preventDefault()
              if (canSend) onSend()
            }
          }}
        />

        {imageWarning && (
          <div className='asst-composer-warn'>
            <ImageIcon size={13} />
            {imageToolsEnabled ? t('参考图会交给图片编辑工具，当前对话模型不会读取图片。') : t('当前模型没有声明图片能力，图可能发不上去。')}
          </div>
        )}

        {files.length > 0 && (
          <div className='asst-composer-warn'>
            <FileIcon size={13} />
            {t('JSON 文件会以文本发送给当前对话模型，其中的凭证也会保存在这条会话里。')}
          </div>
        )}

        <div className='asst-bar'>
          {/* 左：添加图片 / JSON 文件 + 权限模式 */}
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
          <Button size='icon-sm' variant='outline' type='button' className='asst-round rounded-full' title={t('添加图片或 JSON 文件（也可粘贴或拖入）')}
            aria-label={t('添加图片或文件')} onClick={() => fileRef.current?.click()}>
            <PlusIcon size={17} />
          </Button>

          <Popover>
            {/* children 挂在 Trigger 上而不是塞进 render 元素里：Base UI 的 useRender
                会用 Trigger 的 children 覆盖 render 元素的子节点，两边都写会只剩一份。 */}
            <PopoverTrigger
              render={
                <Button size='default' variant='ghost' type='button' className={cn('asst-pill asst-mode-pill', mode === 'full' && 'full')} />
              }
            >
              <span className='asst-pill-ico'>{mode === 'full' ? <BoltIcon size={14} /> : <ShieldIcon size={14} />}</span>
              <span>{modeLabel(mode)}</span>
              <ChevronIcon size={11} className='asst-pill-chev' />
            </PopoverTrigger>
            <PopoverContent side='top' align='start' className='asst-mode-pop w-[276px] p-1.5'>
              <RadioGroup value={mode} onValueChange={value => onModeChange(value as PermissionMode)}>
              {(['confirm', 'full'] as const).map(value => (
                <RadioGroupItem
                  key={value} value={value}
                  className='h-auto w-full justify-start gap-2 whitespace-normal rounded-sm border-0 bg-transparent px-2 py-2 text-left data-checked:bg-primary-soft data-checked:text-primary-fg'
                >
                  <span className='asst-mo-ico'>{value === 'full' ? <BoltIcon size={15} /> : <ShieldIcon size={15} />}</span>
                  <span className='asst-mo-text'>
                    <b>{modeLabel(value)}</b>
                    <small>{value === 'full'
                      ? t('改动类工具直接执行，不再询问')
                      : t('改动类工具先列清单，等你点「执行」')}</small>
                  </span>
                </RadioGroupItem>
              ))}
              </RadioGroup>
              <p className='asst-pop-note'>
                {t('只跳过确认，不扩大工具权限；每次执行仍会记录在对话里。')}
              </p>
            </PopoverContent>
          </Popover>

          <span className='spacer' />

          {/* 右：模型 · 等级 + 发送 */}
          <Popover open={pickerOpen} onOpenChange={setPickerOpen}>
            <PopoverTrigger
              render={<Button size='default' variant='ghost' type='button' className='asst-pill asst-model-pill' title={modelWarning || displayModel} />}
            >
              <StatusDot variant={modelAvailable ? 'success' : 'muted'} />
              <span className='asst-pill-model'>{displayModel}</span>
              <span className='asst-pill-sep'>·</span>
              <span className='asst-pill-level'>{displayLevel}</span>
              <ChevronIcon size={11} className='asst-pill-chev' />
            </PopoverTrigger>
            <PopoverContent side='top' align='end' className='asst-picker-pop w-[288px] p-1.5'>
              {pane === 'root' && (
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
              )}

              {pane === 'model' && (
                <>
                  <div className='asst-sub-head'>
                    <Button size='default' variant='ghost' type='button' className='asst-back' onClick={() => setPane('root')}>
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
                    {filteredModels.length === 0 && <Empty className='asst-sub-empty'>
                      {modelsLoading ? t('正在读取可用模型…') : keyword.trim() ? t('没有匹配的模型') : t('没有可用的模型')}
                    </Empty>}
                    {filteredModels.map(item => (
                      <Button aria-pressed={item.name === selected?.name} size='default' variant='ghost'
                        key={item.name}
                        type='button'
                        disabled={!modelsReady}
                        className={cn('h-auto w-full justify-start whitespace-normal text-left asst-sub-item', item.name === selected?.name && 'selected')}
                        onClick={() => { onPickModel(item.name); setPickerOpen(false) }}
                      >
                        <span className='mono'>{item.name}</span>
                        {item.name === selected?.name && <span className='asst-sub-tail'><CheckIcon size={12} /></span>}
                      </Button>
                    ))}
                  </div>
                </>
              )}

              {pane === 'reason' && (
                <>
                  <div className='asst-sub-head'>
                    <Button size='default' variant='ghost' type='button' className='asst-back' onClick={() => setPane('root')}>
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
                      <span className='asst-sub-tail'>{!reasoning && <CheckIcon size={12} />}</span>
                    </Button>
                    {REASONING_LEVELS.map(level => (
                      <Button aria-pressed={reasoning === level} size='default' variant='ghost'
                        key={level}
                        type='button'
                        className={cn('h-auto w-full justify-start whitespace-normal text-left asst-sub-item', reasoning === level && 'selected')}
                        onClick={() => { onPickReasoning(level); setPickerOpen(false) }}
                      >
                        <span className='mono'>{level}</span>
                        <span className='asst-sub-tail'>{reasoning === level && <CheckIcon size={12} />}</span>
                      </Button>
                    ))}
                  </div>
                </>
              )}
            </PopoverContent>
          </Popover>

          {busy ? (
            <Button size='icon' variant='destructive' type='button' className='asst-send stop rounded-full' title={t('停止')} aria-label={t('停止')} onClick={onStop}>
              <StopIcon size={13} />
            </Button>
          ) : (
            <Button size='icon' variant='default' type='button' className='asst-send rounded-full' title={t('发送')} aria-label={t('发送')}
              disabled={!canSend} onClick={onSend}>
              <SendIcon size={16} />
            </Button>
          )}
        </div>
      </div>

      {modelWarning && (
        <p className='asst-composer-warn' role='status'>{modelWarning}</p>
      )}
    </div>
  )
}

export { levelLabel }
