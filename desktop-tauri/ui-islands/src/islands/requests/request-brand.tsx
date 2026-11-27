import * as React from 'react'
import { useProviderIcon } from '../provider-icon'
import { BUILTIN_PROVIDER_ICONS } from '../../../../ui-shared/provider-icons'
import type { RequestIdentity } from '../../../../ui-shared/model-api-key'

const EXTRA_PROVIDER_ICONS: Readonly<Record<string, string>> = {
  anthropic: 'assets/providers/anthropic.png',
  openai: 'assets/providers/openai.png',
  deepseek: 'assets/providers/deepseek.png',
  openrouter: 'assets/providers/openrouter.png',
  siliconflow: 'assets/providers/siliconflow.png',
  groq: 'assets/providers/groq.svg',
  xai: 'assets/providers/xai.svg',
  glm: 'assets/providers/glm.png',
  'glm-cn': 'assets/providers/glm-cn.png',
  kimi: 'assets/providers/kimi.svg',
  minimax: 'assets/providers/minimax.png',
  'minimax-cn': 'assets/providers/minimax-cn.png',
  'ollama-local': 'assets/providers/ollama-local.png',
  ollama: 'assets/providers/ollama-local.png',
  lmstudio: 'assets/providers/lmstudio.svg',
  newapi: 'assets/providers/newapi.png',
  sub2api: 'assets/providers/sub2api.png',
  nvidia: 'assets/providers/nvidia.png',
  'volcengine-ark': 'assets/providers/volcengine-ark.png',
  mistral: 'assets/providers/mistral-color.svg',
  stepfun: 'assets/providers/stepfun-color.svg',
  huggingface: 'assets/providers/huggingface.png',
  aihubmix: 'assets/providers/aihubmix-color.svg',
  tencentcloud: 'assets/providers/tencentcloud-color.svg',
  sensenova: 'assets/providers/sensenova.png',
  alicode: 'assets/providers/alicode.png',
  'alicode-intl': 'assets/providers/alicode-intl.png',
  'alims-intl': 'assets/providers/alims-intl.png',
  'alitp-intl': 'assets/providers/alitp-intl.png',
  beichen: 'assets/providers/beichen.png',
  longcat: 'assets/providers/longcat.svg',
  'opencode-go': 'assets/providers/opencode-go.png',
  'opencode-zen': 'assets/providers/opencode-zen.png',
  'xiaomi-tokenplan': 'assets/providers/xiaomi-tokenplan.png',
}

type ClientIconSpec = {
  src: string
  rounded?: boolean
}

const CLIENT_ICON_MAP: Readonly<Record<string, ClientIconSpec>> = {
  'claude code': { src: 'assets/agents/claude-code.svg' },
  'claude-code': { src: 'assets/agents/claude-code.svg' },
  'claude-cli': { src: 'assets/agents/claude-code.svg' },
  codex: { src: 'assets/agents/codex.png', rounded: true },
  zcode: { src: 'assets/agents/zcode.png', rounded: true },
  'github copilot': { src: 'assets/providers/copilot.svg', rounded: true },
  'github-copilot': { src: 'assets/providers/copilot.svg', rounded: true },
  copilot: { src: 'assets/providers/copilot.svg', rounded: true },
  cursor: { src: 'assets/providers/cursor.svg', rounded: true },
  'cursor-cli': { src: 'assets/agents/cursor-cli.png', rounded: true },
  opencode: { src: 'assets/agents/opencode.svg', rounded: true },
  workbuddy: { src: 'assets/agents/workbuddy.svg', rounded: true },
  codebuddy: { src: 'assets/agents/workbuddy.svg', rounded: true },
  cline: { src: 'assets/agents/cline.png', rounded: true },
  'cline cli': { src: 'assets/agents/cline.png', rounded: true },
  'gemini cli': { src: 'assets/agents/gemini-cli.svg' },
  'gemini-cli': { src: 'assets/agents/gemini-cli.svg' },
  'kimi code': { src: 'assets/agents/kimi-code.png', rounded: true },
  'kimi-code': { src: 'assets/agents/kimi-code.png', rounded: true },
  zed: { src: 'assets/agents/zed.png', rounded: true },
  crush: { src: 'assets/agents/crush.png', rounded: true },
  'deepseek-harness': { src: 'assets/agents/dsh.svg' },
  'deepseek harness': { src: 'assets/agents/dsh.svg' },
  dsh: { src: 'assets/agents/dsh.svg' },
  qoder: { src: 'assets/agents/qoder.png', rounded: true },
  'factory droid': { src: 'assets/providers/factory.svg', rounded: true },
  'factory-droid': { src: 'assets/providers/factory.svg', rounded: true },
  'qwen code': { src: 'assets/providers/qwenwork.svg', rounded: true },
  'qwen-code': { src: 'assets/providers/qwenwork.svg', rounded: true },
  antigravity: { src: 'assets/providers/antigravity.png', rounded: true },
  agy: { src: 'assets/providers/antigravity.png', rounded: true },
  trae: { src: 'assets/providers/trae.png', rounded: true },
}

function resolveClientIcon(entry: RequestIdentity): ClientIconSpec | null {
  const candidates = [
    entry.client,
    entry.clientLabel,
    entry.clientEvidence?.clientId,
    entry.clientEvidence?.clientName,
    entry.clientEvidence?.candidate,
  ]
  for (const raw of candidates) {
    const key = String(raw ?? '').trim().toLowerCase()
    if (!key) continue
    if (CLIENT_ICON_MAP[key]) return CLIENT_ICON_MAP[key]
  }
  return null
}

