/* ─────────────────────────────────────────────
   移动端卡片化的列名注入（配合 css/mobile.css）
   ─────────────────────────────────────────────
   mobile.css 把表格行 / 请求行拆成 grid 卡片，用 td::before / span::before
   { content: attr(data-col) } 显示列名。但原始 DOM 里 data-col 只挂在
   th / .req-head > span 上，td 与 .req-row > span 上没有 —— 这份脚本读表头
   每个 th（或 req-head 格）的 data-col 与文字，按列序写到对应格子上。

   只在窄屏（≤767px）时执行，桌面端不碰 DOM。
   每次页面渲染 / 刷新后重跑（监听所有 .page 子树，60ms 合并连续触发）。

   两张来源都有的表（模型管理 / 网关 Key / 账号）用 data-col 做精确匹配；
   没有 data-col 的表（网络代理）按位置兜底 —— th[i] 的文字写进 td[i]。
   列设置隐藏一列时会把 th 与 td 一起从 DOM 摘掉，位置序号仍然对齐。
   ───────────────────────────────────────────── */
(() => {
  const MAX_WIDTH = 767;

  function labelOf(el) {
    // 表头格里混着列宽把手（.col-grip）、小问号（.tip-q）与列名补充说明
    // （.th-hint，如「优先级」后面的「全局队列」）—— 手机上的卡片标签只要
    // 主名，克隆一份把附属节点摘掉再取文本
    const clone = el.cloneNode(true);
    clone.querySelectorAll('.col-grip, .th-hint, .tip-q, svg').forEach(n => n.remove());
    const text = (clone.textContent || '').replace(/\s+/g, ' ').trim();
    // 个别表头把补充说明直接写在同一段文本里（模型管理的「模型映射（原始 ID
    // 与别名独立开关）」）—— 尾部括号说明在卡片标签里只会把标签撑成两行
    return text.replace(/[（(][^）)]*[）)]\s*$/, '').trim() || text;
  }

  function already(el) {
    // 已经写过就不覆盖：React 重渲染会重建 DOM（属性被清掉），
    // 那时下一次 paint 自然补上；没有重渲染时跳过省得白写
    return el.getAttribute('data-col');
  }

  /* ─── 表格（.acct-table / .models-table / .keys-table / 代理表）─── */
  function paintTable(table) {
    const ths = [...table.querySelectorAll('thead th')];
    if (!ths.length) return;

    const hasKey = ths.some(th => th.getAttribute('data-col'));
    const map = new Map();
    if (hasKey) {
      ths.forEach(th => {
        const k = th.getAttribute('data-col');
        if (k) map.set(k, labelOf(th));
      });
    }
    // 判据要问「这张表有没有可写的标签」，不能问「有没有 key」：网络代理页与
    // 订阅表的表头一个 data-col 都没有（它们走下面按列序兜底那一支），只看 hasKey
    // 会让这两张表整表跳过 —— 卡片化之后每个格子上方只剩一条空标签，列名全丢。
    if (!hasKey && !ths.some(th => labelOf(th))) return;

    table.querySelectorAll('tbody tr').forEach(tr => {
      const cells = tr.children;
      for (let i = 0; i < cells.length && i < ths.length; i++) {
        const td = cells[i], th = ths[i];
        if (!th || td.tagName !== 'TD' || already(td)) continue;
        const label = hasKey
          ? (map.get(th.getAttribute('data-col')) || labelOf(th))
          : labelOf(th);
        if (label) td.setAttribute('data-col', label);
      }
    });
  }

  /* ─── 请求日志行（.req-head / .req-row 一族）─── */
  function paintReqRows(page) {
    const head = page.querySelector('.req-head');
    if (!head) return;
    const heads = [...head.children];
    const labels = heads.map(h => ({
      key: h.getAttribute('data-col'),
      text: labelOf(h),
    }));
    if (!labels.some(l => l.key || l.text)) return;

    page.querySelectorAll('.req-row').forEach(row => {
      [...row.children].forEach((cell, i) => {
        const h = labels[i];
        if (!h || already(cell)) return;
        const label = h.text;
        if (label) cell.setAttribute('data-col', label);
      });
    });
  }

  function paintAll() {
    document.querySelectorAll('.page').forEach(page => {
      page.querySelectorAll('table').forEach(paintTable);
      paintReqRows(page);
    });
  }

  function run() {
    if (window.innerWidth > MAX_WIDTH) return;
    paintAll();
  }

  // 首次执行 + 窗口尺寸跨过断点时重跑
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', run, { once: true });
  } else {
    run();
  }
  window.addEventListener('resize', run);

  // React 岛重渲染后 tbody / .req-row 里的行会重建（data-col 属性被清掉）。
  // 用 MutationObserver 监听每个 .page 的子树变化，60ms 合并后重跑。
  let timer = null;
  const schedule = () => {
    clearTimeout(timer);
    timer = setTimeout(run, 60);
  };
  document.querySelectorAll('.page').forEach(page => {
    new MutationObserver(schedule).observe(page, { childList: true, subtree: true });
  });
})();
