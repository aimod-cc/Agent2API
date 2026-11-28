import * as React from 'react'
import {
  Badge, Button, Checkbox, Switch, TableCell, cn,
} from '@ui'
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
  formatUnitPrice, pricingSourceLabel, usePricingDisplay,
} from './model-pricing'
import {
  bindingKeyOf, bindingsOf, enabledBindingConflicts, esc, formatTime, getSnapshot, levelOf, models,
  rowKeyOf, runRowAction, same, shared, subscribe, visibilityOf,
  writeBinding, writeRemoveMapping, type Align, type ColumnView, type Binding,
} from './models-panel-state'
import type { ModelDrawerTab } from './models/models-drawer'

const cellClass = (base: string, align?: Align): string => (align ? `${base} ta-${align}` : base)

function formatCredits(credits: unknown): string {
  const match = /x\s*([\d.]+)/i.exec(String(credits || ''))
  if (match) return match[1] === '0' ? t('限免') : `${match[1]}×`
  return String(credits || '')
}

function isFreeCredits(credits: unknown): boolean {
  const match = /x\s*([\d.]+)/i.exec(String(credits || ''))
  return Boolean(match && Number(match[1]) === 0)
}

const CAP_ICON: Partial<Record<CapabilityKey, React.ReactNode>> = {
  supportsToolCall: (
    <svg viewBox='0 0 24 24' aria-hidden='true'>
      <path fill='none' stroke='currentColor' strokeWidth='2' strokeLinecap='round' strokeLinejoin='round'
        d='M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z' />
    </svg>
  ),
  supportsImages: (
    <svg viewBox='0 0 24 24' aria-hidden='true'>
      <g fill='none' stroke='currentColor' strokeWidth='2' strokeLinecap='round' strokeLinejoin='round'>
        <rect x='3' y='3' width='18' height='18' rx='2.5' /><circle cx='8.5' cy='8.5' r='1.5' />
        <path d='M21 15l-5-5L5 21' />
      </g>
    </svg>
  ),
  supportsVideo: (
    <svg viewBox='0 0 24 24' aria-hidden='true'>
      <g fill='none' stroke='currentColor' strokeWidth='2' strokeLinecap='round' strokeLinejoin='round'>
        <circle cx='12' cy='12' r='9' /><path d='M10 8.5l6 3.5-6 3.5z' />
      </g>
    </svg>
  ),
  supportsReasoning: (
    <svg viewBox='0 0 24 24' aria-hidden='true'>
      <g fill='none' stroke='currentColor' strokeWidth='2' strokeLinecap='round' strokeLinejoin='round'>
        <path d='M15 14c.2-1 .7-1.7 1.5-2.5 1-.9 1.5-2.2 1.5-3.5A6 6 0 0 0 6 8c0 1 .2 2.2 1.5 3.5.7.7 1.3 1.5 1.5 2.5' />
        <path d='M9 18h6M10 22h4' />
      </g>
    </svg>
  ),
}

function SlidersIcon() {
  return (
    <svg width='13' height='13' viewBox='0 0 24 24' fill='none' stroke='currentColor' strokeWidth='1.9' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
      <path d='M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3M1 14h6M9 8h6M17 16h6' />
    </svg>
  )
}

