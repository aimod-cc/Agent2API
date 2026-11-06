/**
 * 图片兼容设置面板。
 *
 * ── 设计口径（产品取舍，2026-11 与需求方确认）─────────────────
 *   · 打开就能看懂：一屏之内只有四行 —— 一个总开关、一个二选一（处理方式）、
 *     一个模型选择（切换视觉模型时是「用哪只」的两个选择），
 *     以及一行「现在会这样处理」的结果预览：把这次设置实际会发生什么写成人话；
 *   · 「视觉模型」把旧的「来源（沿用 / 单独指定）＋ 搜索 ＋ 模型 ＋ 思考等级」
 *     并成一个下拉，第一项就是「沿用工具服务的视觉分析模型」；
 *   · 复用识读结果、备选顺序、模型单独配置、图片能力未知时、等待上限
 *     全部收进默认折叠的「更多设置」—— 能力一个没少，只是不再挡在眼前。
 *
 * ── 与后端契约 ────────────────────────────────────────────
 * 读写仍然是 `/api/vision-compatibility` 的 GET / PUT（`revision` + `consent`），
 * 设置结构与字段一个都没改：enabled / mode / assist / reroute / overrides /
 * unknownCapability / timeoutSeconds。合并下拉只是把「沿用」与「单独指定」
 * 两种情况收进了同一个控件：选到具体模型即旧的「单独指定 + 目标模型」。
 */

import { useCallback, useEffect, useRef, useState } from 'react'
import {
  Alert, Button, Card, CardAction, CardContent, CardHeader, CardTitle, Collapsible, CollapsibleContent,
  CollapsibleTrigger, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, Input,
  SegmentedControl, Select, SelectContent, SelectItem, SelectTrigger, SelectValue, StatusDot, cn,
} from '@ui'
import type { Translate } from './assistant-read-types'
import { FooterNote, Group, GroupTitle, ListRow, OutcomeRow, Row, SwitchRow } from './settings-vision-rows'
import { VisionModelSelect, VisionOverrideEditor, VisionReasoningSelect } from './settings-vision-editors'
import { visionTargetKey, type VisionCompatApi, type VisionOverride, type VisionSettings, type VisionTarget, type VisionView } from './settings-vision-types'

/** 「更多设置」入口上的小箭头（与项目其它内联图标同一套几何：24 画布、currentColor） */
function Chevron({ open }: { open: boolean }) {
  return <svg viewBox='0 0 24 24' width={15} height={15} aria-hidden='true' fill='none' stroke='currentColor'
    strokeWidth='1.8' strokeLinecap='round' strokeLinejoin='round'
    className={cn('flex-none text-muted-foreground transition-transform duration-[150ms]', open && 'rotate-90')}>
    <path d='M10 6.5 15.5 12 10 17.5' />
  </svg>
}

