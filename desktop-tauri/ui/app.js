/* Agent2API · 桌面端渲染层 */
/* global workbuddyDesktop, wbI18n */

const api = window.workbuddyDesktop;
const $ = id => document.getElementById(id);
/**
 * 是不是「网页端」形态（headless / Docker：面板由网关自己托管）。
 * 判定口径与组件库标题栏一致 —— 桥接层注入的 platform 为 'web' 即网页端；
 * 桥还没就绪时按桌面形态处理（保守：桌面才是自绘窗口按钮那一套）。
 */
const isWebPanel = () => api?.platform === 'web';
let state = null;
// 最近一次 `refresh()` 的失败原因（成功时清空）。
// 它是给顶栏状态区挂 title 用的：后端不可达时页面上其余数字都只是「上一次的值」，
// 而顶栏是常驻可见的那一处，把原因挂在那里用户悬停就能看到（见 renderTopbarStatus）。
let stateError = '';
// 账号操作与网页登录共用这把锁；只读刷新使用自己的请求队列。
let busy = false;

// ─── 工具 ────────────────────────────────────

function esc(value) {
  return String(value ?? '').replace(/[&<>"']/g, ch => ({
    '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
  }[ch]));
}

function toast(message, type = 'ok') {
  window.wbShell?.toast(message, type);
}

function formatTime(value) {
  if (!value) return '';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return '';
  return date.toLocaleString('zh-CN', { hour12: false });
}

// ─── 主题 ────────────────────────────────────

function applyTheme(mode) {
  document.documentElement.dataset.theme = mode;
  // 同步操作系统标题栏的深浅色：界面切深色但标题栏仍是系统主题时，顶部会留一条白带。
  // 浏览器直开时没有桥，用可选链 + try 吞掉，避免影响主题本身生效。
  // 跟随系统时必须传 null 把窗口主题交回系统，不要在这里把它整形回 dark/light 两态：
  // 窗口一旦被手动主题钉住，WebView2 的 prefers-color-scheme 会跟着窗口走而非系统，
  // 下面读到的就是被污染的值，界面会永远卡死在手动主题上。
  // 这行还要排在读 matchMedia 之前：先交回系统才读得到真实值；
  // 交回系统是异步生效的，读到旧值时靠「change 监听 + 落定后复核」双保险兜住。
  // 「枫叶」两档是独立色板，明暗在名字里就定死了（枫叶·白 / 枫叶·黑），
  // 窗口主题按它们各自的明暗同步，不跟随系统。
  const windowTheme = mode === 'system' ? null : (mode === 'maple-dark' ? 'dark' : mode === 'maple-light' ? 'light' : mode);
  let pendingWindowTheme;
  try {
    pendingWindowTheme = window.workbuddyDesktop?.setWindowTheme?.(windowTheme);
  } catch { /* 非桌面环境忽略 */ }
  // system 模式下 color-scheme 也要跟随系统，否则原生控件会停留在浅色；
  // 枫叶两档按自身明暗定死（见 THEME_DARK_MODES / THEME_LIGHT_MODES）
  const systemDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
  const effective = mode === 'dark' || mode === 'maple-dark' || (mode === 'system' && systemDark) ? 'dark' : 'light';
  document.documentElement.style.colorScheme = effective;
  // 双保险①：文件末尾的 change 监听，兜「运行中系统主题变化」（以及多数切回 system 的场景）。
  // 双保险②：这里的落定后复核，兜「切回 system 时 WebView2 未必派发 change」——
  // 窗口主题真正恢复后若与本次判定不符，说明本次读到的是污染值，就按真实值再算一遍；
  // 再算时 matchMedia 已是真实值、条件自然不成立，所以最多多跑一轮，不会递归下去。
  // 非桌面环境下桥返回 undefined，可选链保证整条链静默跳过。
  pendingWindowTheme?.then?.(() => {
    // 手动 dark/light 是同步钉死窗口主题的语义，不存在污染，无需复核
    if (mode !== 'system') return;
    // 复核期间用户若已改成手动主题，以用户选择为准，不能拿本次判定覆盖回去
    if ((localStorage.getItem('workbuddy-desktop-theme') || 'system') !== 'system') return;
    const settledDark = window.matchMedia('(prefers-color-scheme: dark)').matches;
    if ((settledDark ? 'dark' : 'light') !== effective) applyTheme('system');
  })?.catch?.(() => { /* 命令失败时不做复核，界面保持本次结果 */ });
  localStorage.setItem('workbuddy-desktop-theme', mode);
  // 通知其它控制面（设置页「显示 → 显示模式」那组档位）：主题只有一个入口了
  // （侧边栏底部那组三键已移除），保留事件是因为 index.html 头部内联脚本与
  // 其它窗口仍需要它，插件页另走下面的 postMessage。
  window.dispatchEvent(new CustomEvent('wb:theme', { detail: mode }));
  // 插件页（iframe，独立文档）收不到上面这个事件，也读不到 localStorage ——
  // 主题由这里单独推一次，否则用户切了深浅色、插件页还停在原来的配色。
  // 没停在插件页时 querySelector 返回 null，`pushPanelTheme` 自己早退。
  pushPanelTheme(pluginPageHost()?.querySelector('iframe')?.contentWindow);
}

// ─── 界面缩放 ────────────────────────────────
// 「显示 → 界面缩放」：80%–130%、一档 5%（共 11 档，设置页的下拉就是这份口径）。
//
// app.js 是缩放的**唯一应用入口**（与主题同一取向）：设置页只调 wbApp.applyZoom
// 并听 'wb:zoom' 事件，不自己碰 localStorage、也不直接调桥 —— 缩放要落到
// WebView 层（壳命令 set_zoom，语义与浏览器 Ctrl +/- 相同），那是页面脚本
// 之外的事，由这里统一转达。
const ZOOM_KEY = 'workbuddy-desktop-zoom';
const ZOOM_MIN = 80;
const ZOOM_MAX = 130;
const ZOOM_STEP = 5;

/** 从 localStorage 读一个百分数：缺失 / 空白 / 非数字都算「没设过」，返回 null */
function readStoredPercent(key) {
  const text = localStorage.getItem(key);
  // 空串要单独挡：Number('') 是 0（不是 NaN），不挡就会被当成「0%」一路夹到 80%
  if (text === null || text.trim() === '') return null;
  const value = Number(text);
  return Number.isFinite(value) ? value : null;
}

/** 读上次的缩放（百分数）：越界 / 非档位值 / 读不到一律回落 100（不写回，等下次显式设置） */
function storedZoom() {
  const raw = readStoredPercent(ZOOM_KEY);
  if (raw === null) return 100;
  const snapped = Math.round(raw / ZOOM_STEP) * ZOOM_STEP;
  return Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, snapped));
}

function applyZoom(percent) {
  const raw = Number(percent);
  if (!Number.isFinite(raw)) return;
  const value = Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, Math.round(raw / ZOOM_STEP) * ZOOM_STEP));
  localStorage.setItem(ZOOM_KEY, String(value));
  // 浏览器直开（网页端）时没有桥：可选链静默跳过 —— 那边的缩放归浏览器自己的
  // Ctrl +/-，设置页同一项也是禁用状态（见 DisplayPane）。
  try {
    window.workbuddyDesktop?.setZoom?.(value / 100);
  } catch { /* 非桌面环境忽略 */ }
  window.dispatchEvent(new CustomEvent('wb:zoom', { detail: value }));
}

// ─── 页面导航 ────────────────────────────────

const PAGE_KEY = 'workbuddy-desktop-page';
const PAGES = ['assistant', 'overview', 'accounts', 'checkin', 'routing', 'gateway', 'agent', 'proxies', 'keys', 'docs', 'logs', 'tasks', 'requests', 'plugins', 'settings'];
/** 页签中文名：顶栏面包屑用。overview 的用户可见名是「报表」、gateway 的是「模型管理」
 *  （内部标识保持不变：localStorage 记忆、showPage 与 CSS 的 [data-page] 选择器都依赖它）
 *
 *  `checkin` 是**内置页**（见 index.html 的说明）：页面归网关，签到能力归插件。 */
const PAGE_LABELS = {
  assistant: wbI18n.t('AI 助手'),
  overview: wbI18n.t('报表'),
  accounts: wbI18n.t('账号'),
  checkin: wbI18n.t('签到中心'),
  routing: wbI18n.t('路由'),
  gateway: wbI18n.t('模型管理'),
  agent: wbI18n.t('Agent 配置'),
  proxies: wbI18n.t('网络代理'),
  keys: wbI18n.t('网关 Key'),
  docs: wbI18n.t('文档'),
  logs: wbI18n.t('日志'),
  tasks: wbI18n.t('定时任务'),
  requests: wbI18n.t('请求日志'),
  plugins: wbI18n.t('插件'),
  settings: wbI18n.t('设置'),
};

/** 当前页（子模块据此判断是否需要重新加载） */
let currentPage = 'overview';

