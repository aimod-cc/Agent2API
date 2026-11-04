import * as React from 'react'
import {
  Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle,
  Input, Label, Select, SelectContent, SelectItem, SelectTrigger, SelectValue,
  Tooltip, TooltipContent, TooltipTrigger,
} from '@ui'
import * as customSource from './models-custom-source'
import type { ManageView } from './models-custom-source'
import { CUSTOM_LEVEL, bindingReasoningLevel, modelReasoning, levels as reasoningLevels } from './models-reasoning'
import {
  accept, addMappingError, addModelError, customProviderOptions, errorMessage, getSnapshot,
  levelOf, modelRowOf, providerOptions, shared, toast, upstreamOptions, writeAddModel, writeBinding,
  type CustomModelContext, type MappingContext,
} from './models-panel-state'
import { t } from '../i18n'

/* ─── 映射弹窗 ───────────────────────────────── */

/** 映射等级只在客户端未显式指定思考参数时作为默认值。 */
const REASONING_TIP = t('思考等级绑定是这条映射的默认值，客户端显式指定时不覆盖。WorkBuddy 按当前地区的上游目录注入 reasoning_effort；不支持的正向等级取不高于请求的最高可用档，固定档位不能调高，目录未声明时跳过。off / none 不由映射注入；自定义等级只有提供商能确认支持时才发送。注入、归一与跳过原因写入详细日志。')

/** 上游下拉的初值：候选里有就用候选里的原始拼写（value 必须与 option 逐字相同才会选中） */
function pickUpstream(providerId: string, keep: string, locked: boolean): string {
  const options = upstreamOptions(providerId)
  const wanted = (keep || '').trim()
  const hit = options.find(item => item.id.toLowerCase() === wanted.toLowerCase())
  if (hit) return hit.id
  // 锁定态**保住** keep：孤儿映射的 target 恰恰常常不在该家清单里（那正是它挂不上行的原因），
  // 丢掉它会让「设置思考等级」保存时把三元组换成另一个模型，等级存到错的地方而界面上看不出异常
  if (locked && wanted) return wanted
  return options[0]?.id || ''
}

/**
 * 映射弹窗。入口只剩行内两处 —— 行尾「＋ 映射」新建、点别名 chip 改等级 —— 所以 context
 * 总是带着这一行的身份：提供商与上游模型锁定（改了就变成另一条映射），`context.alias` 有值时
 * 连对外名也锁定，只留等级可动（「设置思考等级」形态）。
 *
 * 上游模型**只能是下拉**（数据来自该家当前清单）。这里曾经放开过「手动输入上游模型 ID」，
 * 已删除：对外名只有在目标模型已被广告时才会跟着进广告视图，而入口校验以广告视图为准 ——
 * 手输一个清单里没有的名字，映射建了也永远调不通。要用清单外的模型，正确做法是顶部的
 * 「＋ 添加模型」把它登记进该家清单，之后它自然出现在这里的下拉里。
 */
