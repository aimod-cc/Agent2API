/* Agent2API · 内联 SVG 图标集 */
/* global wbIcons */

/**
 * 侧边栏导航图标。
 *
 * 用内联 SVG 而不是 emoji / 文字符号（原来用的 ◈ ☰ ⇄ ✳ ≡ ⚙）：
 * 那类是字体字形，同一份 UI 在不同机器上渲染出来粗细、大小、基线都不同，
 * 而且无法随文字颜色精确着色（部分字形会被系统按 emoji 处理成彩色）。
 * SVG 用 currentColor 填充，尺寸、颜色完全受控。
 *
 * 图标按 24×24 画布绘制，通过 width/height 缩放到目标尺寸。
 */
(() => {
  const ICONS = {
    // 概览：仪表盘
    overview: '<path d="M12 3a9 9 0 0 0-9 9 8.94 8.94 0 0 0 2.2 5.9c.3.34.73.53 1.18.53h11.24c.45 0 .88-.2 1.18-.53A8.94 8.94 0 0 0 21 12a9 9 0 0 0-9-9Zm0 2a7 7 0 0 1 7 7 6.95 6.95 0 0 1-1.6 4.5H6.6A6.95 6.95 0 0 1 5 12a7 7 0 0 1 7-7Zm0 1.5a1.5 1.5 0 1 0 0 3 1.5 1.5 0 0 0 0-3ZM11 12v4h2v-4h-2Z"/>',
    // 账号：人群
    accounts: '<path d="M9 11a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7Zm0 2c-3.6 0-6.5 2-6.5 4.5V20h13v-2.5C15.5 15 12.6 13 9 13Zm7.5-2a3 3 0 1 0 0-6 3 3 0 0 0 0 6Zm0 2c-.9 0-1.7.12-2.4.35A6.9 6.9 0 0 1 18 17.5V20h3.5v-2.5c0-2.5-2.6-4.5-5-4.5Z"/>',
    // 签到中心：日历 + 对勾。与 proxies 同款用描边（填充块在 17px 下会糊成一团，
    // 见上面 proxies 那条的说明），粗细同一套约定（1.8 + 圆头圆角）。
    checkin: '<g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">'
      + '<rect x="3.5" y="4.5" width="17" height="16" rx="2.5"/><path d="M3.5 9.5h17M8 2.5v4m8-4v4"/>'
      + '<path d="m9.2 14.7 2 2 3.6-4.1"/></g>',
    // 网关：双向箭头
    gateway: '<path d="M7.4 6.6 4 10l3.4 3.4 1.4-1.4-.6-.6H16V15h2V9.6H8.2l.6-.6-1.4-1.4Zm9.2 6.8L15.2 12l-1.4 1.4.6.6H8v2h6.4l-.6.6 1.4 1.4L19.6 14l-3-3.2Z"/>',
    // 网关 Key：钥匙
    key: '<path d="M14.5 3a6.5 6.5 0 0 0-6.2 8.5L2 17.8V22h4.2v-2.4h2.4v-2.4h2.4l1.5-1.5A6.5 6.5 0 1 0 14.5 3Zm0 2a4.5 4.5 0 1 1 0 9c-.6 0-1.1-.1-1.6-.3l-.6-.2-2 2H7.9v2.4H5.6V19H4v-.4l6.2-6.2-.2-.6A4.5 4.5 0 0 1 14.5 5Zm1.5 2a1.5 1.5 0 1 0 0 3 1.5 1.5 0 0 0 0-3Z"/>',
    // 网络代理：地球（圆 + 赤道 + 经线椭圆）。「这个出口通往哪里」的既有意象。
    // 这一枚用描边而不是填充（同组的其余导航图标都是填充）：17px 下「圆里再挖三条线」
    // 会糊成一团，描边反而清楚；粗细取 1.8，与设置页那一组描边图标同一套约定。
    proxies: '<g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round">'
      + '<circle cx="12" cy="12" r="9"/><path d="M3 12h18"/>'
      + '<ellipse cx="12" cy="12" rx="4.2" ry="9"/></g>',
    // 日志：文档 + 文本行
    logs: '<path d="M6 2h8l4 4v16H6V2Zm2 2v16h8V7h-3V4H8Zm2 6h6v2h-6v-2Zm0 4h6v2h-6v-2Z"/>',
    // 请求日志：列表（点 + 行），与 OmniProxy 的请求日志入口同一意象
    requests: '<path d="M4 5.5A1.5 1.5 0 1 1 5.5 4 1.5 1.5 0 0 1 4 5.5ZM8 4h12v2.5H8V4Zm-4 7.5A1.5 1.5 0 1 1 5.5 10 1.5 1.5 0 0 1 4 11.5ZM8 10h12v2.5H8V10Zm-4 7.5A1.5 1.5 0 1 1 5.5 16 1.5 1.5 0 0 1 4 17.5ZM8 16h12v2.5H8V16Z"/>',
    // 定时任务：时钟（圆 + 时针分针）
    tasks: '<path d="M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18Zm0 2a7 7 0 1 1 0 14 7 7 0 0 1 0-14Zm-1 3v5.4l3.8 2.3 1-1.7-2.8-1.7V8h-2Z"/>',
    // 设置：齿轮
    settings: '<path d="m20.1 12.9-.1-.9.1-.9 2-1.5-2-3.5-2.3 1a7.6 7.6 0 0 0-1.6-.9L15.9 3h-4l-.3 2.5c-.6.2-1.1.5-1.6.9l-2.3-1-2 3.5 2 1.5-.1.9.1.9-2 1.5 2 3.5 2.3-1c.5.4 1 .7 1.6.9l.3 2.5h4l.3-2.5c.6-.2 1.1-.5 1.6-.9l2.3 1 2-3.5-2-1.5ZM13.9 15a3 3 0 1 1 0-6 3 3 0 0 1 0 6Z"/>',
    // 状态灯（侧栏底部网关状态用）
    pulse: '<circle cx="12" cy="12" r="5"/>',
    // AI 助手：四角星 + 一颗小星。主侧栏「智能」组的导航图标与设置页左栏的
    // 分类图标共用（两处都是「助手」这一件事，不该出现两种造型）；填充式，
    // 与主侧栏那组一致 —— 小星在 17px 下仍能看出是两点，不至于糊成一团。
    spark: '<path d="M10.4 3.2 12.1 8.1 17 9.8 12.1 11.5 10.4 16.4 8.7 11.5 3.8 9.8 8.7 8.1 10.4 3.2Z"/>'
      + '<path d="M17.8 13.4 18.7 15.9 21.2 16.8 18.7 17.7 17.8 20.2 16.9 17.7 14.4 16.8 16.9 15.9 17.8 13.4Z"/>',

    /**
     * 设置页左栏的分类图标（2026-09 新增，九个一组）。
     *
     * ── 为什么单独一组（不复用上面那些导航图标）──────────────
     * 上面那组是主侧栏的**填充式**导航图标；设置页分类项的文字是 12.5px、
     * 图标盒 17px，填充块在这个尺寸下比文字重。这九个统一用**描边**画
     * （粗细 1.8 + 圆头圆角，与 arrowDown / eye 同一套约定，17px 下约 1.2px），
     * 几何取自 Feather / Lucide 的成熟图形（与 eye / eyeOff 借 Feather 同理），
     * 小尺寸下笔画不糊、形状可辨。九个图标同一风格，设置页内自成一套。
     */
    // 通用：调节滑杆（三条轨道 + 三个把手）
    sliders: '<g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">'
      + '<path d="M4 21v-7M4 10V3M12 21v-9M12 8V3M20 21v-5M20 12V3"/>'
      + '<path d="M1 14h6M9 8h6M17 16h6"/></g>',
    // 显示：显示器（屏 + 底座）
    display: '<g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">'
      + '<rect x="2" y="3" width="20" height="14" rx="2"/>'
      + '<path d="M8 21h8M12 17v4"/></g>',
    // 网关：双向箭头（进出的流量）
    traffic: '<g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">'
      + '<path d="M8 3 4 7l4 4M4 7h16"/><path d="m16 21 4-4-4-4M20 17H4"/></g>',
    // 路由：一条线分出两条支路（请求分流到不同账号）
    route: '<g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">'
      + '<path d="M4 12h5"/><circle cx="3.5" cy="12" r="1.5"/>'
      + '<path d="M9 12c3 0 3-6 6-6h4"/><path d="m17 3 3 3-3 3"/>'
      + '<path d="M9 12c3 0 3 6 6 6h4"/><path d="m17 15 3 3-3 3"/></g>',
    // 重试：环形箭头
    refresh: '<g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">'
      + '<path d="M23 4v6h-6"/><path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10"/></g>',
    // 超时：秒表（顶部按钮 + 表盘 + 指针）
    timer: '<g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">'
      + '<path d="M10 2h4M12 14l3-3"/><circle cx="12" cy="14" r="8"/></g>',
    // 安全：盾牌
    shield: '<g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">'
      + '<path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z"/></g>',
    // 数据：数据库（三层圆柱）
    database: '<g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">'
      + '<ellipse cx="12" cy="5" rx="9" ry="3"/>'
      + '<path d="M21 12c0 1.66-4 3-9 3s-9-1.34-9-3"/><path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5"/></g>',
    // 反馈与需求：对话气泡
    feedback: '<g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">'
      + '<path d="M21 11.5a8.38 8.38 0 0 1-.9 3.8 8.5 8.5 0 0 1-7.6 4.7 8.38 8.38 0 0 1-3.8-.9L3 21l1.9-5.7a8.38 8.38 0 0 1-.9-3.8 8.5 8.5 0 0 1 4.7-7.6 8.38 8.38 0 0 1 3.8-.9h.5a8.48 8.48 0 0 1 8 8v.5z"/></g>',
    // 更新：向下箭头 + 托盘（下载更新包）
    download: '<g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">'
      + '<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><path d="m7 10 5 5 5-5M12 15V3"/></g>',

    /**
     * 优先级控件的两枚箭头（账号表「↓ 数字 ↑」合并控件用）。
     *
     * ── 为什么不用字符 ↓ / ↑（本次修复）────────────────────────
     * 那两个字是**字体字形**，墨迹在行盒里天生偏下：Segoe UI 下它们的
     * actualBoundingBoxAscent=7 / descent=0（整个字形贴在基线之上、没有下伸
     * 部分），而同一个控件里的数字是 ascent=8 / descent=0 —— 两者都靠
     * `align-items: center` 把行盒居中，于是箭头的**视觉重心比几何中心低
     * 约 0.5px**，在 22.67px 高的按钮里肉眼可见（用户实测反馈「有点靠下」）。
     * 这不是布局错误，换行高 / 加 padding 都治不了（动的是字形之外的东西）。
     *
     * 改成 SVG 之后几何完全受控：图标盒 24×24，箭头在盒内**上下对称**
     * （顶点 y=5、底点 y=19，中心恰好 12），而按钮是 flex 居中的 ——
     * 盒居中即墨迹居中，与字体、字号、平台都无关。
     * 这也正是本文件模块头写的那条理由（字体字形在不同机器上基线不同）。
     *
     * 形状：竖线 + 箭头，用 stroke 画（linecap/linejoin 圆角），
     * 与 brand 那枚双向箭头同一手法。
     *
     * ── 粗细为什么是 1.8（不是 2.4）─────────────────────────────
     * 图标显示尺寸 14px、画布 24 → 换算系数 14/24 ≈ 0.583，
     * 所以属性值 1.8 在屏幕上约 1.05px。旁边的数字（11.5px Segoe UI）
     * 字干约 0.98px —— 两者相称。
     * 初版写成 2.4（≈1.4px）比数字粗四成，箭头显得笨重、抢了数字的视觉权重，
     * 而这一列的主读数是数字（箭头只是改值入口）。
     */
    arrowDown: '<g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">'
      + '<path d="M12 5v14"/><path d="m6 13 6 6 6-6"/></g>',
    arrowUp: '<g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">'
      + '<path d="M12 19V5"/><path d="m6 11 6-6 6 6"/></g>',

    /**
     * 拖动把手（账号页分组模式：分组的组顺位、组内的行顺序都靠它）。
     * 六个圆点两列排布，是「这一处可以拖」的通用意象。光标态交给 CSS
     * （默认 grab、拖动中 grabbing），图标本身不带状态。
     */
    gripVertical: '<g fill="currentColor">'
      + '<circle cx="9" cy="6" r="1.4"/><circle cx="15" cy="6" r="1.4"/>'
      + '<circle cx="9" cy="12" r="1.4"/><circle cx="15" cy="12" r="1.4"/>'
      + '<circle cx="9" cy="18" r="1.4"/><circle cx="15" cy="18" r="1.4"/></g>',
    /** 折叠箭头（分组带的展开 / 收起）：向下 = 已展开；收起态由 CSS 旋转 -90° */
    chevronDown: '<g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">'
      + '<path d="m6 9 6 6 6-6"/></g>',
    /**
     * 折叠面板的展开箭头（`details.panel-fold` 的 summary 里，置于标题之前）。
     * 向右 = 收起；展开后由 CSS 旋转 90° 指向下方 —— 两个状态只差一个变换，
     * 所以只画这一个形状。几何与设置页那组描边图标同一套约定（1.8 粗细、
     * 圆头），显示 14px 时约 1.05px，与 13.5px 的标题字重相称。
     */
    caretRight: '<g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">'
      + '<path d="m9 5 7 7-7 7"/></g>',

    /**
     * 账号名隐私开关的两枚眼睛（账号表「账号」表头的显隐按钮）。
     * 几何沿用 Feather 的 eye / eye-off：睁眼是轮廓 + 瞳孔，闭眼是斜杠 + 裂开的轮廓，
     * 两枚共用同一外轮廓弧线，切换时只有斜杠与缺口出现 / 消失，不觉得是换了一个图标。
     * 与箭头同一手法：stroke 画、粗细 1.8（显示 13px 时约 1px，与表头 10.5px 小字相称）。
     */
    eye: '<g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">'
      + '<path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/></g>',
    eyeOff: '<g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">'
      + '<path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"/>'
      + '<path d="m1 1 22 22"/></g>',

    /**
     * 品牌标：应用图标本体（蓝底圆角方块 + 白色双向箭头）。
     *
     * 几何照搬 build/make-icon.mjs —— 那里画在 1024 画布上，这里按同一比例
     * 换算到 24 画布，两处形状严格一致（换算：x24 = (x归一化 - 0.5) × 24 + 12）：
     *   横杆粗细 2×0.046×24 ≈ 2.2，箭头张开的半高 0.105×24 ≈ 2.5。
     *
     * 颜色不用 currentColor 而写死 #007AFF：这枚标要与 .ico/.png 应用图标
     * 一模一样，而主题里的 --primary 在深浅两套下取值不同（深色主题会偏亮），
     * 跟着它走就不再是同一枚图标了。箭头同理写死纯白。
     */
    brand: '<rect width="24" height="24" rx="5.4" fill="#007AFF"/>'
      + '<g fill="none" stroke="#fff" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">'
      + '<path d="M5.23 9.24h10.71"/>'
      + '<path d="M15.94 6.72 18.77 9.24 15.94 11.76"/>'
      + '<path d="M18.77 14.76H8.06"/>'
      + '<path d="M8.06 12.24 5.23 14.76 8.06 17.28"/>'
      + '</g>',
  };

  /**
   * 渲染图标 SVG。
   * @param {string} name ICONS 里的键
   * @param {number} size 边长（px），默认 16
   */
  function icon(name, size = 16) {
    const path = ICONS[name];
    if (!path) return '';
    return `<svg viewBox="0 0 24 24" width="${size}" height="${size}" fill="currentColor" aria-hidden="true" focusable="false">${path}</svg>`;
  }

  window.wbIcons = { icon, names: Object.keys(ICONS) };
})();
