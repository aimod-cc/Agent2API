import * as React from 'react'
import { Badge, Button, InputGroup, InputGroupAddon, InputGroupInput, SegmentedControl, Switch, cn } from '@ui'
import { t } from '../i18n'
import {
  counts, editingProfile, enabledCount, getAgentStore, selectClient, selectedClient, setFilter,
  setPreview, setQuery, startEditing, subscribeAgentStore, visibleClients, type AgentClient,
  type AgentProfile,
} from './agent-state'
import { ProfileEditor } from './agent-editor'
import { WritePreview } from './agent-preview'

/**
 * 「Agent」页（React 岛）：本机 AI 客户端的配置接管。
 *
 * ── 这一页在转发链里的位置 ─────────────────────────────────
 * 模型管理决定「请求发往哪家上游」，网络代理决定「从哪条线路出去」，
 * 这一页决定**「谁把请求发进来」**：本机装了哪些 AI 客户端、它们的配置
 * 文件在哪、现在指向谁。侧栏里夹在「模型管理」与「网络代理」之间。
 *
 * ── 两种客户端形态（视图按 configMode 分叉）─────────────────
 *   single  一个文件里只有一套生效；多套配置存在我们这边，切换 = 整段覆写
 *   multi   客户端自己管理多套配置（ZCode），切换只改 enabled / 默认项
 * 区别体现在三处：标题旁的徽章、配置行开关的语义、写入预览的口径。
 *
 * ── 布局：左栏客户端 + 右栏详情（照模型管理页的 .mm 两栏）──────
 * 左栏是客户端清单（已安装 / 未安装两组），右栏是选中客户端的配置列表 +
 * 编辑器 + 写入预览。写入预览**独立成块、永远摊开**：它是配置与能力共同
 * 的结果，塞进 Tab 会让「改完看不见写了什么」。
 *
 * ── 挂载：面板岛模式，root 直接建在既有页面区块上 ──────────────
 * 与 accounts-page / proxies-page 同一手法。对外契约
 * `window.wbAgentPanel = { load }`，app.js 切到本页时调。
 */

/* ─── 小组件 ───────────────────────────────── */

/**
 * 客户端头像：优先用品牌真实图标，没有（或加载失败）才退回字母 monogram。
 *
 * 图标来自 magpie 的客户端图标库（各家官方标识）。其中一部分是
 * `currentColor` 的单色 SVG、另一部分是自带配色的彩图，所以在深色主题下
 * 统一垫一层中性底（.agent-avatar 的 background）—— 白底图标在深色页面上
 * 直接贴上去会「消失」，垫底后两种都能看清。
 *
 * 加载失败要能降级：图标文件缺失时 <img> 会显示破图，比字母更难看，
 * 所以 onError 时把 state 翻成 false，渲染回字母。
 */
function ClientAvatar({ client, size = 'md' }: { client: AgentClient; size?: 'sm' | 'md' | 'lg' }) {
  const [broken, setBroken] = React.useState(false)
  const showIcon = Boolean(client.icon) && !broken
  return (
    <span
      className={cn(
        'agent-avatar',
        size === 'lg' && 'is-lg',
        size === 'sm' && 'is-sm',
        showIcon && 'has-icon',
        // 自带整块方形底的图标铺满格子，别在方块里再套方块
        showIcon && client.iconFull && 'is-full',
      )}
      style={{ '--av-h': String(client.hue) } as React.CSSProperties}
      aria-hidden='true'
    >
      {showIcon ? (
        <img src={client.icon} alt='' loading='lazy' onError={() => setBroken(true)} />
      ) : (
        client.letter
      )}
    </span>
  )
}

/** 配置来源的展示档：它指向谁 */
const KIND_META: Record<string, { label: string; tone: 'brand' | 'ok' | 'warn' | 'plain' }> = {
  gateway: { label: t('本机网关'), tone: 'brand' },
  official: { label: t('官方直连'), tone: 'plain' },
  relay: { label: t('第三方中转'), tone: 'warn' },
  local: { label: t('本机服务'), tone: 'ok' },
}