/**
 * 插件申报的页面（`plugin:<pack>:<page>` → `{ label, url }`）。
 *
 * ── 与 PAGES 的关系 ─────────────────────────────────────────
 * `PAGES` 是编译期内置页的清单（`showPage` 的白名单、`data-page` 选择器都按它）。
 * 插件页是**运行期**才知道的：装了什么包、包里申报了哪些页面，只有问过网关
 * 才知道。所以它们单独存在这张表里，`showPage` 与 `renderSidebar` 两处都查
 * 一次 —— 内置页走原路径，插件页走 iframe。
 *
 * ── 没装插件时 ──────────────────────────────────────────────
 * 这张表是空的，侧边栏里没有插件页，`PAGES` 的行为与改造前逐字一致。
 */
const pluginPages = new Map();

/** 插件页菜单键的前缀（与 `sidebar-layout.js` 的 `PLUGIN_PREFIX` 同值） */
const PLUGIN_PAGE_PREFIX = 'plugin:';

/**
 * 从 `/api/plugins` 的响应里收下插件页，并让侧边栏跟上。
 *
 * `packs[].ui[]` 是每个包申报的页面（`{ id, title, icon, url, order }`），
 * 这里拼成 `plugin:<pack>:<page>` 形式的菜单键 —— 包 id 与页面 id 都在键里，
 * 于是「哪个包贡献了哪一页」一眼可见，卸载某个包时也只影响它自己的那几项。
 */
/**
 * 已由**内置页**接管的插件页面 id。
 *
 * ── 为什么要有这张表 ────────────────────────────────────────
 * 「签到中心」原先由插件申报（`pack.json` 的 `ui`），现在页面搬回客户端、成了
 * 内置页（`PAGES` 里的 `checkin`）。但**已发布的插件包仍带着那条申报** ——
 * 不拦的话，侧边栏会同时出现「签到中心」（内置）与「签到中心」（插件的
 * iframe），两个菜单项指向两个前端，正是这次迁移要消灭的东西。
 *
 * 于是按页面 id 去重：内置页已经覆盖的能力，插件的同名申报不再进菜单。
 * 插件包里的静态资源仍然可访问（`/plugins/<pack>/ui/...`），所以这不影响任何
 * 还在引用它的地方；等新页面稳定、旧资产一次性清掉时，这张表一起删。
 */
const BUILTIN_PAGE_IDS = new Set(['checkin'])

function adoptPluginPages(data) {
  const packs = Array.isArray(data?.packs) ? data.packs : [];
  const next = new Map();
  for (const pack of packs) {
    // 停用的包：页面也不该出现（与「插件不在，这家就不存在」同一原则）
    if (pack?.enabled === false) continue;
    const pages = Array.isArray(pack?.ui) ? pack.ui : [];
    for (const page of pages) {
      if (!page || typeof page.id !== 'string' || !page.id) continue;
      if (typeof page.url !== 'string' || !page.url) continue;
      // 内置页已接管这个能力：不再重复注册一个 iframe 菜单项
      if (BUILTIN_PAGE_IDS.has(page.id)) continue;
      const key = `${PLUGIN_PAGE_PREFIX}${pack.id}:${page.id}`;
      next.set(key, {
        key,
        label: String(page.title || page.id),
        url: page.url,
        icon: typeof page.icon === 'string' && page.icon ? page.icon : 'plugins',
        // 插件申报的默认归属分组：侧边栏据此决定「首次补进布局时放哪一组」
        // （见 sidebar-layout 的 normalizeLayout）。没申报就是空串。
        group: typeof page.group === 'string' ? page.group : '',
        order: Number(page.order) || 0,
      });
    }
  }
  const changed = next.size !== pluginPages.size
    || [...next.keys()].some(key => !pluginPages.has(key))
    || [...next].some(([key, page]) => {
      const prev = pluginPages.get(key);
      return !prev || prev.url !== page.url || prev.label !== page.label || prev.group !== page.group;
    });
  if (!changed) return;
  pluginPages.clear();
  for (const [key, page] of next) pluginPages.set(key, page);
  // 交给侧边栏模型（它负责「新菜单补进哪一组」与落盘校验）
  window.wbSidebarLayout?.setDynamicMenus?.([...pluginPages.values()]);
  renderSidebar();
  // 当前正停在某个插件页、而它刚被卸载：退回默认页，别让用户对着一块空白
  if (currentPage.startsWith(PLUGIN_PAGE_PREFIX) && !pluginPages.has(currentPage)) {
    showPage('overview', { persist: false });
  }
}

/**
 * 拉一次插件清单（装 / 卸 / 启停之后、以及启动时各调一次）。
 *
 * 失败静默：插件页是增强项，拉不到就当作「一个都没有」，侧边栏照常显示
 * 内置页 —— 为它弹一条错误提示只会让「没装插件」看起来像故障。
 */
async function refreshPluginPages(payload) {
  if (payload) {
    adoptPluginPages(payload);
    return;
  }
  try {
    const data = await api.getPlugins();
    adoptPluginPages(data);
  } catch {
    /* 读不到 = 没有插件页 */
  }
}

/**
 * 侧边栏布局（分组 / 顺序 / 显隐）：数据口径全部在 ui/sidebar-layout.js，
 * 这里只负责「按它把 #nav 画出来」与「设置页改完后重画」。
 *
 * 每次重画都是整块重建 #nav 的子节点 —— 与账号表「整表重建」同一取舍：
 * 布局变更频率极低（用户手动调一次），增量更新换不来任何收益，却要维护
 * 「哪个节点对应哪个菜单」的映射。三颗角标是**按 id 重建**的，所以重画之后
 * 它们的显示状态会回到初始（隐藏），需要立刻补一次重绘（见 syncNavBadges）。
 */
function sidebarApi() {
  return window.wbSidebarLayout;
}

/**
 * 当前生效的布局（每次都从 localStorage 读：设置页改完只需派事件，不必传值）。
 *
 * ⚠ 读的是**首帧缓存**，权威副本在库里（见 ui/sidebar-layout.js 模块头）。
 * `syncFromServer` 把库里的那份拉回来写进缓存之后，这里读到的就是它。
 */
function readSidebarLayout() {
  return sidebarApi()?.readLayout?.() || null;
}

/**
 * 画导航。
 *
 * 契约（与 index.html 里那段注释同源，改这里必须一起看）：
 *   · 菜单键 = data-page，与 PAGES / PAGE_LABELS 逐字一致；
 *   · 角标 id 固定三颗：nav-count-requests / -logs / -update；
 *   · 分组标题用 .nav-group-label（≤900px 由 layout.css 整列隐藏）。
 * 图标不在这里注入：画完调 paintIcons()，它按 data-icon 从 icons.js 取图形。
 */
function renderSidebar() {
  const api = sidebarApi();
  if (!api) return;
  window.wbShell?.sidebar(api.renderGroups(api.readLayout()));
  markActiveNav();
  syncNavBadges();
}

function markActiveNav() {
  const label = pluginPages.get(currentPage)?.label || PAGE_LABELS[currentPage] || currentPage;
  window.wbShell?.page(currentPage, label);
}

/**
 * 重画之后补一次角标。
 *
 * 三颗角标的数据都由轮询维护在模块级变量里（runningRequests / unreadFailures /
 * lastUpdateInfo / 未读日志水位），重画把 DOM 换掉之后新节点是「隐藏 + 0」，
 * 必须按现有内存值重新点一次，否则用户改完布局要等下一轮轮询（最长 20 秒）
 * 才重新看到提示。查询本身不发 —— 这里只用已经拿到的数。
 *
 * `navBadgesReady` 是必须的：首次渲染发生在模块顶部（那时还不能被桥调用中断），
 * 而那些计数用的是 `let`（TDZ）—— 提前读会抛 ReferenceError，把整段脚本带崩。
 * 首次渲染只画结构就够了（角标本来就该是隐藏的），等这些变量都初始化完再置位。
 */
let navBadgesReady = false;

function syncNavBadges() {
  if (!navBadgesReady) return;
  paintRequestsBadge();
  paintLogsBadgeFromMemory();
  syncUpdateBadge();
}

/**
 * 插件页的宿主 section（`index.html` 里 `data-page="__plugin__"`）。
 *
 * 它不是 `PAGES` 里的页面 —— 插件页的键是运行期才有的，不可能写进静态白名单。
 * 显隐由下面两个函数直接管：切进插件页时建 iframe，切走时整块清掉。
 */
function pluginPageHost() {
  return document.querySelector('.page[data-page="__plugin__"]');
}

