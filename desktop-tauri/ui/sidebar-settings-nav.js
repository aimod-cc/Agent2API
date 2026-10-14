/* Agent2API · 主侧栏「设置」的可折叠子项（设置分类的快捷入口） */
/* global wbApp, wbI18n, wbIcons, wbSettingsPanel */

/**
 * 把设置页的十个分类接到主侧栏「设置」底下，做成可折叠的一组。
 *
 * ── 为什么是宿主侧脚本，不是 React 岛 ──────────────────────────
 * 分类表的权威来源是 ui-islands 的 CATEGORIES（islands/settings-model.ts），
 * 做成岛能直接 import 它、单一来源最干净。但那样要改 ui-islands/src（上游
 * 文件）并重新构建 ui/islands/ui.js（1.1MB 构建产物）—— 对下游 fork 来说，
 * 每次上游更新都得重建一次产物，冲突面大得多。这里改成宿主侧脚本，代价是
 * 分类表在本文件静态声明一份：上游增删分类时最坏只是侧栏这一份少一项，不会崩。
 *
 * ── 选中态怎么同步 ────────────────────────────────────────────
 * 设置页的分类状态（islands/settings-state.ts）只在模块内 publish 给 React
 * 订阅者，不派发 DOM 事件，宿主侧订阅不到。所以反向同步用 MutationObserver
 * 盯 `.settings-nav-item.active` 的 data-cat —— 那里是分类状态的唯一 DOM
 * 投影，无论从哪条路进来（页内点导航、深链）都会经过它。
 * 深链那条路值得点名：update-panel 的「去更新」直接调 showCategory('about')，
 * 侧栏要能跟上，靠的就是这个观察。
 *
 * ── 折叠偏好 ──────────────────────────────────────────────────
 * 键见 OPEN_KEY，与 app.js 的 workbuddy-desktop-page 同一前缀。
 * 开箱是折叠的；**首次进入设置页时自动展开一次并记下**，之后完全听用户的
 * （手动折叠过就不再自动弹开）。窄屏（<=900px）忽略偏好、强制展开 —— 那时
 * 箭头是隐藏的（见 sidebar-subnav.css 末尾），不强制展开用户就没有入口。
 */
