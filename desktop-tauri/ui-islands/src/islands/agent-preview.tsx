import * as React from 'react'
import { Button, cn } from '@ui'
import { t } from '../i18n'
import { editingProfile, getAgentStore, refreshPreview, subscribeAgentStore, type AgentClient } from './agent-state'

/**
 * 写入预览（「Agent」页最下面那一块，永远摊开、不折叠）。
 *
 * ── 为什么独立成块而不是一个 Tab ────────────────────────────
 * 它是「配置 + 提示词 / Skills / MCP 同步」共同的结果，属于**输出**而不是
 * 输入。塞进 Tab 的话，用户改模型时看不见它会写成什么，每次改完都要多点
 * 一次才能确认 —— 而改配置的那一刻正是最需要看到它的时刻。
 *
 * ── 高亮口径 ────────────────────────────────────────────
 * 后端返回整份文件文本 + 改动行号（`changed`）与新增行号（`added`），
 * 都从 1 起。前端只负责染色：新增行给绿底，改动行给黄底，两者重叠时以
 * 新增为准。这里不重新 diff —— 两边各算一次会算出两种结论。
 */

export function WritePreview({ client }: { client: AgentClient }) {
  const store = React.useSyncExternalStore(subscribeAgentStore, getAgentStore, getAgentStore)
  const editing = editingProfile(client)
  const preview = store.preview

  const lines = React.useMemo(() => (preview?.text ? preview.text.split('\n') : []), [preview?.text])
  const changed = React.useMemo(() => new Set(preview?.changed || []), [preview?.changed])
  const added = React.useMemo(() => new Set(preview?.added || []), [preview?.added])

  const reload = () => {
    void refreshPreview(client.id, editing?.id)
  }

  if (!client.installed) {
    return null
  }

  return (
    <div className='agent-card agent-preview-card'>
      <div className='agent-card-head'>
        <h4>{t('写入预览')}</h4>
        <span className='spacer' />
        <code className='agent-preview-path'>{client.configPath}</code>
        <Button variant='ghost' size='xs' onClick={reload} disabled={store.previewing}>
          {store.previewing ? t('刷新中…') : t('刷新')}
        </Button>
      </div>
      <div className='agent-card-body'>
        {preview?.text ? (
          <>
            {/* 图例与副作用提示挤在同一行：注释会被清掉这件事必须说，
                但不值得单独占一整块（用户每看一眼预览就多一行噪音）。 */}
            <div className='agent-preview-legend'>
              <span className='agent-legend-swatch is-changed' /> {t('改动')}
              <span className='agent-legend-swatch is-added' /> {t('新增')}
              {preview.warnings?.length ? (
                <span className='agent-warn-inline' title={preview.warnings.join(' / ')}>
                  · {preview.warnings[0]}
                </span>
              ) : null}
            </div>
            <pre className='agent-code'>
              {lines.map((line, index) => {
                const number = index + 1
                const isAdded = added.has(number)
                const isChanged = changed.has(number)
                return (
                  <span
                    key={number}
                    className={cn('agent-line', isAdded && 'is-added', !isAdded && isChanged && 'is-changed')}
                  >
                    <span className='agent-line-no'>{number}</span>
                    <span className='agent-line-text'>{line || ' '}</span>
                  </span>
                )
              })}
            </pre>
          </>
        ) : (
          <div className='empty'>
            {store.previewing
              ? t('正在生成预览…')
              : t('暂无预览')}
          </div>
        )}
      </div>
    </div>
  )
}
