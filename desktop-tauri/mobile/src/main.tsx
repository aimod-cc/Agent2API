import { createRoot } from 'react-dom/client'
import { App } from './App'
import { setAuthLostHandler } from './api/client'
import { store } from './state/store'
import { initPwa } from './lib/pwa'
import './styles/mobile.css'

/**
 * 手机端入口。
 *
 * 与 ui-islands 的区别：那个是「一堆岛挂到既有页面上」，这个是**独占整个
 * 页面**的应用 —— 所以用 createRoot 挂到 #root，不需要 `display: contents`
 * 那套宿主技巧。
 *
 * 401 接管：客户端自己不做跳转（它不知道当前界面在做什么），由这里注册
 * 回调，交给 store 弹「重新登录 / 补 Key」层。
 */
setAuthLostHandler(mode => {
  store.setAuthLost(mode)
})

/**
 * PWA 运行时必须在 render 之前初始化，不能放进组件的 useEffect：
 * `beforeinstallprompt` 在页面加载后不久就会触发，而事件对象只能捕获一次 ——
 * 等 React 挂载完再挂监听，有可能已经错过（首屏快的时候）。
 * 这里的返回值是清理函数，单页应用整个生命周期不卸载，忽略即可。
 */
initPwa()

const host = document.getElementById('root')
if (host) {
  createRoot(host).render(<App />)
} else {
  // 挂载点缺失时给出可读错误，而不是静默白屏
  document.body.innerHTML =
    '<pre style="padding:20px;font:13px/1.6 monospace;color:#c00">手机端挂载失败：页面缺少 #root 容器。</pre>'
}