/**
 * 插件页的绝对地址。
 *
 * 插件清单里的 `url` 是网关侧的相对路径（`/plugins/<pack>/ui/<entry>`）。
 * **不能直接喂给 iframe**：桌面端的页面来自 Tauri 的自定义协议
 * （`tauri://localhost` 之类），相对路径会解析到壳自己的资源目录 —— iframe
 * 里是一片 404，而网关那边一条请求都收不到。所以要用网关基址拼绝对地址：
 *   · web 模式（面板由网关托管）：`gatewayBase` 就是地址栏的 origin；
 *   · 桌面模式：`gatewayBase` 是 `http://127.0.0.1:<真实端口>`（见 port-panel）。
 * 基址取不到时退回相对路径 —— 那在 web 模式下仍然正确。
 */
function pluginPageUrl(page) {
  const base = window.wbPortPanel?.gatewayBase?.() || '';
  if (!base || !page.url.startsWith('/')) return page.url;
  return base.replace(/\/+$/, '') + page.url;
}

/**
 * 切到某个插件页：在宿主 section 里建（或复用）一个 iframe。
 *
 * ── 为什么每次都重建而不是复用同一个 iframe ─────────────────
 * 两个插件页是两个独立的 SPA，共用一个 iframe 就得让它们互相清理状态。
 * 重建的成本是一次静态资源加载（`no-store`，本来也不缓存），换来的是
 * 「切页即干净」—— 与「切走时清空」是同一条纪律的两面。
 *
 * ── 令牌怎么给 ──────────────────────────────────────────────
 * iframe 加载的是网关地址，跨源、带不上 cookie。所以 `load` 之后用
 * `postMessage` 把面板令牌递进去（不用 URL 参数：那会进 referrer 与历史）。
 * 插件页面按 `agent2api:panel-credentials` 这条消息取令牌。
 */
function showPluginPage(key, { persist = true } = {}) {
  const page = pluginPages.get(key);
  const host = pluginPageHost();
  if (!page || !host) return;
  currentPage = key;
  document.querySelectorAll('.page').forEach(section => {
    section.classList.toggle('active', section.dataset.page === '__plugin__');
  });
  markActiveNav();
  const crumb = $('crumb-page');
  if (crumb) crumb.textContent = page.label;
  window.wbShell?.closeNavigation();
  if (persist) localStorage.setItem(PAGE_KEY, key);
  window.wbColSettings?.close?.();
  // 整块换掉：上一页的 iframe 连同它的定时器一起消失
  host.innerHTML = '';
  const frame = document.createElement('iframe');
  frame.className = 'plugin-page-frame';
  frame.dataset.pluginPage = key;
  frame.setAttribute('title', page.label);
  frame.src = pluginPageUrl(page);
  frame.addEventListener('load', () => {
    // 令牌与主题在每次 load 后各递一次（页面重新加载 / 插件热更新后同样需要）。
    // 页面自己也会主动要一次（见 onPluginMessage）—— 两条路互为兜底。
    void pushPanelToken(frame.contentWindow);
    pushPanelTheme(frame.contentWindow);
  });
  host.appendChild(frame);
}

/** 离开插件页：清掉 iframe（它的定时器跟着停，不再持续打网关） */
function clearPluginPage() {
  const host = pluginPageHost();
  if (host) host.innerHTML = '';
}

/**
 * 把面板凭证 postMessage 给插件页面。
 *
 * `target` 是 iframe 的 `contentWindow`（不是 iframe 元素本身）：页面主动
 * 索取时手上只有 `event.source`，那正是它的 contentWindow。
 *
 * ── 为什么复用 `directCredentials` ─────────────────────────
 * 它已经处理了两端的差别：桌面壳给本机短效会话，网页面板给自己的登录凭证。
 * 插件页面要的是管理 API 的凭证，不是模型调用的 Key。
 *
 * 读不到时什么都不做 —— 插件页面自己会退化成「只用同源 cookie」。
 */
async function pushPanelToken(target) {
  if (!target) return;
  try {
    const headers = await api.directCredentials?.();
    if (!headers) return;
    // `*` 作为 targetOrigin 是刻意的：iframe 的源是网关地址，而网关地址可能是
    // 局域网 IP 的任意端口（用户自己配的），壳侧算不出它。消息体里只有凭证，
    // 接收方是我们自己托管的插件页面 —— 与「把凭证给 iframe」这个前提同级。
    target.postMessage({ type: 'agent2api:panel-credentials', headers }, '*');
  } catch {
    /* 没有凭证就不传 */
  }
}

/**
 * 把当前主题模式 postMessage 给插件页面。
 *
 * ── 为什么要传 ──────────────────────────────────────────────
 * 主题选择存在宿主的 localStorage（`workbuddy-desktop-theme`），iframe 是
 * 独立文档，读不到它；而 `prefers-color-scheme` 只在宿主把窗口主题交回系统
 * 时才跟着变 —— 用户显式选了深色、系统却是浅色时，iframe 里读到的仍是浅色。
 * 所以模式必须由宿主递进去，页面写到自己的 `<html data-theme>` 上，交给
 * `/plugins/theme.css`（网关出的宿主令牌原文）里那套选择器生效。
 *
 * ── 为什么传的是三态而不是算好的 light/dark ─────────────────
 * `system` 必须原样传：只有让 iframe 自己走 `prefers-color-scheme`，它才能
 * 在「系统主题运行中变化」时跟着变（见 tokens.css 末尾那段 @media）。宿主
 * 这里若先算成 light/dark，用户切系统主题时插件页就停在上一次的结果上。
 */
function pushPanelTheme(target) {
  if (!target || typeof target.postMessage !== 'function') return;
  const mode = localStorage.getItem('workbuddy-desktop-theme') || 'system';
  try {
    target.postMessage({ type: 'agent2api:panel-theme', theme: mode }, '*');
  } catch {
    /* 目标已销毁：下次打开会再推一次 */
  }
}

/**
 * 监听插件页发回来的请求（`window.addEventListener('message', …)` 注册在下面）。
 *
 * 三条消息：
 *   · `agent2api:request-credentials` —— 插件页主动索取凭证。页面在脚本里就
 *     发（早于 iframe 的 load），因为宿主的 `load` 钩子可能比页面的首轮请求
 *     晚（桌面壳的 `directCredentials` 要过一次 IPC）。两条路都留着：主动要
 *     是主路径，`load` 那次是页面重新加载后的补发。
 *   · `agent2api:request-theme` —— 插件页主动索取主题模式。与凭证同一套时序
 *     理由：页面首帧就要正确的深浅，不能等宿主的 `load`。
 *   · `agent2api:open-zcode-plans` —— 插件页的「活动福利」行要打开宿主的套餐
 *     明细弹窗（`wbZcodePlans` 是 React 岛，iframe 里拿不到）。让插件页自己
 *     重画一份弹窗等于把那个界面复制一遍，两份会各自漂移。
 */
function onPluginMessage(event) {
  const data = event.data;
  if (!data || typeof data !== 'object') return;
  if (data.type === 'agent2api:request-credentials') {
    // 回给发消息的那个 frame（可能是插件页，也可能是将来的其它嵌入方）
    const source = event.source;
    if (source && typeof source.postMessage === 'function') {
      void pushPanelToken(source);
    }
    return;
  }
  if (data.type === 'agent2api:request-theme') {
    pushPanelTheme(event.source);
    return;
  }
  if (data.type !== 'agent2api:open-zcode-plans') return;
  const accountId = typeof data.accountId === 'string' ? data.accountId : '';
  if (!accountId) return;
  window.wbZcodePlans?.open?.({ id: accountId, name: typeof data.name === 'string' ? data.name : undefined });
}
window.addEventListener('message', onPluginMessage);

