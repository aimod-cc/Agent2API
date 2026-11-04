import { mountInput, observeInputs } from './input'
import { mountSegmented } from './segmented'
import './uikit'
import './styles/islands.css'
// 外壳须先同步创建页面挂载点，业务岛才能接管各 section。
import './desktop-shell'
import './desktop-tooltip'
import './desktop-titlebar'
import './island-registry'

declare global {
  interface Window {
    wbSegmented?: { mount: typeof mountSegmented }
    wbInput?: { mount: typeof mountInput }
  }
}

window.wbSegmented = { mount: mountSegmented }
window.wbInput = { mount: mountInput }
observeInputs()

// 首屏页现在就先挂上：`app.js` 的第一次 `showPage` 要等 DOMContentLoaded，
// 而页面岛改成按需挂载后，首次绘制到那一刻之间内容区会是空的。这里按与
// app.js `PAGE_KEY` 相同的 localStorage 键提前派发一次；之后 `showPage`
// 再派发相同页面时，island-lazy 对已挂载的入口是幂等的。
const initialPage = (() => {
  try {
    return localStorage.getItem('workbuddy-desktop-page') || 'overview'
  } catch {
    return 'overview'
  }
})()
window.dispatchEvent(new CustomEvent('wb:page', { detail: initialPage }))