export function kindLabel(kind: string): string {
  return KIND_META[kind]?.label || kind
}

/** 配置来源对应的徽章档（Badge 的 variant 取值） */
export function kindVariant(kind: string): 'brand' | 'success' | 'warning' | 'outline' {
  return (
    { gateway: 'brand', official: 'outline', relay: 'warning', local: 'success' } as const
  )[kind as 'gateway' | 'official' | 'relay' | 'local'] || 'outline'
}

export function protocolLabel(protocol: string): string {
  return (
    {
      anthropic: 'Anthropic Messages',
      responses: 'OpenAI Responses',
      openai_chat: 'OpenAI Chat',
      gemini: 'Gemini',
    }[protocol] || protocol
  )
}

/* ─── 页面 ─────────────────────────────────── */

export function AgentPage() {
  const store = React.useSyncExternalStore(subscribeAgentStore, getAgentStore, getAgentStore)
  const client = selectedClient()
  const list = visibleClients(store)
  const stats = counts(store)

  const installed = list.filter(item => item.installed)
  const missing = list.filter(item => !item.installed)

  const filterOptions = React.useMemo(
    () => [
      { value: 'all', label: `${t('全部')} ${stats.all}` },
      { value: 'via', label: `${t('走网关')} ${stats.via}` },
      { value: 'direct', label: `${t('未走网关')} ${stats.direct}` },
      { value: 'missing', label: `${t('未安装')} ${stats.missing}` },
    ],
    [stats.all, stats.via, stats.direct, stats.missing],
  )

  return (
    <section className='panel agent-panel'>
      <div className='toolbar agent-toolbar'>
        <InputGroup className='agent-search'>
          <InputGroupInput
            type='search'
            placeholder={t('搜索客户端 / 配置文件…')}
            aria-label={t('搜索客户端')}
            autoComplete='off'
            value={store.query}
            onChange={event => setQuery(event.currentTarget.value)}
          />
          <InputGroupAddon aria-hidden='true'>⌕</InputGroupAddon>
        </InputGroup>
        <SegmentedControl
          options={filterOptions}
          value={store.filter}
          onValueChange={setFilter}
          aria-label={t('按状态筛选')}
          className='shrink-0'
        />
      </div>

      {store.error ? <div className='agent-error'>{store.error}</div> : null}

      <div className='agent-body'>
        {/* ── 左栏：客户端清单 ── */}
        <aside className='agent-rail' aria-label={t('客户端清单')}>
          <div className='agent-rail-scroll'>
            {installed.length ? (
              <>
                <div className='agent-rail-label'>{t('已安装')}（{installed.length}）</div>
                {installed.map(item => (
                  <ClientRow key={item.id} client={item} active={item.id === store.selected} />
                ))}
              </>
            ) : null}
            {missing.length ? (
              <>
                <div className='agent-rail-label'>{t('未安装')}（{missing.length}）</div>
                {missing.map(item => (
                  <ClientRow key={item.id} client={item} active={item.id === store.selected} />
                ))}
              </>
            ) : null}
            {!list.length ? <div className='empty'>{t('没有匹配的客户端')}</div> : null}
          </div>
        </aside>

        {/* ── 右栏：详情 ── */}
        <div className='agent-detail'>
          {store.loading && !store.clients.length ? (
            <div className='empty'>{t('正在读取本机客户端…')}</div>
          ) : client ? (
            <ClientDetail client={client} />
          ) : (
            <div className='empty'>{t('选一个客户端开始')}</div>
          )}
        </div>
      </div>

    </section>
  )
}

/* ─── 左栏一行 ─────────────────────────────── */

