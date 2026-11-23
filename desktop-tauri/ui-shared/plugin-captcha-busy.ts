const busyOwners = new Set<symbol>()
const busyListeners = new Set<() => void>()

export const subscribeCaptchaBusy = (listener: () => void) => {
  busyListeners.add(listener)
  return () => {
    busyListeners.delete(listener)
  }
}

export const isCaptchaBusy = () => busyOwners.size > 0

export function setCaptchaBusy(owner: symbol, busy: boolean): void {
  const had = busyOwners.has(owner)
  if (busy === had) return
  if (busy) busyOwners.add(owner)
  else busyOwners.delete(owner)
  busyListeners.forEach(listener => listener())
}
