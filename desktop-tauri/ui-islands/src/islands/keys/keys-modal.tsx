import * as React from 'react'
import {
  Button,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogSection,
  DialogTitle,
  Input,
  Label,
  MultiSelect,
  type MultiSelectOption,
} from '@ui'
import { t } from '../../i18n'
import {
  draftFromLimit,
  errorMessage,
  escapeHtml,
  parseUsageDraft,
  providerLabel,
  shared,
  stringList,
  toast,
  type KeyEntry,
  type KeysPayload,
  type ProviderOption,
} from './keys-types'
import { UsageLimitEditor } from './keys-usage-limit'

function modelOptions(
  table: Record<string, string[]>,
  selectedProviders: string[],
  selected: string[],
): MultiSelectOption[] {
  const names = new Map<string, string>()
  const put = (value: unknown) => {
    const text = String(value ?? '').trim()
    if (!text) return
    const key = text.toLowerCase()
    if (!names.has(key)) names.set(key, text)
  }
  selectedProviders.forEach(id => {
    const list = table[id] ?? table[String(id).toLowerCase()]
    if (Array.isArray(list)) list.forEach(put)
  })
  selected.forEach(put)
  const out = [...names.values()].map(value => ({ value, label: t(value) }))
  out.sort((a, b) => (a.value.toLowerCase() < b.value.toLowerCase() ? -1 : 1))
  return out
}

function pickedFromOptions(values: readonly string[], options: readonly MultiSelectOption[]): string[] {
  const canonical = new Map(options.map(option => [option.value.toLowerCase(), option.value]))
  const chosen = new Set<string>()
  values.forEach(value => {
    const hit = canonical.get(String(value).toLowerCase())
    if (hit) chosen.add(hit)
  })
  return options.filter(option => chosen.has(option.value)).map(option => option.value)
}

function restrictionSummary(providers: readonly string[], models: readonly string[]): string {
  if (!providers.length && !models.length) {
    return t('当前不限制：这把 Key 可以用全部提供商与全部对外模型')
  }
  const parts: string[] = []
  if (providers.length) parts.push(t('提供商：{names}', { names: providers.map(providerLabel).join(t('、')) }))
  if (models.length) parts.push(t('模型：{names}', { names: models.join(t('、')) }))
  if (!providers.length) {
    parts.push(t('（模型候选需先选提供商；不选则沿用当前这几项，保存后仍按模型白名单生效）'))
  }
  return parts.join('　')
}

export type KeyModalProps = {
  /** 编辑形态的 Key；null = 新建 */
  target: KeyEntry | null
  providers: ProviderOption[]
  modelsByProvider: Record<string, string[]>
  onClose: () => void
  onSaved: (next: KeysPayload | null | undefined, revealId?: string) => void
}

