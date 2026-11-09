/**
 * 图片兼容设置面板（桌面与手机端共用）。
 *
 * ── 设计口径 ──────────────────────────────────────────────────
 *   · 模型看不了图片时只有两种办法，所以这里是**两块固定设置**，不是可以增删的
 *     配置列表：每块只回答两件事 —— 「哪些模型」和「用哪只视觉模型」；
 *   · 没被任何名单选中的模型、以及本来就能看图的模型，一律原样放行，不需要
 *     再写一条「不做兼容」的规则去声明；
 *   · **没有保存步骤**：跟设置页里网关开关、重试、超时一样，改完立即提交。
 *     所以这里没有草稿、没有「撤销 / 保存」，也就没有保存前的授权弹窗 ——
 *     「图片会交给哪只模型」由每块底部的「实际处理」那行直接说明；
 *   · 两块名单重叠时按辅助识图处理，界面把这件事直接指出来，而不是静默二选一。
 *
 * ── 与后端契约 ────────────────────────────────────────────────
 * 读写仍然是 `/api/vision-compatibility` 的 GET / PUT（`revision`）。
 * 旧版（全局二选一 + 模型单独配置）由服务端按相同语义展开成名单返回，
 * 展开结果只在用户真的改了一处之后才按新形式存回去。
 */

import { useCallback, useEffect, useRef, useState } from 'react'
import {
  Alert, Button, Card, CardAction, CardContent, CardHeader, CardTitle, Collapsible, CollapsibleContent,
  CollapsibleTrigger, Input, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, StatusDot, cn,
} from '@ui'
import type { Translate } from './assistant-read-types'
import { FooterNote, Group, Row, SwitchRow, Tip } from './settings-vision-rows'
import { VisionReasoningSelect } from './settings-vision-editors'
import { VisionAssistCard, VisionRerouteCard, visionModelLabel } from './settings-vision-way'
import { visionOverlap, type VisionCompatApi, type VisionSettings, type VisionView } from './settings-vision-types'

/** 「更多设置」入口上的小箭头（与项目其它内联图标同一套几何：24 画布、currentColor） */
function Chevron({ open }: { open: boolean }) {
  return <svg viewBox='0 0 24 24' width={15} height={15} aria-hidden='true' fill='none' stroke='currentColor'
    strokeWidth='1.8' strokeLinecap='round' strokeLinejoin='round'
    className={cn('flex-none text-muted-foreground transition-transform duration-[150ms]', open && 'rotate-90')}>
    <path d='M10 6.5 15.5 12 10 17.5' />
  </svg>
}

function InfoIcon() {
  return <svg viewBox='0 0 24 24' width={14} height={14} aria-hidden='true' fill='none' stroke='currentColor'
    strokeWidth='1.8' strokeLinecap='round' className='mt-0.5 flex-none'>
    <circle cx='12' cy='12' r='9' /><path d='M12 8h.01M11 11.5h1V16h1' />
  </svg>
}

/** 数字项：编辑期间只改本地文案，失焦 / 回车才提交（与设置页其它数字项同一套做法） */
function TimeoutField({ value, disabled, onCommit, t }: {
  value: number
  disabled: boolean
  onCommit(next: number): void
  t: Translate
}) {
  const [text, setText] = useState<string | null>(null)
  const shown = text ?? String(value)
  const parsed = Number(shown)
  const valid = Number.isInteger(parsed) && parsed >= 5 && parsed <= 120
  return <span className='flex items-center gap-2'>
    <Input className={cn('w-20', !valid && 'border-destructive')} type='number' min={5} max={120} step={1}
      disabled={disabled} aria-label={t('兼容处理最长等待')} value={shown}
      onChange={event => setText(event.currentTarget.value)}
      onKeyDown={event => { if (event.key === 'Enter') event.currentTarget.blur() }}
      onBlur={() => { if (text !== null) { if (valid) onCommit(parsed); setText(null) } }} />
    <span className='text-[12px] text-muted-foreground'>{t('秒')}</span>
  </span>
}

function message(failure: unknown): string {
  return failure instanceof Error ? failure.message : String(failure)
}