(() => {
  /** 侧栏子项是否展开：'1' 展开 / '0' 折叠；无记录走 FIRST_OPEN 逻辑 */
  const OPEN_KEY = 'workbuddy-desktop-settings-nav-open'
  /**
   * 分类偏好。必须与 islands/settings-model.ts 的 SETTINGS_CAT_KEY 逐字一致：
   * 侧栏点击时先写它、再切页，设置页 load() 头一行的 restoreCategory() 就是
   * 读这个键把分类铺上去的（见 goCategory 里的顺序说明）。
   */
  const CAT_KEY = 'workbuddy-desktop-settings-cat'
  /** 窄屏断点：与 layout.css 里主侧栏收成纯图标的那个 900px 一致 */
  const NARROW = window.matchMedia('(max-width: 900px)')

  /**
   * 分类表。id / icon 必须与 islands/settings-model.ts 的 CATEGORIES 对齐
   * （id 是切分类的入参，icon 是 icons.js 里的键）；label 是中文即键，
   * 挂到 data-i18n 上，语言切换（整页重载）后由 i18n.js 的 sweep 重新替换。
   *
   * 「更新」的 id 保持 'about' —— 这是上游的内部契约（update-panel.tsx 按
   * `.settings-pane[data-cat="about"]` 找挂载点，用户 localStorage 里存的也是
   * 这个值），标签显示「更新」但 id 不能改。
   */
  const CATEGORIES = [
    { id: 'general', label: '通用', icon: 'sliders' },
    { id: 'display', label: '显示', icon: 'display' },
    { id: 'gateway', label: '网关', icon: 'traffic' },
    { id: 'routing', label: '路由', icon: 'route' },
    { id: 'retry', label: '重试', icon: 'refresh' },
    { id: 'timeout', label: '超时', icon: 'timer' },
    { id: 'security', label: '安全', icon: 'shield' },
    { id: 'data', label: '数据', icon: 'database' },
    { id: 'feedback', label: '反馈与需求', icon: 'feedback' },
    { id: 'about', label: '更新', icon: 'download' },
  ]

  /** 子项按钮：cat id -> <button>，markActive 遍历它切选中态 */
  const items = new Map()
  let navItem = null
  let subWrap = null
  let caret = null

  /** 中文即键的查表（i18n.js 未就绪时回落键本身，也就是简体原文） */
  const t = key => window.wbI18n?.t?.(key) || key

  const readPref = () => {
    try { return localStorage.getItem(OPEN_KEY) } catch { return null }
  }

  const writePref = value => {
    try { localStorage.setItem(OPEN_KEY, value) } catch { /* 存储不可用只是不记忆 */ }
  }

  /**
   * 按「窄屏优先于用户偏好」把当前该有的显隐铺上去。
   *
   * 窄屏下箭头是隐藏的（CSS），这里必须强制展开，否则折叠状态下用户没有任何
   * 入口把子项放出来。**只读偏好、不写** —— 窄屏是被动场景，不该把用户宽屏
   * 下的折叠选择改成展开。
   */
  function applyOpen() {
    const shown = NARROW.matches || readPref() === '1'
    if (subWrap) subWrap.hidden = !shown
    if (caret) caret.setAttribute('aria-expanded', shown ? 'true' : 'false')
  }

  /** 用户主动展开/折叠（箭头）：这个才写偏好 */
  function setOpen(open) {
    writePref(open ? '1' : '0')
    applyOpen()
  }

  /** 选中态是唯一状态：按 cat 切，null 表示一个都不亮 */
  function markActive(cat) {
    for (const [id, btn] of items) btn.classList.toggle('active', id === cat)
  }

  /**
   * 点子项：切到设置页并落到该分类。
   *
   * 顺序不能改 —— 先写 CAT_KEY，再 showPage，最后 showCategory：
   *   showPage('settings') 会触发设置页的 load()，而 load() 的**头一行**就是
   *   同步的 restoreCategory()（读 CAT_KEY）。若先切页后写偏好，偏好会被
   *   上一次的值盖回去，用户点了「重试」却停在「通用」。
   *   末尾那次 showCategory 是兜底：存储不可用时 restoreCategory 拿不到值，
   *   这一句仍能把分类拨到位（showCategory 不写偏好，符合它的既有语义）。
   */
  function goCategory(id) {
    try { localStorage.setItem(CAT_KEY, id) } catch { /* 存储不可用：这次照样切，只是下次不记得 */ }
    window.wbApp?.showPage?.('settings')
    window.wbSettingsPanel?.showCategory?.(id)
    // 乐观高亮：showCategory 的 publish 要等 React 重渲染才反映到页内导航上，
    // MutationObserver 那时才跟上。先手标一次，点击当下就有反馈（随后 observer
    // 的同步是幂等的，不会打架）。
    markActive(id)
  }

  /** 建 DOM：子项容器插在「设置」项之后，折叠箭头挂在「设置」项右端 */
  function build() {
    // 幂等：重复调用（DOMContentLoaded 兜底或重试）不该插出第二份
    if (subWrap) return true
    const nav = document.getElementById('nav')
    if (!nav) return false
    navItem = nav.querySelector('.nav-item[data-page="settings"]')
    if (!navItem) return false

    subWrap = document.createElement('div')
    subWrap.className = 'nav-sub'
    subWrap.id = 'nav-sub-settings'
    subWrap.setAttribute('role', 'group')
    subWrap.setAttribute('aria-label', t('设置'))
    subWrap.hidden = true

    for (const cat of CATEGORIES) {
      const btn = document.createElement('button')
      btn.type = 'button'
      btn.className = 'nav-sub-item'
      btn.dataset.cat = cat.id

      const ico = document.createElement('span')
      ico.className = 'ico'
      // 图标集是 icons.js 那份（设置分类的十个键都在里面），取不到就留空格 ——
      // 与设置页 iconHtml 同一处理，按名字取不到是开发期错误，不该把导航弄崩
      ico.innerHTML = window.wbIcons?.icon?.(cat.icon, 15) || ''

      const label = document.createElement('span')
      label.className = 'label'
      // 「中文即键」：键挂在 data-i18n 上，语言切换后的 sweep() 会把这里一起换掉
      label.dataset.i18n = cat.label
      label.textContent = t(cat.label)

      btn.append(ico, label)
      btn.addEventListener('click', () => goCategory(cat.id))
      subWrap.appendChild(btn)
      items.set(cat.id, btn)
    }

    navItem.after(subWrap)

    caret = document.createElement('span')
    caret.className = 'nav-caret'
    // 它长得像按钮、行为也是按钮，但只能是 span：nav-item 自己就是 <button>，
    // button 里不能再嵌 button（HTML 解析器会把内层的甩出去）。角色与键盘
    // 处理因此都得手写。
    caret.setAttribute('role', 'button')
    caret.setAttribute('tabindex', '0')
    caret.setAttribute('aria-controls', subWrap.id)
    caret.setAttribute('aria-label', t('展开或收起设置分类'))
    caret.innerHTML = window.wbIcons?.icon?.('chevronDown', 14) || ''
    caret.addEventListener('click', event => {
      // app.js 在 #nav 上做了点击委托（closest('.nav-item[data-page]')），
      // 箭头在「设置」项内部 ⇒ 不挡住的话点箭头会顺手切页，而这里只想折叠。
      event.preventDefault()
      event.stopPropagation()
      setOpen(!(readPref() === '1'))
    })
    caret.addEventListener('keydown', event => {
      if (event.key !== 'Enter' && event.key !== ' ') return
      event.preventDefault()
      event.stopPropagation()
      setOpen(!(readPref() === '1'))
    })
    navItem.appendChild(caret)

    applyOpen()
    return true
  }

  /** 反向同步：盯页内导航的 active 项（分类状态的唯一 DOM 投影） */
  function watchSettingsNav() {
    const settingsNav = document.querySelector('.settings-nav')
    if (!settingsNav) return false
    const sync = () => {
      const active = settingsNav.querySelector('.settings-nav-item.active')
      markActive(active?.dataset.cat || null)
    }
    sync()
    new MutationObserver(sync).observe(settingsNav, {
      subtree: true,
      attributes: true,
      attributeFilter: ['class'],
    })
    return true
  }

  /**
   * 首次进入设置页时自动展开一次。
   *
   * 只看「设置页 section 拿到 active」这一个信号 —— 无论用户从哪进来（点侧栏、
   * 深链、恢复上次页面）都会经过它。按偏好是否**已存在**决定要不要动：
   * 有记录就完全听用户的，不再自动弹开。
   */
  function watchPageEntry() {
    const section = document.querySelector('.page[data-page="settings"]')
    if (!section) return false
    new MutationObserver(() => {
      if (!section.classList.contains('active')) return
      if (NARROW.matches) return
      if (readPref() === null) setOpen(true)
    }).observe(section, { attributes: true, attributeFilter: ['class'] })
    return true
  }

  let tries = 0

  /**
   * 等页内导航出现再挂观察者。`.settings-nav` 由 islands/ui.js 的岛在加载期
   * 同步挂出（mount 走 flushSync），本脚本静态排在它之后，正常一次就成；
   * 万一以后被挪到前面，这里按帧等一会儿再试，而不是把异常抛到控制台。
   *
   * 只重试观察者，不重试 build —— build 已经建好了 DOM，整段重来会插第二份。
   */
  function watchSettingsNavEventually() {
    if (watchSettingsNav()) return
    if (++tries > 40) return
    setTimeout(watchSettingsNavEventually, 150)
  }

  /** 初始化（幂等） */
  function init() {
    if (!build()) return false
    watchSettingsNavEventually()
    watchPageEntry()
    // 宽窄窗口来回切时要重算显隐（窄屏强制展开、宽屏回偏好）
    const onBreakpoint = () => applyOpen()
    if (NARROW.addEventListener) NARROW.addEventListener('change', onBreakpoint)
    else if (NARROW.addListener) NARROW.addListener(onBreakpoint)
    return true
  }

  if (!init() && document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init, { once: true })
  }
})()