export function KeyModal({ target, providers, modelsByProvider, onClose, onSaved }: KeyModalProps) {
  const editingId = target?.id ?? null
  const editing = Boolean(editingId)

  const [currentTarget, setCurrentTarget] = React.useState<KeyEntry | null>(target)
  const [name, setName] = React.useState(target?.name ?? '')
  const [keyValue, setKeyValue] = React.useState('')
  const [status, setStatus] = React.useState('')
  const [saving, setSaving] = React.useState(false)
  const [resetting, setResetting] = React.useState(false)
  const savingRef = React.useRef(false)

  const [pickedProviders, setPickedProviders] = React.useState<string[]>(
    () => stringList(target?.allowedProviders),
  )
  const [pickedModels, setPickedModels] = React.useState<string[]>(
    () => stringList(target?.allowedModels),
  )
  const [usageDraft, setUsageDraft] = React.useState(() => draftFromLimit(target?.usageLimit))

  const providerOptions = React.useMemo<MultiSelectOption[]>(
    () => providers.map(item => ({ value: String(item.id), label: t(String(item.label ?? item.id)) })),
    [providers],
  )

  const allowedProviders = React.useMemo(
    () => pickedFromOptions(pickedProviders, providerOptions),
    [pickedProviders, providerOptions],
  )

  const modelCandidates = React.useMemo(
    () => modelOptions(modelsByProvider, allowedProviders, pickedModels),
    [modelsByProvider, allowedProviders, pickedModels],
  )

  const allowedModels = React.useMemo(
    () => pickedFromOptions(pickedModels, modelCandidates),
    [pickedModels, modelCandidates],
  )

  const modelEmptyHint = modelCandidates.length
    ? t('没有匹配的选项')
    : allowedProviders.length
      ? t('这几家当前没有可用模型（账号未登录或清单为空）')
      : t('请先在上面选择可用提供商')

  function stopSaving(): void {
    savingRef.current = false
    setSaving(false)
  }

  async function handleResetUsage(): Promise<void> {
    if (!editingId || resetting || savingRef.current) return
    const api = shared().workbuddyDesktop
    const ask = shared().wbConfirm?.ask
    if (!api?.resetKeyUsage) return
    const label = escapeHtml(name.trim() || currentTarget?.masked || editingId)
    if (ask) {
      const confirmed = await ask({
        title: t('清零 Key 用量'),
        html: t('确定将 Key「<strong>{name}</strong>」的当前周期与累计用量全部清零？清零后不会影响历史请求日志与报表。', { name: label }),
        okText: t('清零'),
      })
      if (!confirmed) return
    }
    setResetting(true)
    setStatus('')
    try {
      const next = await api.resetKeyUsage(editingId)
      onSaved(next)
      const refreshed = next?.keys?.find(item => item.id === editingId) ?? null
      if (refreshed) setCurrentTarget(refreshed)
      toast(t('✅ Key 用量已清零'))
    } catch (error) {
      setStatus(t('清零失败：{reason}', { reason: errorMessage(error) }))
    } finally {
      setResetting(false)
    }
  }

  async function save(): Promise<void> {
    if (savingRef.current) return
    const parsedUsage = parseUsageDraft(usageDraft)
    if (!parsedUsage.ok) {
      setStatus(parsedUsage.error)
      return
    }
    if (!editingId) {
      const trimmedKey = keyValue.trim()
      if (trimmedKey && trimmedKey.length < 8) {
        setStatus(t('Key 至少需要 8 个字符'))
        return
      }
    }
    savingRef.current = true
    setSaving(true)
    setStatus(t('保存中…'))
    try {
      const api = shared().workbuddyDesktop
      if (!api) throw new Error(t('后端桥不可用'))
      if (editingId) {
        const next = await api.updateKey(editingId, {
          name: name.trim(),
          allowedProviders,
          allowedModels,
          usageLimit: parsedUsage.value,
        })
        stopSaving()
        onSaved(next)
        onClose()
        toast(t('✅ Key 设置已保存'))
        return
      }
      const trimmedKey = keyValue.trim()
      const next = await api.createKey({
        name: name.trim(),
        key: trimmedKey || undefined,
        allowedProviders,
        allowedModels,
        usageLimit: parsedUsage.value,
      })
      stopSaving()
      onSaved(next, next?.created?.id)
      onClose()
      toast(t('✅ Key 已创建，记得复制给客户端'))
    } catch (error) {
      setStatus(t('保存失败：{reason}', { reason: errorMessage(error) }))
    } finally {
      stopSaving()
    }
  }

  return (
    <Dialog
      open
      onOpenChange={(next, eventDetails) => {
        if (next) return
        if (savingRef.current || resetting) {
          eventDetails.cancel()
          return
        }
        onClose()
      }}
    >
      <DialogContent className='w-[min(580px,calc(100vw-48px))]'>
        <DialogHeader>
          <DialogTitle>
            {editing
              ? t('编辑 Key · {name}', { name: currentTarget?.name || t('未命名') })
              : t('新建 API Key')}
          </DialogTitle>
        </DialogHeader>
        <DialogBody>
          <DialogSection>
            <div className='flex flex-wrap items-center gap-2.5'>
              <Label htmlFor='key-name' className='text-[12.5px] whitespace-nowrap text-subtle'>{t('名称')}</Label>
              <Input
                id='key-name'
                type='text'
                maxLength={60}
                placeholder={t('例如 Cursor / 公司电脑')}
                autoComplete='off'
                value={name}
                disabled={saving}
                onChange={event => setName(event.currentTarget.value)}
              />
            </div>
            {!editing && (
              <div className='flex flex-wrap items-center gap-2.5'>
                <Label htmlFor='key-value' className='text-[12.5px] whitespace-nowrap text-subtle'>Key</Label>
                <Input
                  id='key-value'
                  type='text'
                  placeholder={t('留空自动生成；手填至少 8 个字符')}
                  autoComplete='off'
                  spellCheck={false}
                  value={keyValue}
                  disabled={saving}
                  onChange={event => setKeyValue(event.currentTarget.value)}
                  onKeyDown={event => { if (event.key === 'Enter') void save() }}
                />
              </div>
            )}
            <div className='flex flex-wrap items-center gap-2.5'>
              <Label className='text-[12.5px] whitespace-nowrap text-subtle'>{t('可用提供商')}</Label>
              <MultiSelect
                value={allowedProviders}
                onValueChange={setPickedProviders}
                options={providerOptions}
                placeholder={t('留空 = 不限制')}
                searchPlaceholder={t('搜索提供商…')}
                aria-label={t('可用提供商')}
                className='flex-auto min-w-[180px] max-w-[260px]'
              />
            </div>
            <div className='flex flex-wrap items-center gap-2.5'>
              <Label className='text-[12.5px] whitespace-nowrap text-subtle'>{t('可用模型')}</Label>
              <MultiSelect
                value={allowedModels}
                onValueChange={setPickedModels}
                options={modelCandidates}
                placeholder={t('留空 = 不限制')}
                emptyHint={modelEmptyHint}
                searchPlaceholder={t('搜索模型…')}
                aria-label={t('可用模型')}
                className='flex-auto min-w-[180px] max-w-[260px]'
              />
            </div>
            <p>
              {t('留空表示不限制；同时设置时请求需同时满足两个条件（模型在白名单内且路由到允许的提供商）。 「可用模型」的候选跟着上面勾选的提供商走：没勾提供商时它是空的（还没有约束范围）， 勾了几家就列出这几家能收的全部对外名。')}
            </p>
            <p id='key-restrict-summary'>{restrictionSummary(allowedProviders, allowedModels)}</p>
          </DialogSection>

          <DialogSection>
            <UsageLimitEditor
              draft={usageDraft}
              onChange={next => {
                setUsageDraft(next)
                setStatus('')
              }}
              target={currentTarget}
              saving={saving}
              resetting={resetting}
              onResetUsage={editing ? () => void handleResetUsage() : undefined}
            />
          </DialogSection>

          <div className='min-h-[18px] text-[11.5px] text-muted-foreground'>{status}</div>
        </DialogBody>
        <DialogFooter>
          <div className='mr-auto' />
          <Button variant='outline' onClick={onClose} disabled={saving || resetting}>
            {t('取消')}
          </Button>
          <Button variant='default' disabled={saving || resetting} onClick={() => void save()}>
            {editing ? t('保存') : t('创建')}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
