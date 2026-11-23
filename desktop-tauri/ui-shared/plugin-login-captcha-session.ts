import * as React from 'react'

type CaptchaFrameMessage = {
  nonce?: unknown
  type?: unknown
  value?: unknown
}

export function useCaptchaPopupSession({
  onChange,
  parseResult,
  onBusyChange,
}: {
  onChange: (value: unknown) => void
  parseResult?: (value: unknown) => unknown
  onBusyChange?: (busy: boolean) => void
}) {
  const frame = React.useRef<HTMLIFrameElement>(null)
  const callbacks = React.useRef({ onChange, parseResult, onBusyChange })
  callbacks.current = { onChange, parseResult, onBusyChange }
  const nonce = React.useId()
  const [open, setOpen] = React.useState(false)
  const [ready, setReady] = React.useState(false)
  const [verified, setVerified] = React.useState(false)
  const [failed, setFailed] = React.useState(false)
  const [attempt, setAttempt] = React.useState(0)

  React.useEffect(() => {
    callbacks.current.onBusyChange?.(open)
  }, [open])

  React.useEffect(() => () => {
    callbacks.current.onBusyChange?.(false)
    callbacks.current.onChange(null)
  }, [])

  React.useEffect(() => {
    if (!open) return
    const closeWith = ({
      nextVerified,
      nextFailed,
      nextValue,
    }: {
      nextVerified: boolean
      nextFailed: boolean
      nextValue: unknown
    }) => {
      setOpen(false)
      setReady(false)
      setVerified(nextVerified)
      setFailed(nextFailed)
      callbacks.current.onChange(nextValue)
    }

    const receive = (event: MessageEvent<CaptchaFrameMessage>) => {
      if (event.source !== frame.current?.contentWindow || event.origin !== 'null' || event.data?.nonce !== nonce) return
      const kind = event.data.type
      if (kind === 'ready') {
        setReady(true)
        frame.current?.contentWindow?.postMessage({ nonce, type: 'start' }, '*')
        return
      }
      if (kind === 'verified') {
        const parsed = callbacks.current.parseResult
          ? callbacks.current.parseResult(event.data.value)
          : event.data.value
        if (!parsed) {
          closeWith({ nextVerified: false, nextFailed: true, nextValue: null })
          return
        }
        closeWith({ nextVerified: true, nextFailed: false, nextValue: parsed })
        return
      }
      if (kind === 'close' || kind === 'reset' || kind === 'error') {
        closeWith({ nextVerified: false, nextFailed: kind === 'error', nextValue: null })
      }
    }

    const timeout = window.setTimeout(() => {
      closeWith({ nextVerified: false, nextFailed: true, nextValue: null })
    }, 180000)

    window.addEventListener('message', receive)
    return () => {
      window.clearTimeout(timeout)
      window.removeEventListener('message', receive)
    }
  }, [nonce, open])

  const start = React.useCallback(() => {
    callbacks.current.onChange(null)
    setVerified(false)
    setFailed(false)
    setReady(false)
    setAttempt(value => value + 1)
    setOpen(true)
  }, [])

  const dismiss = React.useCallback(() => {
    setOpen(false)
    setReady(false)
    setVerified(false)
    callbacks.current.onChange(null)
  }, [])

  return {
    frame,
    nonce,
    open,
    ready,
    verified,
    failed,
    attempt,
    start,
    dismiss,
  }
}
