/**
 * 组件库自己的 i18n 垫片（中文即键，缺译回落键本身）。
 *
 * 为什么 ui-kit 要单独一份、不复用岛工程（ui-islands/src/i18n.ts）的：组件库自带
 * 独立 typecheck（本目录 node_modules），反向 import 岛工程会把两边的编译绑在一起 ——
 * 垫片只有几行，就地复制最省事。
 *
 * 运行时与岛侧 t() 读的是同一个 window.wbI18n（ui/i18n.js 注入，head 阶段就位）；
 * window 不存在或运行时未就位时回落键本身，中文界面照常，不抛错。
 */

type TParams = Record<string, string | number>

/** 取键的译文；params 把 {name} 形参替换成值 */
export function t(key: string, params?: TParams): string {
  if (typeof window === 'undefined') return key
  // 窄类型转型、不 declare global：岛工程已声明过 window.wbI18n，两边同时进一个
  // TS program 时重复声明会触发 TS2717（同名属性必须逐字同类型）
  const api = (window as unknown as { wbI18n?: { t(key: string, params?: TParams): string } }).wbI18n
  return api ? (api.t(key, params) ?? key) : key
}