function showPage(name, { persist = true } = {}) {
  // 插件页走单独一条路：它不是 `<section class="page" data-page>` 里的内置页，
  // 而是一个 iframe（页面由插件包提供、网关托管，见 adoptPluginPages）。
  if (typeof name === 'string' && name.startsWith(PLUGIN_PAGE_PREFIX) && pluginPages.has(name)) {
    showPluginPage(name, { persist });
    return;
  }
  // 老版本的「签到中心」菜单键是 `plugin:<pack>:checkin`（那时页面在插件包里）。
  // 升级后那条申报被内置页接管、不再注册，于是 localStorage 里存的这个键会
  // 认不出来 —— 用户停在签到页升级完，重启会掉到「报表」。这里把它接上。
  let target = name;
  if (typeof target === 'string' && target.startsWith(PLUGIN_PAGE_PREFIX) && target.endsWith(':checkin')) {
    target = 'checkin';
  }
  const page = PAGES.includes(target) ? target : 'overview';
  currentPage = page;
  // 离开插件页时把宿主容器清空：iframe 里的页面会一直跑着定时器，
  // 留着等于在后台持续打网关
  clearPluginPage();
  document.querySelectorAll('.page').forEach(section => {
    section.classList.toggle('active', section.dataset.page === page);
  });
  markActiveNav();
  const crumb = $('crumb-page');
  if (crumb) crumb.textContent = PAGE_LABELS[page] || page;
  // 切页后由组件库外壳收起移动侧边栏。
  window.wbShell?.closeNavigation();
  if (persist) localStorage.setItem(PAGE_KEY, page);
  // 列的设置面板常驻 body（.panel 是 overflow:hidden，留在原处会被裁掉，
  // 见 table-col-settings.js），不随宿主页面的 display:none 一起消失 ——
  // 不收起的话，切页之后它会孤零零地浮在新页面上。它自己不认识「页面」，
  // 由切页这一处统一告诉它。
  window.wbColSettings?.close?.();
  // 切到 AI 助手页时唤醒会话（岛的初始化可能晚于首屏的 showPage —— 它要等
  // islands/ui.js 加载完；activate 自己判重，早调晚调都只初始化一次）
  if (page === 'assistant') {
    window.wbAssistant?.activate?.();
  }
  // 切到报表页时拉一次统计（面板内部自持时间范围与数据，这里只做转发）
  if (page === 'overview') {
    window.wbReport?.load?.();
  }
  // 切到日志页时清掉未读标记，并立即拉一次最新日志
  if (page === 'logs') {
    clearLogsBadge();
    window.wbLogsPanel?.load?.();
  }
  // 请求日志页自持数据与筛选，切进去时拉一次最新；顺带清掉未读失败角标
  if (page === 'requests') {
    clearRequestsBadge();
    window.wbRequestsPanel?.load?.();
  }
  // 定时任务页自持清单与编辑态，切进去时拉一次最新
  if (page === 'tasks') {
    window.wbTasksPanel?.load?.();
  }
  if (page === 'settings') {
    window.wbSettingsPanel?.load?.();
  }
  // 切到签到中心时重拉一次快照：用户可能刚在账号页加/删了账号，或者刚在
  // 「插件」页装完插件（签到能力正是从那儿来的）。页面岛自己也会在首次挂载时
  // 拉一次，但那是「进程内第一次」—— 之后每次切进来都要刷新。
  if (page === 'checkin') {
    window.wbCheckin?.load?.();
  }
  // 提供商插件页自持包列表与宿主状态，切进去时拉一次最新（用户可能刚在
  // 另一个窗口里手工换过包，或者宿主崩溃重启过）
  if (page === 'plugins') {
    void window.wbPluginsPage?.load?.();
  }
  // 切到模型管理页时整页刷新一次：内置清单可能被上游刷新过，自定义家可能刚在
  // 账号页被加过 / 改过 / 删过（提供商记录与它的模型清单），而这一页的数据是
  // 自持的、不随主状态轮询更新。两件事（自定义目录 + 内置清单）都在面板里，
  // 这里只转发 —— 它自己保证「切页这次刷新一定落地」。
  if (page === 'routing') {
    void window.wbRoutingPanel?.load?.();
  }
  if (page === 'gateway') {
    void window.wbModelsPanel?.refreshAll?.();
  }
  // Agent 页自持客户端清单与配置，切进去时拉一次最新（配置文件可能被
  // 客户端自己改过、也可能刚在别的窗口里被别的工具改过）
  if (page === 'agent') {
    void window.wbAgentPanel?.load?.();
  }
  // 网络代理页自持清单（代理池），切进去时拉一次最新：别的窗口 / 别的页面
  // （账号弹窗里改过引用）都可能让它变过。面板内部保证「这次刷新一定落地」
  if (page === 'proxies') {
    window.wbProxiesPanel?.activate?.();
    void window.wbProxiesPanel?.load?.();
  }
  // 切到账号页时立刻补一次连接数：那条 2 秒轮询只在「当时就在账号页」时才发请求，
  // 切走的这段时间里缓存已经过期，不补一下会先看到几秒前的旧数字
  if (page === 'accounts') {
    void window.wbAccountsView?.refreshList?.();
    void window.wbAccountsView?.syncConnections?.();
    // 代理列的下拉读的是与「网络代理」页共用的那份池缓存：那一页的写操作会就地
    // 更新它，但池也可能在应用外被改过（另一个窗口 / 手工改库），切回来时重读一次
    void window.wbAccountsView?.refreshProxyPool?.();
  }
  // 网关 Key 页持有数据，切进去时拉一次最新（模型管理页在上面已经拉过）
  if (page === 'keys') {
    window.wbKeysPanel?.load?.();
  }
  // 文档页显示的是网关地址：切进去时补一次端口同步 —— 端口只在
  // getBackendStatus 的返回里，render() 那一轮之外的变动（用户换了端口重启）
  // 到这页才被发现的话，页面上会先显示一段过时的地址，而它就等着被复制
  if (page === 'docs') {
    void window.wbPortPanel?.sync?.();
  }
  renderTopbarStatus();
  // 导航项上的内容随页面变：日志未读徽标交给日志面板，更新提示在这里重画
  // （校验更新提示的可见性与所在页面有关，见 syncUpdateBadge）
  syncUpdateBadge();
  // 请求日志徽标跟着换页立即重画：离开该页时进行中的请求要在 5 秒轮询之前
  // 就位（进页时则由 paintRequestsBadge 收起 —— 页内有自己的读数）
  paintRequestsBadge();
}

// ─── 顶栏状态 ─────────────────────────────────

/** 顶栏右侧状态区：随当前页给出该页最关心的两三个状态 */
function renderTopbarStatus() {
  const session = state?.session || {};
  const accounts = state?.accounts?.accounts || [];
  const enabled = accounts.filter(account => account.enabled !== false).length;
  const available = accounts.filter(account => account.enabled !== false && account.available !== false).length;
  const modelApi = window.wbAccountsModel;
  const isLimited = modelApi?.isLimited || modelApi?.isRateLimited;
  const limited = isLimited ? accounts.filter(account => account.enabled !== false && isLimited(account)).length : 0;
  const ready = window.wbPortPanel?.isReady?.() === true;
  const port = window.wbPortPanel?.portLabel?.() || "";
  const chip = (text, kind = "", optional = false) => ({ text, kind, optional, dot: true });
  const mirror = id => {
    const badge = $(id);
    return badge?.textContent && badge.textContent !== "—"
      ? [{ text: badge.textContent, kind: badge.dataset.tone || "" }]
      : [];
  };
  const gateway = optional => chip(ready ? wbI18n.t("网关运行中") : wbI18n.t("未就绪"), ready ? "ok" : "bad", optional);
  const views = {
    accounts: () => [chip(wbI18n.t("{n} 个启用", { n: enabled }), enabled ? "ok" : ""), ...(limited ? [chip(wbI18n.t("{n} 个已限流", { n: limited }), "warn")] : [])],
    gateway: () => [chip(ready ? isWebPanel() ? wbI18n.t("网关运行中") : wbI18n.t("监听 127.0.0.1") : wbI18n.t("未就绪"), ready ? "ok" : "bad"), chip(port, "", true)],
    keys: () => mirror("keys-status"),
    docs: () => [gateway(), chip(port, "", true)],
    logs: () => mirror("logs-badge"),
    requests: () => mirror("req-badge"),
    tasks: () => mirror("tasks-badge"),
    settings: () => [gateway(true), ...(enabled ? [chip(wbI18n.t("{n} 个账号启用", { n: enabled }))] : [])],
    overview: () => [gateway(), available ? chip(wbI18n.t("{n} 个账号可用", { n: available }), "ok") : chip(session.loggedIn ? wbI18n.t("已登录") : wbI18n.t("未登录"), session.loggedIn ? "ok" : "warn")],
  };
  window.wbShell?.status(views[currentPage]?.() || views.overview(), stateError ? wbI18n.t("加载失败：{error}", { error: stateError }) : "");
}

// ─── 日志未读徽标（只提示 error） ─────────────

/**
 * 已读水位（日志 id）。null 表示「还没有水位」（首次运行），
 * 必须与 0 区分开：清空日志后水位会合法地落到 0（id 重新从 1 数起），
 * 那时若按首次运行处理，紧接着发生的错误会被当成已读吞掉。
 */
let lastSeenLogId = readSeenLogId();
/** 未读错误查询是否在飞：日志面板轮询与全局轮询都会触发它，同一时刻只该有一次 */
let unreadErrorsInFlight = false;
/**
 * 最近一次查到的未读错误数（0 = 没有）。
 *
 * 为什么要留在内存里：侧边栏重画（用户改了分组 / 顺序 / 显隐）会把角标节点整个
 * 换掉，新节点是「隐藏 + 0」。重画之后必须按这个值重新点一次，否则要等下一轮
 * 轮询（最长 20 秒）提示才回来。查询本身不发 —— 这里只用已经拿到的数。
 */
let unreadLogErrors = 0;

/** 读回持久化的水位；无值 / 值被改坏一律当作「还没有水位」 */
function readSeenLogId() {
  try {
    const raw = localStorage.getItem('workbuddy-desktop-log-seen');
    if (raw === null || raw.trim() === '') return null;
    const value = Number(raw);
    return Number.isInteger(value) && value >= 0 ? value : null;
  } catch {
    return null;
  }
}

