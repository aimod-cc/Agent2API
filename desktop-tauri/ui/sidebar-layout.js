/* Agent2API · 侧边栏布局（分组 / 顺序 / 显隐）—— 库为权威，localStorage 作首帧缓存 */
/* global wbI18n */

/**
 * 侧边栏菜单自定义的**唯一数据口径**。
 *
 * ── 为什么是经典脚本、而不是 ui-islands 里的 .ts ──────────────
 * 侧边栏本身由 ui/app.js（经典脚本）渲染，而设置页的编辑面板是 React 岛
 * （ui-islands/src/islands/sidebar-pane.tsx）。模型放在任一侧，另一侧就得
 * 抄一份同样的规则 —— 两份「默认分组 + 校验 + 兼容补位」必然漂。放在这里，
 * 两侧都读 `window.wbSidebarLayout`：app.js 直接用，岛经 window 取。
 *
 * 它必须排在 app.js 与 islands/ui.js **之前**（index.html 里紧挨 units.js）。
 *
 * ── 存的是什么 ────────────────────────────────────────────────
 * `{ groups: [{ id, label, custom, hidden, items: [menuKey] }], hidden: [menuKey] }`
 *   · groups 的数组顺序 = 侧边栏上的分组顺序；
 *   · items 的数组顺序 = 组内菜单顺序；
 *   · hidden 是「单独隐藏的菜单」，与 group.hidden（整组隐藏）是两件事：
 *     整组隐藏时组内菜单的显隐状态原样保留，重新显示该组时不会丢。
 *
 * ── 存在哪里：库是权威，localStorage 是首帧缓存 ─────────────────
 * 权威副本在库里（`kv` 表的 `sidebarLayout` 键，经 `GET/PUT /api/sidebar-layout`
 * 读写）。为什么必须落库：本产品有两个界面入口 —— 桌面壳（`tauri://localhost`）
 * 与网关托管的网页端（`http://127.0.0.1:<port>`），**两者 origin 不同、
 * localStorage 天然隔离**。只存 localStorage 的话，在 WebUI 里隐藏的菜单
 * 在客户端里照旧显示，用户看到的就是「设置没生效」。
 *
 * 但读盘仍然是**同步读 localStorage**，这是刻意的：
 *   · 侧边栏首帧由 app.js 在模块顶部同步渲染（那条时序是契约，见那里的注释），
 *     改成等一次网络往返会让侧边栏先空一下再出现；
 *   · 桥不可用（浏览器直开、注入失败）时界面照常可用，不该整块空掉。
 * 于是分工是：**同步读缓存画首帧，异步对库做同步**（`syncFromServer`）。
 * 缓存与库不一致的窗口最多是一次网络往返 —— 拉到库里的值就重画。
 *
 * ── 写：先缓存，再推库 ──────────────────────────────────────
 * `writeLayout` 同步写 localStorage（界面立刻生效、刷新不丢），然后
 * fire-and-forget 把整份推给后端。推失败不阻塞界面（与「存不下只影响下次
 * 启动」同一取向），但要 `console.warn` —— 否则「另一台设备看不到这次改动」
 * 会是个无从下手的谜。
 *
 * ── 向前兼容（这一段决定了这个文件为什么不能省）────────────────
 * 读盘一律按 MENU_KEYS 校验：
 *   · 不认识的菜单键丢掉（降级回旧版本时，旧版本不认识新菜单）；
 *   · 新版本新增、但存盘里没有的菜单，自动补进第一组末尾 ——
 *     否则升级后新功能会「凭空消失」，用户只会以为坏了；
 *   · 同一菜单出现在两组时只保留先出现的那一份（数据被手改过时兜底）。
 *
 * 这些校验同时是**从库读回数据时**的防线：库里的那份可能来自更老的版本，
 * 也可能被外部工具改过，`normalizeLayout` 是它进入界面前的唯一关口。
 */
