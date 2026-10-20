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
import { Popover, PopoverContent, PopoverTrigger, cn } from '@ui'
import { t } from '../i18n'
import type { ModelOption } from './assistant-tools'
import type { PermissionMode } from './assistant-state'
import {
  BoltIcon,
  CheckIcon,
  ChevronIcon,
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
  onAddFiles: (files: FileList | File[]) => void
  onRemoveImage: (index: number) => void
  /** 当前模型是否收图（未声明 = 不收）。附件不全时发送前给提示。 */
  modelSupportsImages: boolean
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

/** 按提供商分组（保持后端返回的原始顺序：同一家的模型挨着） */
function groupModels(models: ModelOption[]): Array<{ provider: string; items: ModelOption[] }> {
  const groups: Array<{ provider: string; items: ModelOption[] }> = []
  for (const item of models) {
    const last = groups[groups.length - 1]
    if (last && last.provider === item.provider) last.items.push(item)
    else groups.push({ provider: item.provider, items: [item] })
  }
  return groups
}

export function AssistantComposer(props: Props) {
  const {
    draft, onDraftChange, images, onAddFiles, onRemoveImage,
    modelSupportsImages, models, model, onPickModel,
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
  const canSend = !disabled && (Boolean(draft.trim()) || images.length > 0)
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
      if (file && file.type.startsWith('image/')) files.push(file)
    }
    if (files.length) {
      event.preventDefault()
      onAddFiles(files)
    }
  }

  const groups = React.useMemo(() => {
    const key = keyword.trim().toLowerCase()
    const filtered = key
      ? models.filter(item => item.name.toLowerCase().includes(key) || item.provider.toLowerCase().includes(key))
      : models
    return groupModels(filtered)
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
                <button type='button' className='asst-att-x' title={t('移除')} aria-label={t('移除')}
                  onClick={() => onRemoveImage(index)}>✕</button>
              </div>
            ))}
          </div>
        )}

        <textarea
          ref={taRef}
          rows={1}
          value={draft}
          disabled={disabled}
          placeholder={disabled
            ? t('等这一轮跑完…')
            : t('发消息或做任务…（Enter 发送，Shift+Enter 换行，可粘贴图片）')}
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
            {t('当前模型没有声明图片能力，图可能发不上去。')}
          </div>
        )}

        <div className='asst-bar'>
          {/* 左：添加图片 + 权限模式 */}
          <input
            ref={fileRef}
            type='file'
            accept='image/*'
            multiple
            hidden
            onChange={event => {
              if (event.currentTarget.files?.length) onAddFiles(event.currentTarget.files)
              event.currentTarget.value = ''
            }}
          />
          <button type='button' className='asst-round' title={t('添加图片（也可粘贴或拖入）')}
            aria-label={t('添加图片')} onClick={() => fileRef.current?.click()}>
            <PlusIcon size={17} />
          </button>

          <Popover>
            {/* children 挂在 Trigger 上而不是塞进 render 元素里：Base UI 的 useRender
                会用 Trigger 的 children 覆盖 render 元素的子节点，两边都写会只剩一份。 */}
            <PopoverTrigger
              render={
                <button type='button' className={cn('asst-pill asst-mode-pill', mode === 'full' && 'full')} />
              }
            >
              <span className='asst-pill-ico'>{mode === 'full' ? <BoltIcon size={14} /> : <ShieldIcon size={14} />}</span>
              <span>{modeLabel(mode)}</span>
              <ChevronIcon size={11} className='asst-pill-chev' />
            </PopoverTrigger>
            <PopoverContent side='top' align='start' className='asst-mode-pop w-[276px] p-1.5'>
              {(['confirm', 'full'] as const).map(value => (
                <button
                  key={value}
                  type='button'
                  className={cn('asst-mode-opt', value === 'full' && 'warn', mode === value && 'selected')}
                  onClick={() => onModeChange(value)}
                >
                  <span className='asst-mo-ico'>{value === 'full' ? <BoltIcon size={15} /> : <ShieldIcon size={15} />}</span>
                  <span className='asst-mo-text'>
                    <b>{modeLabel(value)}</b>
                    <small>{value === 'full'
                      ? t('改动类工具直接执行，不再询问')
                      : t('改动类工具先列清单，等你点「执行」')}</small>
                  </span>
                  <span className={cn('asst-radio', mode === value && 'on')} />
                </button>
              ))}
              <p className='asst-pop-note'>
                {t('只跳过确认，不扩大工具权限；每次执行仍会记录在对话里。')}
              </p>
            </PopoverContent>
          </Popover>

          <span className='spacer' />

          {/* 右：模型 · 等级 + 发送 */}
          <Popover open={pickerOpen} onOpenChange={setPickerOpen}>
            <PopoverTrigger
              render={<button type='button' className='asst-pill asst-model-pill' />}
            >
              <span className={cn('asst-dot', model && 'ok')} />
              <span className='asst-pill-model'>{model || t('选择模型…')}</span>
              <span className='asst-pill-sep'>·</span>
              <span className='asst-pill-level'>{levelLabel(reasoning)}</span>
              <ChevronIcon size={11} className='asst-pill-chev' />
            </PopoverTrigger>
            <PopoverContent side='top' align='end' className='asst-picker-pop w-[288px] p-1.5'>
              {pane === 'root' && (
                <>
                  <button type='button' className='asst-pick-row' onClick={() => setPane('model')}>
                    <span className='asst-pr-ico'><LayersIcon size={14} /></span>
                    <span className='asst-pr-k'>{t('模型')}</span>
                    <span className='asst-pr-v'><span className='mono'>{model || t('未选择')}</span><MiniChevronIcon size={11} /></span>
                  </button>
                  <button type='button' className='asst-pick-row' onClick={() => setPane('reason')}>
                    <span className='asst-pr-ico'><SlidersIcon size={14} /></span>
                    <span className='asst-pr-k'>{t('推理等级')}</span>
                    <span className='asst-pr-v'><span>{levelLabel(reasoning)}</span><MiniChevronIcon size={11} /></span>
                  </button>
                </>
              )}

              {pane === 'model' && (
                <>
                  <div className='asst-sub-head'>
                    <button type='button' className='asst-back' onClick={() => setPane('root')}>
                      <ChevronIcon size={11} className='flip' />
                    </button>
                    <span>{t('模型')}</span>
                  </div>
                  <label className='asst-search'>
                    <SearchIcon size={13} />
                    <input
                      autoFocus
                      value={keyword}
                      placeholder={t('搜索模型…')}
                      onChange={event => setKeyword(event.currentTarget.value)}
                    />
                  </label>
                  <div className='asst-sub-list'>
                    {groups.length === 0 && <div className='asst-sub-empty'>{t('没有可用的模型')}</div>}
                    {groups.map(group => (
                      <div key={group.provider}>
                        <div className='asst-sub-group'>{group.provider}</div>
                        {group.items.map(item => (
                          <button
                            key={`${group.provider}-${item.name}`}
                            type='button'
                            className={cn('asst-sub-item', item.name === model && 'selected')}
                            onClick={() => { onPickModel(item.name); setPickerOpen(false) }}
                          >
                            <span className='mono'>{item.name}</span>
                            <span className='asst-sub-tail'>
                              {item.viaMapping && <em>{t('映射')}</em>}
                              {item.name === model && <CheckIcon size={12} />}
                            </span>
                          </button>
                        ))}
                      </div>
                    ))}
                  </div>
                </>
              )}

              {pane === 'reason' && (
                <>
                  <div className='asst-sub-head'>
                    <button type='button' className='asst-back' onClick={() => setPane('root')}>
                      <ChevronIcon size={11} className='flip' />
                    </button>
                    <span>{t('推理等级')}</span>
                    <small>{t('模型不支持时会自动忽略')}</small>
                  </div>
                  <div className='asst-sub-list'>
                    <button
                      type='button'
                      className={cn('asst-sub-item', !reasoning && 'selected')}
                      onClick={() => { onPickReasoning(null); setPickerOpen(false) }}
                    >
                      <span>{t('跟随模型')}</span>
                      <span className='asst-sub-tail'>{!reasoning && <CheckIcon size={12} />}</span>
                    </button>
                    {REASONING_LEVELS.map(level => (
                      <button
                        key={level}
                        type='button'
                        className={cn('asst-sub-item', reasoning === level && 'selected')}
                        onClick={() => { onPickReasoning(level); setPickerOpen(false) }}
                      >
                        <span className='mono'>{level}</span>
                        <span className='asst-sub-tail'>{reasoning === level && <CheckIcon size={12} />}</span>
                      </button>
                    ))}
                  </div>
                </>
              )}
            </PopoverContent>
          </Popover>

          {busy ? (
            <button type='button' className='asst-send stop' title={t('停止')} aria-label={t('停止')} onClick={onStop}>
              <StopIcon size={13} />
            </button>
          ) : (
            <button type='button' className='asst-send' title={t('发送')} aria-label={t('发送')}
              disabled={!canSend} onClick={onSend}>
              <SendIcon size={16} />
            </button>
          )}
        </div>
      </div>

      <div className='asst-footline'>
        <span>{t('Enter 发送 · Shift+Enter 换行')}</span>
      </div>
    </div>
  )
}

export { levelLabel }