/** 推进已读水位；localStorage 只负责跨次启动恢复，本会话的判断都看内存值 */
function markLogsSeen(id) {
  lastSeenLogId = id;
  try {
    localStorage.setItem('workbuddy-desktop-log-seen', String(id));
  } catch {
    // 存储不可用时只影响下次启动的起点，不影响本次会话
  }
}

function clearLogsBadge() {
  const badge = $('nav-count-logs');
  if (badge) badge.style.display = 'none';
  const stats = window.wbLogsPanel?.lastStats?.();
  if (stats?.lastId) markLogsSeen(stats.lastId);
}

/**
 * 导航徽标只提示 **error** 级别，数字含义是「已读水位之后新增的错误条数」。
 *
 * 为什么不再按日志总数统计：info/warn 是常态（签到、切号、刷新模型目录都写一条），
 * 按总数统计等于常挂一个红数字，亮久了就没人再看它；只有错误值得主动打断。
 * 数字口径也要跟着换 —— 显示的是错误条数，不再是「有新日志」的条数。
 *
 * 未读条数交给后端算：`GET /api/logs?level=error&sinceId=<水位>` 的 matched
 * 就是水位之后新增的 error 条数（level 过滤是「该级别及以上」，error 已是最高
 * 级别，等价于「仅 error」）。日志上限 500 条且全在内存里，这个查询代价可忽略。
 */
function updateLogsBadge(stats) {
  const badge = $('nav-count-logs');
  // stats 缺失（接口失败）时什么都不做：不能拿默认的 lastId=0 去判断水位，
  // 否则会被下面「日志被清空」那条分支误判成 id 归零，把水位一起抹掉
  if (!badge || !stats) return;
  const lastId = Number(stats.lastId) || 0;
  // 首次运行：把当前水位记为已读，否则一装上就挂着历史错误
  if (lastSeenLogId === null) {
    markLogsSeen(lastId);
    badge.style.display = 'none';
    return;
  }
  // 日志被清空（id 重新从 1 数起）：水位必须跟着回落，否则新日志的 id
  // 永远小于水位，徽标从此不再出现
  if (lastId < lastSeenLogId) {
    markLogsSeen(lastId);
    badge.style.display = 'none';
    return;
  }
  // 人就在日志页：等于已经看到，水位推进到最新
  // （顺带免掉每 10 秒轮询的这一次查询）
  if (currentPage === 'logs') {
    markLogsSeen(lastId);
    badge.style.display = 'none';
    return;
  }
  void refreshUnreadErrors(badge);
}

/**
 * 按内存里已有的未读数重画日志角标（不发查询）。
 * 侧边栏重画之后由 syncNavBadges 调 —— 新节点是「隐藏 + 0」，
 * 不补这一下，用户改完布局要等下一轮轮询才重新看到提示。
 */
function paintLogsBadgeFromMemory() {
  const badge = $('nav-count-logs');
  if (!badge) return;
  if (currentPage === 'logs' || !unreadLogErrors) {
    badge.style.display = 'none';
    return;
  }
  badge.textContent = unreadLogErrors > 99 ? '99+' : String(unreadLogErrors);
  badge.title = wbI18n.t('{n} 条错误日志未读，点开「日志」查看', { n: unreadLogErrors });
  badge.style.display = '';
}

/** 查未读错误数并重画徽标。失败静默：查询偶尔失败不该反过来抹掉已有提示 */
async function refreshUnreadErrors(badge) {
  if (unreadErrorsInFlight || lastSeenLogId === null) return;
  unreadErrorsInFlight = true;
  const from = lastSeenLogId;
  try {
    const result = await api.getLogs({ limit: 1, level: 'error', sinceId: from });
    // 等待期间水位被推进（用户进了日志页），这次结果已过期，别拿旧数覆盖新状态
    if (from !== lastSeenLogId || currentPage === 'logs') return;
    const unread = Number(result?.matched) || 0;
    unreadLogErrors = unread;
    // 重画过的侧边栏可能已经换掉了这个 badge 节点：按 id 重新取一次，
    // 拿旧引用去改只会改到一个已经不在文档里的元素
    const live = $('nav-count-logs') || badge;
    if (!unread) {
      live.style.display = 'none';
      return;
    }
    live.textContent = unread > 99 ? '99+' : String(unread);
    live.title = wbI18n.t('{n} 条错误日志未读，点开「日志」查看', { n: unread });
    live.style.display = '';
  } catch {
    // 保持上一次的显示
  } finally {
    unreadErrorsInFlight = false;
  }
}

/** 拉一次日志统计再重画徽标：挂在 20 秒全局轮询上，非日志页也能及时看到新错误 */
async function syncLogsBadge() {
  try {
    updateLogsBadge(await api.getLogStats());
  } catch {
    // 静默：统计拿不到时保持现状
  }
}

// ─── 请求日志导航徽标（进行中优先，其次未读失败） ─────────

/**
 * 这颗徽标先后承载过两种数字，现在两种共存、按优先级画：
 *   1. **进行中的请求数**（5 秒轻量轮询）：转发是否卡住在哪个页面都该第一眼看到，
 *      不必等用户进请求日志页 —— 进行中的事是「现在」的，优先于历史的失败；
 *   2. **未读失败数**（20 秒全局轮询 + 水位）：running 归零后，历史失败还有提示在。
 *
 * 未读失败沿用水位机制：请求日志没有日志那样的自增 id（按 ts 升序、用时间分页），
 * 水位只能取毫秒时间戳：进过一次请求日志页就把水位推到当下，之后的失败才算未读。
 * 「请求发起」与「设置水位」落在同一毫秒这种碰撞按未读算（start 是含边界）——
 * 宁可多提示一条，不冒漏提示的险；多提示的代价是进一次页面就清掉。
 * 未读条数复用请求日志接口：`status=error&start=<水位>` 的 matched 正是水位之后的
 * 失败数（与面板/报表同一个成功口径：非 2xx 或带错误摘要），limit 压到 1 只为拿计数。
 */
let lastSeenReqTs = readSeenReqTs();
/** 最近一次查到的进行中请求数（5 秒轻量轮询维护；0 = 没有） */
let runningRequests = 0;
/** 最近一次查到的未读失败数（20 秒全局轮询维护；0 = 没有） */
let unreadFailures = 0;

/** 读回持久化的水位；无值 / 值被改坏一律当作「还没有水位」（首次运行） */
function readSeenReqTs() {
  try {
    const raw = localStorage.getItem('workbuddy-desktop-req-seen');
    if (raw === null || raw.trim() === '') return null;
    const value = Number(raw);
    return Number.isFinite(value) && value >= 0 ? value : null;
  } catch {
    return null;
  }
}

/** 推进已读水位；localStorage 只负责跨次启动恢复，本会话的判断都看内存值 */
function markRequestsSeen(ts) {
  lastSeenReqTs = ts;
  try {
    localStorage.setItem('workbuddy-desktop-req-seen', String(ts));
  } catch {
    // 存储不可用时只影响下次启动的起点，不影响本次会话
  }
}

function clearRequestsBadge() {
  const badge = $('nav-count-requests');
  if (badge) badge.style.display = 'none';
  markRequestsSeen(Date.now());
  // 水位推到当下 = 在此之前的失败都算已读：把内存里的计数一并归零，
  // 否则离开页面后 paintRequestsBadge 会拿旧数把角标重新点亮
  unreadFailures = 0;
}

/**
 * 重画请求日志导航徽标。两种数字共用一颗徽标（都不亮就收起），
 * 写法上收口到这一个函数：两条轮询各自只更新自己的计数，谁也不直接碰 DOM，
 * 避免「20 秒一拍的失败查询把刚亮出的进行中数字又改回去」这种互相覆盖。
 */
function paintRequestsBadge() {
  const badge = $('nav-count-requests');
  if (!badge) return;
  // 人就在请求日志页：页内已有「· M 进行中」读数，导航徽标不再重复
  if (currentPage === 'requests') {
    badge.style.display = 'none';
    return;
  }
  if (runningRequests > 0) {
    badge.textContent = runningRequests > 99 ? '99+' : String(runningRequests);
    badge.title = wbI18n.t('{n} 个请求正在转发中，点开「请求日志」查看', { n: runningRequests });
    badge.style.display = '';
    // 类名用 is-running 而不是 live：layout.css 里 .live 是**侧栏状态灯**
    // （7px 圆点 + 绿底 + 光晕），加上它会把这颗数字角标压成小圆点 ——
    // 两个组件恰好都叫「live 状态」，但一个是灯、一个是数字，样式不可共用。
    badge.classList.add('is-running');
    return;
  }
  badge.classList.remove('is-running');
  if (unreadFailures > 0) {
    badge.textContent = unreadFailures > 99 ? '99+' : String(unreadFailures);
    badge.title = wbI18n.t('{n} 条失败请求未读，点开「请求日志」查看', { n: unreadFailures });
    badge.style.display = '';
    return;
  }
  badge.style.display = 'none';
}