(() => {
  /**
   * 首帧缓存的键（**不是**权威副本 —— 权威在库里的 `kv.sidebarLayout`，见模块头）。
   * 沿用项目既有的 workbuddy-desktop-* 前缀；-v1 留给将来的结构变更。
   */
  const STORAGE_KEY = 'workbuddy-sidebar-layout-v1';

  /**
   * 菜单表 —— 与 app.js 的 PAGES / PAGE_LABELS 是同一份事实的两种表达。
   *
   * ⚠ 加一级页面时三处必须同时改：这里、app.js 的 PAGES / PAGE_LABELS、
   *   index.html 里的 `<section class="page" data-page>`。
   *
   * `label` 在**模块加载期**就取好译文（wbI18n 的字面量调用同时是给
   * i18n 扫描器的登记点：写成数据表 + 运行期用变量查表的话，扫描器
   * 提取不到键，五份词典永远缺这几条）。语言切换是整页刷新（见 i18n.js），
   * 刷新后这张表按新语言重建，不存在「译文过期」这种状态。
   * `icon` 是 icons.js 里的键。
   */
  const MENUS = [
    { key: 'assistant', label: wbI18n.t('AI 助手'), icon: 'spark' },
    { key: 'overview', label: wbI18n.t('报表'), icon: 'overview' },
    { key: 'accounts', label: wbI18n.t('账号'), icon: 'accounts' },
    // 签到中心：**固定项**，与「账号」「模型管理」同级，不随插件装没装而消失。
    //
    // 早先它由插件申报（`pack.json` 的 `ui`），没装插件时菜单里就没有这一项 ——
    // 那让「签到」成了插件的附属品，而不是产品的一个功能。现在反过来切：页面归
    // 网关（本表里的固定项），**能力**归插件（走哪个接口、有哪些档位、有没有保活
    // /旅行这类专属面板，都由插件申报）。没装插件时页面显示空态并指路，比一个
    // 凭空消失的菜单项清楚得多。
    { key: 'checkin', label: wbI18n.t('签到中心'), icon: 'checkin' },
    { key: 'routing', label: wbI18n.t('路由'), icon: 'route' },
    { key: 'gateway', label: wbI18n.t('模型管理'), icon: 'gateway' },
    { key: 'agent', label: wbI18n.t('Agent 配置'), icon: 'agent' },
    // 工具服务：给客户端装上的可选能力（网络搜索 / 视觉分析），排在客户端
    // 配置之后、出网代理之前 —— 前者是「接哪些客户端」，这一项是「接了能用什么」。
    { key: 'tools', label: wbI18n.t('工具服务'), icon: 'tools' },
    { key: 'proxies', label: wbI18n.t('网络代理'), icon: 'proxies' },
    { key: 'keys', label: wbI18n.t('网关 Key'), icon: 'key' },
    { key: 'docs', label: wbI18n.t('文档'), icon: 'logs' },
    { key: 'requests', label: wbI18n.t('请求日志'), icon: 'requests' },
    { key: 'logs', label: wbI18n.t('日志'), icon: 'logs' },
    { key: 'tasks', label: wbI18n.t('定时任务'), icon: 'tasks' },
    // 提供商插件：各家 provider 的转发与账号实现都在插件包里，这一页是装插件的
    // 唯一入口。排在「设置」上面 —— 它是「让软件能用起来」的前置步骤，与设置同级
    // 但更靠前（没装插件时账号页与模型页都是空的）。
    { key: 'plugins', label: wbI18n.t('插件'), icon: 'plugins' },
    // 固定项：不可隐藏，所在分组也不可整组隐藏（隐藏了用户只能清缓存找回来）
    { key: 'settings', label: wbI18n.t('设置'), icon: 'settings', fixed: true },
  ];

  const MENU_BY_KEY = new Map(MENUS.map(menu => [menu.key, menu]));
  const MENU_KEYS = MENUS.map(menu => menu.key);

  /**
   * 插件申报的页面（运行时注册，`key` 形如 `plugin:<pack>:<page>`）。
   *
   * ── 为什么菜单能由插件贡献 ──────────────────────────────────
   * 有些功能只有装了插件才有意义（签到中心：自定义 API 提供商结构上不可能有
   * 签到）。把它们留在客户端意味着「每加一个插件，每个客户端都要改一遍」。
   * 插件在 `pack.json` 里申报页面，网关托管静态资源，客户端只负责把清单里的
   * 页面挂到菜单上 —— 没装插件就没有这一项，卸载后自动消失。
   *
   * ── 为什么与 MENUS 分开 ────────────────────────────────────
   * `MENUS` 是编译期常量（默认布局、校验、固定项都按它算）；插件页面是运行期
   * 才知道的，可能变（装 / 卸 / 升级）。混在一起会让 `defaultLayout()` 依赖
   * 网络状态。所以：静态表管内置页，这张表管插件页，`renderGroups` 把两者合并。
   *
   * ── 落盘兼容 ────────────────────────────────────────────────
   * 用户布局里可能存着某个插件页的 key（上次装过）。插件卸载后再读盘，那个
   * key 变成「不认识的菜单」→ `normalizeLayout` 丢掉它（已有逻辑），重新装上
   * 时会作为「新菜单」补进默认位置。不需要额外迁移。
   */
  const dynamicMenus = new Map();

  /** 插件页面的菜单键前缀（与内置页不会撞） */
  const PLUGIN_PREFIX = 'plugin:';

  /** 某个 key 是不是插件页 */
  function isPluginKey(key) {
    return typeof key === 'string' && key.startsWith(PLUGIN_PREFIX);
  }

  /**
   * 用插件申报的页面替换动态菜单表（每次拿到 `/api/plugins` 都调一次）。
   *
   * `pages` 的元素：`{ key, label, icon, url }`（key 已由调用方拼好前缀）。
   * 传空数组 = 一个插件页都没有（没装包 / 全部停用）。
   *
   * 返回 `true` 表示菜单集合变了 —— 调用方据此决定要不要重画侧边栏
   * （每轮轮询都重画会让用户正在拖动的项闪烁）。
   */
  function setDynamicMenus(pages) {
    const next = new Map();
    for (const page of Array.isArray(pages) ? pages : []) {
      if (!page || typeof page.key !== 'string' || !page.key) continue;
      if (!isPluginKey(page.key)) continue;
      next.set(page.key, {
        key: page.key,
        label: typeof page.label === 'string' ? page.label : page.key,
        icon: typeof page.icon === 'string' && page.icon ? page.icon : 'plugins',
        url: typeof page.url === 'string' ? page.url : '',
        // 插件申报的默认归属分组（内置分组 id）。只用于「首次补进布局时放哪」，
        // 之后以用户存的位置为准；值不认识时 `normalizeLayout` 会退到第一组。
        group: typeof page.group === 'string' ? page.group : '',
        // 插件页永远不是固定项（用户可以隐藏它）
        plugin: true,
      });
    }
    // 集合是否变化：键集合相同且 label / url / 归属组都一致才算没变
    let changed = next.size !== dynamicMenus.size;
    if (!changed) {
      for (const [key, menu] of next) {
        const prev = dynamicMenus.get(key);
        if (!prev || prev.label !== menu.label || prev.url !== menu.url || prev.group !== menu.group) {
          changed = true;
          break;
        }
      }
    }
    if (changed) {
      dynamicMenus.clear();
      for (const [key, menu] of next) dynamicMenus.set(key, menu);
    }
    return changed;
  }

  /** 插件页的菜单项（没注册过 = undefined） */
  function dynamicMenuOf(key) {
    return dynamicMenus.get(key);
  }

  /** 当前全部动态菜单（按注册顺序） */
  function dynamicMenuList() {
    return [...dynamicMenus.values()];
  }

  /**
   * 内置分组的展示名。
   *
   * 分组名是**数据**（存在 localStorage 里、用户可改），渲染时只拿到一个字符串，
   * 所以不能写成运行期用变量查表 —— 那样扫描器提取不到键，五份词典里永远缺这几条，
   * 英文界面会直接露出中文。这里把四个内置名显式列成字面量调用，扫描器能认；
   * 存盘里保存的仍是中文原键，换语言时重新查表即可（用户自建的分组名不在表里，
   * 原样显示 —— 那是他自己写的字）。
   *
   * 为什么表建在模块加载期：界面语言切换是整页刷新（见 i18n.js），
   * 刷新后这张表会按新语言重建，不存在「表里的译文过期」这种状态。
   */
  const GROUP_LABELS = {
    概览: wbI18n.t('概览'),
    转发链路: wbI18n.t('转发链路'),
    诊断: wbI18n.t('诊断'),
    系统: wbI18n.t('系统'),
  };

  /** 分组名的展示文本：内置分组查表，自建分组原样返回 */
  function groupLabel(label) {
    return GROUP_LABELS[label] || label;
  }

  /**
   * 默认布局 —— 「概览 / 转发链路 / 诊断 / 系统」。
   *
   * 与改动前的「智能 / 运行状态 / 转发能力 / 诊断」相比，调整的三处及理由：
   *   · 「智能」只有 AI 助手一项，单项成组只多占一行，且「智能」这个词指向不明
   *     （AI 能力？智能路由？）—— 并入「概览」，与报表 / 账号 / 签到中心一起，
   *     这四项都是「看一眼现状」的页面；
   *   · 「运行状态」名不副实：报表是状态，账号是资源，签到中心是运营动作，
   *     三者不是同一类东西 —— 拆掉，各自归位；
   *   · 「设置」原本塞在「诊断」里，但它是全局系统入口而不是排障工具 ——
   *     单独成「系统」组，固定项也在这一组；
   *   · 「转发能力」改叫「转发链路」：模型管理 / Agent / 网络代理是同一个转发链
   *     的三段（发给谁 / 谁来发 / 从哪出去），网关 Key 与文档是「怎么接进来」。
   *     五项归属本身没问题，只把组名说得更准。
   */
  function defaultLayout() {
    return {
      groups: [
        { id: 'g-overview', label: '概览', custom: false, hidden: false,
          items: ['assistant', 'overview', 'accounts', 'checkin'] },
        { id: 'g-forward', label: '转发链路', custom: false, hidden: false,
          items: ['routing', 'gateway', 'agent', 'tools', 'proxies', 'keys', 'docs'] },
        { id: 'g-diag', label: '诊断', custom: false, hidden: false,
          items: ['requests', 'logs', 'tasks'] },
        { id: 'g-system', label: '系统', custom: false, hidden: false,
          items: ['plugins', 'settings'] },
      ],
      hidden: [],
    };
  }

  /** 深拷贝：快照 / 变更函数都靠它，避免调用方拿到同一个对象互相改 */
  function cloneLayout(layout) {
    return {
      groups: layout.groups.map(group => ({
        id: group.id,
        label: group.label,
        custom: group.custom,
        hidden: group.hidden,
        items: group.items.slice(),
      })),
      hidden: layout.hidden.slice(),
    };
  }

  /**
   * 某个 key 是不是「认得的菜单」—— 内置页看静态表，插件页看动态表。
   *
   * ── 为什么插件页也要过这一关 ────────────────────────────────
   * 落盘的布局里可能存着已经卸载的插件页 key。不校验就会画出一颗点了
   * 什么都不发生的菜单项；校验之后它被丢掉，重新装插件时作为「新菜单」
   * 补回默认位置（见 `normalizeLayout` 的补位逻辑）。
   */
  const isMenuKey = value => typeof value === 'string'
    && (MENU_BY_KEY.has(value) || dynamicMenus.has(value));
  const isFixedMenu = key => MENU_BY_KEY.get(key)?.fixed === true;
  /** 菜单项（内置或插件页；都没有 = undefined） */
  const menuOf = key => MENU_BY_KEY.get(key) || dynamicMenus.get(key);

  /**
   * 把任意来源的数据规整成合法布局。
   *
   * 「读盘」的唯一入口：localStorage、导入的 JSON、别的版本写下的形状，全部先过它。
   * 只做减法与补位、不抛错 —— 一份坏数据最多退回默认布局，不该让侧边栏整块消失。
   */
  function normalizeLayout(raw) {
    const source = raw && typeof raw === 'object' ? raw : {};
    const rawGroups = Array.isArray(source.groups) ? source.groups : [];
    const seen = new Set();
    const groups = [];

    for (let index = 0; index < rawGroups.length; index += 1) {
      const group = rawGroups[index];
      if (!group || typeof group !== 'object') continue;
      const items = [];
      const rawItems = Array.isArray(group.items) ? group.items : [];
      for (const key of rawItems) {
        // 不认识的键（旧版本 / 手改坏了）与重复项都在这里丢掉
        if (!isMenuKey(key) || seen.has(key)) continue;
        seen.add(key);
        items.push(key);
      }
      const label = typeof group.label === 'string' ? group.label.trim() : '';
      groups.push({
        // id 只用于拖动认人与 React key，不参与展示；缺了就补一个稳定的
        id: typeof group.id === 'string' && group.id ? group.id : 'g-' + index,
        label: label || ('分组 ' + (index + 1)),
        custom: group.custom === true,
        hidden: group.hidden === true,
        items,
      });
    }

    // 一条分组都没有（首次运行 / 数据坏了）：直接用默认布局
    if (groups.length === 0) return defaultLayout();

    // 新版本新增的菜单补进**默认位置**：升级后新功能不能「凭空消失」，
    // 也不该被随手塞到第一组末尾 —— 那会让「提供商插件」这种有明确归属
    // （系统组、设置之上）的入口跑到「概览」组里，用户得自己去调。
    //
    // 定位规则（逐级退化，任何一步失败都不丢菜单）：
    //   ① 默认布局里它所在的组存在 → 插到「默认顺序里它后面第一个已在组内的
    //      兄弟」前面（保持相对次序，且不打断用户自己拖过的顺序）；
    //   ② 那组已被用户删掉 → 追加到第一组末尾；
    //   ③ 一条分组都没有 → 上面已经直接返回 defaultLayout()，走不到这里。
    const missing = MENU_KEYS.filter(key => !seen.has(key));
    if (missing.length) {
      const defaults = defaultLayout().groups;
      for (const key of missing) {
        const home = defaults.find(group => group.items.includes(key));
        const target = key === 'routing'
          ? groups.find(group => group.items.includes('gateway')) || (home ? groups.find(group => group.id === home.id) : null)
          : home ? groups.find(group => group.id === home.id) : null;
        if (!target) {
          groups[0].items.push(key);
          continue;
        }
        const order = home.items;
        const next = order
          .slice(order.indexOf(key) + 1)
          .find(sibling => target.items.includes(sibling));
        if (next) target.items.splice(target.items.indexOf(next), 0, key);
        else target.items.push(key);
      }
    }

    // 插件页：补进插件**申报的默认分组**（没申报就进第一组）。
    //
    // ── 为什么要看申报值 ────────────────────────────────────────
    // 客户端不认识插件的业务，猜不出「签到中心该在概览组还是转发链路组」。
    // 早先这里一律补进「转发链路」，于是从内置页迁过去的签到中心位置变了
    // （原本在概览组）—— 迁移不该改变用户已经熟悉的菜单布局。
    // 插件在 `pack.json` 的 `ui[].group` 里申报内置分组 id，这里照办。
    //
    // 申报了但分组不存在（用户删过那组、或插件写了个不认识的 id）→ 退到
    // 第一组，与「没申报」同一条兜底：菜单项绝不能因为归组问题而消失。
    // 用户拖过之后以用户存的位置为准（`seen` 已挡掉）。
    const pluginMissing = [...dynamicMenus.keys()].filter(key => !seen.has(key));
    if (pluginMissing.length) {
      for (const key of pluginMissing) {
        const declared = dynamicMenus.get(key)?.group;
        const target = (declared && groups.find(group => group.id === declared)) || groups[0];
        if (target) target.items.push(key);
      }
    }

    // 含固定项的分组不可整组隐藏
    for (const group of groups) {
      if (group.hidden && group.items.some(isFixedMenu)) group.hidden = false;
    }

    const rawHidden = Array.isArray(source.hidden) ? source.hidden : [];
    const hidden = [];
    for (const key of rawHidden) {
      if (isMenuKey(key) && !isFixedMenu(key) && hidden.indexOf(key) < 0) hidden.push(key);
    }

    return { groups, hidden };
  }

  /**
   * 读**缓存**里的布局；任何异常（无缓存 / 坏 JSON）都退回默认布局。
   *
   * ⚠ 名字是历史遗留：它读的是 localStorage 那份首帧缓存，不是库里的权威副本。
   * 权威副本经 `syncFromServer` 异步拉回并覆盖缓存，之后这里读到的就是它。
   * 之所以保持同步：首帧渲染在模块顶部同步跑（见 app.js 的契约注释）。
   *
   * ── 为什么「没存过」也要过 `normalizeLayout` ────────────────
   * 首次运行（localStorage 里还没有布局）时若直接返回 `defaultLayout()`，
   * 就绕过了 `normalizeLayout` 末尾那段「把插件页补进分组」的逻辑 ——
   * 而插件页**只在动态表里**，`defaultLayout()` 的静态项永远不含它们。
   * 后果：装了插件、页面托管也正常（`/plugins/…` 返回 200），侧边栏里却
   * 永远没有那一项，用户只能手动去「设置 → 显示 → 侧边栏菜单」里找 ——
   * 而那时它压根没被放进布局，连勾选项都不存在。
   *
   * 修法是把默认布局也当成一份「待规整的原始数据」走同一条路：它是合法的
   * 内置布局，`normalizeLayout` 对它的减法（丢未知键）无事可做，补位逻辑
   * 则把插件页补进「转发链路」组。返回值因此与「存过一份默认布局」完全一致。
   */
  function readLayout() {
    try {
      const text = localStorage.getItem(STORAGE_KEY);
      if (!text) return normalizeLayout(defaultLayout());
      return normalizeLayout(JSON.parse(text));
    } catch {
      return normalizeLayout(defaultLayout());
    }
  }

  /**
   * 写布局：先写首帧缓存（同步、立刻生效），再把整份推给库（异步、尽力而为）。
   *
   * 顺序不能反：缓存写失败（隐私模式 / 配额满）时界面仍按内存里的新布局走，
   * 而库那次推送成功就能在刷新后恢复 —— 两条路互为兜底。
   *
   * 推库失败只 `console.warn`，不弹 toast：界面本地已经生效，用户看到的是
   * 「设置成功」；此时弹一句「保存失败」只会让人以为改坏了。真正需要知道的
   * 是「另一台设备看不到这次改动」，而那是排查时的信息，写控制台即可。
   */
  function writeLayout(layout) {
    const normalized = normalizeLayout(layout);
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(normalized));
    } catch {
      /* 存不下只影响下次启动，不影响本次会话 */
    }
    pushToServer(normalized);
  }

  /* ─── 与库同步（权威副本）───────────────── */

  /**
   * 最近一次本地写入的时刻（毫秒）。用于躲开「自己刚写的还没到库」那个窗口：
   * 用户改完布局 → `writeLayout` 立刻写缓存并发出 PUT，而 20 秒轮询的 GET
   * 可能赶在那条 PUT 落库之前回来，读到旧值就把用户刚改的布局又盖回去。
   * 宽限期内的 GET 结果一律不采信（等下一轮，那时 PUT 早已落库）。
   */
  let lastLocalWriteAt = 0;
  const LOCAL_WRITE_GRACE_MS = 3_000;

  /**
   * 把布局推给后端。桥不可用（浏览器直开 / 注入失败）时静默跳过 ——
   * 那种形态下本来就只有 localStorage 这一份，行为与改造前一致。
   *
   * 不做并发合并：一次布局变更是用户的一次明确操作，连点两下就是两次写入，
   * 后到的覆盖先到的正好是用户期望的顺序（后端是整份 UPSERT，天然幂等）。
   */
  function pushToServer(layout) {
    lastLocalWriteAt = Date.now();
    const bridge = window.workbuddyDesktop;
    if (!bridge || typeof bridge.saveSidebarLayout !== 'function') return;
    try {
      Promise.resolve(bridge.saveSidebarLayout(layout)).catch(error => {
        console.warn('侧边栏布局保存到数据库失败（本机已生效，其它设备看不到这次改动）:', error);
      });
    } catch (error) {
      console.warn('侧边栏布局保存到数据库失败（本机已生效，其它设备看不到这次改动）:', error);
    }
  }

  /**
   * 从库里拉一次布局，与缓存对齐。启动时调一次、之后随全局轮询调用。
   *
   * 四种结果：
   *   · 刚在本地写过（宽限期内）→ 跳过这一轮。那条 PUT 可能还没落库，
   *     此刻读到的旧值会把用户刚做的改动盖回去（见 `lastLocalWriteAt`）；
   *   · 库里有布局 → 规整后覆盖缓存；**与缓存不同**才派 `wb:sidebar-layout`
   *     重画（每轮轮询都重画会让用户正在拖动的项闪烁，与 `adoptPluginPages`
   *     同一取舍）；
   *   · 库里没有（`layout: null`）→ 这是首次落库的迁移窗口：把缓存里那份
   *     （老用户本机已调好的布局）**上行**写一次，用户无感升级；
   *   · 拉取失败 → 静默。离线 / 桥不可用时界面照常，下一轮再试。
   *
   * 返回 `true` 表示缓存被更新过（调用方目前不需要，留给将来的调用点）。
   */
  async function syncFromServer() {
    // 宽限期：本地刚写完的这几秒内不采信库里的值（理由见上）
    if (Date.now() - lastLocalWriteAt < LOCAL_WRITE_GRACE_MS) return false;
    const bridge = window.workbuddyDesktop;
    if (!bridge || typeof bridge.getSidebarLayout !== 'function') return false;
    let remote;
    try {
      remote = await bridge.getSidebarLayout();
    } catch {
      return false; // 桥打不通：继续用缓存
    }
    // 等这一次网络往返期间用户又改了：以用户那次为准，本轮结果作废
    if (Date.now() - lastLocalWriteAt < LOCAL_WRITE_GRACE_MS) return false;
    const raw = remote && typeof remote === 'object' ? remote.layout : undefined;

    // 库里没存过：把本机缓存上行（老用户迁移），之后以库为准
    if (!raw || typeof raw !== 'object') {
      pushToServer(readLayout());
      return false;
    }

    const incoming = normalizeLayout(raw);
    const incomingText = JSON.stringify(incoming);
    let cachedText = null;
    try {
      cachedText = localStorage.getItem(STORAGE_KEY);
    } catch {
      cachedText = null;
    }
    // 比较的是「规整后的文本」而不是原始文本：库里那份可能来自旧版本
    // （缺新菜单），规整会补位 —— 那种情况下缓存也该跟着更新，否则
    // 每次启动都要重算一遍补位。
    if (cachedText === incomingText) return false;

    try {
      localStorage.setItem(STORAGE_KEY, incomingText);
    } catch {
      /* 写不进缓存也仍然派事件：本次会话按库里的那份渲染 */
    }
    window.dispatchEvent(new CustomEvent('wb:sidebar-layout'));
    return true;
  }

  /* ─── 查询 ─────────────────────────────── */

  /** 某个菜单在哪一组、组内第几个；不在任何组里时返回 null */
  function locate(layout, key) {
    for (let gi = 0; gi < layout.groups.length; gi += 1) {
      const index = layout.groups[gi].items.indexOf(key);
      if (index >= 0) return { gi, index };
    }
    return null;
  }

  /** 这一组是否含固定项（含则不可整组隐藏） */
  function groupHasFixed(group) {
    return group.items.some(isFixedMenu);
  }

  /** 组内可见（未被单独隐藏）的菜单数 */
  function visibleCount(group, hidden) {
    return group.items.filter(key => hidden.indexOf(key) < 0).length;
  }

  /* ─── 变更（全部返回新对象，不改入参；无变化返回 null）────────── */

  /** 把菜单 key 移到第 toGi 组的 toIndex 位置 */
  function moveItem(layout, key, toGi, toIndex) {
    const from = locate(layout, key);
    if (!from || !layout.groups[toGi]) return null;
    const next = cloneLayout(layout);

    if (from.gi === toGi) {
      const list = next.groups[toGi].items;
      if (toIndex === from.index || toIndex === from.index + 1) return null;
      list.splice(from.index, 1);
      list.splice(toIndex > from.index ? toIndex - 1 : toIndex, 0, key);
      return next;
    }

    next.groups[from.gi].items.splice(from.index, 1);
    const target = next.groups[toGi].items;
    target.splice(Math.max(0, Math.min(toIndex, target.length)), 0, key);
    return next;
  }

  /** 把分组 gid 移到 toIndex 位置（同组内移动语义与 moveItem 一致） */
  function moveGroup(layout, gid, toIndex) {
    const from = layout.groups.findIndex(group => group.id === gid);
    if (from < 0) return null;
    if (toIndex === from || toIndex === from + 1) return null;
    const next = cloneLayout(layout);
    const moved = next.groups.splice(from, 1)[0];
    next.groups.splice(toIndex > from ? toIndex - 1 : toIndex, 0, moved);
    return next;
  }

  /** 设置某个菜单的显隐；固定项永远可见（返回 null = 无需变更） */
  function setItemHidden(layout, key, hidden) {
    if (isFixedMenu(key) && hidden) return null;
    const next = cloneLayout(layout);
    const index = next.hidden.indexOf(key);
    if (hidden && index < 0) next.hidden.push(key);
    else if (!hidden && index >= 0) next.hidden.splice(index, 1);
    else return null;
    return next;
  }

  /** 整组显隐；含固定项的组不可隐藏（返回 null） */
  function setGroupHidden(layout, gid, hidden) {
    const index = layout.groups.findIndex(group => group.id === gid);
    if (index < 0) return null;
    if (hidden && groupHasFixed(layout.groups[index])) return null;
    if (layout.groups[index].hidden === hidden) return null;
    const next = cloneLayout(layout);
    next.groups[index].hidden = hidden;
    return next;
  }

  /** 分组改名；空名或没变化时返回 null */
  function renameGroup(layout, gid, label) {
    const index = layout.groups.findIndex(group => group.id === gid);
    if (index < 0) return null;
    const trimmed = String(label || '').trim();
    if (!trimmed || trimmed === layout.groups[index].label) return null;
    const next = cloneLayout(layout);
    next.groups[index].label = trimmed;
    return next;
  }

  /** 新建一个空分组（排在最后） */
  function addGroup(layout, label) {
    const trimmed = String(label || '').trim();
    if (!trimmed) return null;
    const next = cloneLayout(layout);
    next.groups.push({
      id: 'g-custom-' + Date.now().toString(36) + '-' + Math.random().toString(36).slice(2, 6),
      label: trimmed,
      custom: true,
      hidden: false,
      items: [],
    });
    return next;
  }

  /**
   * 删除一个分组，组内菜单并入第一组。
   *
   * 为什么是「并入」而不是一起删：菜单代表真实页面，删分组只是删掉那行分组标题，
   * 不该顺带把用户的功能入口弄丢。只剩一个分组时不允许删 —— 没有分组就没有
   * 「分组」这个概念了。
   */
  function removeGroup(layout, gid) {
    if (layout.groups.length <= 1) return null;
    const index = layout.groups.findIndex(group => group.id === gid);
    if (index < 0) return null;
    const next = cloneLayout(layout);
    const removed = next.groups.splice(index, 1)[0];
    next.groups[0].items.push(...removed.items);
    return next;
  }

  /* ─── 派生：侧边栏要画什么 ───────────────── */

  /**
   * 把布局压成「侧边栏实际要画的分组」。
   *
   * 规则：整组隐藏的跳过；组内被单独隐藏的跳过；过滤后一个不剩的组也跳过
   * （画一个只有标题的空组，用户会以为加载失败）。返回值可能是空数组 ——
   * 调用方要为此兜底一句提示，否则侧边栏只剩品牌与底部。
   */
  function renderGroups(layout) {
    const out = [];
    for (const group of layout.groups) {
      if (group.hidden) continue;
      const items = [];
      for (const key of group.items) {
        if (layout.hidden.indexOf(key) >= 0) continue;
        const menu = menuOf(key);
        if (menu) items.push(menu);
      }
      if (!items.length) continue;
      out.push({ id: group.id, label: group.label, items });
    }
    return out;
  }

  /** 落地页：侧边栏上第一个可见菜单（一个都不剩时返回 null） */
  function firstVisibleKey(layout) {
    const groups = renderGroups(layout);
    return groups.length ? groups[0].items[0].key : null;
  }

  /** 某个菜单当前是否可见（含「所在分组被隐藏」这一层） */
  function isVisible(layout, key) {
    if (layout.hidden.indexOf(key) >= 0) return false;
    return renderGroups(layout).some(group => group.items.some(menu => menu.key === key));
  }

  window.wbSidebarLayout = {
    STORAGE_KEY,
    MENUS,
    MENU_KEYS,
    defaultLayout,
    normalizeLayout,
    readLayout,
    writeLayout,
    cloneLayout,
    menuOf,
    // 与库同步：`writeLayout` 内部会推库；`syncFromServer` 由 app.js 在启动
    // 与全局轮询里调（见那里的调用点）。桥不可用时两者都是无害的空操作。
    syncFromServer,
    // 插件页（运行时注册，见 `setDynamicMenus`）
    setDynamicMenus,
    dynamicMenuOf,
    dynamicMenuList,
    isPluginKey,
    PLUGIN_PREFIX,
    groupLabel,
    isFixedMenu,
    locate,
    groupHasFixed,
    visibleCount,
    moveItem,
    moveGroup,
    setItemHidden,
    setGroupHidden,
    renameGroup,
    addGroup,
    removeGroup,
    renderGroups,
    firstVisibleKey,
    isVisible,
  };
})();