function ClientRow({ client, active }: { client: AgentClient; active: boolean }) {
  const activeProfile = client.profiles.find(item => item.id === client.activeId)
  const multi = client.configMode === 'multi' && enabledCount(client) > 1
  return (
    <button
      type='button'
      className={cn(
        'agent-client',
        active && 'is-active',
        !client.installed && 'is-missing',
        client.installed && !client.viaGateway && 'is-direct',
      )}
      onClick={() => selectClient(client.id)}
    >
      <ClientAvatar client={client} />
      <span className='agent-client-main'>
        <span className='agent-client-name'>
          {client.name}
          <span className='agent-client-ver'>{client.installed ? client.configPath : t('未检测到')}</span>
        </span>
        <span className='agent-client-sub'>
          {client.installed ? activeProfile?.name || t('无配置') : client.vendor}
          {multi ? (
            <span className='agent-multi' title={t('这个客户端自己支持多套配置并存')}>
              ×{enabledCount(client)}
            </span>
          ) : null}
        </span>
      </span>
      <span
        className={cn(
          'agent-dot',
          !client.installed && 'is-missing',
          client.installed && client.viaGateway && 'is-on',
        )}
        aria-hidden='true'
      />
    </button>
  )
}

/* ─── 右栏详情 ─────────────────────────────── */

function ClientDetail({ client }: { client: AgentClient }) {
  const store = React.useSyncExternalStore(subscribeAgentStore, getAgentStore, getAgentStore)
  const editing = editingProfile(client)
  const multi = client.configMode === 'multi'

  // 换客户端 / 换编辑目标时重新取一次写入预览。
  // 依赖里不能放 `store.preview === null`：预览失败时它一直是 null，会变成
  // 无限重取。切客户端与切配置本来就会改这两个 id，够用了。
  React.useEffect(() => {
    if (!client.installed) {
      setPreview(null)
      return
    }
    void import('./agent-state').then(module => {
      void module.refreshPreview(client.id, editing?.id)
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client.id, editing?.id, client.installed])

  if (!client.installed) {
    return (
      <>
        <div className='agent-dhead'>
          <ClientAvatar client={client} size='lg' />
          <div className='agent-dtitle'>
            <h3>{client.name}</h3>
            <div className='agent-meta'>
              <span>{client.vendor}</span>
              <span>·</span>
              <span>{t('未检测到安装')}</span>
            </div>
          </div>
        </div>
        <div className='agent-empty'>
          <div className='agent-empty-title'>{t('本机没有找到 {name}', { name: client.name })}</div>
          <p>{t('装好后回到这一页点「重新检测」，它会出现在上面的列表里。')}</p>
          <code className='agent-empty-path'>{client.configPath}</code>
        </div>
      </>
    )
  }

  return (
    <>
      <div className='agent-dhead'>
        <ClientAvatar client={client} size='lg' />
        <div className='agent-dtitle'>
          <h3>
            {client.name}
            {multi ? (
              <Badge variant='warning' className='agent-mode-badge'>
                {t('多配置 · 已启用 {n}', { n: enabledCount(client) })}
              </Badge>
            ) : null}
          </h3>
          <div className='agent-meta'>
            <code title={client.resolvedPath || client.configPath}>{client.configPath}</code>
          </div>
        </div>
      </div>

      <ProfileEditor client={client} />
      <WritePreview client={client} />
    </>
  )
}

export default AgentPage

/* ─── 挂载 ─────────────────────────────────── */

const PAGE_SELECTOR = '.page[data-page="agent"]'

let mounted = false

function mount(): void {
  if (mounted) return
  const section = document.querySelector<HTMLElement>(PAGE_SELECTOR)
  if (!section) return
  mounted = true
  section.replaceChildren()
  // 懒引入 createRoot：与其它岛一致（都从 react-dom/client 取）
  void import('react-dom/client').then(({ createRoot }) => {
    createRoot(section).render(<AgentPage />)
  })
}

if (document.querySelector(PAGE_SELECTOR)) mount()
else document.addEventListener('DOMContentLoaded', mount, { once: true })

declare global {
  interface Window {
    wbAgentPanel?: { load: (silent?: boolean) => Promise<void> }
  }
}

void import('./agent-state').then(module => {
  window.wbAgentPanel = { load: module.loadPanel }
})