/** 查水位之后的失败请求数并重画徽标：挂在 20 秒全局轮询上 */
async function syncRequestsBadge() {
  // 首次运行：把当下记为已读，否则一装上就挂着历史失败
  if (lastSeenReqTs === null) {
    markRequestsSeen(Date.now());
    paintRequestsBadge();
    return;
  }
  // 人就在请求日志页：等于已经看到，水位推进到当下
  if (currentPage === 'requests') {
    markRequestsSeen(Date.now());
    paintRequestsBadge();
    return;
  }
  try {
    const result = await api.getStatsRequests({
      status: 'error',
      start: lastSeenReqTs,
      limit: 1,
    });
    // 等待期间进了请求日志页：这次结果已过期，别拿旧数把刚清掉的角标又点亮
    if (currentPage === 'requests') return;
    unreadFailures = Number(result?.matched) || 0;
  } catch {
    return; // 保持上一次的显示：查询偶尔失败不该反过来抹掉已有提示
  }
  paintRequestsBadge();
}

// ── 进行中请求的轻量轮询（5 秒）────────────────
//
// 单独起一条 5 秒定时器而不是搭 20 秒全局轮询：进行中的请求通常几秒就收尾，
// 20 秒一拍会整段错过，徽标就永远等不到亮出的机会。查询压到 limit=1，
// 只要 matched（计数），不拉明细。列表自身的 1 秒轮询只在请求日志页跑
// （见 requests-panel.js 的 startAuto），人不在那页时，推进徽标的只有这里。
let runningQueryBusy = false;

async function syncRunningRequests() {
  if (runningQueryBusy) return;
  // 人已在请求日志页：不发查询（页内读数更准），徽标也由页面接管而不亮
  if (currentPage === 'requests') return;
  runningQueryBusy = true;
  try {
    const result = await api.getStatsRequests({ status: 'running', limit: 1 });
    runningRequests = Number(result?.matched) || 0;
  } catch {
    return; // 保持上一次的显示：查询偶尔失败不该反过来抹掉已有提示
  } finally {
    runningQueryBusy = false;
  }
  paintRequestsBadge();
}

// 窗口隐藏时暂停（与 20 秒全局轮询同一取向）：后台页没有「第一眼」可言
setInterval(() => {
  if (document.hidden) return;
  void syncRunningRequests();
}, 5_000);

// ─── 新版本可用提示 ───────────────────────────

/** 最近一次检查结果。存下来是为了换页时能重画提示（检查本身只在启动与手动触发时跑） */
let lastUpdateInfo = null;
/**
 * 已经看过提示的版本号。看过一次就没必要每次换页再闪一遍 ——
 * 与日志未读徽标的处理同一个取向（不打扰）。存版本号而不是布尔值：
 * 之后又发了更新的版本，提示该重新出现。
 */
let seenUpdateVersion = '';

/**
 * 三颗角标的内存值都初始化完了 —— 从这里开始 syncNavBadges 才有意义。
 *
 * 首次 renderSidebar() 发生在模块顶部（见那里的说明：它不能被桥调用中断），
 * 那时上面这些 `let` 还在 TDZ 里，读一下就抛 ReferenceError。置位放在
 * 「所有角标状态声明之后」是刻意的：早一行都可能踩到 TDZ。
 */
navBadgesReady = true;

/**
 * 按当前状态重画「设置」导航项上的更新提示。
 * 更新是设置页里的功能，所以提示挂在设置项上，不新增顶级页面。
 * 无更新、检查失败、版本号无法比较一律不显示（启动阶段的网络失败不该打扰用户）；
 * 人已经在设置页时也不显示 —— 面板里的「有新版本」徽标已经把话说完了。
 */
function syncUpdateBadge() {
  const badge = $('nav-count-update');
  if (!badge) return;
  const info = lastUpdateInfo;
  const latest = String(info?.latestVersion || '').trim();
  // 进设置页即视为「已看到」：结果与更新日志就在那个面板里
  if (currentPage === 'settings') seenUpdateVersion = latest;
  if (info?.hasUpdate !== true || !latest || latest === seenUpdateVersion) {
    badge.style.display = 'none';
    return;
  }
  // 只放一个「新」字而不是数字：这里没有「未读条数」的含义，
  // 写成数字容易被误会成还有多少个版本可以更新
  badge.textContent = wbI18n.t('新');
  badge.title = wbI18n.t('发现新版本 {latest}（当前 {current}），点开「设置 - 软件更新」可查看更新日志并下载', {
    latest,
    current: info.currentVersion || wbI18n.t('未知'),
  });
  badge.style.display = '';
}

/** 检查更新结束后由软件更新面板调用（result 为 null 表示检查失败） */
function updateUpdateBadge(info) {
  lastUpdateInfo = info || null;
  syncUpdateBadge();
  // 结果递给软件更新面板（另一个岛，见 ui-islands/src/islands/update-panel.tsx）：
  //   · applyStatus：把这份读数铺进面板。后端定时任务查到的新版本、新的检查时刻，
  //     不该等到用户手点一次「检查更新」才出现；
  //   · showUpdateModal：「检测到更新」弹窗。弹与不弹的判定（跳过此次更新的版本号 /
  //     本会话已弹过 / 人已在设置页）与弹窗本体都在那边 —— app.js 不再持有弹窗 DOM、
  //     跳过键与会话守卫，那套状态若两边各留一份，判定必然漂移。
  window.wbUpdatePanel?.applyStatus?.(info);
  window.wbUpdatePanel?.showUpdateModal?.(info);
}

// ─── 渲染：网关 / 模型 / 配置 ──────────────────

/**
 * 网关页的地址展示与侧栏那两条状态都由 port-panel.js 负责
 * （它自持后端状态与端口）。这里只做转发。
 */
function renderGateway() {
  window.wbPortPanel?.render?.();
}

/**
 * 「本地代理状态」卡片已随报表页改造删除（原有信息与侧栏底部的网关状态、
 * 顶栏徽标重复）。这里保留判空只是为了让历史书签 / 旧 DOM 不报错：
 * 元素在就写，不在就静默跳过 —— 不该为了一个已删除的展示位把 render() 拖崩。
 */
function renderProxyStatus() {
  const box = $('proxy-status');
  if (!box) return;
  const health = state?.health;
  // 这里说的是「上游凭证」（有没有可用账号），与网关进程是否在监听是两件事，
  // 所以措辞明确指向账号 —— 别再说成「代理不可用」（那会让人去查端口）
  if (!health?.upstreamConfigured) {
    box.innerHTML = wbI18n.t('<span style="color:var(--danger)">无可用账号：{reason}</span>', {
      reason: esc(health?.unavailableReason || wbI18n.t('尚未登录')),
    });
    return;
  }
  box.textContent = wbI18n.t('运行正常 · {url}', { url: health.upstreamBaseUrl || '' });
}

function render() {
  renderAccountsView();
  renderGateway();
  renderModels();
  renderProxyStatus();
  renderTopbarStatus();
}

/**
 * 模型管理区块由 models-panel.js 负责（表格渲染 / 筛选 / 启停 / 映射 / 刷新清单）。
 * 它自持从 /api/models/manage 拉来的数据；这里的调用只在轮询到新状态时提醒它
 * 「清单可能变了」，由它决定是否重拉。用可选链委托 —— 万一它还没执行完也只是
 * 本轮不画，不抛错把 render() 拖崩。
 */
function renderModels() {
  window.wbModelsPanel?.render();
  if (currentPage === 'gateway') void window.wbModelsPanel?.load();
}

/** 账号列表由 accounts-view 模块负责（含优先级/禁用/代理徽章与行内面板） */
function renderAccountsView() {
  window.wbAccountsView?.render();
}

// ─── 加载 ─────────────────────────────────────

let refreshRequested = 0;
let refreshTask = null;
let refreshStatus = { pending: false, error: '' };
const refreshListeners = new Set();

function publishRefreshStatus(patch) {
  refreshStatus = { ...refreshStatus, ...patch };
  refreshListeners.forEach(listener => listener());
}

function subscribeRefresh(listener) {
  refreshListeners.add(listener);
  return () => refreshListeners.delete(listener);
}

function releaseBusy() {
  busy = false;
}

// 在途请求可能早于插件操作；后来的刷新必须补读，不能复用那份旧响应。
function refresh({ force = true } = {}) {
  if (refreshTask && !force) return refreshTask;
  refreshRequested += 1;
  if (refreshTask) return refreshTask;
  publishRefreshStatus({ pending: true });
  refreshTask = Promise.resolve().then(async () => {
    let applied = false;
    let completed;
    do {
      completed = refreshRequested;
      try {
        const next = await api.getState();
        if (!next || typeof next !== 'object' || !Array.isArray(next.accounts?.accounts)) {
          throw new Error(wbI18n.t('账号列表响应异常'));
        }
        if (completed !== refreshRequested) continue;
        state = next;
        stateError = '';
        const validIds = new Set(state.accounts.accounts.map(account => account.id));
        window.wbAccountsView?.refreshCaches(validIds);
        window.wbAccountPanel?.invalidate();
        render();
        publishRefreshStatus({ error: '' });
        applied = true;
      } catch (error) {
        if (completed !== refreshRequested) continue;
        applied = false;
        stateError = error instanceof Error ? error.message : String(error);
        publishRefreshStatus({ error: stateError });
        renderTopbarStatus();
        console.warn('加载状态失败:', stateError);
      }
    } while (completed !== refreshRequested);
    return applied;
  }).finally(() => {
    refreshTask = null;
    publishRefreshStatus({ pending: false });
  });
  return refreshTask;
}

