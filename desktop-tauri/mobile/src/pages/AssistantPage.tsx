import { useState } from 'react'
import { Icon } from '../components/Icon'
import { useStore, store } from '../state/store'
import { t } from '../i18n'

/**
 * AI 助手。
 *
 * ── 手机端最值得保留的一级入口 ────────────────────────────────
 * 「查一下余额」「把重复的模型加上前缀」这类操作用手机最方便，不必回电脑。
 *
 * ── 为什么先做「本地问答」而不是直连 SSE ──────────────────────
 * 后端 `/api/assistant/chat` 是 SSE 流（OpenAI delta + tool_calls），完整
 * 实现要处理工具循环、确认卡、多轮上限。手机端这一版先把**不需要模型的
 * 快捷查询**做实：它直接打管理 API 拿真实数据，零 Token、零等待 —— 覆盖
 * 「看看余额」「哪些模型重复」「今天多少请求」这类最高频的用法。
 * 自由对话入口保留（引导去桌面端），等这一版验证过再补流式。
 *
 * 这是有意的分阶段：先让手机上真的能办事，而不是先搬一个半成品对话框。
 */

type Message =
  | { role: 'user'; text: string }
  | { role: 'assistant'; text: string; rows?: Array<{ label: string; value: string; mono?: boolean }> }
  | { role: 'tool'; title: string; state: 'done' | 'failed'; text: string }

type QuickAsk = {
  key: string
  label: string
  icon: Parameters<typeof Icon>[0]['name']
}

const QUICK_ASKS: QuickAsk[] = [
  { key: 'balance', label: t('各账号余额还剩多少？'), icon: 'wallet' },
  { key: 'duplicates', label: t('哪些模型在多家重复了？'), icon: 'model' },
  { key: 'today', label: t('今天用了多少 Token？'), icon: 'chart' },
  { key: 'limited', label: t('现在有哪些账号被限流？'), icon: 'alert' },
]