export function VisionCompatibilityPanel({ api, t, active = true }: { api: VisionCompatApi; t: Translate; active?: boolean }) {
  const [view, setView] = useState<VisionView | null>(null)
  const [draft, setDraft] = useState<VisionSettings | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [more, setMore] = useState(false)
  const alive = useRef(true)
  const sequence = useRef(0)
  /** 服务端最新状态：提交要用它的 revision，失败回滚也回到它 */
  const latest = useRef<VisionView | null>(null)
  /** 提交串行化：改动是逐次点出来的，但在途请求不能并发（否则后一次会拿到旧的 revision） */
  const chain = useRef<Promise<unknown>>(Promise.resolve())

  const adopt = (next: VisionView) => {
    latest.current = next
    setView(next)
    setDraft(structuredClone(next.config))
  }

  const load = useCallback(async () => {
    const token = ++sequence.current
    setBusy(true); setError('')
    try {
      const next = await api.load()
      if (alive.current && token === sequence.current) adopt(next)
    } catch (failure) {
      if (alive.current && token === sequence.current) setError(message(failure))
    } finally {
      if (alive.current && token === sequence.current) setBusy(false)
    }
  }, [api])

  useEffect(() => {
    alive.current = true
    return () => { alive.current = false; ++sequence.current }
  }, [])
  useEffect(() => { if (active && !view) void load() }, [active, view, load])

  /** 改一处就提交一次：界面上没有保存按钮，这里也不留草稿状态 */
  const commit = (next: VisionSettings) => {
    setDraft(next)
    setError('')
    chain.current = chain.current.then(async () => {
      const current = latest.current
      if (!current) return
      setBusy(true)
      try {
        const saved = await api.save({ config: next, revision: current.revision })
        if (!alive.current) return
        latest.current = saved
        setView(saved)
      } catch (failure) {
        if (!alive.current) return
        setError(message(failure))
        setDraft(structuredClone(latest.current?.config ?? current.config))
      } finally {
        if (alive.current) setBusy(false)
      }
    })
  }

  if (!view || !draft) {
    return <Card render={<section />} className='panel'>
      <CardHeader><CardTitle>{t('图片兼容')}</CardTitle></CardHeader>
      <CardContent className='flex flex-col gap-4'>
        <p className='m-0 text-[12.5px] text-muted-foreground'>{busy ? t('加载中…') : t('图片兼容设置读取失败')}</p>
        {error ? <Alert variant='destructive'>{error}</Alert> : null}
      </CardContent>
    </Card>
  }

  const overlap = visionOverlap(draft)
  const patch = (next: Partial<VisionSettings>) => commit({ ...draft, ...next })
  // 思考等级是逐只视觉模型存的；两块用同一个选择，所以改一次就写到当前所有目标上
  const levels = [
    ...(draft.assist.target ? [draft.assist.target.reasoning] : []),
    ...draft.reroute.targets.map(target => target.reasoning),
  ]
  const reasoning = levels.length > 0 && levels.every(level => level === levels[0]) ? levels[0] : ''
  const setReasoning = (next: string) => patch({
    assist: { ...draft.assist, target: draft.assist.target ? { ...draft.assist.target, reasoning: next } : null },
    reroute: { ...draft.reroute, targets: draft.reroute.targets.map(target => ({ ...target, reasoning: next })) },
  })

  return <Card render={<section />} className='panel'>
    <CardHeader>
      <CardTitle>{t('图片兼容')}</CardTitle>
      <Tip label={t('图片兼容')} text={t('模型看不了图片时只有两种办法：让别的模型替它读图，或者把整次请求直接交给能看图的模型。没配到的模型、以及本来就能看图的模型，一律原样放行；每个请求只用其中一种方式，不会两种串着做，处理失败也不会改走另一种。改动立即生效，不需要保存。')} />
      <span className='flex items-center gap-1.5 text-[12px] text-muted-foreground'>
        <StatusDot variant={draft.enabled ? 'success' : 'muted'} />{draft.enabled ? t('已开启') : t('已关闭')}
      </span>
      <CardAction>
        <Button size='sm' variant='ghost' disabled={busy} onClick={() => void load()}>{t('刷新')}</Button>
      </CardAction>
    </CardHeader>
    <CardContent className='flex flex-col gap-4'>
      <p className='m-0 max-w-[760px] text-[11.5px] leading-relaxed text-muted-foreground'>
        {t('规则全局生效，不受原路由池限制；指定模型按自己的路由选账号，网关 Key 权限仍然生效。')}
      </p>
      {view.legacyExpanded ? <p className='m-0 max-w-[760px] text-[11.5px] leading-relaxed text-muted-foreground'>
        {t('原来的「全局方式 + 模型单独配置」已按相同意思自动填成下面的模型名单，可以照用也可以重新调整；改动一处之后就会按新形式存下来。')}
      </p> : null}
      {error ? <Alert variant='destructive'>{error}</Alert> : null}

      <Group>
        <SwitchRow first label={t('启用图片兼容')}
          checked={draft.enabled} disabled={busy} onCheckedChange={enabled => patch({ enabled })} />
      </Group>
      {!draft.enabled ? <p className='m-0 max-w-[760px] text-[11.5px] leading-relaxed text-muted-foreground'>
        {t('图片兼容当前是关闭的：下面两块设置照样能改、会被保留，但不会对任何请求生效。')}
      </p> : null}

      <div className='grid items-start gap-[18px] [grid-template-columns:repeat(auto-fit,minmax(min(430px,100%),1fr))]'>
        <VisionAssistCard value={draft.assist} models={view.models} reasoning={reasoning} disabled={busy} t={t}
          onChange={assist => patch({ assist: { ...draft.assist, ...assist } })} />
        <VisionRerouteCard value={draft.reroute} models={view.models} reasoning={reasoning}
          maxTargets={view.maxTargets} disabled={busy} t={t}
          onChange={reroute => patch({ reroute: { ...draft.reroute, ...reroute } })} />
      </div>

      {overlap.length ? <div className='flex items-start gap-2 rounded-md border border-warning-bd bg-warning-soft px-3 py-2.5 text-[12px] leading-relaxed text-warning'>
        <InfoIcon />
        <span>{t('{models} 同时出现在两块设置里，会按「辅助识图」处理。想改的话，把其中一个从这边的模型名单里去掉。', {
          models: overlap.map(entry => visionModelLabel(view.models, entry, t)).join('、'),
        })}</span>
      </div> : null}

      <Collapsible open={more} onOpenChange={setMore}>
        <div className='flex flex-col gap-2'>
          <CollapsibleTrigger render={<Button variant='ghost'
            className='h-auto w-full justify-between gap-3 rounded-sm px-1 py-1.5' />}>
            <span className='flex min-w-0 flex-wrap items-baseline gap-2'>
              <span className='text-[13px] font-medium text-foreground'>{t('更多设置')}</span>
              <span className='text-[12px] text-muted-foreground'>{t('思考等级、图片能力未知、等待上限')}</span>
            </span>
            <Chevron open={more} />
          </CollapsibleTrigger>
          <CollapsibleContent className='flex flex-col gap-2 pt-2'>
            <Group>
              <Row first label={t('视觉模型的思考等级')} tip={t('只影响读图那一步的思考深度；两块设置统一用这个选择。')}
                className='items-center max-[767px]:flex-col max-[767px]:items-stretch max-[767px]:gap-2' control={
                  <VisionReasoningSelect t={t} value={reasoning} disabled={busy} onValueChange={setReasoning} />
                } />
              <Row label={t('图片能力未知时')} tip={t('模型目录没写明是否支持图片时按这里处理。')} className='items-center max-[767px]:flex-col max-[767px]:items-stretch max-[767px]:gap-2' control={
                <Select value={draft.unknownCapability} disabled={busy}
                  onValueChange={unknownCapability => patch({ unknownCapability: unknownCapability as VisionSettings['unknownCapability'] })}>
                  <SelectTrigger className='w-[min(260px,100%)]' aria-label={t('图片能力未知时')}>
                    <SelectValue>{draft.unknownCapability === 'try_original' ? t('先发送原图，明确拒图后补救') : t('直接按上面的设置兼容')}</SelectValue>
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value='try_original'>{t('先发送原图，明确拒图后补救')}</SelectItem>
                    <SelectItem value='compat'>{t('直接按上面的设置兼容')}</SelectItem>
                  </SelectContent>
                </Select>
              } />
              <Row label={t('兼容处理最长等待')} tip={t('范围 5–120 秒，并受原请求时限约束。')} className='items-center max-[767px]:flex-col max-[767px]:items-stretch max-[767px]:gap-2' control={
                <TimeoutField value={draft.timeoutSeconds} disabled={busy} t={t}
                  onCommit={timeoutSeconds => patch({ timeoutSeconds })} />
              } />
            </Group>
            <FooterNote>{t('图片过大、格式错误或下载失败不会被当作模型不支持图片。')}</FooterNote>
          </CollapsibleContent>
        </div>
      </Collapsible>
    </CardContent>
  </Card>
}
