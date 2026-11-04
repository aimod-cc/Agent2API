/**
 * 登录页独立 bundle 的入口。
 *
 * 登录页只需要一张卡片（Card / Input / Button / Label / Verification），
 * 没必要为了它下载主面板那整份 `islands/ui.js`（含报表图表、账号页、
 * 路由工作台等全部页面）。这个入口单独构建成 `ui/islands/login.js`，
 * 由 `ui/login.html` 引用；主面板的 island 注册表则把 login-page 排除在外。
 *
 * 登录岛在模块求值时就同步挂载（见 login-page.tsx 末尾），所以这里只做
 * 一次副作用导入。
 */
import './islands/login-page'