export function AssistantPage() {
  const state = useStore()
  const [messages, setMessages] = useState<Message[]>([])
  const [busy, setBusy] = useState(false)

  const ask = async (item: QuickAsk) => {
    if (busy) return
    setBusy(true)
    setMessages(prev => [...prev, { role: 'user', text: item.label }])
    try {
      const reply = await answer(item.key, state)
      setMessages(prev => [...prev, ...reply])
    } catch (error) {
      setMessages(prev => [
        ...prev,
        {
          role: 'tool',
          title: item.label,
          state: 'failed',
          text: error instanceof Error ? error.message : String(error),
        },
      ])
    } finally {
      setBusy(false)
    }
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', height: '100%' }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 9, padding: '10px 14px 10px', flex: 'none' }}>
        <div style={{ flex: 1, minWidth: 0 }}>
          <b style={{ fontSize: 15 }}>{t('AI 助手')}</b>
          <span style={{ display: 'block', fontSize: 11, color: 'var(--m-text-3)', marginTop: 1 }}>
            {t('快捷查询走本地网关，不消耗额度')}
          </span>
        </div>
        <button
          type='button'
          className='m-icon-btn'
          aria-label={t('清空')}
          onClick={() => setMessages([])}
        >
          <Icon name='refresh' />
        </button>
      </div>

      <div
        style={{
          flex: 1,
          minHeight: 0,
          overflowY: 'auto',
          padding: '4px 14px 8px',
          display: 'flex',
          flexDirection: 'column',
          gap: 12,
          scrollbarWidth: 'none',
        }}
      >
        {messages.length === 0 ? (
          <div style={{ display: 'flex', gap: 8 }}>
            <Avatar />
            <div style={bubbleStyle(false)}>
              {t('我可以直接查网关里的真实数据：余额、模型重复、今日用量、限流账号。')}
              <br />
              {t('要改配置的话，桌面端的助手支持完整对话与工具调用。')}
            </div>
          </div>
        ) : null}

        {messages.map((message, index) => {
          if (message.role === 'user') {
            return (
              <div key={index} style={{ display: 'flex', justifyContent: 'flex-end' }}>
                <div style={bubbleStyle(true)}>{message.text}</div>
              </div>
            )
          }
          if (message.role === 'tool') {
            return (
              <div key={index} style={{ display: 'flex', gap: 8 }}>
                <Avatar />
                <div
                  style={{
                    border: '1px solid var(--m-line)',
                    borderRadius: 13,
                    background: 'var(--m-surface)',
                    overflow: 'hidden',
                    maxWidth: 288,
                  }}
                >
                  <div
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      gap: 7,
                      padding: '9px 11px',
                      background: 'var(--m-surface-2)',
                      borderBottom: '1px solid var(--m-line)',
                      fontSize: 12,
                      fontWeight: 600,
                    }}
                  >
                    <Icon name='request' style={{ width: 14, height: 14, color: 'var(--m-primary)' }} />
                    {message.title}
                    <span
                      style={{
                        marginLeft: 'auto',
                        fontSize: 10.5,
                        color: message.state === 'done' ? 'var(--m-ok)' : 'var(--m-danger)',
                      }}
                    >
                      {message.state === 'done' ? t('✓ 完成') : t('✗ 失败')}
                    </span>
                  </div>
                  <div style={{ padding: '10px 11px', fontSize: 12.3, color: 'var(--m-text-2)', lineHeight: 1.6 }}>
                    {message.text}
                  </div>
                </div>
              </div>
            )
          }
          return (
            <div key={index} style={{ display: 'flex', gap: 8 }}>
              <Avatar />
              <div style={{ ...bubbleStyle(false), padding: 0, overflow: 'hidden' }}>
                <div style={{ padding: '10px 13px', fontSize: 13, lineHeight: 1.6 }}>{message.text}</div>
                {message.rows?.length ? (
                  <div style={{ borderTop: '1px solid var(--m-line)' }}>
                    {message.rows.map((row, rowIndex) => (
                      <div
                        key={rowIndex}
                        style={{
                          display: 'flex',
                          alignItems: 'center',
                          gap: 8,
                          padding: '8px 13px',
                          borderBottom: rowIndex === (message.rows?.length || 0) - 1 ? 0 : '1px solid var(--m-line)',
                          fontSize: 12,
                        }}
                      >
                        <span style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                          {row.label}
                        </span>
                        <span className={row.mono ? 'm-mono' : undefined} style={{ color: 'var(--m-text-2)', fontSize: 11.5 }}>
                          {row.value}
                        </span>
                      </div>
                    ))}
                  </div>
                ) : null}
              </div>
            </div>
          )
        })}

        {messages.length === 0 ? (
          <div style={{ display: 'flex', flexDirection: 'column', gap: 7, paddingTop: 2 }}>
            {QUICK_ASKS.map(item => (
              <button
                key={item.key}
                type='button'
                onClick={() => void ask(item)}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: 8,
                  padding: '10px 12px',
                  borderRadius: 11,
                  border: '1px solid var(--m-line)',
                  background: 'var(--m-surface)',
                  fontSize: 12.5,
                  color: 'var(--m-text-2)',
                  textAlign: 'left',
                }}
              >
                <Icon name={item.icon} style={{ width: 14, height: 14, color: 'var(--m-text-3)', flex: 'none' }} />
                {item.label}
              </button>
            ))}
          </div>
        ) : null}

        {busy ? (
          <div style={{ display: 'flex', gap: 8 }}>
            <Avatar />
            <div style={bubbleStyle(false)}>{t('查询中…')}</div>
          </div>
        ) : null}
      </div>

      <div
        style={{
          flex: 'none',
          padding: '8px 12px calc(12px + var(--m-safe-b))',
          borderTop: '1px solid var(--m-line)',
          background: 'color-mix(in srgb, var(--m-bg) 88%, transparent)',
          backdropFilter: 'blur(12px)',
        }}
      >
        <button
          type='button'
          onClick={() => store.toast(t('完整对话在桌面端「AI 助手」页'))}
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 8,
            width: '100%',
            padding: '7px 7px 7px 13px',
            borderRadius: 20,
            background: 'var(--m-surface)',
            border: '1px solid var(--m-line-strong)',
            color: 'var(--m-text-3)',
            fontSize: 13,
            textAlign: 'left',
          }}
        >
          <span style={{ flex: 1 }}>{t('自由对话请用桌面端…')}</span>
          <span
            style={{
              width: 34,
              height: 34,
              borderRadius: '50%',
              background: 'var(--m-primary)',
              color: '#fff',
              display: 'grid',
              placeItems: 'center',
              flex: 'none',
              opacity: 0.55,
            }}
          >
            <Icon name='up' style={{ width: 17, height: 17 }} />
          </span>
        </button>
        <div style={{ textAlign: 'center', fontSize: 10.5, color: 'var(--m-text-3)', marginTop: 7 }}>
          {t('上面四个快捷查询直接读网关数据，不经过模型')}
        </div>
      </div>
    </div>
  )
}

function Avatar() {
  return (
    <span
      style={{
        width: 26,
        height: 26,
        borderRadius: 8,
        flex: 'none',
        display: 'grid',
        placeItems: 'center',
        background: 'var(--m-primary-soft)',
        color: 'var(--m-primary)',
      }}
    >
      <Icon name='spark' style={{ width: 15, height: 15 }} />
    </span>
  )
}

