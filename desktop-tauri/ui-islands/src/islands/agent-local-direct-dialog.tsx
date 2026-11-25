import * as React from 'react'
import {
  Alert, Badge, Button, Dialog, DialogBody, DialogContent,
  DialogFooter, DialogHeader, DialogTitle, Input,
} from '@ui'
import { t } from '../i18n'
import { AgentIcon } from './agent-actions'
import { errorMessage, isDesktopEnvironment } from './agent-state'

export type LocalDirectRoot = {
  path: string
  kind: string
  kindLabel: string
  custom: boolean
  exists: boolean
}

export type LocalDirectAgentStatus = {
  id: string
  name: string
  description: string
  enabled: boolean
  detected: boolean
  detectedRoots: LocalDirectRoot[]
  customPaths: string[]
  requests: number
  totalTokens: number
  lastTs: number | null
}

type BridgeWithFolder = {
  workbuddyDesktop?: {
    pickFolder?: (defaultDir?: string | null) => Promise<{
      canceled?: boolean
      unsupported?: boolean
      path?: string
    }>
  }
}

export function LocalDirectPathDialog({
  agent,
  onSave,
  onClose,
}: {
  agent: LocalDirectAgentStatus
  onSave: (agentId: string, customPaths: string[]) => Promise<void>
  onClose: () => void
}) {
  const [paths, setPaths] = React.useState<string[]>(() => [...(agent.customPaths || [])])
  const [draft, setDraft] = React.useState('')
  const [picking, setPicking] = React.useState(false)
  const [saving, setSaving] = React.useState(false)
  const [error, setError] = React.useState('')
  const desktop = isDesktopEnvironment()
  const detectedRoots = (agent.detectedRoots || []).filter(r => !r.custom)

  function addCandidate(raw: string) {
    const clean = raw.trim()
    if (!clean) return
    if (paths.some(item => item.toLowerCase() === clean.toLowerCase())) {
      setDraft('')
      return
    }
    setPaths(prev => [...prev, clean])
    setDraft('')
    setError('')
  }

  async function browseFolder() {
    if (picking || saving) return
    const picker = (window as unknown as BridgeWithFolder).workbuddyDesktop?.pickFolder
    if (!picker) {
      setError(t('当前网页环境暂不支持系统文件夹选择，请直接粘贴路径'))
      return
    }
    setPicking(true)
    setError('')
    try {
      const picked = await picker(draft.trim() || paths[0] || detectedRoots[0]?.path || null)
      if (picked?.unsupported) {
        setError(t('当前环境不支持弹窗选目录，请在上方输入框粘贴目录路径'))
      } else if (!picked?.canceled && picked?.path) {
        addCandidate(picked.path)
      }
    } catch (err) {
      setError(errorMessage(err))
    } finally {
      setPicking(false)
    }
  }

  async function handleSave() {
    if (saving || picking) return
    const pending = draft.trim()
    const next = pending && !paths.some(item => item.toLowerCase() === pending.toLowerCase())
      ? [...paths, pending]
      : paths
    setSaving(true)
    setError('')
    try {
      await onSave(agent.id, next)
      onClose()
    } catch (err) {
      setError(errorMessage(err))
      setSaving(false)
    }
  }

  return (
    <Dialog open onOpenChange={(open, details) => { if (!open) { if (saving || picking) details.cancel(); else onClose() } }}>
      <DialogContent className='max-w-[540px]'>
        <DialogHeader>
          <DialogTitle>{t('{name} · 数据目录设置', { name: agent.name })}</DialogTitle>
        </DialogHeader>
        <DialogBody className='gap-4'>
          <div className='flex flex-col gap-1.5'>
            <div className='text-xs font-medium text-foreground'>{t('自动识别的本机目录')}</div>
            {detectedRoots.length ? (
              <div className='flex flex-col gap-1.5'>
                {detectedRoots.map(root => (
                  <div key={root.path} className='flex items-center justify-between gap-2 rounded-md border border-border bg-surface-2 px-2.5 py-1.5 text-xs'>
                    <span className='min-w-0 truncate font-mono text-[11.5px] text-foreground' title={root.path}>{root.path}</span>
                    <Badge variant={root.exists ? 'success' : 'outline'} shape='tag' className='shrink-0'>
                      {root.kindLabel}
                    </Badge>
                  </div>
                ))}
              </div>
            ) : (
              <div className='rounded-md border border-dashed border-border px-3 py-2 text-xs text-muted-foreground'>
                {t('未在默认位置检测到 {name} 目录，可在下方补充自定义数据目录', { name: agent.name })}
              </div>
            )}
          </div>

          <div className='flex flex-col gap-2'>
            <div className='text-xs font-medium text-foreground'>{t('自定义目录')}</div>
            <div className='flex items-center gap-2'>
              <Input
                value={draft}
                disabled={saving || picking}
                placeholder={t('例如：C:\\Users\\你的名字\\.gemini\\antigravity')}
                onChange={event => setDraft(event.currentTarget.value)}
                onKeyDown={event => {
                  if (event.key === 'Enter') {
                    event.preventDefault()
                    addCandidate(draft)
                  }
                }}
              />
              <Button type='button' variant='outline' size='sm' disabled={saving || picking || !draft.trim()} onClick={() => addCandidate(draft)}>
                {t('添加')}
              </Button>
              {desktop ? (
                <Button type='button' variant='outline' size='sm' disabled={saving || picking} onClick={() => void browseFolder()}>
                  <AgentIcon name='folder' />
                  {picking ? t('选择中…') : t('选择文件夹')}
                </Button>
              ) : null}
            </div>

            {paths.length ? (
              <div className='flex flex-col gap-1.5'>
                {paths.map(path => {
                  const status = agent.detectedRoots?.find(r => r.custom && r.path === path)
                  const missing = status && !status.exists
                  return (
                    <div key={path} className='flex items-center justify-between gap-2 rounded-md border border-border bg-surface px-2.5 py-1.5 text-xs'>
                      <span className='min-w-0 truncate font-mono text-[11.5px]' title={path}>{path}</span>
                      <div className='flex shrink-0 items-center gap-1.5'>
                        {missing ? <Badge variant='warning' shape='tag'>{t('目录不存在')}</Badge> : null}
                        <Button
                          type='button'
                          variant='ghost'
                          size='xs'
                          disabled={saving}
                          onClick={() => setPaths(prev => prev.filter(item => item !== path))}
                        >
                          {t('移除')}
                        </Button>
                      </div>
                    </div>
                  )
                })}
              </div>
            ) : (
              <p className='text-[11.5px] text-muted-foreground'>
                {t('支持填 Antigravity 数据根目录或其中的 conversations / brain 子目录')}
              </p>
            )}
          </div>

          {error ? <Alert variant='destructive'>{error}</Alert> : null}
        </DialogBody>
        <DialogFooter>
          <Button type='button' variant='outline' disabled={saving || picking} onClick={onClose}>
            {t('取消')}
          </Button>
          <Button type='button' disabled={saving || picking} onClick={() => void handleSave()}>
            {saving ? t('保存中…') : t('保存')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