// ─── 账号操作（列表按钮统一入口；余额查询由账号视图自行消化） ───

async function runAccountAction(action, id) {
  if (busy) return;
  // 设置是弹窗内的编辑流程，不占用列表的 busy 锁（弹窗自己管自己的保存态）
  if (action === 'settings') {
    window.wbAccountPanel?.open(id);
    return;
  }
  busy = true;
  try {
    if (action === 'switch') {
      const result = await api.switchAccount(id);
      await refresh();
      toast(result?.changed === false
        ? wbI18n.t('账号已在全局队列第一位')
        : wbI18n.t('✅ 已将账号优先级调整到全局第一位'));
    } else if (action === 'refresh') {
      await api.refreshAccountToken(id);
      await refresh();
      toast(wbI18n.t('✅ Token 已刷新'));
    } else if (action === 'remove') {
      const account = state?.accounts?.accounts?.find(a => a.id === id);
      // 与岛内 displayNameOf 同一口径：nameCustom 恒赢，未打标维持旧口径（昵称优先）
      const name = esc(account?.nameCustom && account?.name ? account.name : (account?.nickname || account?.name || id));
      // 原生 confirm 在 Tauri 的 WebView 里不弹窗、直接放行（等于没有确认），
      // 危险确认一律走自绘弹窗（wbConfirm，见 confirm-dialog.js）—— 下同
      const note = window.wbAccountsModel?.isDesktopAccount?.(account) ? wbI18n.t('（不会影响客户端登录态）') : '';
      if (!(await window.wbConfirm?.ask?.({
        title: wbI18n.t('删除账号'),
        html: wbI18n.t('确定删除账号「<strong>{name}</strong>」？{note}', { name, note: esc(note) }),
        okText: wbI18n.t('删除'),
        okClass: 'danger',
      }))) return;
      await api.removeAccount(id);
      await refresh();
      toast(wbI18n.t('账号已删除'));
    }
  } catch (error) {
    toast(wbI18n.t('操作失败：{message}', { message: error.message }), 'err');
  } finally {
    releaseBusy();
  }
}

// 模型搜索框的 `input` 监听不在这里：过滤逻辑（applyModelFilter）已随模型区块
// 一起搬进 models-panel.js，监听留在原地会让两边各持一半 —— 见该文件的说明。

// ─── 事件绑定 ─────────────────────────────────

window.matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => {
  if ((localStorage.getItem('workbuddy-desktop-theme') || 'system') === 'system') applyTheme('system');
});
applyTheme(localStorage.getItem('workbuddy-desktop-theme') || 'system');
// 界面缩放同样在这里落一次（index.html 的头部内联脚本已按同一份 localStorage
// 抢先交过一次，见那边的说明）：这里这行是**权威**的一次 —— 头部那次只是
// 尽力而为的反闪烁，若桥当时还没就绪就会静默失败，由这行补齐。
applyZoom(storedZoom());

/**
 * 侧边栏首帧：按本机布局画出 #nav。
 *
 * ⚠ 位置是**契约**：必须在第一个桥调用（下面这行 `api.onStateChanged`）之前。
 *
 * 为什么不能再往后放：以前侧边栏是 index.html 里的静态 DOM，桥没就绪时页面
 * 照样有导航；改成数据渲染之后，这一句要是排在桥调用后面，只要桥缺失
 * （浏览器直开 / 注入失败）在那一行抛错，整段脚本中断，#nav 就永远是空的 ——
 * 用户看到的是一个没有导航的窗口，比任何功能失效都糟。
 * 它只读 localStorage、不依赖任何岛与桥，没有任何理由排在后面。
 *
 * 也不能挪进 DOMContentLoaded：那会让首帧闪一下空侧边栏。
 */
renderSidebar();

/**
 * 设置页改完布局后的重画入口。
 *
 * 为什么走事件而不是直接调用：设置页的编辑面板是 React 岛，它改完只知道自己
 * 手里那份数据变了；重画侧边栏是 app.js 的职责。事件里不带数据 —— 布局的唯一
 * 真相在 localStorage，重画时现读，避免「事件传的值」与「存盘的值」在某个
 * 中间态对不上（比如用户连点两下）。
 *
 * ⚠ 与 renderSidebar 同一个理由排在桥调用之前：它只读 localStorage、只改 DOM，
 * 不依赖桥。排到 `api.onStateChanged` 后面的话，桥缺失时这条监听根本不会注册，
 * 用户在设置页改完布局、左侧侧边栏毫无反应 —— 而且没有任何报错。
 *
 * 顺带处理「当前页被隐藏了」：用户把自己正看着的那一页藏起来之后，侧边栏里
 * 没有它的入口了，但页面还开着 —— 看起来像卡住。这时落到第一个可见页。
 */
window.addEventListener('wb:sidebar-layout', () => {
  renderSidebar();
  const api = sidebarApi();
  const layout = api?.readLayout?.();
  if (!layout || !api) return;
  if (api.isVisible(layout, currentPage)) return;
  const fallback = api.firstVisibleKey(layout);
  if (fallback) showPage(fallback);
});

api.onStateChanged(next => {
  if (!next?.accounts && !next?.session && !next?.health) return;
  // 事件快照可能早于插件操作，只作失效通知；最新状态仍经统一刷新读取。
  void refresh();
});

// ─── 共享给子模块（账号视图 / 词表面板 / 日志面板 / 账号设置面板）───
window.wbApp = {
  esc,
  toast,
  formatTime,
  showPage,
  get currentPage() { return currentPage; },
  getState: () => state,
  runAccountAction,
  refresh,
  getRefreshStatus: () => refreshStatus,
  subscribeRefresh,
  // 插件页清单重拉（插件页装 / 卸 / 启停之后由插件页的岛调它）：
  // 侧边栏的插件页菜单按这份清单生成，不重拉就会「装完了菜单里还是没有」
  refreshPluginPages,
  renderTopbarStatus,
  syncNavBadges,
  updateLogsBadge,
  syncLogsBadge,
  // 软件更新面板在每次检查结束后回调它，把「有新版本」翻译成导航上的提示
  updateUpdateBadge,
  // 显示偏好（设置页「显示」分类的两个入口）：主题与缩放都只有这一处实现，
  // 设置页改完调这里，再靠 'wb:theme' / 'wb:zoom' 事件跟随变化
  applyTheme,
  applyZoom,
};

// ─── 账号的自动维护结果 ───────────────────────
// 启动时不再由主进程额外拉一次临期凭证：凭证维护是「定时任务」页里的一条后端
// 任务，排期（含失败冷却）持久化，重启只补跑已经到期的那一轮 —— 见
// `core::scheduled_tasks`。维护完成后账号页按 20 秒主状态轮询自然跟上，
// 因此这里不再订阅 `accounts:auto-maintained`（那条事件随启动维护一并删除）。

// ─── 初始化 ───────────────────────────────────

// 导航点击由组件库外壳处理，切页与页面记忆仍通过 wbApp.showPage。


// ─── 端口状态与冲突处置 ───────────────────────
//
// 侧栏那两条状态（网关进程 / 可用账号）与端口冲突时的两个出口
// （结束占用进程 / 更换端口）都在 port-panel.js 里 —— 它自持后端状态与
// 一整套弹窗交互，留在本文件会让这里继续膨胀。本文件只负责在 render()
// 里委托它重画，并在首屏主动问一次后端状态。

// ─── 初始页恢复：等待后续业务引擎也就绪 ─────────────────────
// 外壳与页面岛已经同步注册；提供商、列设置及升级引擎仍在 app.js 之后加载。
// 初始切页放在 DOMContentLoaded，避免业务加载时缺少这些依赖。
//
// 恢复前先过一道「这页还看得见吗」：用户可能在上次退出前把它（或它所在的分组）
// 隐藏了。停在一个侧边栏里找不到入口的页面上，看起来就像页面丢了 —— 这时落到
// 第一个可见页。一个菜单都不剩时保持原值（那种情况下用户会看到侧边栏的提示语，
// 页面本身仍是可用的，没必要再猜一个）。
function restoredPage() {
  const saved = localStorage.getItem(PAGE_KEY) || 'overview';
  const api = sidebarApi();
  const layout = api?.readLayout?.();
  if (!layout || !api) return saved;
  if (api.isVisible(layout, saved)) return saved;
  return api.firstVisibleKey(layout) || saved;
}
const initialPage = restoredPage();
if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', () => {
    showPage(initialPage, { persist: false });
    restorePortPanel();
  }, { once: true });
} else {
  showPage(initialPage, { persist: false });
  restorePortPanel();
}

