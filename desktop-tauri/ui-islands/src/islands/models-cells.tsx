import * as React from 'react'
import { Badge, Button, Checkbox, Switch, TableCell, cn } from '@ui'
import { t } from '../i18n'
import * as customSource from './models-custom-source'
import type { ManageModel } from './models-custom-source'
import { bindingReasoningLevel, modelReasoning } from './models-reasoning'
import { testBlockReason, type ModelTestTarget } from './model-test-dialog'
import {
  BOOLEAN_KEYS, CAPABILITY_LABELS, capabilitiesOf, capabilityState, capabilityTip, capSourceOf,
  exactTokens, formatTokens, normalizeOverrides, type CapabilityKey,
} from './model-capability'
import {
  bindingKeyOf, bindingsOf, enabledBindingConflicts, esc, formatTime, getSnapshot, levelOf, models,
  openCapability, openMapping, rowKeyOf, runRowAction, same, shared, subscribe, visibilityOf,
  writeBinding, writeRemoveMapping, writeRemoveModel, type Align, type ColumnView, type Binding,
} from './models-panel-state'

const cellClass = (base: string, align?: Align): string => (align ? `${base} ta-${align}` : base)
const formatCredits = (credits: unknown): string => {
  const match = /x\s*([\d.]+)/i.exec(String(credits || ''))
  return match ? `${match[1]}x` : String(credits || '')
}

/* ─── 能力图标（「能力」列的四枚圆点）──────────────────────
   文字徽章 → 图标圆点：四个键在每一行都出现，文字版是满屏的小框框；
   图标版扫表时读的是「几个亮圆」，具体语义由悬停提示兜底。几何与 icons.js
   同一套约定（24 画布 / 描边 2 / 圆头圆角），内联在本文件 —— icons.js 是
   vanilla 层的字符串表，岛这边拿不到，也不该为四枚图标跨层取。 */

const CAP_ICON: Partial<Record<CapabilityKey, React.ReactNode>> = {
  supportsToolCall: (
    // 扳手（Feather「tool」）
    <svg viewBox='0 0 24 24' aria-hidden='true'>
      <path fill='none' stroke='currentColor' strokeWidth='2' strokeLinecap='round' strokeLinejoin='round'
        d='M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z' />
    </svg>
  ),
  supportsImages: (
    // 相片（Feather「image」：框 + 焦点 + 山形）
    <svg viewBox='0 0 24 24' aria-hidden='true'>
      <g fill='none' stroke='currentColor' strokeWidth='2' strokeLinecap='round' strokeLinejoin='round'>
        <rect x='3' y='3' width='18' height='18' rx='2.5' /><circle cx='8.5' cy='8.5' r='1.5' />
        <path d='M21 15l-5-5L5 21' />
      </g>
    </svg>
  ),
  supportsVideo: (
    // 播放（圆 + 三角）：「视频」在小尺寸下比摄像机轮廓更可辨
    <svg viewBox='0 0 24 24' aria-hidden='true'>
      <g fill='none' stroke='currentColor' strokeWidth='2' strokeLinecap='round' strokeLinejoin='round'>
        <circle cx='12' cy='12' r='9' /><path d='M10 8.5l6 3.5-6 3.5z' />
      </g>
    </svg>
  ),
  supportsReasoning: (
    // 灯泡（Lucide「lightbulb」）
    <svg viewBox='0 0 24 24' aria-hidden='true'>
      <g fill='none' stroke='currentColor' strokeWidth='2' strokeLinecap='round' strokeLinejoin='round'>
        <path d='M15 14c.2-1 .7-1.7 1.5-2.5 1-.9 1.5-2.2 1.5-3.5A6 6 0 0 0 6 8c0 1 .2 2.2 1.5 3.5.7.7 1.3 1.5 1.5 2.5' />
        <path d='M9 18h6M10 22h4' />
      </g>
    </svg>
  ),
}

