export function createRefreshRequest(run: (isCurrent: () => boolean) => Promise<void>): (afterChange?: boolean) => Promise<void> {
  let inFlight: Promise<void> | null = null
  let revision = 0

  return (afterChange = false) => {
    if (afterChange) revision += 1
    if (inFlight) return inFlight
    inFlight = Promise.resolve().then(async () => {
      try {
        for (;;) {
          const current = revision
          try {
            await run(() => current === revision)
          } catch (error) {
            if (current === revision) throw error
          }
          if (current === revision) return
        }
      } finally {
        inFlight = null
      }
    })
    return inFlight
  }
}
