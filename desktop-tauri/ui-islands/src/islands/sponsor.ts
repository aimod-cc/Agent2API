/**
 * 赞助位（ProxyLane）在客户端的投放素材：外链与品牌 Logo 的**唯一定义点**。
 *
 * ── 为什么单独一个模块 ────────────────────────────────────────
 * 客户端的投放点有两处（「新增代理」弹窗里的一行提示、「新增订阅」弹窗的
 * 「家庭宽带」一档），而链接与素材路径都是赞助方约定的固定值 —— 复制成两份
 * 迟早出现「一处改了另一处没改」，分位置统计就此断掉。页面与弹窗要用一律
 * 从这里取。
 *
 * ── 链接不要动 ───────────────────────────────────────────────
 * `utm_content=client` 是赞助方的**桌面客户端**位置标记（四个投放位各一条：
 * `github_readme` / `docs` / `client` / `qq`，见 README 与 docs/ 的赞助段）。
 * 同一个位置下的多个投放点**共用同一条链接**（赞助方的口径），所以这里只有
 * 一条常量；别改参数、别换用别处的链接，否则对方的分位置统计会断。
 */

/** 桌面客户端投放位的外链（经 update-shared 的 handleExternalClick 交系统浏览器打开） */
export const PROXYLANE_URL =
  'https://proxylane.dev/zh-cn/isp-proxies?utm_source=agent2api&utm_medium=partnership&utm_campaign=agent2api_202610&utm_content=client'

/** Logo 的浅色底版本（深色主题下换用下面那张，见网页面板的 <picture> 同款手法） */
export const PROXYLANE_LOGO_LIGHT = 'assets/proxylane-logo-for-light-bg.png'
/** Logo 的深色底版本 —— 两版都是 486×85，等宽等高，切换时不会跳版 */
export const PROXYLANE_LOGO_DARK = 'assets/proxylane-logo-for-dark-bg.png'