export function ModelCell({ columns, model, selection, togglePick, setTestTarget }: {
  columns: ColumnView[]
  model: ManageModel
  selection: ReadonlySet<string>
  togglePick: (key: string, next: boolean) => void
  setTestTarget: (target: ModelTestTarget) => void
}) {
  const state = React.useSyncExternalStore(subscribe, getSnapshot)
  const busyRow = state.pending.has(rowKeyOf(model))
  /** chip 上那枚等级标（未绑定时也渲染虚线样式，否则「怎么绑等级」会变成一个查不出的问题） */
  function levelBadge(alias: string, target: string, providerId: string, busy: boolean) {
    const level = levelOf(alias, target, providerId)
    const model = models().find(item => same(item.provider, providerId) && same(item.id, target))
    const info = modelReasoning(model)
    const effective = info.bindingSupported === true ? bindingReasoningLevel(level, model) : ''
    const title = info.bindingSupported === false
      ? t('当前通道未声明可用档位，映射等级不会注入；请先获取模型目录。')
      : info.fixedLevel
        ? t('上游固定为 {level}，绑定其他等级也无法调高。', { level: info.fixedLevel })
        : effective && effective.toLowerCase() !== level.toLowerCase()
          ? t('绑定 {requested}，按当前通道将发送 {effective}。', { requested: level, effective })
          : level ? t('思考等级 {level}（点击修改）', { level }) : t('设置思考等级（当前未绑定）')
    return (
      // 16px 小件（2xs 档）：原来那枚 mono 10px 的小药丸。已绑定走实心档、未绑定走虚线档，
      // 与旧 CSS 的 `.alias-level` / `.alias-level.unset` 同一套语义（mono 粗体也照旧）；
      // shadow-none 是清掉通用 button 规则的投影（旧 CSS 同样显式清过）
      <Button variant={level ? 'secondary' : 'dashed'} size='2xs' disabled={busy}
        className='font-mono font-bold shadow-none'
        title={title}
        onClick={() => openMapping({ alias, target, provider: providerId })}>
        {info.fixedLevel ? `${t('固定思考档位')} ${info.fixedLevel}` : effective && effective.toLowerCase() !== level.toLowerCase()
          ? `${level} → ${effective}` : level || t('＋等级')}
      </Button>
    )
  }

  /** 映射 chips：每条 chip 属于自己所在的那一行（提供商 × 上游模型），删除时带三元组精确定位 */
  function aliasCell(model: ManageModel) {
    const providerId = model.provider || ''
    const bindings = bindingsOf(model)
    const [head, ...rest] = bindings

    /**
     * 删除一条映射：同名映射允许多条，按（对外名 + 上游模型 + 提供商）三元组定位。
     * 不用先弹确认框的那条路（等级标）见下 —— 它只写一个字段，保存前还能取消。
     */
    async function confirmUnmap(alias: string): Promise<void> {
      const ok = await shared().wbConfirm?.ask?.({
        title: t('删除映射'),
        html: t('确定删除映射「<strong>{alias} → {target}</strong>」？', { alias: esc(alias), target: esc(model.id) }),
        okText: t('删除'),
        okClass: 'danger',
      })
      if (!ok) return
      void runRowAction(
        bindingKeyOf(alias, model.id, providerId),
        () => writeRemoveMapping(providerId, alias, model.id),
        t('映射已删除'),
      )
    }

    const chip = (binding: Binding) => {
      const alias = binding.alias
      const on = binding.enabled !== false
      const conflicts = on ? [] : enabledBindingConflicts(providerId, alias, model.id)
      const busy = state.pending.has(bindingKeyOf(alias, model.id, providerId))
      return (
        <span className={cn('alias', !on && 'map-off')} key={`${alias}\u0001${binding.isDefault ? 'd' : 'm'}`}>
          {/* 映射自己的开关（关掉 = 这条别名暂时不存在，可再打开）。行禁用时 chips 随行压淡，
              映射开关另用 map-off 弱化 —— 两个维度独立，一眼可辨。size='sm' 是 24×14 的小号，
              与旧 CSS 里 `.alias .switch .track` 同尺寸：标准档（36×21）塞进 22px 的药丸里
              会把 chip 连同整张表的行高一起撑高 */}
          <Switch size='sm' checked={on} disabled={busy}
            title={on ? t('映射已启用，点击关闭') : conflicts.length
              ? t('启用后将自动关闭该提供商的其他同名绑定：{bindings}', { bindings: conflicts.map(binding => `${binding.alias} → ${binding.target}`).join('、') })
              : t('映射已关闭，点击启用')}
            aria-label={t('{action}映射 {alias}', { action: on ? t('关闭') : t('启用'), alias })}
            onCheckedChange={next => {
              void runRowAction(
                bindingKeyOf(alias, model.id, providerId),
                () => writeBinding(providerId, alias, model.id, { enabled: next }),
                next ? (conflicts.length ? t('映射已启用，其他同名绑定已自动关闭') : t('映射已启用')) : t('映射已关闭'),
              )
            }} />
          {/* 名字本身就是复制入口（data-copy 走 clipboard.js 的全局委托，点一下
              toast「已复制」）；title 同时承担两件事 —— 提示可点、长名字被
              ellipsis 截断时悬停能看全 */}
          <span className='t' data-copy={alias} title={t('点击复制：{name}', { name: alias })}>{alias}</span>
          {binding.isDefault ? <span className='binding-default'>{t('默认')}</span> : null}
          {levelBadge(alias, model.id, providerId, busy)}
          {binding.isDefault ? null : (
            // 16px 的删除小件（icon-2xs 档）：字号 / 行高照旧 CSS 的 `.alias .x` 给，
            // 免得 × 跟着 chip 的 11px 一起缩水
            <Button variant='ghost' size='icon-2xs' className='text-[12px] leading-none'
              disabled={busy} title={t('删除映射 {alias}', { alias })}
              onClick={() => void confirmUnmap(alias)}>×</Button>
          )}
        </span>
      )
    }
    // 「＋ 映射」并排跟在**默认**那条右边（它是这一列的入口，不是一条绑定）：默认绑定永远
    // 存在（没有映射时是合成出来的那条），所以这一行永远有内容
    return (
      <div className='aliases'>
        <div className='alias-row'>
          {head ? chip(head) : null}
          {/* dashed = 「这里还能再添一个」的入口语义（对应旧的 .alias-add），与旁边那些
              实心按钮一眼分开 */}
          <Button variant='dashed' size='xs'
            onClick={() => openMapping({ target: model.id, provider: providerId })}>{t('＋ 映射')}</Button>
        </div>
        {rest.map(chip)}
      </div>
    )
  }

  /**
   * 「来源」列：这一家的清单当前是远程拉的还是内置静态表（后端给的 source，前端只做文案映射）。
   * 呈现为「色点 + 小字」（.src）而不是描边徽标：这一列每行都在重复同一个词，徽标的框
   * 只会添噪 —— 点色语义见 page-gateway.css（远程 = 品牌蓝 / 内置 = 灰 / 手动 = 琥珀）。
   */
  function sourceCell(model: ManageModel) {
    if (model.source === 'manual') {
      return <span className='src manual' title={t('手动登记的上游模型；移除它会直接删掉这条登记')}><i aria-hidden='true' />{t('手动')}</span>
    }
    // 自定义家的行（source 留空串）：只在聚合视图里出现 —— 选中该家时这一列整个
    // 按视图隐藏。给个明确的「自定义」档而不是落「—」：聚合视图里「—」读起来像
    // 「来源未知」，而它恰恰是唯一确定的一档（清单就是用户登记的）
    if (!model.source && customSource.isCustom(model.provider)) {
      return <span className='src manual' title={t('用户登记的自定义模型；整份清单只属于这一家')}><i aria-hidden='true' />{t('自定义')}</span>
    }
    if (model.source !== 'remote' && model.source !== 'builtin') return <span className='rate'>—</span>
    const remote = model.source === 'remote'
    const at = formatTime(Number(model.refreshedAt) || 0)
    const hint = remote
      ? (at ? t('来自上游目录接口，清单拉取于 {at}；刷新失败时保留上一份成功结果', { at }) : t('来自上游目录接口；刷新失败时保留上一份成功结果'))
      : t('上游目录尚未拉到，用的是内置静态清单；点「刷新模型清单」可重试')
    return <span className={cn('src', remote ? 'remote' : 'builtin')} title={hint}><i aria-hidden='true' />{remote ? t('远程') : t('内置')}</span>
  }

  /**
   * 「可见性」列：这个模型在**官方客户端**里会不会被列出来。
   *
   * ── 三档 + 一档「没有判据」──────────────────────────────────
   *   · **不展示**（`hidden`）：上游说有、官方客户端不列。本仓已默认关闭它的
   *     默认绑定（`model_rules::seed_hidden_disabled`）—— 也就是说它既不出现在
   *     `/v1/models`，也不接受点名调用。用户仍可在这一行手动打开（打开就是
   *     「我知道它不展示，但我要用」）。
   *   · **可见**（`shown`）：官方客户端会列它。
   *   · **未声明**（`undeclared`）：这家有判据、但这条上游没表态。
   *   · **`—`**（没有 `visibility` 字段）：这家**根本没有**可见性判据。
   *
   * 最后两档必须分开显示：`undeclared` 是「这家说不知道」，`—` 是「这家压根
   * 没这个信息」。混成一个，用户会以为所有没有判据的家都「确认可见」。
   */
  function visibilityCell(model: ManageModel) {
    const visibility = visibilityOf(model)
    if (visibility === 'hidden') {
      return (
        <span className='src hidden-visibility'
          title={t('上游能列出这个模型，但官方客户端不展示它；本仓已默认关闭它的默认绑定（点这一行的开关可以手动打开）')}>
          <i aria-hidden='true' />{t('不展示')}
        </span>
      )
    }
    if (visibility === 'visible') {
      return (
        <span className='src shown-visibility' title={t('官方客户端会展示这个模型')}>
          <i aria-hidden='true' />{t('可见')}
        </span>
      )
    }
    if (visibility === 'undeclared') {
      return <span className='rate' title={t('这家有可见性判据，但这一条上游没有表态 —— 按可见处理')}>—</span>
    }
    // 没有判据的家：这一格本来就该按视图隐藏吗？不 —— 家是逐行判的（聚合视图里
    // 有判据的家与没判据的家混在同一张表上），所以给一个带说明的 `—`。
    return <span className='rate' title={t('这家提供商没有可见性判据（目前只有 Antigravity / Qoder / 小浣熊 / CodeArts 有）')}>—</span>
  }

  /**
   * 「上下文 / 输出」列：两个 token 数合在一格（上下文 / 最大输出）。
   *
   * 展示的是**生效值**（清单原值 + 覆盖，归一后读）；被覆盖过的那一项挂一枚
   * 小点（`.cap-mark`，title 说明），未声明的显示 `—`。整格是一个按钮：
   * 点它打开能力弹窗（两列入口同一个 —— 能力是一个整体，没必要各开一个）。
   *
   * **来源也标出来**：这两个数正是 models.dev 补得最多的地方（上游普遍不给），
   * 所以它补的那些值带一条虚线底边（`.filled`）—— 文字上加不了圆点那样的环，
   * 虚线底边是同一套语义里唯一贴得上去的写法；悬停提示里还会逐项写明
   * 「；其中 最大输出 Token 由 models.dev 补齐」。不标的话，用户没法知道这个数
   * 是上游说的还是社区库说的 —— 两者的可信度不一样。
   */
  function budgetCell(model: ManageModel) {
    const caps = capabilitiesOf(model)
    const overrides = normalizeOverrides(model.capOverrides)
    const input = typeof caps.maxInputTokens === 'number' ? caps.maxInputTokens : null
    const output = typeof caps.maxOutputTokens === 'number' ? caps.maxOutputTokens : null
    const hint = t('上下文窗口 {input} / 最大输出 Token {output} —— 点击修改模型能力', { input: exactTokens(input), output: exactTokens(output) })
      + tokenOriginSuffix(model)
    return (
      <Button size='default' variant='ghost' type='button' className='caps-open' title={hint}
        onClick={() => openCapability(model.provider || '', model.id)}>
        <span className={cn('cap-num', input === null && 'unset', capSourceOf(model, 'maxInputTokens') === 'modelsdev' && 'filled')}>
          {formatTokens(input)}
          {overrides.includes('maxInputTokens') ? <i className='cap-mark' title={t('已覆盖上游值')} /> : null}
        </span>
        <span className='cap-sep'>/</span>
        <span className={cn('cap-num', output === null && 'unset', capSourceOf(model, 'maxOutputTokens') === 'modelsdev' && 'filled')}>
          {formatTokens(output)}
          {overrides.includes('maxOutputTokens') ? <i className='cap-mark' title={t('已覆盖上游值')} /> : null}
        </span>
      </Button>
    )
  }

  /**
   * 悬停提示的**来源尾巴**（只写「不是上游给的」那两档；上游声明/未声明不必写）：
   * 一格里两个数可能来自不同地方，所以逐项指名，而不是一句笼统的「部分来自
   * models.dev」。
   */
  function tokenOriginSuffix(model: ManageModel): string {
    const filled = (['maxInputTokens', 'maxOutputTokens'] as const)
      .filter(key => capSourceOf(model, key) === 'modelsdev')
      .map(key => CAPABILITY_LABELS[key])
    return filled.length ? t('；其中 {keys} 由 models.dev 补齐', { keys: filled.join(t('、')) }) : ''
  }

  /**
   * 「能力」列：四枚图标圆点（工具 / 图片 / 视频 / 思考），三态各有一副样式 ——
   * 支持（品牌淡底）/ 明确不支持（灰实底）/ 未声明（虚线圈）。语义全靠悬停提示
   * （`capabilityTip` 会写明「上游未声明时下游按不支持处理」这类事实），
   * 图标只负责「扫一眼知道有几项是亮的」。
   *
   * 图片模型走另一支（[`imageCapsCell`]）：那四枚圆点问的是**对话**模型的能力，
   * 放在图片模型上是四枚「不支持」，读起来像「这个模型什么都不会」。
   */
  function capsCell(model: ManageModel) {
    if (model.kind === 'image') return imageCapsCell(model)
    const caps = capabilitiesOf(model)
    const overrides = normalizeOverrides(model.capOverrides)
    return (
      <Button size='default' variant='ghost' type='button' className='caps-open caps-badges'
        title={t('对下游声明的能力（工具调用 / 图片识别 / 视频识别 / 支持思考）—— 点击修改')}
        onClick={() => openCapability(model.provider || '', model.id)}>
        {BOOLEAN_KEYS.map(key => {
          const value = caps[key] ?? null
          const state = capabilityState(value)
          const overridden = overrides.includes(key)
          const source = capSourceOf(model, key)
          return (
            <span key={key} className={cn('cap-dot', state, source === 'modelsdev' && 'filled', overridden && 'marked')}
              title={capabilityTip(key, value, overridden, source)}>
              {CAP_ICON[key]}
              {overridden ? <i className='cap-mark' /> : null}
            </span>
          )
        })}
      </Button>
    )
  }

  /**
   * 图片模型的能力格：两枚标签（图片生成 / 图片编辑）上下层叠、整叠居中。
   *
   * ── 判据用能力位，不用 `kind` ──────────────────────────────
   * 「图片生成」只在 `supports_image_generation` 为真时出现：只有参考图编辑的
   * 型号（如 ModelScope 的 Qwen-Image-Edit）不该挂生成标签 —— 改造前那两枚标签
   * 在模型 ID 后面，`kind === 'image'` 就挂「图片生成」，这类型号被多挂了一枚。
   * 两个能力位都没表态时退回「图片生成」（与改造前一致），不让这一格变成空白按钮。
   *
   * ── 为什么两枚是竖排 ────────────────────────────────────────
   * 「能力」列只有 11.5% 宽，横排两枚标签会把这一列顶宽（列表头与数值都跟着让位）；
   * 竖排的宽度就是最长那枚标签，列宽不用改。层叠与居中在 CSS（`.caps-open.caps-image`）。
   */
  function imageCapsCell(model: ManageModel) {
    const tags: Array<{ key: string; label: string; title: string }> = []
    if (model.supports_image_generation) {
      tags.push({ key: 'generate', label: t('图片生成'), title: 'POST /v1/images/generations' })
    }
    if (model.supports_image_editing) {
      tags.push({ key: 'edit', label: t('图片编辑'), title: 'POST /v1/images/edits' })
    }
    if (!tags.length) {
      tags.push({ key: 'generate', label: t('图片生成'), title: 'POST /v1/images/generations' })
    }
    return (
      <Button size='default' variant='ghost' type='button' className='caps-open caps-image'
        title={t('这个模型的图片能力（图片生成 / 图片编辑）—— 点击修改')}
        onClick={() => openCapability(model.provider || '', model.id)}>
        {tags.map(tag => (
          <span key={tag.key} className='binding-default' title={tag.title}>{tag.label}</span>
        ))}
      </Button>
    )
  }

  /**
   * 各列的单元格（不含对齐类）。放在一个 switch 里而不是行内联的三元链，是为了让「某一列长
   * 什么样」只有一处实现 —— 列设置重排时才不会各画一个样。
   *
   * 操作列只移除「可移除的」：内置家里是手动登记的那些（source=manual，上游目录带来的行由
   * 清单决定存在性，开关才是它的手段），自定义家里每一行都是用户自己登记进来的、都可以移除。
   * 对外名称统一在模型映射列切换。
   */
  function cellFor(column: ColumnView, model: ManageModel, busyRow: boolean) {
    /**
     * 「移除」删掉的是那条**登记**（它的存在完全由这次登记决定，没有「上游刷新会把它带回来」
     * 这回事，移除后 /v1/models、路由同时消失）。判据用后端给的 source，前端不自己推断。
     */
    async function confirmRemoveModel(): Promise<void> {
      const ok = await shared().wbConfirm?.ask?.({
        title: t('移除自定义模型'),
        html: t('确定移除自定义模型「<strong>{id}</strong>」？这条登记会被<b>直接移除</b>，之后 <code>/v1/models</code> 不再广告它、请求它也会被拒。', { id: esc(model.id) }),
        okText: t('移除'),
        okClass: 'danger',
      })
      if (!ok) return
      void runRowAction(
        rowKeyOf(model),
        () => writeRemoveModel(model.provider || '', model.id),
        // 文案按**行**的归属判（聚合视图里两家的行混在一张表上，视图级 custom 不再适用）
        customSource.isCustom(model.provider || '') ? t('模型已移除') : t('自定义模型已移除'),
      )
    }

    switch (column.key) {
      case 'check':
        return (
          <TableCell className={cellClass('cell-check', column.align)}>
            <Checkbox checked={selection.has(rowKeyOf(model))} title={t('勾选后可批量操作')}
              aria-label={t('勾选后可批量操作')}
              onCheckedChange={next => togglePick(rowKeyOf(model), next === true)} />
          </TableCell>
        )
      case 'model':
        return (
          <TableCell className={cellClass('cell-model', column.align)}>
            {/* 名字本身就是复制入口（data-copy 走 clipboard.js 的全局委托，点一下
                toast「已复制」）；title 同时承担两件事 —— 提示可点、长 ID 被
                ellipsis 截断时悬停能看全。曾经这里另挂一颗 ⧉ 小按钮，已去掉：
                点名字更省事，也少一个悬停才显形的控件 */}
            <div className='mid'>
              <span className='t' data-copy={model.id} title={t('点击复制：{name}', { name: model.id })}>{model.id}</span>
              {model.upstreamStatus === 'undeployed' ? (
                <Badge variant='warning' shape='tag'
                  title={t('上游目录列出了这个模型，但推理端尚未识别；保留在清单中，后续探测成功后自动清除标记')}>
                  {t('上游未部署')}
                </Badge>
              ) : null}
            </div>
            {model.name && model.name !== model.id ? <div className='mname'>{model.name}</div> : null}
          </TableCell>
        )
      case 'rate':
        return (
          <TableCell className={cellClass('cell-rate', column.align)}>
            <span className='rate'>{model.credits ? formatCredits(model.credits) : '—'}</span>
          </TableCell>
        )
      case 'source':
        return <TableCell className={cellClass('cell-source', column.align)}>{sourceCell(model)}</TableCell>
      case 'visibility':
        return <TableCell className={cellClass('cell-visibility', column.align)}>{visibilityCell(model)}</TableCell>
      case 'budget':
        return <TableCell className={cellClass('cell-budget', column.align)}>{budgetCell(model)}</TableCell>
      case 'caps':
        return <TableCell className={cellClass('cell-caps', column.align)}>{capsCell(model)}</TableCell>
      case 'alias':
        return <TableCell className={cellClass('cell-alias', column.align)}>{aliasCell(model)}</TableCell>
      case 'act': {
        // 「测试」的门禁（该家没有可用账号）由 model-test-dialog 统一判定，这里只把
        // 理由挂到 title 上 —— 禁用而不说原因等于让用户猜。未启用的行**不置灰**：
        // 「先测通、再决定要不要启用」正是这颗按钮的用法（后端给测试开了直达跳，
        // 见 model-test-dialog 的 testBlockReason）
        const blocked = testBlockReason(model.provider || '', model)
        // 移除按钮判据按**行**判（聚合视图里内置与自定义的行混在一张表上，视图级的
        // custom 布尔不再适用）：内置家只露手动登记的（source=manual，上游目录带来的
        // 行由清单决定存在性），自定义家的每一行都是用户登记的、都可以移除
        const removable = model.source === 'manual' || customSource.isCustom(model.provider || '')
        return (
          <TableCell className={cellClass('cell-act r', column.align)}>
            <div className='row-actions'>
              <Button variant='ghost' size='sm' disabled={Boolean(blocked)}
                title={blocked || t('以这个上游模型发一次最小请求，走真实转发链路')}
                onClick={() => setTestTarget({ provider: model.provider || '', id: model.id })}>
                {t('测试')}
              </Button>
              {removable ? (
                <Button variant='ghost' size='sm' className='text-destructive' disabled={busyRow}
                  onClick={() => void confirmRemoveModel()}>{t('移除')}</Button>
              ) : null}
            </div>
          </TableCell>
        )
      }
      default:
        return null
    }
  }

  return <>{columns.map(column => <React.Fragment key={column.key}>{cellFor(column, model, busyRow)}</React.Fragment>)}</>
}
