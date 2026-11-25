import * as React from 'react'
import {
  Badge,
  Checkbox,
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
  Label,
} from '@ui'

type Translate = (key: string, vars?: Record<string, string | number>) => string

function LinkGlyph() {
  return (
    <svg width='13' height='13' viewBox='0 0 24 24' fill='none' stroke='currentColor' strokeWidth='1.8' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
      <path d='M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71' />
      <path d='M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71' />
    </svg>
  )
}

function CopyGlyph() {
  return (
    <svg width='13' height='13' viewBox='0 0 24 24' fill='none' stroke='currentColor' strokeWidth='1.8' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
      <rect x='9' y='9' width='13' height='13' rx='2' />
      <path d='M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1' />
    </svg>
  )
}

function CheckGlyph() {
  return (
    <svg width='13' height='13' viewBox='0 0 24 24' fill='none' stroke='currentColor' strokeWidth='2' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
      <path d='M20 6 9 17l-5-5' />
    </svg>
  )
}

function OpenGlyph() {
  return (
    <svg width='13' height='13' viewBox='0 0 24 24' fill='none' stroke='currentColor' strokeWidth='1.8' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
      <path d='M15 3h6v6' />
      <path d='M10 14 21 3' />
      <path d='M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6' />
    </svg>
  )
}

async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text)
      return true
    }
  } catch {
    // 局域网 HTTP 等非安全上下文走降级复制
  }
  try {
    const area = document.createElement('textarea')
    area.value = text
    area.setAttribute('readonly', '')
    area.style.position = 'fixed'
    area.style.opacity = '0'
    area.style.pointerEvents = 'none'
    document.body.appendChild(area)
    area.focus()
    area.select()
    const ok = document.execCommand('copy')
    area.remove()
    return ok
  } catch {
    return false
  }
}

export function PluginLoginExternalLink({
  id,
  url,
  placeholder,
  browserOpened,
  disabled,
  opening = false,
  showNoAutoOpen = true,
  noAutoOpen = false,
  onNoAutoOpenChange,
  onOpenExternal,
  onCopied,
  t,
}: {
  id: string
  url: string
  placeholder?: string
  browserOpened?: boolean
  disabled?: boolean
  opening?: boolean
  showNoAutoOpen?: boolean
  noAutoOpen?: boolean
  onNoAutoOpenChange?: (next: boolean) => void
  onOpenExternal?: (url: string) => void | Promise<void>
  onCopied?: (ok: boolean) => void
  t: Translate
}): React.ReactElement {
  const inputRef = React.useRef<HTMLInputElement>(null)
  const copyTimer = React.useRef<number | null>(null)
  const [copied, setCopied] = React.useState(false)
  const hasUrl = Boolean(url)

  React.useEffect(() => {
    setCopied(false)
    if (inputRef.current) inputRef.current.scrollLeft = 0
  }, [url])

  React.useEffect(() => () => {
    if (copyTimer.current !== null) window.clearTimeout(copyTimer.current)
  }, [])

  async function handleCopy(): Promise<void> {
    if (!url) return
    const ok = await copyText(url)
    if (ok) {
      setCopied(true)
      if (copyTimer.current !== null) window.clearTimeout(copyTimer.current)
      copyTimer.current = window.setTimeout(() => setCopied(false), 2000)
    } else {
      inputRef.current?.focus()
      inputRef.current?.select()
    }
    onCopied?.(ok)
  }

  function handleWheel(event: React.WheelEvent<HTMLInputElement>): void {
    const el = event.currentTarget
    if (el.scrollWidth <= el.clientWidth) return
    if (Math.abs(event.deltaY) > Math.abs(event.deltaX)) {
      event.preventDefault()
      el.scrollLeft += event.deltaY
    }
  }

  return (
    <div className='plugin-login-external-box'>
      <div className='plugin-login-link-head'>
        <Label htmlFor={id}>{t('授权链接')}</Label>
        {hasUrl && browserOpened !== undefined ? (
          <Badge variant={browserOpened ? 'info' : 'secondary'} shape='tag'>
            {browserOpened ? t('已唤起浏览器') : t('待手动打开')}
          </Badge>
        ) : null}
      </div>

      <InputGroup className='plugin-login-link-group' data-disabled={!hasUrl || undefined}>
        <InputGroupAddon align='inline-start'>
          <LinkGlyph />
        </InputGroupAddon>
        <InputGroupInput
          ref={inputRef}
          id={id}
          type='text'
          readOnly
          spellCheck={false}
          autoComplete='off'
          autoCapitalize='none'
          className='plugin-login-link-input'
          value={url}
          placeholder={placeholder || t('点击下方按钮后生成本次授权链接')}
          title={hasUrl ? t('只读链接，支持左右滚动或点击右侧按钮复制') : undefined}
          onWheel={handleWheel}
          onFocus={event => {
            if (url) event.currentTarget.select()
          }}
        />
        <InputGroupAddon align='inline-end' className='plugin-login-link-actions'>
          {hasUrl && onOpenExternal ? (
            <>
              <InputGroupButton
                size='icon-xs'
                disabled={opening}
                className='plugin-login-link-btn'
                title={t('在默认浏览器中打开')}
                aria-label={t('在默认浏览器中打开')}
                onClick={() => void onOpenExternal(url)}
              >
                <OpenGlyph />
              </InputGroupButton>
              <span className='plugin-login-link-sep' aria-hidden='true' />
            </>
          ) : null}
          <InputGroupButton
            size='xs'
            disabled={!hasUrl}
            className={`plugin-login-link-btn${copied ? ' is-copied' : hasUrl ? ' is-primary-action' : ''}`}
            onClick={() => void handleCopy()}
          >
            {copied ? <CheckGlyph /> : <CopyGlyph />}
            {copied ? t('已复制') : t('复制')}
          </InputGroupButton>
        </InputGroupAddon>
      </InputGroup>

      {showNoAutoOpen && onNoAutoOpenChange ? (
        <div className='plugin-login-auto-open-row'>
          <Label className='plugin-login-checkbox-label' data-disabled={disabled ? 'true' : undefined}>
            <Checkbox
              checked={noAutoOpen}
              disabled={disabled}
              onCheckedChange={next => onNoAutoOpenChange(next === true)}
            />
            <span>{t('不自动打开浏览器')}</span>
          </Label>
        </div>
      ) : null}
    </div>
  )
}
