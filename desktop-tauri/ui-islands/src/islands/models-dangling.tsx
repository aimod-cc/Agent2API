import * as React from 'react'
import { Badge, Button } from '@ui'
import { t } from '../i18n'
import { ALL_CUSTOM, type ManageMapping } from './models-custom-source'
import {
  bindingKeyOf, currentProvider, esc, getSnapshot, mappings, providerLabelOf, runRowAction, same,
  shared, subscribe, writeRemoveMapping,
} from './models-panel-state'

/**
 * 「未挂载的模型映射」区块。
 *
 * ── 它解决什么 ────────────────────────────────────────────────
 * 模型映射挂在哪一列，取决于它的**目标上游模型**还在不在清单里：管理页按
 * `(提供商 × 上游模型 id)` 分行渲染，映射的 chip 只长在那一行上（见
 * `models-panel-state::bindingsOf`）。于是上游把一个模型下线（或换了 id）之后，
 * 指向它的映射就**没有任何宿主行了** —— 后端 `catalog::manage_view` 仍然如实
 * 把它返回出来（`dangling: true`），但界面此前的渲染路径整条按模型行来，这条
 * 映射既看不见也删不掉，用户只能去手改 config.json。
 *
 * 这个区块就是那批条目的落点：直接读快照里 `dangling` 的那些映射（含关闭状态
 * 与自定义家侧），列出来、给删除入口。删掉之后该对外名即可重新使用（同名的
 * 新映射不会再被判「已有启用绑定」）。
 *
 * ── 为什么读 `mappings()` 而不是模型行 ────────────────────────
 * `viewData().mappings` 已经**含全部映射**（启用 / 关闭 / 悬空都在），
 * `dangling` 是后端与自定义家适配层各自算好的「没有宿主行」标记 —— 前端不再
 * 自己推断「目标在不在清单里」（那需要一份清单，而这里只该信任后端给的事实）。
 *
 * ── 与「搜索 / 筛选」的关系（刻意不参与）─────────────────────
 * 它是**数据健康提醒**而不是列表：当前筛选出多少模型与「有没有失效映射」是两件
 * 事。所以它不跟着搜索词 / 状态筛选缩水，只要当前视图下有悬空条目就显示。
 */
export function DanglingMappings() {
  const state = React.useSyncExternalStore(subscribe, getSnapshot)
  const provider = currentProvider()
  /** 两个聚合视图（内置「全部」/ 自定义「全部」）显示各自范围内的全部悬空条目 */
  const aggregate = provider === 'all' || provider === ALL_CUSTOM
  const items = mappings()
    .filter(mapping => mapping.dangling === true)
    // 单家视图里只显示「属于这一家」与「旧版全局条目（provider 为空，对所有家生效）」
    // 两类 —— 与行内 chip 的口径一致（见 models-panel-state::bindingsOf）
    .filter(mapping => aggregate || !mapping.provider || same(mapping.provider, provider))
    .sort((left, right) => (left.provider || '').localeCompare(right.provider || '')
      || left.alias.localeCompare(right.alias))
  if (!items.length) return null

  /**
   * 删除一条：三元组精确定位（同名映射可有多条），走页面既有的写队列与 toast。
   * 删除后后端会把最新的 manage 视图整份带回来（`writeRemoveMapping` → `accept`），
   * 这个区块随之重算 —— 删到一条不剩时它自己消失。
   */
  async function remove(item: ManageMapping): Promise<void> {
    const alias = String(item.alias)
    const target = String(item.target)
    const ok = await shared().wbConfirm?.ask?.({
      title: t('删除未挂载的映射'),
      html: t('确定删除映射「<strong>{alias} → {target}</strong>」？它指向的上游模型已不在清单里。', {
        alias: esc(alias), target: esc(target),
      }),
      okText: t('删除'),
      okClass: 'danger',
    })
    if (!ok) return
    void runRowAction(
      bindingKeyOf(alias, target, item.provider || ''),
      () => writeRemoveMapping(item.provider || '', alias, target),
      t('映射已删除'),
    )
  }

  return (
    <section className='models-dangling' aria-label={t('未挂载的模型映射')}>
      <div className='dg-head'>
        <span className='dg-title'>{t('未挂载的模型映射')}</span>
        <Badge variant='warning' shape='tag'>{items.length}</Badge>
      </div>
      <p className='dg-hint'>
        {t('这些映射指向的上游模型已不在当前清单里，挂不到任何模型行上：它们不会出现在 /v1/models，用对应别名发请求也会失败。删除后该对外名即可重新使用。')}
      </p>
      <ul className='dg-list'>
        {items.map(item => {
          const providerId = item.provider || ''
          const alias = String(item.alias)
          const target = String(item.target)
          // 自映射（alias == target）= 模型自己的默认绑定。用户对该模型启停过，配置里
          // 就会留下这一条；模型随后下线，它就挂不到行了。显示成 `x → x` 只会让人困惑，
          // 所以只写一次并挂一枚「默认绑定」标注。
          const isDefault = item.isDefault === true || same(alias, target)
          const busy = state.pending.has(bindingKeyOf(alias, target, providerId))
          return (
            <li className='dg-item' key={`${providerId}\u0001${alias}\u0001${target}`}>
              <span className='dg-prov'>
                {providerId ? providerLabelOf(providerId) : t('全部提供商')}
              </span>
              <span className='dg-map'>
                <code data-copy={alias} title={t('点击复制：{name}', { name: alias })}>{alias}</code>
                {isDefault ? null : (
                  <>
                    <span className='dg-arrow' aria-hidden='true'> → </span>
                    <code data-copy={target} title={t('点击复制：{name}', { name: target })}>{target}</code>
                  </>
                )}
              </span>
              {isDefault ? <span className='dg-tag'>{t('默认绑定')}</span> : null}
              {/* 目标是「上游下线」还是「承载它的家当前不在清单里」，指向的改法不同，
                  所以把这两种情形分在 title 里说清（见后端 manage_view 的 carried） */}
              {item.carried === false ? (
                <span className='dg-state' title={t('上游已经不再提供这个模型（可能已下线或改了 ID）')}>{t('目标已下线')}</span>
              ) : (
                <span className='dg-state' title={t('这个模型仍在承载它的提供商清单里，但那一家的模型行当前没有出现（例如该家没有可用账号）')}>{t('未接入')}</span>
              )}
              <Button variant='ghost' size='sm' className='text-destructive' disabled={busy}
                title={t('删除这条映射')}
                onClick={() => void remove(item)}>{t('删除')}</Button>
            </li>
          )
        })}
      </ul>
    </section>
  )
}
