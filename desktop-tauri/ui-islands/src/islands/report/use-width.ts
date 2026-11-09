import * as React from 'react'

/** 图表容器宽度：按容器宽度决定标签疏密的图（热力图、趋势折线）都要量一次。
 *  `fallback` 只是量之前的占位值，useLayoutEffect 在首帧绘制前就会换成真实宽度。 */
export function useWidth(fallback = 680) {
  const ref = React.useRef<HTMLDivElement>(null)
  const [width, setWidth] = React.useState(fallback)
  React.useLayoutEffect(() => {
    const element = ref.current
    if (!element) return
    const update = () => { if (element.clientWidth) setWidth(element.clientWidth) }
    update()
    const observer = new ResizeObserver(update)
    observer.observe(element)
    return () => observer.disconnect()
  }, [])
  return { ref, width }
}
