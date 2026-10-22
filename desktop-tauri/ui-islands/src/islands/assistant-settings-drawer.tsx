/**
 * AI 助手的设置抽屉：从对话页右上角「助手设置」打开，落在页面右侧。
 *
 * ── 为什么从设置页搬到这里 ──────────────────────────────────
 * 这三块（默认行为 / 系统提示词 / 工具）都是**聊着聊着就想改**的东西：
 * 换模型、加一个工具、改一句提示词。放在「设置 → AI 助手」意味着每次调整都
 * 要离开对话页、改完再切回来，而对话本身是有上下文的。搬进来之后不打断会话。
 *
 * ── 保存语义 ────────────────────────────────────────────────
 * 与设置页其它开关一致：**改动即时保存**（没有「保存」按钮）。只有系统提示词
 * 的正文在弹窗里编辑，弹窗自带取消 / 保存。
 *
 * ── 数据流 ──────────────────────────────────────────────────
 * 配置由父组件（assistant-page）持有并透传：聊天框里的选择器也读同一份，
 * 抽屉改了要立刻反映到输入区胶囊上。本文件只负责渲染与发起保存请求。
 */

import * as React from 'react'
import {
  Badge,
  Button,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
  Sheet,
  SheetBody,
  SheetContent,
  SheetFooter,
  SheetHeader,
  SheetTitle,
  Switch,
  Textarea,
  cn,
} from '@ui'
import { t } from '../i18n'
import { ASSISTANT_TOOLS, TOOL_META } from './assistant-tools'
import { DEFAULT_SYSTEM_PROMPT, type AssistantConfig, type PermissionMode } from './assistant-state'
import { BoltIcon, CheckIcon, ShieldIcon, SlidersIcon } from './assistant-icons'
import { levelLabel, REASONING_LEVELS } from './assistant-composer'

type Props = {
  open: boolean
  onOpenChange: (open: boolean) => void
  config: AssistantConfig | null
  models: string[]
  busy: boolean
  failed: boolean
  status: string
  onSave: (next: AssistantConfig, note?: string) => void
}

/** 一块配置卡 */
function Section({ title, sub, extra, children }: {
  title: string
  sub?: string
  extra?: React.ReactNode
  children: React.ReactNode
}) {
  return (
    <section className='asst-set-sect'>
      <div className='asst-set-head'>
        <h3>{title}</h3>
        {sub && <span>{sub}</span>}
        {extra}
      </div>
      {children}
    </section>
  )
}

