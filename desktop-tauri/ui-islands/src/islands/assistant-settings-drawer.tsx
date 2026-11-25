/**
 * AI 助手的设置抽屉：从对话页右上角「设置」打开，落在页面右侧。
 *
 * ── 极简克制改版 ─────────────────────────────────────────────
 * 去掉过时的迁移横幅与双层套框卡片，改为发丝线分隔的扁平分区；
 * 思考等级改用紧凑横向芯片；工具行改用无框悬停行 + 克制文本标记；
 * 未打开时不挂载内部复杂表单，消除关闭状态下的闲置重绘开销。
 */

import * as React from 'react'
import {
  Button,
  Checkbox,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Label,
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
import { SearchableSelect } from '../../../ui-shared/searchable-select'
import { ASSISTANT_TOOLS, TOOL_META } from './assistant-tools'
import { DEFAULT_SYSTEM_PROMPT, type AssistantConfig, type PermissionMode } from './assistant-state'
import { levelLabel, REASONING_LEVELS } from './assistant-composer'

type Props = {
  open: boolean
  onOpenChange: (open: boolean) => void
  config: AssistantConfig | null
  models: string[]
  modelAvailable: boolean
  modelsReady: boolean
  modelWarning: string
  onRefreshModels: () => void
  busy: boolean
  failed: boolean
  status: string
  onSave: (next: AssistantConfig, note?: string) => void
  apiKeyField: React.ReactNode
}

function Section({ title, sub, extra, children }: {
  title: string
  sub?: string
  extra?: React.ReactNode
  children: React.ReactNode
}) {
  return (
    <section className='asst-drawer-sect'>
      <div className='asst-drawer-sect-head'>
        <h3>{title}</h3>
        {sub ? <span className='asst-drawer-sect-sub'>{sub}</span> : null}
        {extra}
      </div>
      {children}
    </section>
  )
}

export const AssistantSettingsDrawer = React.memo(function AssistantSettingsDrawer({
  open, onOpenChange, config, models, modelAvailable, modelsReady, modelWarning, onRefreshModels, busy, failed, status, onSave, apiKeyField,
}: Props) {
  const [editorOpen, setEditorOpen] = React.useState(false)
  const [editorDraft, setEditorDraft] = React.useState('')

  React.useEffect(() => {
    if (open) onRefreshModels()
  }, [open, onRefreshModels])

  const ready = config !== null
  const currentModel = React.useMemo(
    () => models.find(name => name.toLowerCase() === config?.model?.trim().toLowerCase()),
    [models, config?.model],
  )
  const modelOptions = React.useMemo(
    () => models.map(name => ({ value: name, label: name })),
    [models],
  )
  const modelDisplay = config?.model
    ? !modelsReady || modelAvailable ? config.model : `${config.model} · ${t('不可用')}`
    : t('选择模型…')

  const enabledTools = React.useMemo(() => {
    const map: Record<string, boolean> = {}
    for (const tool of ASSISTANT_TOOLS) map[tool.id] = config?.tools?.[tool.id] ?? false
    return map
  }, [config?.tools])
  const enabledCount = React.useMemo(
    () => Object.values(enabledTools).filter(Boolean).length,
    [enabledTools],
  )
  const groupedMeta = React.useMemo(() => {
    const read = TOOL_META.filter(meta => ASSISTANT_TOOLS.find(item => item.id === meta.id)?.kind === 'read')
    const write = TOOL_META.filter(meta => ASSISTANT_TOOLS.find(item => item.id === meta.id)?.kind !== 'read')
    return { read, write }
  }, [])
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
        <SheetContent side='right' className='asst-drawer w-[min(396px,94vw)] max-w-none'>
          <SheetHeader className='asst-drawer-head'>
            <SheetTitle>{t('助手设置')}</SheetTitle>
          </SheetHeader>

          {open ? (
            <>
              <SheetBody className='asst-drawer-body'>
                <Section title={t('默认行为')} sub={t('新建对话时使用')}>
                  {apiKeyField}
                  <div className='asst-field'>
                    <Label>{t('模型')}</Label>
                    <SearchableSelect
                      value={currentModel ?? ''}
                      disabled={!ready || busy || !modelsReady}
                      className='asst-select-trigger'
                      aria-label={t('模型')}
                      searchPlaceholder={t('搜索模型')}
                      displayValue={modelDisplay}
                      emptyHint={models.length ? t('没有匹配的模型') : t('没有可用的模型')}
                      options={modelOptions}
                      onValueChange={name => {
                        if (config && name) onSave({ ...config, model: name }, t('对话模型已切换'))
                      }}
                    />
                  </div>
                  {modelWarning ? <p className='asst-hint bad' role='status'>{modelWarning}</p> : null}

                  <div className='asst-field col'>
                    <Label>{t('思考等级')}</Label>
                    <div className='asst-reason-grid'>
                      <Button
                        aria-pressed={!config?.reasoning}
                        size='xs'
                        variant='ghost'
                        type='button'
                        className={cn('asst-reason-chip', !config?.reasoning && 'is-active')}
                        disabled={!ready || busy}
                        onClick={() => { if (config) onSave({ ...config, reasoning: null }, t('思考等级已切换')) }}
                      >
                        {t('跟随模型')}
                      </Button>
                      {REASONING_LEVELS.map(level => (
                        <Button
                          aria-pressed={config?.reasoning === level}
                          size='xs'
                          variant='ghost'
                          key={level}
                          type='button'
                          className={cn('asst-reason-chip', config?.reasoning === level && 'is-active')}
                          disabled={!ready || busy}
                          onClick={() => { if (config) onSave({ ...config, reasoning: level }, t('思考等级已切换')) }}
                        >
                          {levelLabel(level)}
                        </Button>
                      ))}
                    </div>
                  </div>

                  <div className='asst-field col'>
                    <Label>{t('权限模式')}</Label>
                    <div className='grid grid-cols-2 gap-2'>
                      {(['confirm', 'full'] as const).map(value => {
                        const active = config?.mode === value
                        return (
                          <Button
                            key={value}
                            aria-pressed={active}
                            size='default'
                            variant='ghost'
                            type='button'
                            className={cn('asst-mode-flat', active && 'is-active')}
                            disabled={!ready || busy}
                            onClick={() => setMode(value)}
                          >
                            <span className='min-w-0'>
                              <span className={cn('block text-[12px] font-medium', value === 'full' && active ? 'text-warning' : 'text-foreground')}>
                                {value === 'full' ? t('完全访问') : t('变更前确认')}
                              </span>
                              <span className='block text-[11px] font-normal text-muted-foreground'>
                                {value === 'full' ? t('改动直接执行') : t('改动先列清单')}
                              </span>
                            </span>
                          </Button>
                        )
                      })}
                    </div>
                  </div>
                </Section>

                <Section
                  title={t('系统提示词')}
                  extra={
                    <div className='ml-auto flex items-center gap-2.5'>
                      <Button
                        variant='ghost'
                        size='xs'
                        type='button'
                        disabled={!ready}
                        onClick={() => {
                          setEditorDraft(config?.systemPrompt?.trim() ? config.systemPrompt : DEFAULT_SYSTEM_PROMPT)
                          setEditorOpen(true)
                        }}
                      >
                        {t('编辑')}
                      </Button>
                      <Switch
                        checked={config?.systemPromptEnabled !== false}
                        disabled={!ready || busy}
                        aria-label={t('注入系统提示词')}
                        onCheckedChange={next => {
                          if (!config) return
                          onSave({ ...config, systemPromptEnabled: next }, next ? t('系统提示词已开启') : t('系统提示词已关闭'))
                        }}
                      />
                    </div>
                  }
                >
                  <div className='text-[11.5px] leading-[1.6] text-muted-foreground'>
                    {config?.systemPromptEnabled === false
                      ? t('当前：已关闭（助手收不到任何系统提示词，工具定义照发）')
                      : customPrompt
                        ? t('当前：已开启 · 自定义内容 {n} 字', { n: config?.systemPrompt.trim().length ?? 0 })
                        : t('当前：已开启 · 使用内置默认提示词')}
                  </div>
                </Section>

                <Section
                  title={t('工具')}
                  extra={
                    <>
                      <span className='text-[11px] text-muted-foreground tabular-nums'>
                        {t('{n} / {total}', { n: enabledCount, total: ASSISTANT_TOOLS.length })}
                      </span>
                      <span className='ml-auto flex items-center gap-1'>
                        <Button variant='ghost' size='xs' className='h-6 px-1.5 text-[11px]' disabled={!ready || busy} onClick={() => setAll(true)}>{t('全选')}</Button>
                        <Button variant='ghost' size='xs' className='h-6 px-1.5 text-[11px]' disabled={!ready || busy} onClick={() => setAll(false)}>{t('全不选')}</Button>
                      </span>
                    </>
                  }
                >
                  {(['read', 'write'] as const).map(kind => (
                    <div key={kind} className='flex flex-col'>
                      <div className='asst-tool-group'>
                        {kind === 'read' ? t('只读 · 不会改动配置') : t('改动类 · 默认需要确认')}
                      </div>
                      {groupedMeta[kind].map(meta => (
                        <Label className='asst-tool-flat-row' key={meta.id}>
                          <Checkbox
                            checked={enabledTools[meta.id] === true}
                            disabled={!ready || busy}
                            aria-label={meta.label}
                            onCheckedChange={checked => toggleTool(meta.id, checked === true)}
                          />
                          <span className='tt'>
                            <span className='tt-name'>
                              <span>{meta.label}</span>
                              {meta.tag === 'cost' ? <span className='text-[10.5px] font-normal text-warning'>{t('消耗额度')}</span> : null}
                              {meta.tag === 'danger' ? <span className='text-[10.5px] font-normal text-destructive'>{t('高危')}</span> : null}
                            </span>
                            <span className='tt-desc'>{meta.desc}</span>
                          </span>
                        </Label>
                      ))}
                    </div>
                  ))}
                </Section>
              </SheetBody>

              <SheetFooter className='asst-drawer-foot'>
                <span className={cn('asst-hint', failed && 'bad')}>
                  {failed && status ? status : t('会话里的选择优先于这里的默认值')}
                </span>
                <span className='spacer' />
                <Button size='sm' onClick={() => onOpenChange(false)}>{t('完成')}</Button>
              </SheetFooter>
            </>
          ) : null}
        </SheetContent>
      </Sheet>

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
})