function bubbleStyle(mine: boolean): React.CSSProperties {
  return {
    maxWidth: '82%',
    padding: '10px 13px',
    borderRadius: mine ? '14px 14px 5px 14px' : '14px 14px 14px 5px',
    background: mine ? 'var(--m-primary)' : 'var(--m-surface)',
    color: mine ? '#fff' : 'var(--m-text)',
    border: mine ? 0 : '1px solid var(--m-line)',
    fontSize: 13.5,
    lineHeight: 1.62,
  }
}

/* ─── 四个快捷查询的答案 ─────────────────────── */

async function answer(key: string, state: ReturnType<typeof useStore>): Promise<Message[]> {
  const { fetchAccounts, fetchManage, fetchStatsSummary } = await import('../api/gateway')

  if (key === 'balance') {
    const data = await fetchAccounts()
    const accounts = data.accounts || []
    if (!accounts.length) {
      return [{ role: 'tool', title: t('查询账号'), state: 'done', text: t('还没有账号。') }]
    }
    const rows = accounts.slice(0, 8).map(item => ({
      label: (item.email || item.name || item.id) as string,
      value: balanceText(item),
    }))
    return [
      {
        role: 'assistant',
        text: t('共 {n} 个账号，前 {m} 个的余额读数：', { n: accounts.length, m: rows.length }),
        rows,
      },
      {
        role: 'tool',
        title: t('读取账号列表'),
        state: 'done',
        text: t('余额是缓存读数，点账号详情里的「刷新凭证」可触发实时查询。'),
      },
    ]
  }

  if (key === 'duplicates') {
    const view = await fetchManage()
    const models = view.models || []
    const byName = new Map<string, Set<string>>()
    models.forEach(item => {
      const name = String(item.id || '')
      if (!name) return
      const set = byName.get(name) || new Set<string>()
      set.add(String(item.provider || ''))
      byName.set(name, set)
    })
    const dupes = [...byName.entries()].filter(([, providers]) => providers.size > 1)
    if (!dupes.length) {
      return [
        { role: 'assistant', text: t('当前没有同名模型分布在多家。') },
        { role: 'tool', title: t('读取模型清单'), state: 'done', text: t('共 {n} 个模型条目。', { n: models.length }) },
      ]
    }
    return [
      {
        role: 'assistant',
        text: t('找到 {n} 组同名模型：', { n: dupes.length }),
        rows: dupes.slice(0, 8).map(([name, providers]) => ({
          label: name,
          value: t('{n} 家', { n: providers.size }),
          mono: true,
        })),
      },
      {
        role: 'tool',
        title: t('读取模型清单'),
        state: 'done',
        text: t('共 {n} 个模型条目。映射与改名请在桌面端「模型管理」里做。', { n: models.length }),
      },
    ]
  }

  if (key === 'today') {
    const summary = await fetchStatsSummary('today')
    const overview = summary.overview
    const tokens = Number(overview?.tokens) || 0
    const requests = Number(overview?.requests) || 0
    const successful = Number(overview?.successful) || 0
    return [
      {
        role: 'assistant',
        text: t('今天用了 {tokens} Token，共 {requests} 次请求。', {
          tokens: tokens.toLocaleString('zh-CN'),
          requests: requests.toLocaleString('zh-CN'),
        }),
        rows: [
          { label: t('总请求'), value: String(requests) },
          { label: t('成功'), value: String(successful) },
          { label: t('成功率'), value: requests ? `${((successful / requests) * 100).toFixed(1)}%` : '—' },
          { label: t('Top 模型'), value: String(overview?.topModel?.model || '—'), mono: true },
        ],
      },
    ]
  }

  if (key === 'limited') {
    const data = await fetchAccounts()
    const limited = (data.accounts || []).filter(
      item => item.enabled && item.rateLimits && Object.keys(item.rateLimits).length,
    )
    if (!limited.length) {
      return [{ role: 'assistant', text: t('现在没有账号被限流。') }]
    }
    return [
      {
        role: 'assistant',
        text: t('{n} 个账号正在限流：', { n: limited.length }),
        rows: limited.slice(0, 8).map(item => ({
          label: (item.email || item.name || item.id) as string,
          value: t('{n} 个模型', { n: Object.keys(item.rateLimits || {}).length }),
        })),
      },
    ]
  }

  void state
  return [{ role: 'assistant', text: t('这个查询还没有实现。') }]
}

function balanceText(account: { [key: string]: unknown }): string {
  const candidates = ['balance', 'credits', 'points', 'available']
  for (const key of candidates) {
    const value = account[key]
    if (typeof value === 'number' && Number.isFinite(value)) return value.toLocaleString('zh-CN')
    if (typeof value === 'string' && value) return value
  }
  return '—'
}