function FallbackClientGlyph({ client }: { client?: string }) {
  if (client === 'checkin_keepalive') {
    return (
      <svg width='14' height='14' viewBox='0 0 16 16' fill='none' stroke='currentColor' strokeWidth='1.5' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
        <path d='M1.5 8h3l1.7-4.2 2.6 8.4L10.6 8h3.9' />
      </svg>
    )
  }
  if (client === 'model_test') {
    return (
      <svg width='14' height='14' viewBox='0 0 16 16' fill='none' stroke='currentColor' strokeWidth='1.5' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
        <path d='M6 2.5h4M7 2.5v3.2L3.5 12a1.2 1.2 0 0 0 1.05 1.8h6.9A1.2 1.2 0 0 0 12.5 12L9 5.7V2.5' />
      </svg>
    )
  }
  if (client === 'assistant' || client === 'assistant_images' || client === 'assistant_title' || client === 'image-generation') {
    return (
      <svg width='14' height='14' viewBox='0 0 16 16' fill='none' stroke='currentColor' strokeWidth='1.5' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
        <path d='M8 2.2 9.4 6.6 13.8 8 9.4 9.4 8 13.8 6.6 9.4 2.2 8 6.6 6.6 8 2.2Z' />
      </svg>
    )
  }
  return (
    <svg width='14' height='14' viewBox='0 0 16 16' fill='none' stroke='currentColor' strokeWidth='1.5' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
      <rect x='2' y='3' width='12' height='10' rx='2' />
      <path d='m5 6.5 2 1.5-2 1.5M8.5 9.5H11' />
    </svg>
  )
}

function FallbackProviderGlyph({ localDirect = false }: { localDirect?: boolean }) {
  if (localDirect) {
    return (
      <svg width='14' height='14' viewBox='0 0 16 16' fill='none' stroke='currentColor' strokeWidth='1.5' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
        <rect x='2' y='2.5' width='12' height='8.5' rx='1.5' />
        <path d='M5.5 13.5h5M8 11v2.5' />
      </svg>
    )
  }
  return (
    <svg width='14' height='14' viewBox='0 0 16 16' fill='none' stroke='currentColor' strokeWidth='1.5' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
      <circle cx='8' cy='8' r='5.8' />
      <path d='M2.5 8h11M8 2.2a8.5 8.5 0 0 1 0 11.6M8 2.2a8.5 8.5 0 0 0 0 11.6' />
    </svg>
  )
}

export function ClientIcon({
  client,
  label,
  size = 18,
  className = '',
}: {
  client?: string
  label?: string
  size?: number
  className?: string
}) {
  const icon = resolveClientIcon({ client, clientLabel: label })
  const [failed, setFailed] = React.useState(false)
  React.useEffect(() => { setFailed(false) }, [icon?.src])

  if (icon && !failed) {
    return (
      <img
        src={icon.src}
        alt=''
        aria-hidden='true'
        width={size}
        height={size}
        className={`req-brand-logo${icon.rounded ? ' is-rounded' : ''}${className ? ` ${className}` : ''}`}
        style={{ width: size, height: size }}
        onError={() => setFailed(true)}
      />
    )
  }

  return (
    <span
      className={`req-brand-glyph${className ? ` ${className}` : ''}`}
      style={{ width: size, height: size }}
      aria-hidden='true'
    >
      <FallbackClientGlyph client={client} />
    </span>
  )
}

export function ClientBrand({
  entry,
  label,
  title,
}: {
  entry: RequestIdentity
  label: string
  title?: string
}) {
  const icon = resolveClientIcon(entry)
  const [failed, setFailed] = React.useState(false)
  React.useEffect(() => { setFailed(false) }, [icon?.src])

  return (
    <span className='req-brand-cell' title={title || label}>
      {icon && !failed ? (
        <img
          src={icon.src}
          alt=''
          aria-hidden='true'
          className={`req-brand-logo${icon.rounded ? ' is-rounded' : ''}`}
          onError={() => setFailed(true)}
        />
      ) : (
        <span className='req-brand-glyph' aria-hidden='true'>
          <FallbackClientGlyph client={entry.client} />
        </span>
      )}
      <span className='req-brand-text'>{label}</span>
    </span>
  )
}

export function ProviderIcon({
  providerId,
  size = 18,
  className = '',
}: {
  providerId: string
  size?: number
  className?: string
}) {
  const normalized = providerId.trim().toLowerCase()
  const isLocalDirect = normalized === 'local-direct' || normalized === 'local_direct'
  const fallback = isLocalDirect
    ? ''
    : (BUILTIN_PROVIDER_ICONS[providerId]
      || BUILTIN_PROVIDER_ICONS[normalized]
      || EXTRA_PROVIDER_ICONS[normalized]
      || '')
  const { src, onError } = useProviderIcon(isLocalDirect ? '' : providerId, fallback)

  if (src) {
    return (
      <img
        src={src}
        alt=''
        aria-hidden='true'
        width={size}
        height={size}
        className={`req-brand-logo is-rounded${className ? ` ${className}` : ''}`}
        style={{ width: size, height: size }}
        onError={onError}
      />
    )
  }

  return (
    <span
      className={`req-brand-glyph${className ? ` ${className}` : ''}`}
      style={{ width: size, height: size }}
      aria-hidden='true'
    >
      <FallbackProviderGlyph localDirect={isLocalDirect} />
    </span>
  )
}

export function ProviderBrand({
  providerId,
  label,
  title,
}: {
  providerId: string
  label: string
  title?: string
}) {
  return (
    <span className='req-brand-cell' title={title || label}>
      <ProviderIcon providerId={providerId} size={15} />
      <span className='req-brand-text'>{label}</span>
    </span>
  )
}
