import * as React from 'react'
import { parsePrompt, type Method, type Prompt } from './add-account-dynamic-fields'

type RequestPrompts = (path: string, body: Record<string, unknown>) => Promise<unknown>

export function usePluginPrompts(provider: string, method: Method | undefined, inputs: Record<string, string>, active: boolean, request: RequestPrompts) {
  const [state, setState] = React.useState<{ key: string; prompts: Prompt[]; pending: boolean; error: string }>({
    key: '', prompts: [], pending: false, error: '',
  })
  const identity = `${provider}:${method?.methodIndex ?? ''}`
  const defaults = Object.fromEntries((method?.prompts || []).filter(prompt => !prompt.secret && prompt.default !== undefined)
    .map(prompt => [prompt.key, prompt.default]))
  const supplied = { ...defaults, ...inputs }
  const key = method?.dynamicPrompts ? JSON.stringify([identity, supplied]) : ''
  const previousIdentity = React.useRef(identity)
  React.useEffect(() => {
    if (!active || !method?.dynamicPrompts) return
    let current = true
    const changed = previousIdentity.current !== identity
    previousIdentity.current = identity
    setState(previous => ({ ...previous, prompts: changed ? [] : previous.prompts, pending: true, error: '' }))
    const timer = window.setTimeout(() => {
      void request('/api/plugins/scripts/prompts', { provider, methodIndex: method.methodIndex, inputs: supplied })
        .then(value => {
          const result = value as { prompts?: unknown[] }
          if (!Array.isArray(result.prompts)) throw new Error('Invalid login fields')
          const prompts = result.prompts.map(parsePrompt)
          if (current) setState(previous => ({ key, pending: false, error: '', prompts: [
            ...previous.prompts.filter(prompt => !method.prompts.some(item => item.key === prompt.key)
              && Object.hasOwn(inputs, prompt.key) && !prompts.some(item => item.key === prompt.key)),
            ...prompts,
          ] }))
        }).catch(error => {
          if (current) setState(previous => ({ ...previous, key, pending: false, error: error instanceof Error ? error.message : String(error) }))
        })
    }, 180)
    return () => { current = false; window.clearTimeout(timer) }
  }, [active, key])
  if (!method?.dynamicPrompts) return { method, pending: false, error: '' }
  return { method: { ...method, prompts: state.prompts }, pending: state.pending || state.key !== key, error: state.error }
}