export function VisionCompatibilityPanel({ api, t, active = true }: { api: VisionCompatApi; t: Translate; active?: boolean }) {
  const [view, setView] = useState<VisionView | null>(null)
  const [draft, setDraft] = useState<VisionSettings | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [saved, setSaved] = useState(false)
  const [confirm, setConfirm] = useState(false)
  const [reload, setReload] = useState(false)
  const [more, setMore] = useState(false)
  const [candidate, setCandidate] = useState('')
  const [editor, setEditor] = useState<{ index?: number; initial?: VisionOverride } | null>(null)
  const alive = useRef(true)
  const sequence = useRef(0)

  const sharedChanged = Boolean(view && draft?.assist.source === 'shared'
    && JSON.stringify(view.sharedTarget) !== JSON.stringify(draft.assist.sharedTarget))
  const dirty = Boolean(view && draft && (JSON.stringify(view.config) !== JSON.stringify(draft) || sharedChanged))
  const timeoutValid = Boolean(draft && Number.isInteger(draft.timeoutSeconds)
    && draft.timeoutSeconds >= 5 && draft.timeoutSeconds <= 120)
  const load = useCallback(async () => {
    const token = ++sequence.current
    setBusy(true); setError(''); setSaved(false)
    try {
      const next = await api.load()
      if (alive.current && token === sequence.current) { setView(next); setDraft(structuredClone(next.config)); setReload(false) }
    } catch (failure) {
      if (alive.current && token === sequence.current) setError(failure instanceof Error ? failure.message : String(failure))
    } finally { if (alive.current && token === sequence.current) setBusy(false) }
  }, [api])
  useEffect(() => { alive.current = true; return () => { alive.current = false; ++sequence.current } }, [])
  useEffect(() => { if (active && !view) void load() }, [active, view, load])
  const patch = (value: Partial<VisionSettings>) => { setDraft(previous => previous ? { ...previous, ...value } : null); setSaved(false); setError('') }
  const save = async (consent: boolean) => {
    if (!view || !draft || busy) return
    setBusy(true); setError('')
    try {
      const config = { ...draft, assist: { ...draft.assist, sharedTarget: view.sharedTarget } }
      const next = await api.save({ config, revision: view.revision, consent })
      if (alive.current) { setView(next); setDraft(structuredClone(next.config)); setConfirm(false); setSaved(true) }
    } catch (failure) { if (alive.current) setError(failure instanceof Error ? failure.message : String(failure)) }
    finally { if (alive.current) setBusy(false) }
  }
  const modelName = (target: VisionTarget | VisionOverride) => {
    const item = view?.models.find(model => visionTargetKey(model) === visionTargetKey(target))
    return `${item?.providerLabel || target.provider} · ${item?.name || target.model}`
  }
  const overrideModeLabel = (mode: VisionOverride['mode']) => mode === 'assist' ? t('辅助识图')
    : mode === 'reroute' ? t('切换视觉模型') : t('不做图片兼容')
  const moveTarget = (index: number, direction: number) => {
    if (!draft) return
    const targets = [...draft.reroute.targets]
    const next = index + direction
    if (next < 0 || next >= targets.length) return
    ;[targets[index], targets[next]] = [targets[next], targets[index]]
    patch({ reroute: { ...draft.reroute, targets } })
  }

  if (!view || !draft) {
    return <Card render={<section />} className='panel'>
      <CardHeader>
        <CardTitle>{t('图片兼容')}</CardTitle>
      </CardHeader>
      <CardContent className='flex flex-col gap-4'>
        <p className='m-0 text-[12.5px] text-muted-foreground'>{busy ? t('加载中…') : t('图片兼容设置读取失败')}</p>
        {error ? <Alert variant='destructive'>{error}</Alert> : null}
      </CardContent>
    </Card>
  }

  const assistTarget = draft.assist.source === 'shared' ? view.sharedTarget : draft.assist.target
  const firstTarget = draft.reroute.targets[0]
  const fallbackText = draft.reroute.fallback === 'stop'
    ? t('找不到就停止并说明原因。')
    : firstTarget
      ? t('找不到就按顺序试这 {count} 个备选（先是 {model}）。', { count: draft.reroute.targets.length, model: modelName(firstTarget) })
      : t('找不到就按指定备选顺序尝试（还没加备选）。')
  const outcome = draft.mode === 'assist'
    ? (assistTarget
      ? t('图片先交给 {model} 识读，再由原模型回答这次请求。', { model: modelName(assistTarget) })
      : t('还没有可用的视觉模型。可以在「MCP & Skills」里配好那只视觉分析模型，或把上面的「视觉模型」换成某一只具体模型。'))
    : (draft.reroute.preferCurrentRoute
      ? t('先在当前路由范围内找可用的视觉模型，这次请求由视觉模型直接回答，不回原模型；{fallback}', { fallback: fallbackText })
      : t('这次请求由视觉模型直接回答，不回原模型；{fallback}', { fallback: fallbackText }))

  const candidateModel = view.models.find(model => visionTargetKey(model) === candidate)
  const candidateDuplicated = Boolean(candidateModel
    && draft.reroute.targets.some(item => candidateModel && visionTargetKey(item) === visionTargetKey(candidateModel)))
  const canAddCandidate = Boolean(candidateModel) && !candidateDuplicated && draft.reroute.targets.length < view.maxTargets
  const canRenderAssist = draft.mode === 'assist' || draft.overrides.some(item => item.mode === 'assist')
  const canRenderReroute = draft.mode === 'reroute' || draft.overrides.some(item => item.mode === 'reroute')

  return <Card render={<section />} className='panel'>
    <CardHeader>
      <CardTitle>{t('图片兼容')}</CardTitle>
      <span className='flex items-center gap-1.5 text-[12px] text-muted-foreground'>
        <StatusDot variant={draft.enabled ? 'success' : 'muted'} />{draft.enabled ? t('已开启') : t('已关闭')}
      </span>
      <CardAction>
        <Button size='sm' variant='ghost' disabled={busy} onClick={() => { if (dirty) setReload(true); else void load() }}>{t('刷新')}</Button>
      </CardAction>
    </CardHeader>
    <CardContent className='flex flex-col gap-4'>
      <p className='m-0 max-w-[640px] text-[12.5px] leading-relaxed text-muted-foreground'>
        {t('模型自己看不了图片时，按下面选好的方式处理；能直接看图的请求原样放行，不会多一次识图调用。')}
      </p>
      {error ? <Alert variant='destructive'>{error}</Alert> : null}
      {saved ? <Alert>{t('图片兼容设置已保存，仅对新请求生效。')}</Alert> : null}
      {sharedChanged && view.config.enabled ? <Alert variant='destructive'>{t('共享视觉模型已变更，请重新确认并保存，避免把图片交给未经确认的模型。')}</Alert> : null}

      <Group>
        <SwitchRow first label={t('启用图片兼容')} description={t('默认关闭，先配置好再开启；关闭后配置会保留。')}
          checked={draft.enabled} disabled={busy} onCheckedChange={enabled => patch({ enabled })} />
        <Row label={t('处理方式')} control={
          <SegmentedControl value={draft.mode} disabled={busy} aria-label={t('处理方式')}
            onValueChange={mode => patch({ mode: mode as VisionSettings['mode'] })}
            options={[{ value: 'assist', label: t('辅助识图') }, { value: 'reroute', label: t('切换视觉模型') }]} />
        } />
        {draft.mode === 'assist' ? <Row label={t('视觉模型')}
          description={t('识读这张图用哪只模型，第一项是「MCP & Skills」里配好的那只。')} control={
            <VisionModelSelect models={view.models} t={t} includeShared sharedUnset={!view.sharedTarget} disabled={busy}
              value={draft.assist.source === 'shared' ? 'shared' : (draft.assist.target ? visionTargetKey(draft.assist.target) : '')}
              onValueChange={next => {
                if (next === 'shared') { patch({ assist: { ...draft.assist, source: 'shared' } }); return }
                const model = view.models.find(item => visionTargetKey(item) === next)
                if (model) patch({ assist: { ...draft.assist, source: 'custom', target: { provider: model.provider, model: model.model, reasoning: draft.assist.target?.reasoning || '' } } })
              }} />
          } /> : <>
          <SwitchRow label={t('优先使用当前路由内的视觉模型')}
            description={t('路由组只使用组内成员；普通路由只使用参与范围内的账号和模型。')}
            checked={draft.reroute.preferCurrentRoute} disabled={busy}
            onCheckedChange={preferCurrentRoute => patch({ reroute: { ...draft.reroute, preferCurrentRoute } })} />
          <Row label={t('找不到可用视觉模型时')} control={
            <Select value={draft.reroute.fallback} disabled={busy}
              onValueChange={fallback => patch({ reroute: { ...draft.reroute, fallback: fallback as 'stop' | 'designated' } })}>
              <SelectTrigger className='w-[260px]' aria-label={t('找不到可用视觉模型时')}>
                <SelectValue>{draft.reroute.fallback === 'stop' ? t('停止并说明原因') : t('按指定备选顺序尝试')}</SelectValue>
              </SelectTrigger>
              <SelectContent>
                <SelectItem value='stop'>{t('停止并说明原因')}</SelectItem>
                <SelectItem value='designated'>{t('按指定备选顺序尝试')}</SelectItem>
              </SelectContent>
            </Select>
          } />
        </>}
        <OutcomeRow>{t('现在会这样处理：')}{outcome}</OutcomeRow>
      </Group>

      <Collapsible open={more} onOpenChange={setMore}>
        <div className='flex flex-col gap-2'>
          <CollapsibleTrigger render={<Button variant='ghost'
            className='h-auto w-full justify-between gap-3 rounded-sm px-1 py-1.5' />}>
            <span className='flex min-w-0 flex-wrap items-baseline gap-2'>
              <span className='text-[13px] font-medium text-foreground'>{t('更多设置')}</span>
              <span className='text-[12px] text-muted-foreground'>{t('复用与备选、单独配置、兼容细节')}</span>
            </span>
            <Chevron open={more} />
          </CollapsibleTrigger>
          <CollapsibleContent className='flex flex-col gap-4 pt-2'>
            {draft.mode === 'assist' ? <div className='flex flex-col gap-2'>
              <Group>
                {draft.assist.source === 'custom' ? <Row first label={t('思考等级')}
                  description={t('只影响识读那一步的思考深度。')} control={
                    <VisionReasoningSelect t={t} value={draft.assist.target?.reasoning || ''} disabled={busy || !draft.assist.target}
                      onValueChange={reasoning => { if (draft.assist.target) patch({ assist: { ...draft.assist, target: { ...draft.assist.target, reasoning } } }) }} />
                  } /> : null}
                <SwitchRow first={draft.assist.source !== 'custom'} label={t('复用相同图片的识读结果')}
                  description={t('同一会话、同一问题、相同权限范围内复用；追问新细节会重新识读，缓存只在本机内存。')}
                  checked={draft.assist.reuseResults} disabled={busy}
                  onCheckedChange={reuseResults => patch({ assist: { ...draft.assist, reuseResults } })} />
              </Group>
              <FooterNote>{t('识读会保留图片里的文字、代码、数字、布局与状态；识读失败就停止，不删图后继续回答。')}</FooterNote>
            </div> : null}

            {draft.mode === 'reroute' && draft.reroute.fallback === 'designated' ? <div className='flex flex-col gap-2'>
              <GroupTitle>{t('备选顺序 · 从上到下依次尝试')}</GroupTitle>
              <Group>
                {draft.reroute.targets.length ? draft.reroute.targets.map((item, index) => <ListRow key={visionTargetKey(item)} index={index} first={index === 0}
                  title={modelName(item)}
                  actions={<>
                    <Button size='xs' variant='ghost' disabled={busy || index === 0} onClick={() => moveTarget(index, -1)}>{t('上移')}</Button>
                    <Button size='xs' variant='ghost' disabled={busy || index === draft.reroute.targets.length - 1} onClick={() => moveTarget(index, 1)}>{t('下移')}</Button>
                    <Button size='xs' variant='ghost' disabled={busy}
                      onClick={() => patch({ reroute: { ...draft.reroute, targets: draft.reroute.targets.filter((_, i) => i !== index) } })}>{t('移除')}</Button>
                  </>} />)
                  : <Row first label={t('还没有备选模型')} description={t('找不到可用视觉模型时会先停止并说明原因。')} />}
                <Row label={t('加入备选')} control={<>
                  <VisionModelSelect models={view.models} t={t} value={candidate} disabled={busy} className='w-[220px]'
                    ariaLabel={t('加入备选')} onValueChange={setCandidate} />
                  <Button size='xs' variant='outline' disabled={busy || !canAddCandidate} onClick={() => {
                    if (candidateModel) {
                      patch({ reroute: { ...draft.reroute, targets: [...draft.reroute.targets, { provider: candidateModel.provider, model: candidateModel.model, reasoning: '' }] } })
                      setCandidate('')
                    }
                  }}>{t('加入')}</Button>
                </>} />
              </Group>
              <FooterNote>{candidateDuplicated
                ? t('这只模型已经在备选里了。')
                : t('最多 {count} 个备选；备选仍受当前请求权限与路由范围限制，不会跳到路由组外，也不会转为辅助识图。', { count: view.maxTargets })}</FooterNote>
            </div> : null}

            <Group>
              <Row first label={t('模型单独配置')} description={t('只覆盖指定提供商的指定模型，其余跟随全局。')} control={<>
                <span className='tabular text-[12px] text-muted-foreground'>{draft.overrides.length} / {view.maxOverrides}</span>
                <Button size='xs' variant='outline' disabled={busy || draft.overrides.length >= view.maxOverrides}
                  onClick={() => setEditor({})}>{t('添加')}</Button>
              </>} />
              {draft.overrides.map((entry, index) => <ListRow key={visionTargetKey(entry)} title={modelName(entry)} note={overrideModeLabel(entry.mode)}
                actions={<>
                  <Button size='xs' variant='ghost' disabled={busy} onClick={() => setEditor({ index, initial: entry })}>{t('编辑')}</Button>
                  <Button size='xs' variant='ghost' disabled={busy}
                    onClick={() => patch({ overrides: draft.overrides.filter((_, i) => i !== index) })}>{t('移除')}</Button>
                </>} />)}
            </Group>

            <div className='flex flex-col gap-2'>
              <Group>
                <Row first label={t('图片能力未知时')} description={t('模型目录没写明是否支持图片时按这里处理。')} control={
                  <Select value={draft.unknownCapability} disabled={busy}
                    onValueChange={unknownCapability => patch({ unknownCapability: unknownCapability as VisionSettings['unknownCapability'] })}>
                    <SelectTrigger className='w-[260px]' aria-label={t('图片能力未知时')}>
                      <SelectValue>{draft.unknownCapability === 'try_original' ? t('先发送原图，明确拒图后补救') : t('直接按当前方式兼容')}</SelectValue>
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value='try_original'>{t('先发送原图，明确拒图后补救')}</SelectItem>
                      <SelectItem value='compat'>{t('直接按当前方式兼容')}</SelectItem>
                    </SelectContent>
                  </Select>
                } />
                <Row label={t('兼容处理最长等待')} description={t('范围 5–120 秒，并受原请求时限约束。')} control={
                  <span className='flex items-center gap-2'>
                    <Input className='w-20 text-right' type='number' min={5} max={120} step={1} disabled={busy}
                      aria-label={t('兼容处理最长等待')} value={String(draft.timeoutSeconds)}
                      onChange={event => patch({ timeoutSeconds: Number(event.currentTarget.value) })} />
                    <span className='text-[12px] text-muted-foreground'>{t('秒')}</span>
                  </span>
                } />
              </Group>
              <FooterNote>{t('图片过大、格式错误或下载失败不会被当作模型不支持图片。')}</FooterNote>
            </div>
          </CollapsibleContent>
        </div>
      </Collapsible>

      <div className='flex flex-wrap items-center justify-between gap-3 border-t border-hairline pt-4'>
        <FooterNote>{t('保存后只对之后的新请求生效；两种方式不会互相自动切换。')}</FooterNote>
        <div className='flex flex-wrap items-center gap-3'>
          {!timeoutValid ? <span className='text-[12px] text-destructive'>{t('等待上限需在 5–120 秒之间')}</span>
            : dirty ? <span className='text-[12px] text-muted-foreground'>{t('未保存')}</span> : null}
          <Button disabled={busy || !dirty || !timeoutValid} onClick={() => { if (draft.enabled) setConfirm(true); else void save(false) }}>
            {busy ? t('保存中…') : t('保存设置')}
          </Button>
        </div>
      </div>
    </CardContent>

    <Dialog open={confirm} onOpenChange={open => { if (!busy) setConfirm(open) }}><DialogContent>
      <DialogHeader><DialogTitle>{t('确认图片处理授权')}</DialogTitle></DialogHeader>
      <DialogBody className='space-y-3'>
        <p className='m-0 text-[12.5px] leading-relaxed'>{t('图片与相关对话将交给配置的视觉模型处理，可能增加等待时间和费用。请确认这些服务可以接收你的图片。')}</p>
        <p className='m-0 text-[12.5px] leading-relaxed'>{t('辅助识图保留原模型回答；切换视觉模型会让视觉模型直接回答。本次授权也包含已设置的模型单独配置。')}</p>
        <ul className='m-0 flex list-disc flex-col gap-1 pl-4 text-[12px] leading-relaxed text-muted-foreground'>
          {canRenderAssist ? <li>{t('辅助识图')}：{assistTarget ? modelName(assistTarget) : t('尚未配置视觉分析模型')}</li> : null}
          {canRenderReroute ? <>
            {draft.reroute.preferCurrentRoute ? <li>{t('当前路由内的可用视觉模型')}</li> : null}
            {draft.reroute.fallback === 'designated'
              ? draft.reroute.targets.map(item => <li key={visionTargetKey(item)}>{modelName(item)}</li>) : null}
          </> : null}
        </ul>
      </DialogBody>
      <DialogFooter>
        <Button variant='outline' disabled={busy} onClick={() => setConfirm(false)}>{t('取消')}</Button>
        <Button disabled={busy} onClick={() => void save(true)}>{t('确认并保存')}</Button>
      </DialogFooter>
    </DialogContent></Dialog>

    <Dialog open={reload} onOpenChange={open => { if (!busy) setReload(open) }}><DialogContent>
      <DialogHeader><DialogTitle>{t('重新读取设置')}</DialogTitle></DialogHeader>
      <DialogBody>{t('重新读取会替换当前未保存的草稿。')}</DialogBody>
      <DialogFooter>
        <Button variant='outline' disabled={busy} onClick={() => setReload(false)}>{t('取消')}</Button>
        <Button disabled={busy} onClick={() => void load()}>{t('重新读取')}</Button>
      </DialogFooter>
    </DialogContent></Dialog>

    {editor ? <VisionOverrideEditor models={view.models} initial={editor.initial} t={t} onClose={() => setEditor(null)} onSave={entry => {
      if (draft.overrides.some((item, i) => i !== editor.index && visionTargetKey(item) === visionTargetKey(entry))) return t('同一提供商的同一模型不能重复配置')
      patch({ overrides: editor.index === undefined ? [...draft.overrides, entry] : draft.overrides.map((item, i) => i === editor.index ? entry : item) })
      setEditor(null)
    }} /> : null}
  </Card>
}
