// 主面板的岛注册：把 islands/ 下的页面岛一次性全部引入。模块仍然打进同一个
// IIFE 产物，但**不再在求值时直接挂载** —— 每个页面岛用 mountOnPage 登记，
// 等 app.js 的 showPage 第一次切到那一页才创建 React root（见
// src/island-lazy.ts）。隐藏页面因此不参与首屏渲染，也不会顺手拉数据。
//
// login-page 不在其中：登录页单独构建成 ui/islands/login.js（见
// src/login-entry.tsx 与 vite.login.config.ts）。如果留在主包里，登录页
// 为了显示一张卡片又要下载整份 2.3 MB 的主面板脚本 —— 那正是这次要拆掉的。
const islandModules = import.meta.glob(['./islands/*.tsx', '!./islands/login-page.tsx'], {
  eager: true,
})
void islandModules