export function MappingDialog({ context, onClose }: { context: MappingContext; onClose: () => void }) {
  const editing = Boolean(context.alias)
  const [provider, setProvider] = React.useState(() => context.provider || providerOptions()[0]?.id || '')
  const [alias, setAlias] = React.useState(context.alias || '')
  // 初值用上面那个 provider（context.provider 为空时它已经落到了首项），两者必须同源
  const [upstream, setUpstream] = React.useState(() => pickUpstream(provider, context.target, true))
  const [status, setStatus] = React.useState('')
  const [saving, setSaving] = React.useState(false)
  /**
   * 思考等级：`select` 是下拉的当前值（'' = 不覆盖 / 某个等级 / CUSTOM_LEVEL），`custom` 是
   * 自定义输入框里的字。keep 不在候选里（用户上次填的自定义值、或后端调整过候选表）时落到
   * 「自定义」那一项并把值填进输入框 —— 直接丢掉会让「改别的字段时顺手把等级抹掉」。
   *
   * 候选表按**当前选中的目标模型**取（三级回落见 `models-reasoning.levels`）：先把等级绑在
   * A 模型上、再把上游改成 B，B 不认识这个档位时同样落到「自定义」，不静默改值。
   */
  const [reasoning, setReasoning] = React.useState(() => {
    const candidates = reasoningLevels(getSnapshot().data, modelRowOf(provider, upstream))
    const current = editing ? levelOf(context.alias, context.target, context.provider) : ''
    if (current && candidates.includes(current)) return { select: current, custom: '' }
    return { select: current ? CUSTOM_LEVEL : '', custom: current }
  })

  const targetModel = modelRowOf(provider, upstream)
  const reasoningInfo = modelReasoning(targetModel)
  const candidates = reasoningLevels(getSnapshot().data, targetModel)
  const allowsCustomLevel = reasoningInfo.bindingSupported === null
  /** 提供商候选：表格里出现过的家，再补上上下文那一家（孤儿映射可能属于「整个没进表格」的家） */
  const providerChoices = (() => {
    const options = providerOptions()
    for (const id of [context.provider, provider]) {
      if (id && !options.some(item => item.id === id)) {
        options.push({ id, label: t(shared().wbProviders?.labelOf?.(id) || id) })
      }
    }
    return options
  })()
  /** 上游候选：锁定态下 keep 不在清单里时补进去（见 pickUpstream 的说明） */
  const upstreamChoices = (() => {
    const options = upstreamOptions(provider)
    const wanted = (upstream || '').trim()
    if (wanted && !options.some(item => item.id.toLowerCase() === wanted.toLowerCase())) {
      options.unshift({ id: wanted, label: t('{name}（不在该家当前清单里）', { name: wanted }), off: true })
    }
    return options
  })()

  const providerLabel = providerChoices.find(item => item.id === provider)?.label || provider || t('(全局)')
  /**
   * 显示用的等级：已选的档位不在当前候选里（换了上游 / 后端换了候选表）时按「自定义」呈现
   * —— 与初始化同一条规则，值留在输入框里而不是被静默抹掉（派生态，不改 state）。
   */
  const active = reasoning.select && reasoning.select !== CUSTOM_LEVEL && !candidates.includes(reasoning.select)
    ? { select: CUSTOM_LEVEL, custom: reasoning.custom || reasoning.select }
    : reasoning
  const level = active.select === CUSTOM_LEVEL ? active.custom.trim() : active.select
  const showCustomLevel = active.select === CUSTOM_LEVEL
  const effectiveLevel = reasoningInfo.bindingSupported === true
    ? bindingReasoningLevel(level, targetModel) : ''
  const reasoningNotice = reasoningInfo.bindingSupported === false
    ? t('当前通道未声明可用档位，映射等级不会注入；请先获取模型目录。')
    : reasoningInfo.fixedLevel
      ? t('上游固定为 {level}，绑定其他等级也无法调高。', { level: reasoningInfo.fixedLevel })
      : effectiveLevel && effectiveLevel.toLowerCase() !== level.toLowerCase()
        ? t('绑定 {requested}，按当前通道将发送 {effective}。', { requested: level, effective: effectiveLevel })
        : reasoningInfo.source === 'modelsdev'
          ? t('社区参考档位，不代表当前上游通道支持；实际以该通道目录为准。')
          : reasoningInfo.declared && reasoningInfo.levels.length
            ? t('上游可用档位：{levels}', { levels: reasoningInfo.levels.join(' / ') })
            : ''

  /** 换了一家，上游候选整体换掉（保留同名项，切回来时不用重选） */
  function changeProvider(next: string): void {
    setProvider(next)
    setUpstream(pickUpstream(next, upstream, true))
  }

  const duplicateError = !editing && alias.trim() ? addMappingError(provider, alias.trim(), upstream) : ''
  async function save(): Promise<void> {
    if (saving) return
    const wanted = editing ? (context.alias || '') : alias.trim()
    const target = upstream
    if (!wanted) { setStatus(t('请填写对外映射名')); return }
    // 下拉为空 = 这一家清单里一个模型都没有（还没加账号 / 清单没拉到）
    if (!target) { setStatus(t('该提供商当前没有可选的上游模型')); return }
    if (!provider) { setStatus(t('请选择提供商')); return }
    if (level && reasoningInfo.bindingSupported === true && !effectiveLevel) {
      setStatus(t('该等级无法注入，请选择上游可用档位或清除绑定。'))
      return
    }
    if (!editing) {
      const error = addMappingError(provider, wanted, target)
      if (error) { setStatus(error); return }
    }
    setSaving(true)
    setStatus(t('保存中…'))
    try {
      // `reasoning` **总是显式给出**（空串 = 清空绑定）：三元组相同走的也是这条接口，而用户
      // 在这个弹窗里看到的就是他要的结果 —— 传 undefined（= 不改）会让「从 high 改成不覆盖」
      // 这一步静默无效
      accept(await writeBinding(provider, wanted, target, { reasoning: level, create: !editing }))
      setSaving(false)
      onClose()
      const suffix = level ? t(' · 思考等级 {level}', { level }) : ''
      // 展示名走注册表 / 自定义目录：直接印 provider id 时，自定义家会显示成一串
      // custom-3f2a91b04c7e，用户认不出是哪一家
      const label = t(shared().wbProviders?.labelOf?.(provider) || provider)
      toast(editing ? t('✅ 已更新 {name} 的思考等级', { name: wanted }) : t('✅ 已添加映射 {name} → {target}（{label}）{suffix}', { name: wanted, target, label, suffix }))
    } catch (error) {
      setStatus(t('保存失败：{message}', { message: errorMessage(error) }))
      setSaving(false)
    }
  }

  const onEnter = (event: React.KeyboardEvent) => {
    if (event.key !== 'Enter') return
    event.preventDefault()
    void save()
  }

  return (
    <Dialog open onOpenChange={(next, eventDetails) => {
      if (next) return
      // 保存中不许关：关掉会让「到底存没存进去」变成未知状态。必须走 eventDetails.cancel()
      // —— 光「不更新 open prop」拦不住 Base UI 的 store
      if (saving) { eventDetails.cancel(); return }
      onClose()
    }}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{editing ? t('设置思考等级') : t('添加模型映射')}</DialogTitle>
        </DialogHeader>
        <DialogBody>
          <div className='flex flex-col gap-1.5'>
            <Label htmlFor='mapping-provider'>{t('提供商')}</Label>
            {/* 锁定 = 行内入口：提供商与上游模型就是这一行，不允许改 */}
            <Select value={provider} disabled onValueChange={next => changeProvider(String(next))}>
              <SelectTrigger id='mapping-provider' className='w-full'>
                <SelectValue>{providerChoices.find(item => item.id === provider)?.label || provider}</SelectValue>
              </SelectTrigger>
              <SelectContent>
                {providerChoices.map(item => (
                  <SelectItem key={item.id} value={item.id}>{item.label}</SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className='grid grid-cols-[1fr_24px_1fr] items-end gap-2.5'>
            <div className='flex min-w-0 flex-col gap-1.5'>
              <Label htmlFor='mapping-alias'>{t('对外映射名（下游请求时用）')}</Label>
              <Input id='mapping-alias' maxLength={128} placeholder={t('例如 gpt-4o')} autoComplete='off'
                autoFocus={!editing} value={alias} disabled={editing}
                onChange={event => setAlias(event.currentTarget.value)} onKeyDown={onEnter} />
            </div>
            <div className='pb-2 text-center text-muted-foreground'>→</div>
            <div className='flex min-w-0 flex-col gap-1.5'>
              <Label htmlFor='mapping-upstream'>{t('转发到上游模型')}</Label>
              <Select value={upstream} disabled onValueChange={next => setUpstream(String(next))}>
                <SelectTrigger id='mapping-upstream' className='w-full'>
                  <SelectValue>{upstreamChoices.find(item => item.id === upstream)?.label || upstream}</SelectValue>
                </SelectTrigger>
                <SelectContent>
                  {upstreamChoices.map(item => (
                    <SelectItem key={item.id} value={item.id}>
                      {item.label}{item.off ? t('（已禁用）') : ''}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </div>
          {/* 预览：当前选择会变成哪一条映射（等级一起显示，保存前就能核对） */}
          <div className='rounded-md border border-border bg-surface-inset px-3 py-2.5 font-mono text-[12px] text-subtle'>
            {t('下游请求 ')}<b className='text-primary-fg'>{alias.trim() || t('<对外名>')}</b>{t(' → 转发')}{' '}
            <b className='text-primary-fg'>{upstream || t('<上游模型>')}</b>{t('（{label}）', { label: providerLabel })}
            {level ? <>{t(' · 思考等级 ')}<b className='text-primary-fg'>{level}</b></> : null}
          </div>
          <div className='flex flex-col gap-1.5'>
            <div className='flex items-center gap-[7px]'>
              <Label htmlFor='mapping-reasoning'>{t('思考等级')}</Label>
              {/* 问号走组件库 Tooltip（旧的 data-tip + desktop-tooltip.tsx 是同一目标的更简陋版本） */}
              <Tooltip>
                <TooltipTrigger render={<span className='tip-q' tabIndex={0} aria-label={t('思考等级的注入规则')} />}>?</TooltipTrigger>
                <TooltipContent>{REASONING_TIP}</TooltipContent>
              </Tooltip>
            </div>
            <Select value={active.select} onValueChange={next => {
              const value = String(next)
              setReasoning(prev => ({ ...prev, select: value }))
            }}>
              {/* 改等级形态没别的可填，把焦点直接放在等级下拉上（旧实现是 setTimeout 里 focus） */}
              <SelectTrigger id='mapping-reasoning' className='w-full' autoFocus={editing}>
                <SelectValue>{active.select === CUSTOM_LEVEL
                  ? t('自定义等级…')
                  : (active.select || t('不覆盖'))}</SelectValue>
              </SelectTrigger>
              <SelectContent>
                <SelectItem value=''>{t('不覆盖')}</SelectItem>
                {candidates.map(candidate => (
                  <SelectItem key={candidate} value={candidate}>{candidate}</SelectItem>
                ))}
                {allowsCustomLevel || showCustomLevel
                  ? <SelectItem value={CUSTOM_LEVEL}>{t('自定义等级…')}</SelectItem> : null}
              </SelectContent>
            </Select>

            {showCustomLevel ? (
              <Input id='mapping-reasoning-custom' maxLength={32} autoFocus
                placeholder={t('自定义等级（例如 custom-high）')} autoComplete='off'
                disabled={saving} value={active.custom}
                onChange={event => setReasoning(prev => ({ ...prev, custom: event.currentTarget.value }))}
                onKeyDown={onEnter} />
            ) : null}
            {reasoningNotice ? <p className='text-xs leading-[1.65] text-subtle'>{reasoningNotice}</p> : null}
            {reasoningInfo.defaultLevel && !reasoningInfo.fixedLevel
              ? <p className='text-xs text-subtle'>{t('默认档位：{level}', { level: reasoningInfo.defaultLevel })}</p> : null}
          </div>
          <p className='text-xs leading-[1.65] text-subtle'>
            {t('对外名可以是中文、字母、数字与 ')}<code>- _ . / :</code>{t(' 这类符号，但不能含空格或换行，且不超过 128 个字符。同一提供商内，同名绑定最多启用一条；已有启用绑定时不能新增，启用备用绑定会自动关闭其他同名绑定。不同提供商可各启用一条同名绑定，用于主备切换。')}
          </p>
          {/* 状态行：高度固定，出现错误时弹窗不跳高 */}
          <div className='min-h-[18px] text-xs text-subtle' role='status'>{duplicateError || status}</div>
        </DialogBody>
        <DialogFooter>
          <div className='mr-auto' />
          <Button variant='outline' disabled={saving} onClick={onClose}>{t('取消')}</Button>
          <Button variant='default' disabled={saving || Boolean(duplicateError)} onClick={() => void save()}>
            {editing ? t('保存等级') : t('保存映射')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

/* ─── 添加模型弹窗 ───────────────────────────── */

/**
 * 「＋ 添加模型」：手动登记一个「上游目录里没有、但实际能路由」的模型（灰度中的新模型、按
 * 账号下发却没进目录的模型）。登记后它**真的进入该家清单**（后端在 `catalog::manifest_for`
 * 里拼接），于是表格里出现这一行、`/v1/models` 会广告它、路由与转发也都认它 —— 与内置模型
 * 相比只少几个能力位元数据（用户无从知道那些值，编一个等于对下游撒谎）。
 */
export function CustomModelDialog({ initial, onClose }: { initial: CustomModelContext; onClose: () => void }) {
  const [provider, setProvider] = React.useState(initial.provider)
  const [id, setId] = React.useState('')
  const [status, setStatus] = React.useState('')
  const [saving, setSaving] = React.useState(false)

  const choices = (() => {
    const options = customProviderOptions()
    if (provider && !options.some(item => item.id === provider)) {
      options.push({ id: provider, label: t(shared().wbProviders?.labelOf?.(provider) || provider) })
    }
    return options
  })()
  // 候选里没有当前值时退回首项（原生 select 赋一个不存在的值也是这个结果）
  const providerValue = choices.some(item => item.id === provider) ? provider : (choices[0]?.id || '')
  const providerLabel = choices.find(item => item.id === providerValue)?.label || providerValue || t('(未选)')
  const duplicateError = id.trim() ? addModelError(providerValue, id.trim()) : ''

  async function save(): Promise<void> {
    if (saving) return
    const value = id.trim()
    if (!providerValue) { setStatus(t('请选择提供商')); return }
    if (!value) { setStatus(t('请填写上游模型 ID')); return }
    const error = addModelError(providerValue, value)
    if (error) { setStatus(error); return }
    setSaving(true)
    setStatus(t('保存中…'))
    try {
      const next = await writeAddModel(providerValue, value)
      accept(next)
      setSaving(false)
      onClose()
      const label = choices.find(item => item.id === providerValue)?.label || providerValue
      if (customSource.isCustom(providerValue)) {
        // 自定义家的清单就是用户自己的登记表，登记了必然出现在表里，所以只有一句成功提示
        toast(t('✅ 已登记模型 {name}（{label}）', { name: value, label }))
      } else {
        // 内置家：登记成功但表格里看不到这一行时，必须说清为什么 —— 表格只列「当前有可用
        // 登录态」的家，给一个还没加账号的家登记模型不会立刻出现。不说的话用户会以为没保存成功
        const view = next as ManageView | null
        const visible = Array.isArray(view?.models)
          && view.models.some(item => item.id === value && (item.provider || '') === providerValue)
        if (visible) toast(t('✅ 已登记自定义模型 {name}（{label}）', { name: value, label }))
        else toast(t('✅ 已登记 {name}（{label}），但该提供商还没有可用账号，这一行要加上账号后才会显示', { name: value, label }), 'err')
      }
    } catch (error) {
      setStatus(t('保存失败：{message}', { message: errorMessage(error) }))
      setSaving(false)
    }
  }

  const onEnter = (event: React.KeyboardEvent) => {
    if (event.key !== 'Enter') return
    event.preventDefault()
    void save()
  }

  return (
    <Dialog open onOpenChange={(next, eventDetails) => {
      // 保存中不许关：关掉会让「到底存没存进去」变成未知状态
      if (next) return
      if (saving) { eventDetails.cancel(); return }
      onClose()
    }}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t('添加自定义模型')}</DialogTitle>
        </DialogHeader>
        <DialogBody>
          <div className='flex flex-col gap-1.5'>
            <Label htmlFor='custom-model-provider'>{t('提供商')}</Label>
            <Select value={providerValue} disabled={initial.locked}
              onValueChange={next => setProvider(String(next))}>
              <SelectTrigger id='custom-model-provider' className='w-full'>
                <SelectValue>{providerLabel}</SelectValue>
              </SelectTrigger>
              <SelectContent>
                {choices.map(item => (
                  <SelectItem key={item.id} value={item.id}>{item.label}</SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className='flex flex-col gap-1.5'>
            <Label htmlFor='custom-model-id'>{t('上游模型 ID')}</Label>
            <Input id='custom-model-id' maxLength={128} placeholder={t('例如 gpt-5.5-preview')}
              autoComplete='off' spellCheck={false} autoFocus value={id}
              onChange={event => setId(event.currentTarget.value)} onKeyDown={onEnter} />
          </div>
          <div className='rounded-md border border-border bg-surface-inset px-3 py-2.5 font-mono text-[12px] text-subtle'>
            {t('在 ')}<b className='text-primary-fg'>{providerLabel}</b>{t(' 上登记上游模型')}{' '}
            <b className='text-primary-fg'>{id.trim() || t('<上游模型 ID>')}</b>{t('（登记后即可用这个名字请求）')}
          </div>
          <p className='text-xs leading-[1.65] text-subtle'>
            {t('填上游真正认识的那个模型 ID（不是给下游用的名字）。登记后它会进入该家的模型清单： 表格里出现这一行、')}<code>/v1/models</code>{t(' 会广告它、请求它也会被转发到这家。 支持字母、数字与 ')}<code>- _ . / :</code>{t('。')}
          </p>
          <p className='text-xs leading-[1.65] text-subtle'>
            {t('「来源」列会标成')}<b>{t('手动')}</b>{t('，与远程目录 / 内置清单区分开。删除自定义模型是')}<b>{t('直接移除')}</b>{t('这条登记（不像内置模型那样只是隐藏），因为它的存在完全由这次登记决定。')}
          </p>
          <div className='min-h-[18px] text-xs text-subtle' role='status'>{duplicateError || status}</div>
        </DialogBody>
        <DialogFooter>
          <div className='mr-auto' />
          <Button variant='outline' disabled={saving} onClick={onClose}>{t('取消')}</Button>
          <Button variant='default' disabled={saving || Boolean(duplicateError)} onClick={() => void save()}>{t('保存')}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

