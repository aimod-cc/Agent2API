/**
 * 模型管理页的**批量操作弹窗**（表头勾选列 + 「批量操作」按钮的落地动作）。
 *
 * 入口：勾选若干行后，「获取模型」左边的「批量操作」按钮（models-page.tsx）。
 * 可对选中的行做：启用 / 禁用 / 设置思考等级 / 删除。前三个对**每一行的全部映射**
 * （默认绑定 + 别名 chips）生效 —— 行级「已启用」的判定就是「还有任一条映射开着」
 * （`rowEnabled`），所以批量开关映射才能让整行真的启用 / 禁用；思考等级同样按
 * 映射记（`ManageMapping.reasoning`），设置 / 清除必然落到每条映射上。
 *
 * ── 删除的边界（与操作列那颗「移除」同一条）────────────────────
 * 只有**手动登记**的模型（内置家的 `source === 'manual'`、自定义家的一切）才存在
 * 「删除登记」这回事；上游目录带回来的远程 / 内置行由清单决定存在性，开关映射才是
 * 它的手段。批量删除会跳过不可删除的行并计数，弹窗里先给一行说明。
 *
 * ── 执行是逐条串行 ─────────────────────────────────────────
 * 内置家的每个写操作都返回**最新整份视图**（`accept` 就地替换）；自定义家每次写
 * 都是整表提交。串行执行让中间结果逐条落定，任一失败停下时界面上就是「成功的那
 * 一部分已生效」的真实状态，不会整批回滚成更早期的样子。
 *
 * ── 失败的处理 ─────────────────────────────────────────────
 * 全部成功 → 关弹窗、清空勾选；有失败 → 弹窗留着、勾选保留（可改完条件重试），
 * 结果行列出失败数与首次失败原因。
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
  Input,
  Label,
  RadioGroup,
  RadioGroupItem,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@ui'
import { isCustom, type ManageModel } from './models-custom-source'
import { CUSTOM_LEVEL, levels as reasoningLevels } from './models-reasoning'
import {
  accept, bindingsOf, errorMessage, getSnapshot, shared, toast, writeBinding, writeRemoveModel,
} from './models-panel-state'
import { t } from '../i18n'

/** 批量动作（RadioGroup 的取值；文案与执行动词共用这里） */
const ACTIONS = [
  { value: 'enable', label: t('启用') },
  { value: 'disable', label: t('禁用') },
  { value: 'reasoning', label: t('设置思考等级') },
  { value: 'delete', label: t('删除') },
] as const

type BatchAction = (typeof ACTIONS)[number]['value']

/** 「删除登记」的边界：手动登记（内置家 manual）或自定义家的一切（见文件头） */
function isRemovable(model: ManageModel): boolean {
  return model.source === 'manual' || isCustom(model.provider)
}

/** 模型名（与账号页同一口径：有名字用名字，没有回退 id） */
function modelName(model: ManageModel): string {
  return (model.name && model.name !== model.id ? t('{name}（{id}）', { name: model.name, id: model.id }) : model.id) || model.id
}

