import * as React from 'react'
import {
  Alert, Badge, Button, Dialog, DialogBody, DialogContent, DialogFooter,
  DialogHeader, DialogTitle, Input, Label, Select, SelectContent, SelectItem,
  SelectTrigger, SelectValue, Switch, Table, TableBody, TableCell, TableHead,
  TableHeader, TableRow, Textarea,
} from '@ui'
import { t } from '../i18n'
import {
  privacyActionLabel, privacyKindLabel, sanitizeKindInput, splitTerms,
  validatePrivacyRule, type PrivacyAction, type PrivacyCustomRule,
  type PrivacyLimits, type PrivacyPreviewResult, type PrivacyState,
} from './settings-privacy-model'

type RuleEditorProps = {
  rule: PrivacyCustomRule
  isNew: boolean
  limits: PrivacyLimits
  onApply: (rule: PrivacyCustomRule) => void
  onClose: () => void
}

function PrivacyRuleEditorDialog({ rule, isNew, limits, onApply, onClose }: RuleEditorProps) {
  const [draft, setDraft] = React.useState(rule)
  const [error, setError] = React.useState('')

  const matchTypes = [
    { value: 'prefix', label: t('密钥前缀') },
    { value: 'regex', label: t('正则表达式') },
  ]
  const actionOptions = [
    { value: 'default', label: t('跟随全局处理方式') },
    { value: 'mask', label: t('遮罩替换') },
    { value: 'warn', label: t('仅提醒') },
    { value: 'block', label: t('拦截请求') },
  ]

  function update(patch: Partial<PrivacyCustomRule>): void {
    setDraft(current => ({ ...current, ...patch }))
    setError('')
  }

  function apply(): void {
    const message = validatePrivacyRule(draft, limits)
    if (message) {
      setError(message)
      return
    }
    onApply({
      ...draft,
      name: draft.name.trim(),
      kind: sanitizeKindInput(draft.kind, limits.maxKindChars),
      pattern: draft.pattern.trim(),
    })
  }

  const matchLabel = matchTypes.find(item => item.value === draft.matchType)?.label ?? t('密钥前缀')
  const actionVal = draft.action ?? 'default'
  const actionLabel = actionOptions.find(item => item.value === actionVal)?.label ?? t('跟随全局处理方式')

  return (
    <Dialog open onOpenChange={open => { if (!open) onClose() }}>
      <DialogContent overlayForceRender className='w-[min(640px,calc(100vw-48px))]'>
        <DialogHeader>
          <DialogTitle>{isNew ? t('新增识别规则') : t('编辑识别规则')}</DialogTitle>
        </DialogHeader>
        <DialogBody>
          <div className='grid grid-cols-1 gap-3 sm:grid-cols-2'>
            <div className='flex flex-col gap-1.5'>
              <Label htmlFor='privacy-rule-name'>{t('规则名称')}</Label>
              <Input id='privacy-rule-name' value={draft.name}
                placeholder={t('例如：内部访问令牌')}
                onChange={event => update({ name: event.currentTarget.value })} />
            </div>
            <div className='flex flex-col gap-1.5'>
              <Label htmlFor='privacy-rule-kind'>{t('占位符类别名')}</Label>
              <Input id='privacy-rule-kind' value={draft.kind}
                placeholder='CUSTOM'
                onChange={event => update({ kind: event.currentTarget.value })} />
              <div className='hint'>{t('仅用大写字母、数字或下划线；用于生成 {{类别_xxxxxxxx}}')}</div>
            </div>
          </div>
          <div className='flex flex-wrap items-center gap-3'>
            <Label htmlFor='privacy-rule-type'>{t('匹配方式')}</Label>
            <Select value={draft.matchType} onValueChange={value => {
              if (value === 'prefix' || value === 'regex') update({ matchType: value })
            }}>
              <SelectTrigger id='privacy-rule-type' className='w-[150px]'>
                <SelectValue>{matchLabel}</SelectValue>
              </SelectTrigger>
              <SelectContent>
                {matchTypes.map(item => <SelectItem key={item.value} value={item.value}>{item.label}</SelectItem>)}
              </SelectContent>
            </Select>
            <Label htmlFor='privacy-rule-action'>{t('命中动作')}</Label>
            <Select value={actionVal} onValueChange={value => {
              if (value === 'default') update({ action: undefined })
              else if (value === 'mask' || value === 'warn' || value === 'block') update({ action: value })
            }}>
              <SelectTrigger id='privacy-rule-action' className='w-[170px]'>
                <SelectValue>{actionLabel}</SelectValue>
              </SelectTrigger>
              <SelectContent>
                {actionOptions.map(item => <SelectItem key={item.value} value={item.value}>{item.label}</SelectItem>)}
              </SelectContent>
            </Select>
            <Label htmlFor='privacy-rule-enabled' className='ml-auto'>{t('启用')}</Label>
            <Switch id='privacy-rule-enabled' checked={draft.enabled}
              onCheckedChange={enabled => update({ enabled })} />
          </div>
          <div className='flex flex-col gap-1.5'>
            <Label htmlFor='privacy-rule-pattern'>
              {draft.matchType === 'prefix' ? t('密钥前缀') : t('正则表达式')}
            </Label>
            <Textarea id='privacy-rule-pattern' rows={3} spellCheck={false} value={draft.pattern}
              placeholder={draft.matchType === 'prefix' ? 'corp_sec_' : '(?i)internal_secret_[a-z0-9]{12,}'}
              onChange={event => update({ pattern: event.currentTarget.value })} />
            <div className='hint'>{draft.matchType === 'prefix'
              ? t('输入前缀后，网关会自动匹配该前缀及后面至少 8 位的字母/数字/下划线字符。')
              : t('保存或测试时由后端校验语法；不支持环视和反向引用。')}</div>
          </div>
          {error ? <Alert variant='destructive' role='alert' className='text-xs text-destructive'>{error}</Alert> : null}
        </DialogBody>
        <DialogFooter>
          <Button variant='outline' onClick={onClose}>{t('取消')}</Button>
          <Button onClick={apply}>{t('确定')}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

export function PrivacyCustomRulesSection({
  rules,
  limits,
  disabled,
  onChange,
}: {
  rules: PrivacyCustomRule[]
  limits: PrivacyLimits
  disabled: boolean
  onChange: (rules: PrivacyCustomRule[]) => void
}) {
  const [editor, setEditor] = React.useState<{ rule: PrivacyCustomRule; isNew: boolean } | null>(null)

  function addRule(): void {
    setEditor({
      isNew: true,
      rule: {
        id: `priv-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`,
        name: '',
        kind: 'CUSTOM',
        enabled: true,
        matchType: 'prefix',
        pattern: '',
      },
    })
  }

  return (
    <div className='flex flex-col gap-2 rounded-md border border-border p-3'>
      <div className='flex flex-wrap items-center gap-2'>
        <h3 className='text-[12.5px] font-semibold'>{t('自定义识别规则')}</h3>
        <span className='hint'>{t('支持密钥前缀或正则表达式，已添加 {count} 条', { count: rules.length })}</span>
        <Button id='btn-privacy-rule-add' variant='outline' size='sm' className='ml-auto'
          disabled={disabled || rules.length >= limits.maxCustomRules} onClick={addRule}>
          {t('新增规则')}
        </Button>
      </div>
      {rules.length ? (
        <Table containerClassName='max-h-[260px] rounded-md border border-border'>
          <TableHeader><TableRow>
            <TableHead>{t('启用')}</TableHead>
            <TableHead>{t('名称 / 类别')}</TableHead>
            <TableHead>{t('匹配表达式')}</TableHead>
            <TableHead>{t('动作')}</TableHead>
            <TableHead className='text-right'>{t('操作')}</TableHead>
          </TableRow></TableHeader>
          <TableBody>
            {rules.map(rule => (
              <TableRow key={rule.id}>
                <TableCell>
                  <Switch checked={rule.enabled} disabled={disabled}
                    aria-label={t('启用规则：{name}', { name: rule.name })}
                    onCheckedChange={enabled => onChange(rules.map(item => item.id === rule.id ? { ...item, enabled } : item))} />
                </TableCell>
                <TableCell>
                  <div className='text-xs font-medium'>{rule.name}</div>
                  <div className='mt-0.5 flex flex-wrap gap-1'>
                    <Badge variant='outline'>{rule.matchType === 'prefix' ? t('密钥前缀') : t('正则表达式')}</Badge>
                    <Badge variant='outline'>{rule.kind}</Badge>
                  </div>
                </TableCell>
                <TableCell>
                  <span className='line-clamp-2 max-w-[240px] break-all font-mono text-xs'>{rule.pattern}</span>
                </TableCell>
                <TableCell>
                  <Badge variant='outline'>{rule.action ? privacyActionLabel(rule.action) : t('跟随全局')}</Badge>
                </TableCell>
                <TableCell>
                  <div className='flex justify-end gap-1'>
                    <Button variant='outline' size='xs' disabled={disabled}
                      onClick={() => setEditor({ rule, isNew: false })}>{t('编辑')}</Button>
                    <Button variant='destructive' size='xs' disabled={disabled}
                      onClick={() => onChange(rules.filter(item => item.id !== rule.id))}>{t('删除')}</Button>
                  </div>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      ) : null}
      {editor ? (
        <PrivacyRuleEditorDialog
          rule={editor.rule}
          isNew={editor.isNew}
          limits={limits}
          onClose={() => setEditor(null)}
          onApply={nextRule => {
            onChange(editor.isNew
              ? [...rules, nextRule]
              : rules.map(item => item.id === nextRule.id ? nextRule : item))
            setEditor(null)
          }}
        />
      ) : null}
    </div>
  )
}

export function PrivacyTermsSection({
  words,
  wordsIgnoreCase,
  wordsWholeWord,
  allowlist,
  limits,
  disabled,
  onChangeWords,
  onChangeIgnoreCase,
  onChangeWholeWord,
  onChangeAllowlist,
}: {
  words: string[]
  wordsIgnoreCase: boolean
  wordsWholeWord: boolean
  allowlist: string[]
  limits: PrivacyLimits
  disabled: boolean
  onChangeWords: (next: string[]) => void
  onChangeIgnoreCase: (next: boolean) => void
  onChangeWholeWord: (next: boolean) => void
  onChangeAllowlist: (next: string[]) => void
}) {
  const [wordsText, setWordsText] = React.useState(words.join('\n'))
  const [allowText, setAllowText] = React.useState(allowlist.join('\n'))

  React.useEffect(() => {
    setWordsText(prev => JSON.stringify(splitTerms(prev)) === JSON.stringify(words) ? prev : words.join('\n'))
  }, [words])

  React.useEffect(() => {
    setAllowText(prev => JSON.stringify(splitTerms(prev)) === JSON.stringify(allowlist) ? prev : allowlist.join('\n'))
  }, [allowlist])

  return (
    <div className='grid grid-cols-1 gap-3 md:grid-cols-2'>
      <div className='flex flex-col gap-2 rounded-md border border-border p-3'>
        <div className='flex items-center justify-between gap-2'>
          <Label htmlFor='privacy-words'>{t('自定义保密词（每行一条）')}</Label>
          <span className='hint tabular-nums'>{words.length} / {limits.maxWords}</span>
        </div>
        <Textarea id='privacy-words' rows={4} spellCheck={false} disabled={disabled}
          value={wordsText}
          placeholder={t('例如：内部项目代号、姓名、私密域名或日期（1990-05-20 会自动匹配同日多种写法）')}
          onChange={event => {
            const raw = event.currentTarget.value
            setWordsText(raw)
            onChangeWords(splitTerms(raw))
          }} />
        <div className='flex flex-wrap gap-4'>
          <Label htmlFor='privacy-words-icase' className='flex items-center gap-2 text-xs'>
            <Switch id='privacy-words-icase' checked={wordsIgnoreCase} disabled={disabled}
              onCheckedChange={onChangeIgnoreCase} />
            {t('忽略大小写')}
          </Label>
          <Label htmlFor='privacy-words-whole' className='flex items-center gap-2 text-xs'>
            <Switch id='privacy-words-whole' checked={wordsWholeWord} disabled={disabled}
              onCheckedChange={onChangeWholeWord} />
            {t('仅全词匹配')}
          </Label>
        </div>
      </div>

      <div className='flex flex-col gap-2 rounded-md border border-border p-3'>
        <div className='flex items-center justify-between gap-2'>
          <Label htmlFor='privacy-allowlist'>{t('白名单（每行一条，永不脱敏）')}</Label>
          <span className='hint tabular-nums'>{allowlist.length} / {limits.maxAllowlist}</span>
        </div>
        <Textarea id='privacy-allowlist' rows={4} spellCheck={false} disabled={disabled}
          value={allowText}
          placeholder={t('例如：example@company.com 或测试示例手机号')}
          onChange={event => {
            const raw = event.currentTarget.value
            setAllowText(raw)
            onChangeAllowlist(splitTerms(raw))
          }} />
        <div className='hint'>{t('命中白名单的内容会原样发送给上游，不进行遮罩或拦截。')}</div>
      </div>
    </div>
  )
}

export function PrivacyTestSection({
  draft,
  disabled,
  onRunTest,
}: {
  draft: PrivacyState
  disabled: boolean
  onRunTest: (text: string, draft: PrivacyState) => Promise<PrivacyPreviewResult>
}) {
  const [sample, setSample] = React.useState('')
  const [testing, setTesting] = React.useState(false)
  const [error, setError] = React.useState('')
  const [result, setResult] = React.useState<PrivacyPreviewResult | null>(null)

  async function run(): Promise<void> {
    if (!sample.trim() || testing) return
    setTesting(true)
    setError('')
    try {
      setResult(await onRunTest(sample, draft))
    } catch (err) {
      setResult(null)
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setTesting(false)
    }
  }

  return (
    <div className='flex flex-col gap-2 rounded-md border border-border p-3'>
      <div className='flex flex-wrap items-center gap-2'>
        <h3 className='text-[12.5px] font-semibold'>{t('试一试（实时预览）')}</h3>
        <span className='hint'>{t('使用临时映射测试当前草稿效果，不会污染运行中映射或写入日志')}</span>
        <Button type='button' variant='ghost' size='xs' className='ml-auto' disabled={disabled || testing}
          onClick={() => setSample(t('请帮我查看 sk-proj-demo1234567890abcd，password=mysecret，联系人 alice@company.com / 13800138000，订单 order_id=13800138000'))}>
          {t('填入示例')}
        </Button>
      </div>
      <div className='flex flex-col gap-2 sm:flex-row'>
        <Textarea id='privacy-test-input' rows={2} className='flex-1' spellCheck={false}
          disabled={disabled || testing} value={sample}
          placeholder={t('输入包含密钥、邮箱、手机号或保密词的文本进行模拟测试')}
          onChange={event => setSample(event.currentTarget.value)} />
        <Button id='btn-privacy-test' variant='outline' className='shrink-0 sm:self-start'
          disabled={disabled || testing || !sample.trim()} onClick={() => void run()}>
          {testing ? t('检测中…') : t('立即测试')}
        </Button>
      </div>
      {error ? <Alert variant='destructive' role='alert' className='text-xs text-destructive'>{error}</Alert> : null}
      {result ? (
        <div className='flex flex-col gap-2 rounded-md bg-muted/40 p-2.5 text-xs'>
          <div className='flex flex-wrap items-center gap-1.5'>
            <span className='font-medium text-muted-foreground'>{t('检测命中：')}</span>
            {result.matches.length ? result.matches.map((m, idx) => (
              <Badge key={`${m.kind}-${idx}`} shape='tag'
                variant={m.action === 'block' ? 'destructive' : m.action === 'warn' ? 'warning' : 'info'}>
                {privacyKindLabel(m.kind)} · {privacyActionLabel(m.action)} × {m.count}
              </Badge>
            )) : <span className='text-muted-foreground'>{t('未命中敏感内容')}</span>}
          </div>
          {result.blocked ? (
            <Alert variant='destructive' role='alert' className='text-xs text-destructive'>
              {result.blockMessage || t('该请求将被拦截')}
            </Alert>
          ) : (
            <div className='grid grid-cols-1 gap-2 md:grid-cols-2'>
              <div>
                <div className='mb-1 text-muted-foreground'>{t('发往上游预览')}</div>
                <pre className='max-h-[120px] overflow-auto whitespace-pre-wrap break-all rounded border border-border bg-background p-2 font-mono text-[11.5px]'>
                  {result.maskedText}
                </pre>
              </div>
              <div>
                <div className='mb-1 text-muted-foreground'>{t('还原回客户端预览')}</div>
                <pre className='max-h-[120px] overflow-auto whitespace-pre-wrap break-all rounded border border-border bg-background p-2 font-mono text-[11.5px]'>
                  {result.restoredText}
                </pre>
              </div>
            </div>
          )}
        </div>
      ) : null}
    </div>
  )
}
