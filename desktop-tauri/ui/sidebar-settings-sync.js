/* Agent2API · 设置分类 ↔ 主侧栏一级项 / 面包屑 的选中态同步 */
/* global wbApp, wbI18n, wbSidebarLayout */

/**
 * 把「设置页当前停在哪个分类」这一个事实，投影到三个地方：
 *   ① 主侧栏那 13 个设置分类一级项的选中态（点「重试」只有「重试」亮）；
 *   ② 顶栏面包屑（「设置 / 重试」）；
 *   ③ <html data-settings-cat>，作为宿主侧读取该状态的**唯一出口**
 *      （app.js 的 markActiveNav / crumbText 都读它）。
 *
 * ── 为什么需要这个脚本（分类状态在 React 岛里）────────────────
 * 分类状态由 islands/settings-state.ts 持有，它只在模块内 publish 给 React
 * 订阅者，**不派发 DOM 事件**，宿主侧订阅不到。所以反向同步只能观察它的
 * **唯一 DOM 投影** —— 设置页左栏 `.settings-nav-item.active` 的 data-cat。
 * 无论从哪条路进来（侧栏点击、页内点分类、深链 showCategory('about')）都会
 * 经过它，因此一个 MutationObserver 就够。
 *
 * 深链那条路值得点名：update-panel 的「去更新」直接调 showCategory('about')
 * 并 showPage('settings')，侧栏要能跟上，靠的就是这个观察。
 *
 * ── 为什么脚本、而不是做成 React 岛 ───────────────────────────
 * 做进岛确实能直接读状态、单一来源最干净，但那样要改 ui-islands/src（上游
 * 文件）并重新构建 ui/islands/ui.js（1.4MB 产物）—— 对下游 fork 而言每次上游
 * 更新都得重建一次产物，冲突面大得多。宿主侧脚本的代价只是「分类表在
 * sidebar-layout.js 里另有一份」，而那一份本来就要有（侧栏要画它们）。
 *
 * ── 落点：<html data-settings-cat> ───────────────────────────
 * 写在 documentElement 而不是某个具体元素上，是因为读它的地方有两处
 * （app.js 的 markActiveNav 与 crumbText），而两侧**不该**互相直接调用：
 * 设属性是一行、读属性也是一行，中间多一层函数调用只会多一处可失效的耦合。
 */
(() => {
  /** 分类状态的唯一投影：设置页左栏导航 */
  const NAV_SELECTOR = '.settings-nav'
  const ITEM_SELECTOR = '.settings-nav-item'
  const ATTR = 'settingsCat'

  /** 中文即键的查表（i18n.js 未就绪时回落键本身，也就是简体原文） */
  const t = key => window.wbI18n?.t?.(key) || key

  /** 当前已知的分类（null = 还不知道 / 不在设置页） */
  let currentCat = null

  /**
   * 把分类同步到侧栏与面包屑。
   *
   * 侧栏那 13 项的高亮判断在 app.js 的 markActiveNav 里（它读 data-cat 与
   * 这里写下的属性比对），所以这里只做两件事：写属性、并把面包屑重刷一次。
   * 面包屑由 showPage 写文本，但**切分类不经过 showPage**（页内点导航只走
   * 岛的 showCategory），所以这里得主动补一次。
   */
  function sync(cat) {
    if (cat === currentCat) return
    currentCat = cat
    if (cat) document.documentElement.dataset[ATTR] = cat
    else delete document.documentElement.dataset[ATTR]

    // 侧栏高亮：委托给 app.js 那一份实现（它同时管非分类项，不该有两套）
    window.wbApp?.syncNavActive?.()

    // 更新角标：它的「看过了」判据从「在设置页」细化到「在更新那一栏」
    // （见 app.js 的 syncUpdateBadge）—— 切到 about 要立刻把它收起来，
    // 所以分类一变就补刷一次
    window.wbApp?.syncNavBadges?.()

    // 面包屑：只在设置页时写（在别的页时它由 showPage 管，不该被这里覆盖）
    if (window.wbApp?.currentPage !== 'settings') return
    const crumb = document.getElementById('crumb-page')
    if (!crumb) return
    const base = t('设置')
    const label = cat ? window.wbSidebarLayout?.menusByCat?.()[cat] : ''
    // 拼接而不是 t('{base} / {cat}')：含 {xxx} 的字面量会被 i18n 扫描器当成
    // 插值模板、提取不到键。分类名本身已是译文，这里只是加个分隔符。
    crumb.textContent = label ? `${base} / ${t(label)}` : base
  }

  /** 从页内导航读出当前分类（active 项不存在时返回 null，而不是猜一个） */
  function readCat(nav) {
    const active = nav.querySelector(`${ITEM_SELECTOR}.active`)
    return active?.dataset?.cat || null
  }

  let observed = null

  /**
   * 挂观察者。`.settings-nav` 由 islands/ui.js 的岛在加载期挂出（mount 走
   * flushSync），本脚本静态排在它之后，正常一次就成；万一以后被挪到前面，
   * 这里按帧等一会儿再试，而不是把异常抛到控制台。
   */
  function attach() {
    const nav = document.querySelector(NAV_SELECTOR)
    if (!nav) return false
    // 换过节点（岛整页重挂）时重新观察，避免盯着一个已脱离文档的旧节点
    if (observed === nav) return true
    observed = nav
    sync(readCat(nav))
    new MutationObserver(() => sync(readCat(nav))).observe(nav, {
      subtree: true,
      attributes: true,
      attributeFilter: ['class'],
    })
    return true
  }

  let tries = 0
  function attachEventually() {
    if (attach()) return
    if (++tries > 40) return
    setTimeout(attachEventually, 150)
  }

  attachEventually()

  /**
   * 进入设置页时补一次同步。
   *
   * 首次进入、或从别的页面切回来时，`.settings-nav-item.active` 可能本来就是
   * 那个分类（属性没变过 ⇒ MutationObserver 不会触发），只靠观察会漏掉
   * 「切页回来」这一路。盯设置页 section 的 class 变化，拿到 active 就重读一次。
   */
  const section = document.querySelector('.page[data-page="settings"]')
  if (section) {
    new MutationObserver(() => {
      if (!section.classList.contains('active')) return
      const nav = document.querySelector(NAV_SELECTOR)
      if (nav) sync(readCat(nav))
    }).observe(section, { attributes: true, attributeFilter: ['class'] })
  }
})()