export function AssistantSettingsDrawer({
  open, onOpenChange, config, models, busy, failed, status, onSave,
}: Props) {
  const [editorOpen, setEditorOpen] = React.useState(false)
  const [editorDraft, setEditorDraft] = React.useState('')

  const ready = config !== null
  const enabledTools = React.useMemo(() => {
    const map: Record<string, boolean> = {}
    for (const tool of ASSISTANT_TOOLS) map[tool.id] = config?.tools?.[tool.id] ?? false
    return map
  }, [config])
  const enabledCount = Object.values(enabledTools).filter(Boolean).length
  const customPrompt = Boolean(config?.systemPrompt?.trim())

  const setMode = (mode: PermissionMode) => {
    if (!config) return
    onSave({ ...config, mode }, mode === 'full' ? t('已切换到完全访问') : t('已切换到变更前确认'))
  }

  const toggleTool = (id: string, on: boolean) => {
    if (!config) return
    const label = TOOL_META.find(meta => meta.id === id)?.label || id
    onSave({ ...config, tools: { ...config.tools, [id]: on } }, on ? t('已勾选「{label}」', { label }) : t('已取消「{label}」', { label }))
  }

  const setAll = (on: boolean) => {
    if (!config) return
    const tools: Record<string, boolean> = {}
    for (const tool of ASSISTANT_TOOLS) tools[tool.id] = on
    onSave({ ...config, tools }, on ? t('已全部勾选') : t('已全部取消勾选'))
  }

  return (
    <>
      <Sheet open={open} onOpenChange={onOpenChange}>
        <SheetContent side='right' className='asst-drawer w-[min(408px,94vw)] max-w-none'>
          <SheetHeader className='asst-drawer-head'>
            <SheetTitle>{t('助手设置')}</SheetTitle>
          </SheetHeader>

          <SheetBody className='asst-drawer-body'>
            <div className='asst-moved'>
              <SlidersIcon size={13} />
              {t('原先在「设置 → AI 助手」的配置都搬到这里了')}
            </div>

            {/* ① 默认行为 */}
            <Section title={t('默认行为')} sub={t('新建对话时使用')}>
              <div className='asst-field'>
                <label>{t('模型')}</label>
                {/* 与聊天框里的模型列表同一份数据（父组件透传 names）：两处的
                    候选口径必须逐字一致，否则会出现「这里能选、聊天框选不到」。 */}
                <Select
                  value={config?.model ?? ''}
                  disabled={!ready || busy}
                  onValueChange={value => {
                    if (!config) return
                    const name = String(value)
                    if (!name) return
                    onSave({ ...config, model: name }, t('对话模型已切换'))
                  }}
                >
                  <SelectTrigger className='asst-select-trigger' aria-label={t('模型')}>
                    <SelectValue>
                      {config?.model ?? t('选择模型…')}
                    </SelectValue>
                  </SelectTrigger>
                  <SelectContent>
                    {models.length === 0 && <SelectItem value='none' disabled>{t('没有可用的模型')}</SelectItem>}
                    <SelectGroup>
                      <SelectLabel>{t('已启用的模型')}</SelectLabel>
                      {models.map(name => (
                        <SelectItem key={name} value={name}>{name}</SelectItem>
                      ))}
                    </SelectGroup>
                  </SelectContent>
                </Select>
              </div>

              {/* 思考等级：与模型选择同一种「列表」形态（一行一项、右侧打勾），
                  不做成 chips 九宫格 —— 档位是有序的强弱序列，竖排列表读起来
                  才是「从弱到强」，网格会把它读成一组互不相干的选项。 */}
              <div className='asst-field col'>
                <label>{t('思考等级')}</label>
                <div className='asst-level-list'>
                  <button type='button'
                    className={cn('asst-level-row', !config?.reasoning && 'on')}
                    disabled={!ready || busy}
                    onClick={() => { if (config) onSave({ ...config, reasoning: null }, t('思考等级已切换')) }}>
                    <span>{t('跟随模型')}</span>
                    <span className='asst-level-hint'>{t('不指定，交给映射绑定')}</span>
                    {!config?.reasoning && <CheckIcon size={12} />}
                  </button>
                  {REASONING_LEVELS.map(level => (
                    <button key={level} type='button'
                      className={cn('asst-level-row', config?.reasoning === level && 'on')}
                      disabled={!ready || busy}
                      onClick={() => { if (config) onSave({ ...config, reasoning: level }, t('思考等级已切换')) }}>
                      <span className='mono'>{level}</span>
                      <span className='asst-level-hint'>{levelLabel(level)}</span>
                      {config?.reasoning === level && <CheckIcon size={12} />}
                    </button>
                  ))}
                </div>
              </div>

              <div className='asst-field col'>
                <label>{t('权限模式')}</label>
                <div className='asst-mode-cards'>
                  {(['confirm', 'full'] as const).map(value => (
                    <button key={value} type='button'
                      className={cn('asst-mode-card', config?.mode === value && 'on', value === 'full' && 'warn')}
                      disabled={!ready || busy}
                      onClick={() => setMode(value)}>
                      <span className='asst-mc-ico'>
                        {value === 'full' ? <BoltIcon size={14} /> : <ShieldIcon size={14} />}
                      </span>
                      <span className='asst-mc-txt'>
                        <b>{value === 'full' ? t('完全访问') : t('变更前确认')}</b>
                        <small>{value === 'full' ? t('改动直接执行') : t('改动先列清单')}</small>
                      </span>
                      {config?.mode === value && <CheckIcon size={12} className='asst-mc-check' />}
                    </button>
                  ))}
                </div>
              </div>
            </Section>

            {/* ② 系统提示词 */}
            <Section title={t('系统提示词')} sub={t('作为 system 消息注入')}>
              <div className='asst-switchline'>
                <Switch
                  checked={config?.systemPromptEnabled !== false}
                  disabled={!ready || busy}
                  onCheckedChange={next => {
                    if (!config) return
                    onSave({ ...config, systemPromptEnabled: next }, next ? t('系统提示词已开启') : t('系统提示词已关闭'))
                  }}
                />
                <span>{t('注入系统提示词')}</span>
                <span className='spacer' />
                <Button variant='outline' size='sm' disabled={!ready}
                  onClick={() => {
                    setEditorDraft(config?.systemPrompt?.trim() ? config.systemPrompt : DEFAULT_SYSTEM_PROMPT)
                    setEditorOpen(true)
                  }}>
                  {t('编辑')}
                </Button>
              </div>
              <div className='asst-prompt-preview'>
                {config?.systemPromptEnabled === false
                  ? t('当前：已关闭（助手收不到任何系统提示词，工具定义照发）')
                  : customPrompt
                    ? t('当前：已开启 · 自定义内容 {n} 字', { n: config?.systemPrompt.trim().length ?? 0 })
                    : t('当前：已开启 · 使用内置默认提示词')}
              </div>
            </Section>

            {/* ③ 工具 */}
            <Section
              title={t('工具')}
              sub={t('勾选即注入')}
              extra={<Badge variant='brand' shape='tag'>{t('{n} / {total}', { n: enabledCount, total: ASSISTANT_TOOLS.length })}</Badge>}
            >
              <div className='asst-tools-bar'>
                <Button variant='ghost' size='sm' disabled={!ready || busy} onClick={() => setAll(true)}>{t('全选')}</Button>
                <Button variant='ghost' size='sm' disabled={!ready || busy} onClick={() => setAll(false)}>{t('全不选')}</Button>
              </div>
              {(['read', 'write'] as const).map(kind => (
                <React.Fragment key={kind}>
                  <div className='asst-tool-group'>
                    {kind === 'read' ? t('只读 · 不会改动配置') : t('改动类 · 默认需要确认')}
                  </div>
                  {TOOL_META.filter(meta => {
                    const tool = ASSISTANT_TOOLS.find(item => item.id === meta.id)
                    return kind === 'read' ? tool?.kind === 'read' : tool?.kind !== 'read'
                  }).map(meta => {
                    const tool = ASSISTANT_TOOLS.find(item => item.id === meta.id)
                    return (
                      <label className='asst-tool-row' key={meta.id}>
                        <input type='checkbox'
                          checked={enabledTools[meta.id] === true}
                          disabled={!ready || busy}
                          onChange={event => toggleTool(meta.id, event.currentTarget.checked)} />
                        <span className='tt'>
                          <span className='tt-name'>
                            {meta.label}
                            {meta.tag === 'cost' && <Badge variant='warning' shape='tag'>{t('消耗额度')}</Badge>}
                            {meta.tag === 'danger' && <Badge variant='destructive' shape='tag'>{t('高危')}</Badge>}
                          </span>
                          <span className='tt-desc'>{meta.desc}</span>
                        </span>
                      </label>
                    )
                  })}
                </React.Fragment>
              ))}
            </Section>
          </SheetBody>

          <SheetFooter className='asst-drawer-foot'>
            {/* 这里只显示**常驻**状态（保存失败 / 配置读不出来）。成功的那类
                反馈（「已勾选 X」「已切换到完全访问」）由父组件走顶部 toast，
                不再占这一行 —— 否则每点一下开关，这行字就换一次。 */}
            <span className={cn('asst-hint', failed && 'bad')}>
              {failed && status ? status : t('会话里的选择优先于这里的默认值')}
            </span>
            <span className='spacer' />
            <Button size='sm' onClick={() => onOpenChange(false)}>{t('完成')}</Button>
          </SheetFooter>
        </SheetContent>
      </Sheet>

      {/* 系统提示词编辑弹窗 */}
      <Dialog open={editorOpen} onOpenChange={setEditorOpen}>
        <DialogContent className='w-[min(680px,calc(100vw-48px))]'>
          <DialogHeader><DialogTitle>{t('编辑系统提示词')}</DialogTitle></DialogHeader>
          <DialogBody>
            <p className='hint'>
              {t('这段文本作为 system 消息排在对话最前面。工具的定义与当前配置概览由网关自动附加，不用写在这里。')}
            </p>
            <Textarea
              rows={14}
              className='min-h-[300px]'
              spellCheck={false}
              value={editorDraft}
              onChange={event => setEditorDraft(event.currentTarget.value)}
            />
            <div className='hint'>{t('清空 = 回到内置默认（与「恢复默认」等价）。')}</div>
          </DialogBody>
          <DialogFooter>
            <Button variant='outline' className='mr-auto'
              onClick={() => setEditorDraft(DEFAULT_SYSTEM_PROMPT)}>{t('恢复默认')}</Button>
            <Button variant='outline' onClick={() => setEditorOpen(false)}>{t('取消')}</Button>
            <Button
              variant='default'
              onClick={() => {
                if (!config) return
                const text = editorDraft.trim() === DEFAULT_SYSTEM_PROMPT.trim() ? '' : editorDraft
                onSave({ ...config, systemPrompt: text }, t('系统提示词已保存'))
                setEditorOpen(false)
              }}>{t('保存')}</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}
