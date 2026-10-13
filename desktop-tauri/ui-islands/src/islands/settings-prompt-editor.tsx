/**
 * 设置页 · 系统提示词的三处「编辑正文」按钮与弹窗。
 *
 * 从 settings-page.tsx 拆出来：那一份已经装了整个设置页的骨架与十几个面板，
 * 这三个编辑器各带自己的草稿状态与弹窗，塞进去只会让它更没法读。依赖是单向的
 * （本文件 import settings-state / settings-model，它们不认识这里），页面只负责
 * 把按钮摆到对应的行里。
 *
 * ── 三处编辑分别是什么 ───────────────────────────────────────
 *   ① 全局提示词正文（替换 / 追加模式下网关要发的那份文本）；
 *   ② 某一家的提示词正文（同一件事，但只对这一家生效）；
 *   ③ 某一家的**网关自带提示词**正文（网关自己装上去的官方三段，见
 *      `core::providers::zcode::OFFICIAL_PROMPT_NOTE`）。
 * ①② 是「客户端 system 怎么处理」那一维的文本来源，③ 是「网关自己装什么」那一维
 * 的文本 —— 两维的开关与文本都互不牵连，界面上也分两行。
 *
 * ── 为什么都是弹窗而不是行内输入框 ──────────────────────────
 * 提示词是几百行级别的文本，行内输入框里既看不清前后文也没法换行阅读；而这一页
 * 的每一行是一个「设置项」，几十行文本常驻会把整页的节奏打散。弹窗里给大输入框
 * 与完整说明，行里只留一个按钮与一句状态。
 *
 * ── 保存语义（与后端一致，写在按钮与提示语里）──────────────
 *   · ① ②：保存后**以这里的文本为准**（优先于提示词文件）；清空 = 这一份不要了、
 *     回落文件 / 内置默认（所以「恢复默认」= 存一份空文本，而不是删掉哪个配置项）；
 *   · ③：三段**一起**保存（上游认的是「三段各自成块」这个形状，编辑器里也是三段
 *     并排）；某一段清空 = 那一段回到官方原文，三段都清空 = 整家回到官方原文。
 */

import * as React from 'react'
import {
  Button,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogSection,
  DialogTitle,
  Label,
  Textarea,
} from '@ui'
import { NOTES, type GatewayBlocks } from './settings-model'
import {
  savePromptText,
  saveProviderGatewayText,
  saveProviderPromptText,
  type PromptState,
  type ProviderPromptOption,
  type ProviderPromptState,
} from './settings-state'
import { t } from '../i18n'

/** 三段的名字与顺序（就是后端 `GatewayBlocks::FIELDS` 的那三个；只在这里各写一遍）。
 *  `short` 是行内状态说明里的短名：gatewayEditedText 用它拼「已改哪几段」。 */
const BLOCK_FIELDS: { key: keyof GatewayBlocks; label: string; short: string; hint: string }[] = [
  { key: 'identity', label: t('第一段（身份句）'), short: t('第一段'), hint: t('官方原文只有一句：声明自己是 ZCode。') },
  { key: 'stable', label: t('第二段（稳定段）'), short: t('第二段'), hint: t('工具用法与环境说明那一大段。') },
  { key: 'dynamic', label: t('第三段（动态段）'), short: t('第三段'), hint: t('沟通方式 / 上下文管理 + Environment 段。') },
]

/** 正文来源的中文说法（'inline' = 这份就是界面里编辑的；'none' = 透传，没有生效正文） */
function sourceText(source: string): string {
  if (source === 'inline') return t('界面里编辑的正文')
  if (source === 'file') return t('提示词文件')
  if (source === 'builtin') return t('内置默认提示词')
  return t('（未生效）')
}

/* ─── ①② 提示词正文（全局 / 某一家的）──────────── */

/**
 * 正文编辑器弹窗。
 *
 * `save` 返回是否成功：失败时留在弹窗里（文本还在，用户可以直接改），
 * 成功才关窗 —— 保存失败却把用户敲的几百行关掉，是最气人的那种交互。
 */