export function ModelCell({ columns, model, selection, togglePick, setTestTarget, onOpenDrawer }: {
  columns: ColumnView[]
  model: ManageModel
  selection: ReadonlySet<string>
  togglePick: (key: string, next: boolean) => void
  setTestTarget: (target: ModelTestTarget) => void
  onOpenDrawer: (model: ManageModel, tab: ModelDrawerTab) => void
}) {
  const pricingDisplay = usePricingDisplay()
  const state = React.useSyncExternalStore(subscribe, getSnapshot)
  const busyRow = state.pending.has(rowKeyOf(model))
  const showRateCol = columns.some(col => col.key === 'rate')
  const showVisibilityCol = columns.some(col => col.key === 'visibility')
  const showSourceCol = columns.some(col => col.key === 'source')

  function levelBadge(alias: string, target: string, providerId: string, busy: boolean) {
    const level = levelOf(alias, target, providerId)
    const targetRow = models().find(item => same(item.provider, providerId) && same(item.id, target))
    const info = modelReasoning(targetRow)
    const effective = info.bindingSupported === true ? bindingReasoningLevel(level, targetRow) : ''
    const title = info.bindingSupported === false
      ? t('当前通道未声明可用档位，映射等级不会注入；请先获取模型目录。')
      : info.fixedLevel
        ? t('上游固定为 {level}，绑定其他等级也无法调高。', { level: info.fixedLevel })
        : effective && effective.toLowerCase() !== level.toLowerCase()
          ? t('绑定 {requested}，按当前通道将发送 {effective}。', { requested: level, effective })
          : level ? t('思考等级 {level}（点击修改）', { level }) : t('设置思考等级（当前未绑定）')
    return (
      <Button variant={level ? 'secondary' : 'dashed'} size='2xs' disabled={busy}
        className='font-mono font-bold shadow-none'
        title={title}
        onClick={event => {
          event.stopPropagation()
          onOpenDrawer(model, 'mapping')
        }}>
        {info.fixedLevel ? `${t('固定思考档位')} ${info.fixedLevel}` : effective && effective.toLowerCase() !== level.toLowerCase()
          ? `${level} → ${effective}` : level || t('＋等级')}
      </Button>
    )
  }

  function aliasCell(row: ManageModel) {
    const providerId = row.provider || ''
    const bindings = bindingsOf(row)
    const shownBindings = bindings.slice(0, 2)
    const extraCount = bindings.length - shownBindings.length

    async function confirmUnmap(alias: string): Promise<void> {
      const ok = await shared().wbConfirm?.ask?.({
        title: t('删除映射'),
        html: t('确定删除映射「<strong>{alias} → {target}</strong>」？', { alias: esc(alias), target: esc(row.id) }),
        okText: t('删除'),
        okClass: 'danger',
      })
      if (!ok) return
      void runRowAction(
        bindingKeyOf(alias, row.id, providerId),
        () => writeRemoveMapping(providerId, alias, row.id),
        t('映射已删除'),
      )
    }

    const chip = (binding: Binding) => {
      const alias = binding.alias
      const on = binding.enabled !== false
      const conflicts = on ? [] : enabledBindingConflicts(providerId, alias, row.id)
      const busy = state.pending.has(bindingKeyOf(alias, row.id, providerId))
      return (
        <span className={cn('alias', !on && 'map-off')} key={`${alias}\u0001${binding.isDefault ? 'd' : 'm'}`}
          onClick={event => event.stopPropagation()}>
          <Switch size='sm' checked={on} disabled={busy}
            title={on ? t('映射已启用，点击关闭') : conflicts.length
              ? t('启用后将自动关闭该提供商的其他同名绑定：{bindings}', { bindings: conflicts.map(b => `${b.alias} → ${b.target}`).join('、') })
              : t('映射已关闭，点击启用')}
            aria-label={t('{action}映射 {alias}', { action: on ? t('禁用') : t('启用'), alias })}
            onCheckedChange={next => {
              void runRowAction(
                bindingKeyOf(alias, row.id, providerId),
                () => writeBinding(providerId, alias, row.id, { enabled: next }),
                next ? (conflicts.length ? t('映射已启用，其他同名绑定已自动关闭') : t('映射已启用')) : t('映射已关闭'),
              )
            }} />
          <span className='t' data-copy={alias} title={t('点击复制：{name}', { name: alias })}>{alias}</span>
          {binding.isDefault ? <span className='binding-default'>{t('默认')}</span> : null}
          {levelBadge(alias, row.id, providerId, busy)}
          {binding.isDefault ? null : (
            <Button variant='ghost' size='icon-2xs' className='text-[12px] leading-none'
              disabled={busy} title={t('删除映射 {alias}', { alias })}
              onClick={() => void confirmUnmap(alias)}>×</Button>
          )}
        </span>
      )
    }

    return (
      <div className='aliases'>
        {shownBindings.map((binding, idx) => (
          <div className='alias-row' key={`${binding.alias}\u0001${binding.isDefault ? 'd' : 'm'}`}>
            {chip(binding)}
            {idx === 0 && extraCount === 0 ? (
              <Button variant='dashed' size='xs'
                onClick={event => { event.stopPropagation(); onOpenDrawer(row, 'mapping') }}>
                {t('＋ 映射')}
              </Button>
            ) : null}
          </div>
        ))}
        {extraCount > 0 ? (
          <div className='alias-row'>
            <Button variant='outline' size='xs'
              onClick={event => { event.stopPropagation(); onOpenDrawer(row, 'mapping') }}>
              {t('还有 {n} 条映射', { n: extraCount })}
            </Button>
            <Button variant='dashed' size='xs'
              onClick={event => { event.stopPropagation(); onOpenDrawer(row, 'mapping') }}>
              {t('＋ 映射')}
            </Button>
          </div>
        ) : null}
      </div>
    )
  }

  function sourceCell(row: ManageModel) {
    if (row.source === 'manual') {
      return <span className='src manual' title={t('手动登记的上游模型；移除它会直接删掉这条登记')}><i aria-hidden='true' />{t('手动')}</span>
    }
    if (!row.source && customSource.isCustom(row.provider)) {
      return <span className='src manual' title={t('用户登记的自定义模型；整份清单只属于这一家')}><i aria-hidden='true' />{t('自定义')}</span>
    }
    if (row.source !== 'remote' && row.source !== 'builtin') return <span className='rate'>—</span>
    const remote = row.source === 'remote'
    const at = formatTime(Number(row.refreshedAt) || 0)
    const hint = remote
      ? (at ? t('来自上游目录接口，清单拉取于 {at}；刷新失败时保留上一份成功结果', { at }) : t('来自上游目录接口；刷新失败时保留上一份成功结果'))
      : t('上游目录尚未拉到，用的是内置静态清单；点「刷新模型清单」可重试')
    return <span className={cn('src', remote ? 'remote' : 'builtin')} title={hint}><i aria-hidden='true' />{remote ? t('远程') : t('内置')}</span>
  }

  function visibilityCell(row: ManageModel) {
    const visibility = visibilityOf(row)
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
    return <span className='rate' title={t('这家提供商没有可见性判据（目前只有 Antigravity / Qoder / 小浣熊 / CodeArts 有）')}>—</span>
  }

  function tokenOriginSuffix(row: ManageModel): string {
    const filled = (['maxInputTokens', 'maxOutputTokens'] as const)
      .filter(key => capSourceOf(row, key) === 'modelsdev')
      .map(key => CAPABILITY_LABELS[key])
    return filled.length ? t('；其中 {keys} 由 models.dev 补齐', { keys: filled.join(t('、')) }) : ''
  }

  function specCell(row: ManageModel) {
    const caps = capabilitiesOf(row)
    const overrides = normalizeOverrides(row.capOverrides)
    const input = typeof caps.maxInputTokens === 'number' ? caps.maxInputTokens : null
    const output = typeof caps.maxOutputTokens === 'number' ? caps.maxOutputTokens : null
    const tokenHint = t('上下文窗口 {input} / 最大输出 Token {output} —— 点击修改模型能力', {
      input: exactTokens(input), output: exactTokens(output),
    }) + tokenOriginSuffix(row)

    const isImage = row.kind === 'image'
    const imageTags: Array<{ key: string; label: string; title: string }> = []
    if (isImage) {
      if (row.supports_image_generation) imageTags.push({ key: 'generate', label: t('图片生成'), title: 'POST /v1/images/generations' })
      if (row.supports_image_editing) imageTags.push({ key: 'edit', label: t('图片编辑'), title: 'POST /v1/images/edits' })
      if (!imageTags.length) imageTags.push({ key: 'generate', label: t('图片生成'), title: 'POST /v1/images/generations' })
    }

    return (
      <Button size='default' variant='ghost' type='button' className='caps-open mm-spec-open' title={tokenHint}
        onClick={event => { event.stopPropagation(); onOpenDrawer(row, 'caps') }}>
        <span className='mm-spec-tokens'>
          <span className={cn('cap-num', input === null && 'unset', capSourceOf(row, 'maxInputTokens') === 'modelsdev' && 'filled')}>
            {formatTokens(input)}
            {overrides.includes('maxInputTokens') ? <i className='cap-mark' title={t('已覆盖上游值')} /> : null}
          </span>
          <span className='cap-sep'>/</span>
          <span className={cn('cap-num', output === null && 'unset', capSourceOf(row, 'maxOutputTokens') === 'modelsdev' && 'filled')}>
            {formatTokens(output)}
            {overrides.includes('maxOutputTokens') ? <i className='cap-mark' title={t('已覆盖上游值')} /> : null}
          </span>
        </span>
        {isImage ? (
          <span className='mm-spec-img-tags'>
            {imageTags.map(tag => (
              <span key={tag.key} className='binding-default' title={tag.title}>{tag.label}</span>
            ))}
          </span>
        ) : (
          <span className='mm-spec-dots'>
            {BOOLEAN_KEYS.map(key => {
              const value = caps[key] ?? null
              const capState = capabilityState(value)
              const overridden = overrides.includes(key)
              const source = capSourceOf(row, key)
              return (
                <span key={key} className={cn('cap-dot', capState, source === 'modelsdev' && 'filled', overridden && 'marked')}
                  title={capabilityTip(key, value, overridden, source, row.toolCallingMode)}>
                  {CAP_ICON[key]}
                  {overridden ? <i className='cap-mark' /> : null}
                </span>
              )
            })}
          </span>
        )}
      </Button>
    )
  }

  function cellFor(column: ColumnView, row: ManageModel) {
    switch (column.key) {
      case 'check':
        return (
          <TableCell className={cellClass('cell-check', column.align)} onClick={e => e.stopPropagation()}>
            <Checkbox checked={selection.has(rowKeyOf(row))} title={t('勾选后可批量操作')}
              aria-label={t('勾选后可批量操作')}
              onCheckedChange={next => togglePick(rowKeyOf(row), next === true)} />
          </TableCell>
        )
      case 'model': {
        const isCustomModel = customSource.isCustom(row.provider)
        const providerRecord = isCustomModel ? customSource.record(row.provider) : null
        const defaultProtocol = customSource.normalizeCustomProtocol(providerRecord?.protocol)
          || customSource.normalizeCustomProtocol(row.providerProtocol)
          || 'chat_completions'
        const inferredProtocol = isCustomModel ? customSource.inferPresetModelProtocol(providerRecord, row.id) : undefined
        const explicitProtocol = customSource.normalizeCustomProtocol(row.protocol)
        const effectiveProtocol = explicitProtocol || inferredProtocol || defaultProtocol
        const vis = visibilityOf(row)
        return (
          <TableCell className={cellClass('cell-model', column.align)}>
            <div className='mid'>
              <span className='t' data-copy={row.id} title={t('点击复制：{name}', { name: row.id })}
                onClick={e => e.stopPropagation()}>{row.id}</span>
              {row.upstreamStatus === 'undeployed' ? (
                <Badge variant='warning' shape='tag' className='mm-inline-badge'
                  title={t('上游目录列出了这个模型，但推理端尚未识别；保留在清单中，后续探测成功后自动清除标记')}>
                  {t('上游未部署')}
                </Badge>
              ) : null}
              {!showVisibilityCol && vis === 'hidden' ? (
                <Badge variant='warning' shape='tag' className='mm-inline-badge'
                  title={t('官方客户端不展示该模型，默认已关闭')}>
                  {t('不展示')}
                </Badge>
              ) : null}
              {!showSourceCol && row.source === 'manual' ? (
                <Badge variant='outline' shape='tag' className='mm-inline-badge'
                  title={t('手动登记的上游模型')}>
                  {t('手动')}
                </Badge>
              ) : null}
              {!showSourceCol && row.source === 'builtin' ? (
                <Badge variant='outline' shape='tag' className='mm-inline-badge'
                  title={t('来自内置静态清单，尚未拉取远程目录')}>
                  {t('内置')}
                </Badge>
              ) : null}
            </div>
            {(row.name && row.name !== row.id) || isCustomModel ? (
              <div className='mm-model-subrow'>
                {row.name && row.name !== row.id ? <span className='mname'>{row.name}</span> : null}
                {isCustomModel ? (
                  <button
                    type='button'
                    className={cn(
                      'mm-proto-tag',
                      (explicitProtocol || (inferredProtocol && inferredProtocol !== defaultProtocol)) && 'is-custom',
                    )}
                    title={t('点击在右侧抽屉修改此模型的请求协议')}
                    onClick={e => { e.stopPropagation(); onOpenDrawer(row, 'caps') }}
                  >
                    {customSource.customProtocolLabel(effectiveProtocol, true)}
                    {explicitProtocol ? ` · ${t('独立')}` : (inferredProtocol && inferredProtocol !== defaultProtocol ? ` · ${t('自动')}` : '')}
                  </button>
                ) : null}
              </div>
            ) : null}
          </TableCell>
        )
      }
      case 'rate':
        return (
          <TableCell className={cellClass('cell-rate', column.align)}>
            <span className='rate'>{row.credits ? formatCredits(row.credits) : '—'}</span>
          </TableCell>
        )
      case 'pricing': {
        const info = row.pricing
        const eff = info?.effective
        const srcCurrency = eff?.currency || info?.currency || 'usd'
        const priced = Boolean(info?.priced && eff)
        const titleLines: string[] = []
        if (priced && eff) {
          titleLines.push(`${t('来源')}：${pricingSourceLabel(info?.source ?? null)}${info?.sameAs ? ` (${info.sameAs})` : ''}`)
          titleLines.push(`${t('输入')}：${formatUnitPrice(eff.input, pricingDisplay.currency, pricingDisplay.exchangeRate, srcCurrency)} / M`)
          titleLines.push(`${t('输出')}：${formatUnitPrice(eff.output, pricingDisplay.currency, pricingDisplay.exchangeRate, srcCurrency)} / M`)
          if ((eff.cacheRead ?? 0) > 0) titleLines.push(`${t('缓存读')}：${formatUnitPrice(eff.cacheRead, pricingDisplay.currency, pricingDisplay.exchangeRate, srcCurrency)} / M`)
          if ((eff.cacheWrite ?? 0) > 0) titleLines.push(`${t('缓存写')}：${formatUnitPrice(eff.cacheWrite, pricingDisplay.currency, pricingDisplay.exchangeRate, srcCurrency)} / M`)
          if (info?.priceRate && info.priceRate !== 1) titleLines.push(`${t('提供商倍率')}：×${info.priceRate}`)
          if (info?.activeWindowRate && info.activeWindowRate !== 1) titleLines.push(`${t('当前时段倍率')}：×${info.activeWindowRate}`)
          if (info?.inPeak) titleLines.push(t('当前处于高峰时段 (2×)'))
          if (eff.tiers && eff.tiers.length > 0) titleLines.push(t('含 {n} 档长上下文阶梯定价', { n: eff.tiers.length }))
        } else {
          titleLines.push(t('点击配置模型计价或参考模型'))
        }
        if (row.credits) titleLines.push(`${t('积分倍率')}：${formatCredits(row.credits)}`)
        return (
          <TableCell className={cellClass('cell-pricing', column.align)}>
            <Button size='default' variant='ghost' type='button' className='caps-open pricing-open'
              title={titleLines.join('\n')}
              onClick={event => { event.stopPropagation(); onOpenDrawer(row, 'pricing') }}>
              <span className='mm-cost-top'>
                {priced && eff ? (
                  <span className='max-w-full truncate font-mono text-[11.5px] font-medium'>
                    {eff.input === 0 && eff.output === 0
                      ? t('免费')
                      : `${formatUnitPrice(eff.input, pricingDisplay.currency, pricingDisplay.exchangeRate, srcCurrency)} / ${formatUnitPrice(eff.output, pricingDisplay.currency, pricingDisplay.exchangeRate, srcCurrency)}`}
                  </span>
                ) : (
                  <span className='max-w-full truncate text-[11px] text-[var(--text-3)] underline decoration-dashed underline-offset-2'>
                    {t('未定价')}
                  </span>
                )}
                {!showRateCol && row.credits ? (
                  <span className={cn('mm-rate-pill', isFreeCredits(row.credits) && 'is-free')}>
                    {formatCredits(row.credits)}
                  </span>
                ) : null}
              </span>
              <span className='max-w-full truncate text-[10.5px] text-[var(--text-3)]'>
                {priced ? pricingSourceLabel(info?.source ?? null) : t('点击设置')}
                {info?.inPeak ? ' · Peak 2×' : info?.activeWindowRate && info.activeWindowRate !== 1 ? ` · ×${info.activeWindowRate}` : info?.priceRate && info.priceRate !== 1 ? ` · ×${info.priceRate}` : ''}
                {eff?.tiers && eff.tiers.length > 0 ? ` · ${t('阶梯')}` : ''}
              </span>
            </Button>
          </TableCell>
        )
      }
      case 'source':
        return <TableCell className={cellClass('cell-source', column.align)}>{sourceCell(row)}</TableCell>
      case 'visibility':
        return <TableCell className={cellClass('cell-visibility', column.align)}>{visibilityCell(row)}</TableCell>
      case 'spec':
        return <TableCell className={cellClass('cell-spec', column.align)}>{specCell(row)}</TableCell>
      case 'alias':
        return <TableCell className={cellClass('cell-alias', column.align)}>{aliasCell(row)}</TableCell>
      case 'act': {
        const blocked = testBlockReason(row.provider || '', row)
        return (
          <TableCell className={cellClass('cell-act r', column.align)} onClick={e => e.stopPropagation()}>
            <div className='row-actions'>
              <Button variant='ghost' size='sm' disabled={Boolean(blocked)}
                title={blocked || t('以这个上游模型发一次最小请求，走真实转发链路')}
                onClick={() => setTestTarget({ provider: row.provider || '', id: row.id })}>
                {t('测试')}
              </Button>
              <Button variant='ghost' size='icon-sm' disabled={busyRow}
                aria-label={t('打开配置抽屉')}
                title={t('配置映射、能力与计价')}
                onClick={() => onOpenDrawer(row, 'mapping')}>
                <SlidersIcon />
              </Button>
            </div>
          </TableCell>
        )
      }
      default:
        return null
    }
  }

  return <>{columns.map(column => <React.Fragment key={column.key}>{cellFor(column, model)}</React.Fragment>)}</>
}