export function ModelBatchDialog({ models, onClose, onDone }: {
  models: ManageModel[]
  onClose: () => void
  onDone: () => void
}) {
  const [action, setAction] = React.useState<BatchAction>('enable')
  // 思考等级草稿（照 MappingDialog 的形态）：'' = 清除 / 不覆盖、候选等级、CUSTOM_LEVEL
  const [reasoning, setReasoning] = React.useState(() => {
    const candidates = reasoningLevels(getSnapshot().data)
    return { select: candidates[0] || '', custom: '' }
  })
  const [busy, setBusy] = React.useState(false)
  const [result, setResult] = React.useState<React.ReactNode>('')
  const candidates = reasoningLevels(getSnapshot().data)
  const level = reasoning.select === CUSTOM_LEVEL ? reasoning.custom.trim() : reasoning.select
  const removableCount = models.filter(isRemovable).length
  const label = ACTIONS.find(item => item.value === action)?.label || t('操作')

  /** 批量删除不可逆，按数量做二次确认（与账号页批量删除同一口子，原生 confirm 在
   *  Tauri WebView 里直接放行，所以一律走 wbConfirm 的应用内确认框） */
  async function confirmRemove(count: number): Promise<boolean> {
    return Promise.resolve(shared().wbConfirm?.ask?.({
      title: t('删除选中的模型'),
      html: t('确定删除选中的 <strong>{count}</strong> 个模型登记？删除后 <code>/v1/models</code> 不再广告它们、请求也会被拒。', { count })
        + (models.length > count ? '<br/>' + t('另有 {n} 个非手动登记的模型将被跳过。', { n: models.length - count }) : ''),
      okText: t('删除'),
      okClass: 'danger',
    }) ?? false)
  }

  async function run(): Promise<void> {
    if (busy) return
    if (!models.length) { toast(t('没有选中的模型'), 'err'); return }
    if (action === 'reasoning' && reasoning.select === CUSTOM_LEVEL && !level) {
      setResult(<span className='text-destructive'>{t('请填写自定义思考等级，或改选候选档 / 「清除」')}</span>)
      return
    }
    if (action === 'delete') {
      if (removableCount === 0) {
        setResult(<span className='text-destructive'>{t('选中的模型都不是手动登记，无法删除（开关映射才是它们的手段）')}</span>)
        return
      }
      if (!(await confirmRemove(removableCount))) return
    }

    setBusy(true)
    setResult('')
    let ok = 0
    let skipped = 0
    let failed = 0
    let firstError = ''
    for (const model of models) {
      const provider = model.provider || ''
      try {
        if (action === 'delete') {
          if (!isRemovable(model)) { skipped += 1; continue }
          accept(await writeRemoveModel(provider, model.id))
        } else if (action === 'enable' || action === 'disable') {
          const next = action === 'enable'
          for (const binding of bindingsOf(model)) {
            accept(await writeBinding(provider, binding.alias, model.id, { enabled: next }))
          }
        } else {
          for (const binding of bindingsOf(model)) {
            accept(await writeBinding(provider, binding.alias, model.id, { reasoning: level }))
          }
        }
        ok += 1
      } catch (error) {
        failed += 1
        if (!firstError) firstError = errorMessage(error)
        setResult(<span className='text-destructive'>
          {t('失败 {n} 个 · 首个失败：{error}', { n: failed, error: firstError })}
        </span>)
      }
    }
    if (failed === 0) {
      toast(skipped
        ? t('✅ 批量{action}完成：共 {n} 个模型（跳过不可删除的 {skipped} 个）',
          { action: label, n: ok, skipped })
        : t('✅ 批量{action}完成：共 {n} 个模型', { action: label, n: ok }))
      onDone()
    } else {
      toast(t('批量{action}完成：成功 {ok} 个，失败 {failed} 个', { action: label, ok, failed }), 'err')
      setResult(<span className='text-destructive'>
        {t('成功 {ok} 个，失败 {failed} 个', { ok, failed })}
        {skipped ? t('，跳过 {n} 个', { n: skipped }) : ''}
        {firstError ? t('；首个失败：{error}', { error: firstError }) : ''}
      </span>)
    }
    setBusy(false)
  }

  return (
    <Dialog open onOpenChange={next => { if (next || busy) return; onClose() }}>
      <DialogContent>
        <DialogHeader><DialogTitle>{t('批量操作 · 已选 {n} 个模型', { n: models.length })}</DialogTitle></DialogHeader>
        <DialogBody>
          <DialogSection>
            <h3>{t('将作用于以下模型')}</h3>
            <p style={{ maxHeight: 84, overflowY: 'auto' }}>{models.map(modelName).join(t('、'))}</p>
          </DialogSection>
          <DialogSection>
            <h3>{t('操作')}</h3>
            <RadioGroup value={action} onValueChange={next => setAction(next as BatchAction)}
              className='flex-row flex-wrap items-center gap-5' aria-label={t('批量动作')}>
              {ACTIONS.map(item => (
                <Label key={item.value} className='inline-flex cursor-pointer items-center gap-2 font-normal'>
                  <RadioGroupItem value={item.value} />{item.label}
                </Label>
              ))}
            </RadioGroup>
            {action === 'reasoning' ? (
              <div className='mt-2.5'>
                <div className='field-row'>
                  <label htmlFor='batch-model-reasoning'>{t('思考等级')}</label>
                  <Select value={reasoning.select} onValueChange={next => setReasoning(prev => ({ ...prev, select: String(next) }))}>
                    <SelectTrigger id='batch-model-reasoning' className='min-w-[220px]'>
                      <SelectValue>
                        {reasoning.select === CUSTOM_LEVEL ? t('自定义等级…')
                          : (reasoning.select || t('清除（不覆盖）'))}
                      </SelectValue>
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value=''>{t('清除（不覆盖）')}</SelectItem>
                      {candidates.map(levelName => (
                        <SelectItem key={levelName} value={levelName}>{levelName}</SelectItem>
                      ))}
                      <SelectItem value={CUSTOM_LEVEL}>{t('自定义等级…')}</SelectItem>
                    </SelectContent>
                  </Select>
                </div>
                {reasoning.select === CUSTOM_LEVEL ? (
                  <div className='field-row mt-2'>
                    <label htmlFor='batch-model-reasoning-custom'>{t('自定义等级')}</label>
                    <Input id='batch-model-reasoning-custom' maxLength={32} value={reasoning.custom}
                      onChange={event => setReasoning(prev => ({ ...prev, custom: event.currentTarget.value }))} />
                  </div>
                ) : null}
                <p className='detail mt-1.5'>
                  {t('设置 / 清除会应用到选中模型的')}<b>{t('全部映射')}</b>{t('（默认绑定与别名 chips）；「清除」= 不给这些映射指定等级（选「不覆盖」的口径与映射弹窗一致）')}
                </p>
              </div>
            ) : null}
            {action === 'delete' ? (
              <p className='detail mt-2'>
                {removableCount === 0
                  ? t('选中的都不是手动登记的模型，删除不可用（上游目录带回来的行由清单决定存在性）')
                  : t('可删除 {n} 个（手动登记 / 自定义家）；', { n: removableCount })
                    + (models.length > removableCount
                      ? t('其余 {n} 个会被跳过。', { n: models.length - removableCount })
                      : '')}
              </p>
            ) : null}
          </DialogSection>
          <div className='detail' style={{ minHeight: 18 }}>{result}</div>
        </DialogBody>
        <DialogFooter>
          <div className='mr-auto' />
          <Button variant='outline' disabled={busy} onClick={onClose}>{t('关闭')}</Button>
          <Button variant={action === 'delete' ? 'destructive' : 'default'} disabled={busy} onClick={() => void run()}>
            {busy ? t('执行中…') : t('执行{action}', { action: label })}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