function BodyEditor({ title, text, source, file, disabled, save, onClose }: {
  title: string
  /** 打开时的生效正文（也是没有编辑时窗口里的初始值） */
  text: string
  source: string
  /** 当前配的提示词文件（用于说明「保存后它就不再使用」） */
  file: string
  disabled: boolean
  save: (text: string) => Promise<boolean>
  onClose: () => void
}) {
  const [draft, setDraft] = React.useState(text)
  const [saving, setSaving] = React.useState(false)
  const busy = saving || disabled
  const edited = draft !== text

  async function commit(next: string): Promise<void> {
    setSaving(true)
    try {
      if (await save(next)) onClose()
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open onOpenChange={open => {
      // 保存中不许关：关掉会让「到底存没存进去」变成未知状态（与账号弹窗同款）
      if (open || busy) return
      onClose()
    }}>
      <DialogContent className='w-[min(880px,calc(100vw-48px))]'>
        <DialogHeader><DialogTitle>{title}</DialogTitle></DialogHeader>
        <DialogBody>
          <DialogSection>
            <h3>{t('保存后以这里的文本为准')}</h3>
            <p>
              {t('这一份就是「替换 / 追加」模式下网关要发的 system 正文。')}
              {/* `source === 'none'` 与「当前是透传模式」等价：解析层只在透传时给
                  none（没有生效的正文）—— 见 `config::resolve_choice` */}
              {source === 'none'
                ? t('当前是透传模式：存下来也不会发出，切成「替换 / 追加」之后才生效。')
                : t('当前生效的正文来自{source}{file}。保存后这一份会盖过提示词文件与内置默认{note}。', {
                    source: sourceText(source),
                    file: file.trim() && source !== 'inline' ? t('（文件：{path}）', { path: file.trim() }) : '',
                    note: source !== 'inline' && file.trim()
                      ? t('，文件里后续的改动在清空这里之前不再生效')
                      : '',
                  })}
              {t('想回到原来那份，把这里清空再保存即可（不会删掉你配的文件路径）。')}
            </p>
          </DialogSection>
          <div className='flex flex-col gap-1.5'>
            <Label htmlFor='settings-prompt-body'>{t('提示词正文')}</Label>
            <Textarea
              id='settings-prompt-body'
              rows={18}
              className='min-h-[320px]'
              spellCheck={false}
              disabled={busy}
              value={draft}
              onChange={event => setDraft(event.currentTarget.value)}
            />
            <div className='hint'>
              {draft.trim()
                ? t('{lines} 行、{chars} 字符（按当前草稿计）', { lines: draft.split('\n').length, chars: draft.length })
                : t('空 = 不要这一份，回落提示词文件 / 内置默认提示词。')}
            </div>
          </div>
        </DialogBody>
        <DialogFooter>
          <Button
            variant='outline'
            className='mr-auto'
            // 只在**确实有一份界面正文**时可点：没有这一份时点它不会有任何效果
            // （后端把空串当「清除」，而本来就空 = 什么都不变），点得动却像没反应
            disabled={busy || source !== 'inline'}
            onClick={() => void commit('')}
          >
            {t('清除这里的正文')}
          </Button>
          <Button variant='outline' disabled={busy} onClick={onClose}>{t('取消')}</Button>
          <Button
            variant='default'
            disabled={busy || !edited}
            onClick={() => void commit(draft)}
          >
            {saving ? t('保存中…') : t('保存')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

/** 全局提示词正文的「编辑正文」按钮（对话框随按钮一起挂载） */
export function PromptTextButton({ prompt, locked, busy }: {
  prompt: PromptState
  locked: boolean
  busy: boolean
}) {
  const [open, setOpen] = React.useState(false)
  return (
    <>
      <Button
        id='btn-prompt-edit-body'
        variant='outline'
        disabled={locked || busy}
        onClick={() => setOpen(true)}
      >
        {t('编辑正文')}
      </Button>
      {open ? (
        <BodyEditor
          title={t('编辑提示词正文 · 全局')}
          text={prompt.text}
          source={prompt.source}
          file={prompt.file}
          disabled={busy}
          save={savePromptText}
          onClose={() => setOpen(false)}
        />
      ) : null}
    </>
  )
}

/** 某一家提示词正文的「编辑正文」按钮（与全局那个同款，落点在这一家） */
export function ProviderPromptTextButton({ item, label, locked, busy }: {
  item: ProviderPromptState
  label: string
  locked: boolean
  busy: boolean
}) {
  const [open, setOpen] = React.useState(false)
  return (
    <>
      <Button
        id={`btn-prompt-edit-body-${item.id}`}
        variant='outline'
        disabled={locked || busy}
        onClick={() => setOpen(true)}
      >
        {t('编辑正文')}
      </Button>
      {open ? (
        <BodyEditor
          title={t('编辑提示词正文 · {label}', { label })}
          text={item.text}
          source={item.source}
          file={item.file}
          disabled={busy}
          save={text => saveProviderPromptText(item, text)}
          onClose={() => setOpen(false)}
        />
      ) : null}
    </>
  )
}

/* ─── ③ 网关自带提示词的三段正文 ─────────────────── */

/** 三段正文编辑器（官方原文来自注册表给的后端模板，改过的段在 prompt.gatewayText 里） */
function GatewayEditor({ id, title, official, over, enabled, disabled, onClose }: {
  id: string
  title: string
  official: GatewayBlocks
  /** 已存下来的覆盖（只含改过的段；缺的段用 official 补） */
  over: GatewayBlocks | undefined
  /** 这一家的「网关自带」开关（关着时改的正文也不发出，见下面那句提示） */
  enabled: boolean
  disabled: boolean
  onClose: () => void
}) {
  const [draft, setDraft] = React.useState<GatewayBlocks>(() => ({
    identity: over?.identity?.trim() ? over.identity : official.identity,
    stable: over?.stable?.trim() ? over.stable : official.stable,
    dynamic: over?.dynamic?.trim() ? over.dynamic : official.dynamic,
  }))
  const [saving, setSaving] = React.useState(false)
  const busy = saving || disabled
  const edited = BLOCK_FIELDS.some(field => draft[field.key] !== official[field.key])
  const editedNow = BLOCK_FIELDS.filter(field => over?.[field.key]?.trim()).map(field => field.label)

  async function commit(next: GatewayBlocks | null): Promise<void> {
    setSaving(true)
    try {
      if (await saveProviderGatewayText(id, next)) onClose()
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open onOpenChange={open => {
      if (open || busy) return
      onClose()
    }}>
      <DialogContent className='w-[min(880px,calc(100vw-48px))]'>
        <DialogHeader><DialogTitle>{title}</DialogTitle></DialogHeader>
        <DialogBody>
          <DialogSection>
            <h3>{t('三段各自成块，所以分开编辑')}</h3>
            <p>
              {t('上游按')}<strong>{t('结构')}</strong>
              {t('校验身份：这三段必须各自成块排在最前（实测：三段并成一段会被回 405 / 3012）。因此这里按段编辑，不能合成一段。改过的段以你的文本为准，没改的段继续用官方原文；清空某一段 = 那一段恢复官方原文。')}
              {editedNow.length ? t('当前已改过：{edited}。', { edited: editedNow.join(t('、')) }) : t('当前三段都是官方原文。')}
              {/* 开关关着时改这里的文本不会发出（`apply_start_plan` 直接跳过官方段，
                  连覆盖一起跳过）—— 不说这句，用户会以为自己改的生效了 */}
              {enabled ? '' : t('注意：这一家的「网关自带」开关现在是关的，改过的正文要等开关打开才生效。')}
            </p>
            <p>{NOTES.promptGatewayText}</p>
          </DialogSection>
          {BLOCK_FIELDS.map(field => (
            <div className='flex flex-col gap-1.5' key={field.key}>
              <Label htmlFor={`settings-prompt-block-${id}-${field.key}`}>
                {field.label}
                <span className='ml-2 font-normal text-subtle'>
                  {draft[field.key] === official[field.key] ? t('（官方原文）') : t('（已改）')}
                </span>
              </Label>
              <Textarea
                id={`settings-prompt-block-${id}-${field.key}`}
                rows={field.key === 'identity' ? 2 : 10}
                className={field.key === 'identity' ? 'min-h-[64px]' : 'min-h-[200px]'}
                spellCheck={false}
                disabled={busy}
                value={draft[field.key]}
                onChange={event => {
                  const value = event.currentTarget.value
                  setDraft(current => ({ ...current, [field.key]: value }))
                }}
              />
              <div className='hint'>{field.hint}</div>
            </div>
          ))}
        </DialogBody>
        <DialogFooter>
          <Button
            variant='outline'
            className='mr-auto'
            disabled={busy || !editedNow.length}
            onClick={() => void commit(null)}
          >
            {t('恢复官方原文')}
          </Button>
          <Button variant='outline' disabled={busy} onClick={onClose}>{t('取消')}</Button>
          <Button variant='default' disabled={busy || !edited} onClick={() => void commit(draft)}>
            {saving ? t('保存中…') : t('保存')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

/**
 * 网关自带提示词的「编辑正文」按钮。
 *
 * 官方原文（模板）由后端随 `promptProviderOptions` 一起给：资源坏了时它是 null，
 * 此时**不渲染按钮** —— 没有初始值可编辑，让用户对着一片空白保存只会把官方原文
 * 清掉。开关与它是两件事：拨开关照旧，不因为这里改了正文而改变。
 */
export function GatewayTextButton({ item, option, over, locked, busy }: {
  item: ProviderPromptState
  option: ProviderPromptOption
  /** 这一家已存的正文覆盖（缺省 = 全是官方原文） */
  over: GatewayBlocks | undefined
  locked: boolean
  busy: boolean
}) {
  const [open, setOpen] = React.useState(false)
  const official = option.gatewayBlocks
  if (!official) return null
  return (
    <>
      <Button
        id={`btn-prompt-gateway-text-${item.id}`}
        variant='outline'
        disabled={locked || busy}
        onClick={() => setOpen(true)}
      >
        {t('编辑正文')}
      </Button>
      {open ? (
        <GatewayEditor
          id={item.id}
          title={t('编辑网关自带提示词 · {label}', { label: option.label })}
          official={official}
          over={over}
          enabled={item.gateway}
          disabled={busy}
          onClose={() => setOpen(false)}
        />
      ) : null}
    </>
  )
}

/** 这一家改过哪几段（用于行内的状态说明；没改过给空串） */
export function gatewayEditedText(over: GatewayBlocks | undefined): string {
  if (!over) return ''
  return BLOCK_FIELDS
    .filter(field => (over[field.key] || '').trim())
    .map(field => field.short)
    .join(t('、'))
}
