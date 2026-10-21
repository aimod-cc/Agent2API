/* Agent2API · 侧边栏布局（分组 / 顺序 / 显隐）—— 纯前端偏好 */
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
 * ── 为什么存 localStorage 而不是后端配置 ──────────────────────
 * 与主题 / 缩放 / 语言 / 计量单位同族：只影响本机这台设备的界面排布，
 * 不碰转发、不碰账号，也不需要跨设备同步。放进后端 config 只会多出
 * 「读配置失败时侧边栏怎么画」这条分支，而它的兜底仍然是「用默认值」——
 * 与 localStorage 读不到时完全一样，却多一次网络往返和一份迁移逻辑。
 *
 * ── 向前兼容（这一段决定了这个文件为什么不能省）────────────────
 * 读盘一律按 MENU_KEYS 校验：
 *   · 不认识的菜单键丢掉（降级回旧版本时，旧版本不认识新菜单）；
 *   · 新版本新增、但存盘里没有的菜单，自动补进第一组末尾 ——
 *     否则升级后新功能会「凭空消失」，用户只会以为坏了；
 *   · 同一菜单出现在两组时只保留先出现的那一份（数据被手改过时兜底）。
 */
(() => {
  /** 持久化键：沿用项目既有的 workbuddy-desktop-* 前缀；-v1 留给将来的结构变更 */
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
    { key: 'checkin', label: wbI18n.t('签到中心'), icon: 'checkin' },
    { key: 'gateway', label: wbI18n.t('模型管理'), icon: 'gateway' },
    { key: 'agent', label: wbI18n.t('Agent'), icon: 'agent' },
    { key: 'proxies', label: wbI18n.t('网络代理'), icon: 'proxies' },
    { key: 'keys', label: wbI18n.t('网关 Key'), icon: 'key' },
    { key: 'docs', label: wbI18n.t('文档'), icon: 'logs' },
    { key: 'requests', label: wbI18n.t('请求日志'), icon: 'requests' },
    { key: 'logs', label: wbI18n.t('日志'), icon: 'logs' },
    { key: 'tasks', label: wbI18n.t('定时任务'), icon: 'tasks' },
    // 固定项：不可隐藏，所在分组也不可整组隐藏（隐藏了用户只能清缓存找回来）
    { key: 'settings', label: wbI18n.t('设置'), icon: 'settings', fixed: true },
  ];

  const MENU_BY_KEY = new Map(MENUS.map(menu => [menu.key, menu]));
  const MENU_KEYS = MENUS.map(menu => menu.key);

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
          items: ['gateway', 'agent', 'proxies', 'keys', 'docs'] },
        { id: 'g-diag', label: '诊断', custom: false, hidden: false,
          items: ['requests', 'logs', 'tasks'] },
        { id: 'g-system', label: '系统', custom: false, hidden: false,
          items: ['settings'] },
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

  const isMenuKey = value => typeof value === 'string' && MENU_BY_KEY.has(value);
  const isFixedMenu = key => MENU_BY_KEY.get(key)?.fixed === true;

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

    // 新版本新增的菜单补进第一组：升级后新功能不能「凭空消失」
    const missing = MENU_KEYS.filter(key => !seen.has(key));
    if (missing.length) groups[0].items.push(...missing);

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

  /** 读本机布局；任何异常（无存储 / 坏 JSON）都退回默认布局 */
  function readLayout() {
    try {
      const text = localStorage.getItem(STORAGE_KEY);
      if (!text) return defaultLayout();
      return normalizeLayout(JSON.parse(text));
    } catch {
      return defaultLayout();
    }
  }

  /** 写本机布局；存储不可用（隐私模式 / 配额满）时静默失败，界面照常工作 */
  function writeLayout(layout) {
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(normalizeLayout(layout)));
    } catch {
      /* 存不下只影响下次启动，不影响本次会话 */
    }
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
        const menu = MENU_BY_KEY.get(key);
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
    menuOf: key => MENU_BY_KEY.get(key),
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