refresh();

/**
 * 启动即与库对齐一次侧边栏布局。
 *
 * 首帧（上面模块顶部的 `renderSidebar()`）用的是 localStorage 缓存 —— 那一步
 * 必须同步，不能等网络。而布局的权威副本在库里：桌面壳与网页端是两个 origin、
 * localStorage 互不相通，所以「在 WebUI 里隐藏的菜单客户端也跟着隐藏」只能
 * 靠这一下把库里的那份拉回来（拉回后 `syncFromServer` 自己派
 * `wb:sidebar-layout` 重画，不需要在这里再画一次）。
 *
 * 失败静默：桥不可用（浏览器直开）或离线时继续用缓存，与改造前一致。
 * 首次落库（库里还是空的）也在这条路径里完成 —— `syncFromServer` 会把
 * 本机缓存上行，老用户调好的布局无感迁移进库。
 */
void window.wbSidebarLayout?.syncFromServer?.();

/** 首屏问一次后端状态：此时 state 还没回来，侧栏两条状态各自显示
 * 「正在检查…」，拿到结果后立刻变成真值（端口冲突会直接给出失败原因）。
 * port-panel 也是岛，同样要等注册完，所以与初始页恢复放在一起。 */
function restorePortPanel() {
  void window.wbPortPanel?.sync?.();
}

/**
 * 启动即把更新状态铺一次：读**后端缓存**里的最近一次检查结果（定时任务按间隔
 * 查一次并落库，默认 20 分钟），有新版本时在「设置」导航项上给提示。
 *
 * 这里**不自己打 GitHub**：dev 模式热重载一天要重载几十次，每次加载都
 * 查一遍会把匿名限额（60 次/小时，按出口 IP 计）耗光 —— 见底之后连定时任务
 * 也跟着失败，而本该做的只是等下一个检查窗口。要立刻查有设置页的「检查更新」
 * 按钮。
 *
 * 放在 DOMContentLoaded 里而不是直接调用：app.js 在 index.html 里排得比
 * update-panel.js 靠前，脚本执行到这里时 window.wbUpdatePanel 还没挂上，
 * 直接调会静默什么都不做；DOMContentLoaded 在所有同步脚本执行完之后触发，
 * 那时面板已经就位。不 await（void 触发）—— 首屏不该等它；失败静默
 * （面板按「未检查」显示，导航提示不亮）。
 *
 * 面板的 load() 同样只读缓存，所以这里这一下不会和它重复请求。
 */
document.addEventListener('DOMContentLoaded', () => {
  void window.wbUpdatePanel?.syncFromCache?.();
  // 数据结构升级：这次更新把数据存储换成了单个 SQLite 库，启动时后端只探测
  // 「还有没有旧文件没搬进库」，有待迁移就直接导入（**不弹窗** —— 升级没有
  // 选项也不能取消，弹窗只是多余的一道坎）。同样放 DOMContentLoaded：
  // 面板脚本排在 app.js 之后。
  // 它**不是**更新检查那种「有新版本就提示」的可选动作 —— 没升级时账号是空的，
  // 所以过程与结果都要 toast 报出来，不能让用户面对一个「账号怎么空了」的疑问。
  void window.wbUpgradePanel?.check?.();
  // 启动即读一次后端的余额结果快照（每账号自动查询的最新结论，重启也在），
  // 只靠下面那条 20 秒轮询的话，用户启动后第一眼看到的是账号页一片「未查询」，
  // 要等最久一整轮才变出余额 —— 那正是「定时查询好像没生效、必须手动点」的来源。
  // 放在 DOMContentLoaded：islands（wbAccountsView 的注册处，见 index.html 的
  // 加载顺序）是同步脚本，此刻已经就位。
  void window.wbAccountsView?.syncBalancesSnapshot?.();
  // 插件申报的页面 → 侧边栏动态菜单。放这里而不是模块顶部：首帧侧边栏先按
  // 内置页画出来（不闪空白），拿到清单后再补上插件页（多一次重画）。
  void refreshPluginPages();
}, { once: true });

// 窗口重新可见时立刻补读一次余额快照：20 秒轮询在页面隐藏期间整体跳过
// （见下面 setInterval 的 document.hidden 判定），恢复可见的瞬间界面上还是
// 隐藏前的旧读数 —— 这一下让用户切回来就看到定时任务的最近一轮结果。
// 快照时间戳没变时 syncBalancesSnapshot 自己会早退，重复触发没有代价。
document.addEventListener('visibilitychange', () => {
  if (document.hidden) return;
  if (currentPage === 'accounts') void window.wbAccountsView?.refreshList?.();
  void window.wbAccountsView?.syncBalancesSnapshot?.();
  if (api.platform !== 'web') {
    const frame = pluginPageHost()?.querySelector('iframe');
    if (frame) void pushPanelToken(frame.contentWindow);
  }
});

// 定时轮询：限额标记（429 + 恢复时间）与账号状态变化自动刷新；窗口隐藏时暂停
setInterval(() => {
  if (document.hidden) return;
  refresh({ force: false });
  // 后端状态一起轮询：网关可能在这期间起停（端口冲突、用户结束占用进程后重启），
  // 而管理 API 打不通时 refresh() 只会静默失败，看不出发生了什么
  void window.wbPortPanel?.sync?.();
  // 日志未读错误也一起轮询：否则人不在日志页时，只有日志面板那次 10 秒轮询
  // 才会更新徽标 —— 而那个轮询恰恰只在日志页可见时才发请求（见 logs-panel.js）
  void syncLogsBadge();
  // 请求日志的未读失败同理：人不在请求日志页时也要有人推进角标
  void syncRequestsBadge();
  // 余额快照也跟着这一轮读一次：后端的每账号自动查询在跑，
  // 界面得跟上它（否则用户不点按钮就永远停在启动那次的旧余额上）。
  // 快照时间戳没变时它自己会早退，不会造成无谓的重绘。
  void window.wbAccountsView?.syncBalancesSnapshot?.();
  // 侧边栏布局也跟着对一次库：另一端（WebUI 与客户端是两个 origin，
  // localStorage 不通）刚改过布局时，这一端最迟在这一轮跟上。
  // 内容没变时 `syncFromServer` 自己早退，不会重画（重画会打断正在拖动的项）。
  void window.wbSidebarLayout?.syncFromServer?.();
  // 插件页缓存着管理凭证，桌面会话提前续期后要把新值递进去。
  if (api.platform !== 'web') {
    const frame = pluginPageHost()?.querySelector('iframe');
    if (frame) void pushPanelToken(frame.contentWindow);
  }
}, 20_000);

// 「软件版本检查」结果的轮询：读**后端缓存**，不自己打 GitHub。
//
// 真正的检查在后端定时任务里跑（定时任务页的「软件版本检查」，默认每 20 分钟一次，
// 排期与结果都持久化在库里）；这里只是读一次缓存来亮/灭侧栏徽标与弹更新弹窗 ——
// 匿名限额 60 次/小时且**按出口 IP 计**，前端再自己查一遍就是白白多花一份额度。
//
// ── 为什么是自适应节奏而不是固定 1 分钟 ──────────────────────
// 首屏可能正好落在「后端刚起、定时任务那一轮还在跑」的窗口里：这时缓存还没有
// 结果（checked:false），固定 1 分钟会让用户盯着「未检查」等满一分钟 —— 而改造
// 前前端自己查，结果是立刻出来的。所以没有结果时加密到 5 秒一次，拿到结果
// （或等满 2 分钟仍没有，例如用户关掉了这条任务）就回到常态的 1 分钟。
// 用 setTimeout 自排期而不是 setInterval：节奏要变，固定间隔做不到。
let updatePollDelay = 5_000;
const updatePollStartedAt = Date.now();

function pollUpdateStatus() {
  const settle = () => { setTimeout(pollUpdateStatus, updatePollDelay); };
  if (document.hidden) { settle(); return; }
  void api.getUpdateStatus?.().then(info => {
    if (info?.channelChanged) {
      wbApp.updateUpdateBadge(info);
      updatePollDelay = 5_000;
      return;
    }
    if (info && info.checked !== false) {
      // 有结果了（含「已是最新」）：回到常态节奏，徽标/弹窗由同一出口处理
      updatePollDelay = 60_000;
      wbApp.updateUpdateBadge(info);
      return;
    }
    if (Date.now() - updatePollStartedAt > 120_000) updatePollDelay = 60_000;
  }).catch(() => {
    // 静默：下一次轮询自然重试
  }).finally(settle);
}

setTimeout(pollUpdateStatus, updatePollDelay);