/* Agent2API · 模型管理 / 网关 Key / 请求日志，以及「获取模型」弹窗两张表的
   列宽拖动与持久化 */
/* global wbApp */

/**
 * 表头右缘的把手：拖它改列宽，双击还原这一列，宽度存 localStorage。
 *
 * ── 为什么另起一份，不并进 accounts-columns.js ──────────────────
 * 账号表的列宽是一组写死的像素预算（九个定宽列 + 账号列吃剩余），拖宽之后整张表
 * 横向滚动。这里三张表的默认列宽是**相对**的（CSS 给百分数 / fr）：拖动只把被拖
 * 的那一列钉成像素，其余列保持原样 —— 没拖过的弹性列按权重分掉剩下的宽度，装不
 * 下就整张表横向滚动。列宽是绝对量：我调的是这一列，别的列不该跟着动。
 *
 * ── 别把拖动写成「重新分配」（踩过）──────────────────────────
 * 曾经让其余列在被拖列变宽时按比例让位，好把总宽维持在容器内。结果拖宽一列就得
 * 从别处扣：拖前面的列后面的变窄、拖后面的列前面的变窄，两边互相牵制，**永远
 * 拖不宽**。要滚动就让它滚，比互相挤压好解释得多。
 *
 * ── 拖动一律可以拖出容器（本次统一）────────────────────────────
 * 上面那条「不一定成立」原先只落在 grid 模式上：table 模式另有一套上限，算的是
 * 「其余列各按 56px 下限占掉之后还剩多少」——于是拖宽一列时旁边的列一起变窄、
 * 表格总宽纹丝不动、也就永远没有横向滚动条。用户看到的观感是**这张表拖不动**
 * （模型管理与请求日志的行为不一致，正是这次要修的东西）。现在两种模式同一套：
 *   · 上限只做防呆：两倍可用宽度（再宽就该换窗口了），拖动不再被容器卡住；
 *   · table 模式把「固定列 + 其余列各 56px」写成表格的 min-width，其余列让到
 *     下限就不再让，多出来的部分交给容器的横向滚动条 —— 列不会被压成 0 宽
 *     （实测：不写 min-width 时其余列会被压成 0，表头直接消失）。
 *   grid 模式不需要这一步：行宽下限由渲染方算好写在 --req-row-min 上
 *   （见 page-requests.css 的 #req-list）。
 *
 * ── 实测过的三条事实（Chrome，table-layout: fixed）──────────────
 * · 全是 px 且总和小于容器 → 浏览器按原比例放大填满，不会留空；
 * · px 与 % 混用 → % 列按权重吃掉**剩余**宽度（px 列先扣掉）；
 * · 表格 min-width 大于容器时，% 列按「min-width − px 列之和」的权重分，
 *   整张表溢出容器 → 横向滚动，且各列都还看得见。
 *
 * ── 两种表格形态 ──────────────────────────────────────────────
 * · table 模式（模型管理 / 网关 Key）：<table> + <colgroup>，宽度写回 <col>。
 *   没拖过的列不带 inline style，继续走 CSS 的百分数 —— 与拖动前完全一致。
 * · grid 模式（请求日志）：整张表是 display: grid，行由渲染方整体重绘。
 *   轨道列表写成**容器上的 CSS 变量**（--req-cols），靠继承作用到每一行 ——
 *   重绘出来的新行自动拿到同一套轨道，渲染方一行都不用改。
 *
 * ── 把手的定位与末列 ──────────────────────────────────────────
 * 把手按 right: -4px 定位（让竖线正好压住列边界），钉在表格最右缘会顶出一条
 * 横向滚动条（实测 scrollWidth 比 clientWidth 大 4px），所以末列改用
 * `.col-grip-end`（right: 0，往内让 4px）。早先的做法是「末列干脆不给把手」，
 * 代价是模型管理 / 网关 Key 的最后一列拖不了 —— 与请求日志（末列有容器内边距、
 * 照常给把手）又不一致，现在统一成「末列也有把手，只是往里挪」。
 *
 * ── 自动接入（本次新增）───────────────────────────────────────
 * 注册时 root 还不存在（React 岛还没提交首帧、弹窗还没打开）不再是「必须由渲染方
 * 补一次 repaint()」：register 会挂一个「等它出现」的观察者，root 一出现就自己
 * 完成 恢复列宽 / 补把手 / 绑事件。渲染方那侧的 repaint 调用保留即可（幂等），
 * 但不再是非做不可的事 —— 漏调一次就整张表拖不动的坑，到此为止。
 *
 * 默认宽度在两处：CSS 里的 width / grid-template-columns（没拖过的列走它）与下面
 * TABLES 里各列的 track（拖动时整条轨道列表以它为底）。改默认列宽时两处要同步。
 */
