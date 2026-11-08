import * as React from 'react'

/**
 * 下拉触发器宽度的测定工具。
 *
 * 「按列表内容的最长文案定宽、不随容器自适应」需要一个像素值，而选项里
 * 常混着 CJK 与拉丁字符，`ch` 一类的纯 CSS 单位量不准。这里用一块挂在
 * body 上的隐藏离屏 span 实测：字体族取 body 的计算值，字号用触发器的
 * `text-[12.5px]`，与真实渲染同源，量出来的就是实际占宽。
 */

/** 同一文案只量一次（换主题 / 换字体后仍是同一字号，缓存不构成漂移） */
const measured = new Map<string, number>()
/** 离屏测量元素：首次测量时创建，之后复用（组件卸载不回收，量文本而已） */
let probe: HTMLSpanElement | null = null

/** 触发器自身的固定开销：左右内边距（10×2）+ 图标（10）+ 间距（6）+ 描边（2） */
const TRIGGER_CHROME = 38
/** 与测量无关、纯为观感留的余量：整宽取偶，避免亚像素裁切 */
const ROUND_SLACK = 4

function measureLabel(label: string): number {
  const cached = measured.get(label)
  if (cached !== undefined) return cached
  if (!probe) {
    probe = document.createElement('span')
    probe.style.cssText = 'position:absolute;top:-9999px;left:-9999px;visibility:hidden;white-space:nowrap;'
    document.body.appendChild(probe)
  }
  probe.style.font = `400 12.5px ${getComputedStyle(document.body).fontFamily}`
  probe.textContent = label
  const width = probe.offsetWidth
  measured.set(label, width)
  return width
}

/**
 * 一组选项文案的「最长者 + 触发器固定开销」，单位像素；尚无法测量（如 SSR）
 * 时返回 0，调用方以 0 为「不设宽度」的信号。宽度按文案内容固定，不随
 * 容器伸缩 —— 窄屏溢出由调用方用 `max-w-full` 兜住。
 */
export function useOptionsWidth(labels: readonly string[]): number {
  // labels 数组通常每次渲染都是新引用，用拼接串做依赖才不会反复测量
  const key = labels.join('\n')
  const [width, setWidth] = React.useState(0)
  React.useLayoutEffect(() => {
    const list = key ? key.split('\n') : []
    if (!list.length) { setWidth(0); return }
    const compute = () => setWidth(Math.ceil(Math.max(...list.map(measureLabel))) + TRIGGER_CHROME + ROUND_SLACK)
    compute()
    // Web 字体（如果将来引入）加载完成前后宽度不同：加载完再校准一次
    document.fonts?.ready.then(compute).catch(() => {})
  }, [key])
  return width
}
