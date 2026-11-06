import * as React from 'react'
import { Select as BaseSelect } from '@base-ui/react/select'

function useSelectBoundary(trigger: HTMLButtonElement | null, open: boolean) {
  const [boundary, setBoundary] = React.useState<BaseSelect.Positioner.Props['collisionBoundary']>()

  React.useLayoutEffect(() => {
    if (!open) return
    const dialog = trigger?.closest('[role="dialog"], [role="alertdialog"]')
    const view = trigger?.ownerDocument.defaultView
    if (!trigger || !dialog || !view) {
      setBoundary(undefined)
      return
    }
    const containers: Element[] = [dialog]
    // 只收集实际滚动区，普通卡片的 overflow:hidden 不应把菜单挤到一行高。
    for (let parent = trigger.parentElement; parent && parent !== dialog; parent = parent.parentElement) {
      const style = view.getComputedStyle(parent)
      if (/(auto|scroll)/.test(`${style.overflowX} ${style.overflowY}`)
        || parent.matches('[data-slot="dialog-body"], [data-slot="sheet-body"], [data-slot="alert-dialog-body"]')) {
        containers.push(parent)
      }
    }
    setBoundary(containers)
  }, [trigger, open])

  return boundary
}

export { useSelectBoundary }