(() => {
  const STORE_PREFIX = 'agent2api-col-widths:';

  /** 拖动的下限：再窄就该点不准里面的控件了（与账号表同一个值） */
  const MIN_WIDTH = 56;

  /** 默认轨道写成纯像素的才算「钉死」；fr / % 这类会跟着容器变的都不算 */
  const FIXED_TRACK = /^(\d+(?:\.\d+)?)px$/;

  const GRIP_CLASS = 'col-grip';
  /** 末列的把手（往内让 4px，见模块头的「把手的定位与末列」） */
  const GRIP_END_CLASS = 'col-grip-end';
  const gripHtml = end => `<span class="${GRIP_CLASS}${end ? ` ${GRIP_END_CLASS}` : ''}" title="${wbI18n.t('拖动调整列宽（双击还原）')}"></span>`;

  /**
   * 六张表的登记：columns 的顺序就是列顺序。
   *
   * · table 模式：只给 key，靠表头与 <col> 上的 data-col 属性定位（不依赖列序，
   *   以后在中间插一列不会让旧数据错位）。
   * · grid 模式：给表头单元格的选择器 sel 与默认轨道 track（与 page-requests.css
   *   的 grid-template-columns 一一对应），varName 是写到容器上的变量名。
   * · 「获取模型」弹窗的两张表是**动态表**：注册时元素还不存在（弹窗未开），
   *   恢复 / 补把手 / 绑定等它出现后由 awaitRoot 自己完成（见下），弹窗每次重建
   *   表头后的 repaint 也算一路（幂等）。
   *
   * ── columnsOf：列设置与列宽的同源 ─────────────────────────────
   * 四张表都接了列设置（能藏列、能换顺序），而列宽这一层有多处要按**当前可见列**
   * 算：覆盖值落到哪个 <col>、轨道列表有几条、末列的把手该不该往里挪。各自的
   * columnsOf 从那张表的渲染模块取可见列，再用本登记的列对象对齐 —— 本登记是
   * 列宽（track / 默认宽度）的权威，列设置那头只管显隐与顺序，两者同一个答案。
   * 取不到（脚本未加载、表格还没就绪）就退回全部列，行为与接入前一致。
   */
  const visibleColumnsOf = read => function columnsOf() {
    const visible = read();
    if (!Array.isArray(visible)) return this.columns;
    const byKey = new Map(this.columns.map(column => [column.key, column]));
    return visible.map(column => byKey.get(column.key)).filter(Boolean);
  };

  const TABLES = [
    {
      id: 'models',
      mode: 'table',
      root: '.page[data-page="gateway"] table.models-table',
      columns: ['check', 'model', 'rate', 'source', 'budget', 'caps', 'alias', 'act'].map(key => ({ key })),
      columnsOf: visibleColumnsOf(() => window.wbModelsPanel?.visibleColumns?.()),
    },
    {
      id: 'keys',
      mode: 'table',
      root: 'table.keys-table',
      columns: ['name', 'key', 'time', 'state', 'act'].map(key => ({ key })),
      columnsOf: visibleColumnsOf(() => window.wbKeysPanel?.visibleColumns?.()),
    },
    {
      id: 'requests',
      mode: 'grid',
      root: '#req-list',
      head: '.req-head',
      varName: '--req-cols',
      /** 行宽下限的变量名（见 rowSpanOf）：行与表头靠它撑到内容宽，溢出那截才有背景 */
      spanVar: '--req-row-min',
      columns: [
        { key: 'time', sel: '.req-time', track: '92px' },
        { key: 'target', sel: '.req-target', track: 'minmax(0, 1.1fr)' },
        { key: 'retry', sel: '.req-retry', track: '52px' },
        { key: 'status', sel: '.req-status', track: '96px' },
        { key: 'model', sel: '.req-model', track: 'minmax(0, 1.3fr)' },
        { key: 'dur', sel: '.req-dur', track: '96px' },
        { key: 'usage', sel: '.req-usage', track: 'minmax(0, 1.6fr)' },
        { key: 'error', sel: '.req-error-cell', track: 'minmax(0, 1.2fr)' },
        { key: 'detail', sel: '.req-detail', track: '60px' },
      ],
      /**
       * 当前该渲染哪些列、按什么顺序（见上面 visibleColumnsOf 的说明）。
       *
       * 只返回**可见**列：`track` 从本登记取（这里才是列宽的权威），
       * 列设置那头只管显隐与顺序。
       */
      columnsOf: visibleColumnsOf(() => window.wbRequestsPanel?.visibleColumns?.()),
    },
    // 「获取模型」弹窗的表（islands/models-fetch-modal.tsx 动态创建，关闭即移除）。
    // 两个形态各一张表、各存一份列宽：列集合不同（内置家六列 / 自定义家三列），
    // 混用一份覆盖值会让「拖过的 provider 列宽」串到自定义家去（那边没有这列）。
    // 初始化时表还不存在（弹窗未开）→ 这三个函数都按「找不到 root」空转，
    // 真正的恢复 / 补把手 / 绑定发生在弹窗每次渲染后的 repaint（见下）。
    {
      id: 'fetch-models',
      mode: 'table',
      root: '#fm-table-intl',
      columns: ['provider', 'source', 'state', 'count', 'note', 'updated'].map(key => ({ key })),
    },
    {
      id: 'fetch-models-custom',
      mode: 'table',
      root: '#fm-table-custom',
      columns: ['pick', 'model', 'state'].map(key => ({ key })),
    },
  ];

  // ─── 覆盖值（内存 + localStorage）─────────────

  /** 每张表的覆盖值：列 key → 像素宽（没拖过的列没有键） */
  const states = new Map();

  function stateOf(table) {
    const cached = states.get(table.id);
    if (cached) return cached;
    const overrides = {};
    try {
      const raw = JSON.parse(localStorage.getItem(STORE_PREFIX + table.id) || '{}');
      for (const [key, value] of Object.entries(raw || {})) {
        const width = Number(value);
        if (table.columns.some(column => column.key === key) && Number.isFinite(width) && width >= MIN_WIDTH) {
          overrides[key] = Math.round(width);
        }
      }
    } catch { /* 存坏了就当没拖过：默认列宽照样能用 */ }
    const state = { overrides };
    states.set(table.id, state);
    return state;
  }

  function persist(table) {
    try {
      localStorage.setItem(STORE_PREFIX + table.id, JSON.stringify(stateOf(table).overrides));
    } catch { /* 隐私模式等存不了就算了：本次会话内仍然生效 */ }
  }

  // ─── 定位与落笔 ──────────────────────────────

  const rootOf = table => document.querySelector(table.root);

  /** 表头单元格：量宽度、插把手都用它（table 模式是 <th>，grid 模式是表头里的那格） */
  function headCellOf(table, root, column) {
    if (table.mode === 'table') return root.querySelector(`th[data-col="${column.key}"]`);
    return root.querySelector(`${table.head} > ${column.sel}`);
  }

  /** 可供各列排布的宽度：表格模式取表格自身，网格模式取**列表容器**的可视宽 */
  function trackSpace(table, root) {
    if (table.mode === 'table') return root.getBoundingClientRect().width;
    const head = root.querySelector(table.head);
    if (!head) return root.getBoundingClientRect().width;
    const style = getComputedStyle(head);
    // 列间距不参与分列，必须扣掉：算漏了就会拖出一条横向滚动条。
    // 缝隙数是**当前可见列**减一（藏列之后轨道也跟着少，见 activeColumns）。
    const gap = (parseFloat(style.columnGap) || 0) * Math.max(0, activeColumns(table).length - 1);
    // 量列表容器而不是表头自身：表头在溢出时会被 --req-row-min 撑到内容宽
    // （见 page-requests.css 的 #req-list），拿它的 clientWidth 会把溢出那一截
    // 也算成「可用宽度」，上限跟着虚高。容器是滚动容器，clientWidth 只含可视区。
    return root.clientWidth
      - (parseFloat(style.paddingLeft) || 0)
      - (parseFloat(style.paddingRight) || 0)
      - gap;
  }

  /**
   * 一张表**当前该渲染哪些列**（顺序即渲染顺序）。
   *
   * 列设置（table-col-settings.js）可以藏列、换顺序，所以「列宽」这一层也必须
   * 跟着变 —— grid 模式的轨道列表是逐列拼出来的，条数与顺序一旦与渲染出的
   * 格子对不上，整行会错位（第一格吃掉第二格的宽度）。
   * 表自己在 spec 里给 `columnsOf`（返回可见列及其 track）；没给就是全部列，
   * 也就是「没有接入列设置」的那些表。
   */
  const activeColumns = table => (table.columnsOf ? table.columnsOf() : table.columns);

  /** 把当前覆盖值落到 DOM 上（恢复、拖动中、还原都走它） */
  function paint(table) {
    const root = rootOf(table);
    if (!root) return;
    const { overrides } = stateOf(table);

    if (table.mode === 'table') {
      for (const column of activeColumns(table)) {
        const col = root.querySelector(`col[data-col="${column.key}"]`);
        if (!col) continue;
        const width = overrides[column.key];
        // 没拖过的列不留 inline style：CSS 的百分数仍然管着它
        if (width) col.style.width = width + 'px';
        else col.style.removeProperty('width');
      }
      // 表格的宽度下限：固定列（拖过的 + 默认就写死像素的）加上其余列各自的
      // 56px 兜底（**合计**下限，见 tableMinWidthOf）。容器更宽时它不起作用
      // （width: 100% 更大）；只有拖动把总宽顶出容器之后，它才让其余列停在
      // 兜底份额上，多出来的部分交给横向滚动条 —— 不写它的话其余列会被压成
      // 0 宽（见模块头的实测记录）。
      root.style.minWidth = tableMinWidthOf(table, activeColumns(table)) + 'px';
      return;
    }

    // grid 模式：整条轨道列表写到容器上，行由继承拿到。
    //
    // 写出的条件有两个，**缺一不可**：
    //   · 有拖过的列 → 用户宽度得有人表达；
    //   · 当前可见列与登记的全集不同（藏了列 / 换了顺序）→ CSS 里那条写死的
    //     默认轨道（page-requests.css 的 9 条）是按**全集 + 原始顺序**排的，
    //     藏一列就少一格、换个顺序就对不上位置，不写变量的话格子数与轨道数
    //     对不上，整行错位（第一格吃掉第二格的宽度）。这一条以前漏了：
    //     藏列之后只要没拖过任何列宽，就退回 CSS 默认 —— 正是最容易踩到的路径
    //     （新用户第一次试列设置）。
    // 两者都不成立时摘掉变量，走 CSS 默认 —— 与接入列设置之前完全一致。
    const columns = activeColumns(table);
    const untouched = columns.length === table.columns.length
      && columns.every((column, index) => column === table.columns[index]);
    if (untouched && !columns.some(column => overrides[column.key])) {
      root.style.removeProperty(table.varName);
    } else {
      root.style.setProperty(table.varName, columns
        .map(column => (overrides[column.key] ? `${overrides[column.key]}px` : column.track))
        .join(' '));
    }
    // 行宽下限单独算，与「拖没拖过」无关：没拖过也可能溢出（默认轨道里的定宽列
    // 加起来就超了）。固定列占满容器时行与表头要跟着撑到内容宽，溢出那截才有
    // 背景与下边框 —— 否则横向滚动到右边，表头会缺一块、行的底色也只铺半截。
    if (table.spanVar) {
      root.style.setProperty(table.spanVar, rowSpanOf(table, root, columns) + 'px');
    }
  }

  /**
   * 表头里补把手，并把不该留的摘掉。
   *
   * 「哪一列在最后」跟着列设置走（末列的把手要换成往里挪的那一档，见模块头），
   * 所以每次列集合变化都要重新对一遍：藏列 / 换顺序之后，上一轮插下的把手会跟着
   * 它所在的 <th> 一起留着 —— 那一列可能已经不是末列（该换成普通把手）、也可能
   * 原本普通的那一列变成了末列。已有的不重复插，所以重绘后反复调用是安全的。
   */
  function paintGrips(table) {
    const root = rootOf(table);
    if (!root) return;
    const columns = activeColumns(table);
    const last = columns[columns.length - 1];

    for (const grip of [...root.querySelectorAll(`.${GRIP_CLASS}`)]) {
      const column = columnOfCell(table, root, grip.parentElement);
      // 认不出所属列（列表头被重绘过）→ 留着，由渲染方那边的重绘逻辑接管
      if (!column) continue;
      // 末列（仅 table 模式）的把手要往里挪：表格右缘顶一个 -4px 的把手会凭空
      // 多出 4px 横向溢出（见模块头）。普通列反过来要摘掉往里那一档。
      if (table.mode !== 'table') continue;
      const end = column === last;
      if (end !== grip.classList.contains(GRIP_END_CLASS)) grip.remove();
    }

    columns.forEach((column, index) => {
      const cell = headCellOf(table, root, column);
      if (!cell || cell.querySelector(`:scope > .${GRIP_CLASS}`)) return;
      cell.insertAdjacentHTML('beforeend', gripHtml(table.mode === 'table' && index === columns.length - 1));
    });
  }

  // ─── 拖动 ────────────────────────────────────

  /** 默认轨道里的像素值（弹性轨道没有，返回 0） */
  function fixedPxOf(table, column) {
    const fixed = FIXED_TRACK.exec(table.mode === 'grid' ? column.track || '' : '');
    return fixed ? Math.round(parseFloat(fixed[1])) : 0;
  }

  /**
   * 表格的宽度下限（table 模式的 min-width，见 paint）：
   * 拖过的列按覆盖值，其余列各按「默认像素（若有）与 56px 里的大者」。
   *
   * 百分数列没有「固有最小宽度」这回事（CSS 里只有 25% 这种相对量），所以给它
   * 一个统一的 56px 兜底 —— 与 MIN_WIDTH 同一个语义：再窄就点不准里面的控件了。
   *
   * 注意这是**合计**下限，不是「每一列都不低于 56px」：多出来的宽度仍按各列的
   * 百分数权重分给它们（实测：25/7/8/48/12 的五列在 900px 的容器里，剩 224px 时
   * 7% 的那列拿到 30px）。与请求日志（grid 模式）的弹性列被压扁是同一种取舍 ——
   * 拖得极端时窄列会被压到只剩省略号，想让它恢复就把把手双击还原。
   */
  function tableMinWidthOf(table, columns) {
    const { overrides } = stateOf(table);
    let min = 0;
    for (const column of columns) {
      min += overrides[column.key] || Math.max(MIN_WIDTH, fixedPxOf(table, column));
    }
    return Math.round(min);
  }

  /**
   * 一行至少要占多宽：把不会伸缩的列加起来（有覆盖值的 + 默认轨道写死像素的），
   * 再补上列间距与左右内边距。有弹性列吃剩余空间时它就是容器可视宽；弹性列被
   * 压到 0 之后才是这个值 —— 也就是表格真正溢出、该横向滚动的那一刻。
   *
   * 行与表头都拿它当 min-width。**不能**改用 CSS 的 max-content 顶这件事：那是
   * 按内容的固有宽度算的，同一列在表头（「用量」两个字）和数据行（两行读数）里
   * 会算出两个宽度，表头与数据直接错位；而且列被内容撑开就不再触发省略号。
   */
  function rowSpanOf(table, root, columns) {
    const { overrides } = stateOf(table);
    let fixed = 0;
    for (const column of columns) fixed += overrides[column.key] || fixedPxOf(table, column);
    const head = root.querySelector(table.head);
    const style = head ? getComputedStyle(head) : null;
    const gap = style ? (parseFloat(style.columnGap) || 0) * Math.max(0, columns.length - 1) : 0;
    const padding = style
      ? (parseFloat(style.paddingLeft) || 0) + (parseFloat(style.paddingRight) || 0)
      : 0;
    return Math.max(root.clientWidth, fixed + gap + padding);
  }

  /**
   * 被拖这一列的宽度上限。
   *
   * 两种模式**同一套**（本次统一，见模块头）：不设「容器宽度」这种硬墙 ——
   * 列宽是绝对量，拖多宽就多宽，装不下的部分交给整张表横向滚动。上限只做防呆，
   * 取两倍可用宽度（再宽就该拆列或者换窗口了）。
   *
   * table 模式原先算的是「其余列按各自的预留下限占掉之后还剩多少」，而不会伸缩的
   * 列本身就能把容器占满 —— 差值恒为负、上限退化成当前宽度，拖动整个失效
   * （模型管理页「拖不动」就是这一条；请求日志的「错误列拖不动」是同一个成因，
   * 那边先修过）。保留起见也记一句：那条路和上面「别写成重新分配」的教训是同一个。
   */
  function limitOf(table, total, key, startWidth) {
    return Math.max(MIN_WIDTH, Math.round(total * 2), Math.round(startWidth));
  }

  const clamp = (value, min, max) => Math.min(Math.max(value, min), max);

  /**
   * 落一次宽度：写内存 → 立即重画（拖动中每帧都调）。
   *
   * **只动被拖的那一列**，其余列一律保持各自宽度 —— 这是这张表本来就该有的语义：
   * 我调的是这一列，别的列不该跟着动；超出容器的部分交给整张表横向滚动（与账号表
   * 同一条取舍：表格不压缩列宽）。
   *
   * 曾经把它写成「重排」：拖动时按比例压缩其余列、把总宽维持在容器内。那样拖宽
   * 一列就得从别处扣，拖前面的列后面的变窄、拖后面的列前面的变窄，两边互相牵制，
   * 结果是**永远拖不宽**。列宽是绝对量，不是一份要按比例分完的预算。
   *
   * 弹性列（没拖过的 fr 列）仍由 CSS 按权重吃剩余空间：空间不足时它先被压到 0，
   * 那是它的本分，不算被挤窄。
   */
  function applyWidth(table, key, width) {
    stateOf(table).overrides[key] = Math.round(width);
    paint(table);
  }

  /** 命中的表头格属于哪一列（按**当前可见列**找，藏列后索引会变） */
  function columnOfCell(table, root, cell) {
    if (!cell) return null;
    return activeColumns(table).find(column => headCellOf(table, root, column) === cell) || null;
  }

  /**
   * 委托绑定：pointerdown 开拖、dblclick 还原。挂在表格/列表容器上一次即可，
   * 内部节点被重绘后监听仍然有效（委托到容器，不依赖具体节点）。
   *
   * 用指针事件 + setPointerCapture，而不是鼠标事件：**松手必须收得到**。
   * 鼠标事件只在窗口内派发 —— 指针拖出窗口（或拖到别的窗口 / 面板上）再松手，
   * 那次 mouseup 会被浏览器丢掉，拖动就永远不结束。此后鼠标一动列宽就跟着走，
   * 表现为「一按住列就自己往后拓宽」，且把手一直亮着。捕获之后事件直接回到
   * 把手，拖出窗口也收得到；再加两道兜底（buttons 为 0 立即收尾、窗口失焦收尾），
   * 任何情况下拖动都会结束。
   *
   * 全程按**列 key** 而不是索引定位：列设置能换顺序、能藏列，索引随时会变，
   * 而 key 是稳定的身份（宽度覆盖值也是按 key 存的，两处口径一致）。
   */
  function bind(table) {
    const root = rootOf(table);
    if (!root) return;
    // 动态表（弹窗里的表）每次打开都是新元素：同一个 root 不重复绑，
    // 换过 root（旧表已随弹窗移除）就重新绑一次 —— 监听挂在旧元素上，
    // 元素没了监听也跟着没了，不补绑的话拖动会在第二次打开后失效。
    if (table.boundRoot === root) return;
    table.boundRoot = root;

    root.addEventListener('pointerdown', event => {
      // 只接左键：右键/中键按下会弹菜单，不该顺手把拖动开起来
      if (event.button !== 0) return;
      const grip = event.target.closest?.(`.${GRIP_CLASS}`);
      // 委托挂在各自的表上，所以命中的把手一定在这张表里；columnOfCell 再确认它属于哪一列
      const column = columnOfCell(table, root, grip?.parentElement);
      if (!column) return;
      event.preventDefault();

      const startX = event.clientX;
      const startWidth = grip.parentElement.getBoundingClientRect().width;
      const total = trackSpace(table, root);
      const limit = limitOf(table, total, column.key, startWidth);
      grip.classList.add('active');
      document.body.classList.add('col-resizing');
      // 捕获失败（合成事件、老内核）不影响功能：全局监听那条路照旧
      try { grip.setPointerCapture(event.pointerId); } catch { /* 退回全局监听 */ }

      const move = moveEvent => {
        // 兜底一：没按住任何键就不是拖动（捕获失效时事件会漏到这里）
        if (moveEvent.buttons === 0) { up(); return; }
        applyWidth(table, column.key, clamp(startWidth + moveEvent.clientX - startX, MIN_WIDTH, limit));
      };
      const up = () => {
        grip.classList.remove('active');
        document.body.classList.remove('col-resizing');
        // 指针抬起时浏览器已自动释放捕获，这里再释放一次会抛 NotFoundError
        try { grip.releasePointerCapture(event.pointerId); } catch { /* 已自动释放 */ }
        window.removeEventListener('pointermove', move);
        window.removeEventListener('pointerup', up);
        window.removeEventListener('pointercancel', up);
        window.removeEventListener('blur', up);
        persist(table);
      };
      window.addEventListener('pointermove', move);
      window.addEventListener('pointerup', up);
      window.addEventListener('pointercancel', up);
      // 兜底二：拖到一半切窗口/切标签，收不到 pointerup 也要收尾
      window.addEventListener('blur', up);
    });

    root.addEventListener('dblclick', event => {
      const grip = event.target.closest?.(`.${GRIP_CLASS}`);
      if (!grip) return;
      const column = columnOfCell(table, root, grip.parentElement);
      if (!column) return;
      const { overrides } = stateOf(table);
      if (!overrides[column.key]) return;
      // 还原这一列：删掉覆盖值 → 它回到 CSS 的默认宽度，腾出的宽度由其余列按权重分掉
      delete overrides[column.key];
      persist(table);
      paint(table);
    });
  }

  /**
   * 观察一张表的 root：表头每次被重建（React 重渲染、列设置就地重排）都会把把手
   * 一起换掉，不看着点就再也补不回来；列宽覆盖值也要重新落到新的 <col> 上。
   *
   * 两种模式都装（原先只有 grid 模式）：table 模式的 tbody 会被整体重绘（换筛选、
   * 轮询刷新），childList 一动就补一遍，成本只是几次幂等的 style 写入。
   *
   * 观察者只对子节点变动触发；paint 写的是元素自身的 style 与 <col> 的 style，
   * 都不是 childList 变动，所以不存在自激循环。对已有把手直接跳过，重复调用安全。
   * 回调合并到下一帧：一次 React 提交会连着来几十条 mutation，合并后只重画一遍。
   */
  function watch(table) {
    if (typeof MutationObserver !== 'function') return;
    const root = rootOf(table);
    if (!root) return;
    if (table.watchedRoot === root) return;
    table.watchedRoot = root;
    let pending = false;
    new MutationObserver(() => {
      if (pending) return;
      pending = true;
      const flush = () => { pending = false; paint(table); paintGrips(table); };
      // rAF 在后台标签页不跑；没有它（或页面被挂起）时退回微任务，别漏掉这一轮
      if (typeof requestAnimationFrame === 'function') requestAnimationFrame(flush);
      else Promise.resolve().then(flush);
    }).observe(root, { childList: true, subtree: true });
  }

  /**
   * 等 root 出现：注册那一刻表还没渲染出来（React 岛的首帧、弹窗里的表）时，
   * 盯着文档等它挂上去，出现后立刻完成 恢复列宽 / 补把手 / 绑事件。
   *
   * 这一条是**兜底**：渲染方那侧仍可以在表头重建后调 `repaint(id)`（幂等），
   * 但漏调不再等于「这张表永远拖不动」—— 那正是模型管理页与请求日志行为不一致的
   * 一部分成因（请求日志踩过同一个坑，当时是靠渲染方补一次调用绕过的）。
   */
  function awaitRoot(table) {
    if (typeof MutationObserver !== 'function') return;
    if (table.awaiting) return;
    table.awaiting = true;
    let pending = false;
    const observer = new MutationObserver(() => {
      if (pending) return;
      pending = true;
      const flush = () => {
        pending = false;
        if (!rootOf(table)) return;
        observer.disconnect();
        table.awaiting = false;
        attach(table);
      };
      if (typeof requestAnimationFrame === 'function') requestAnimationFrame(flush);
      else Promise.resolve().then(flush);
    });
    observer.observe(document.documentElement, { childList: true, subtree: true });
  }

  /** 接入一张表：恢复列宽 → 补把手 → 绑事件 → 装上重绘观察者 */
  function attach(table) {
    bind(table);
    paint(table);
    paintGrips(table);
    watch(table);
  }

  /** 注册：root 在就立刻接入，不在就等它出现（见 awaitRoot） */
  function register(table) {
    TABLES.push(table);
    if (rootOf(table)) attach(table);
    else awaitRoot(table);
  }

  for (const table of TABLES.slice()) register(table);

  /**
   * 按 id 重画某张表：列设置改了显隐 / 顺序之后调它。
   *
   * 列宽与把手两件事都要重对一遍 —— 轨道条数（grid 模式）、覆盖值落到哪个
   * <col>（table 模式）、以及「末列把手往里挪」这条规则，全都按当前可见列算。
   *
   * 也负责**补绑**：动态表（弹窗）的表头被重建之后，绑定与把手要重来一遍
   * （见 attach）。渲染方调它即可 —— 但漏调也有 awaitRoot / watch 兜底。
   */
  function repaint(id) {
    const table = TABLES.find(item => item.id === id);
    if (!table) return;
    if (!rootOf(table)) { awaitRoot(table); return; }
    attach(table);
  }

  // 供别处重画用（表格自己重绘了列宽骨架时调一次）
  window.wbTableColumns = { paint, grips: paintGrips, register, repaint };
})();
