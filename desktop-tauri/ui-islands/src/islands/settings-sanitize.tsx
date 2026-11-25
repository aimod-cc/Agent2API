import * as React from 'react'
import {
  Alert, Badge, Button, Card, CardContent, CardHeader, Dialog, DialogBody, DialogClose,
  DialogContent, DialogFooter, DialogHeader, DialogTitle, DialogTrigger, Label,
  MultiSelect, SegmentedControl, Switch,
} from '@ui'
import { t } from '../i18n'
import { NOTES, STATES, TIPS } from './settings-model'
import { refreshSanitize, saveSanitizeConfig, type SettingsSnapshot } from './settings-state'
import { sameRules, type SanitizePatch, type SanitizeRule } from './settings-sanitize-model'
import { SanitizeRuleEditor, SanitizeRulesTable } from './settings-sanitize-rules'

export function SanitizePanel({ snap }: { snap: SettingsSnapshot }) {
  const state = snap.sanitize
  const [on, setOn] = React.useState(state.on)
  const [rules, setRules] = React.useState(state.rules)
  const [providers, setProviders] = React.useState(state.providers)
  const [scope, setScope] = React.useState<'all' | 'selected'>(state.providers.length ? 'selected' : 'all')
  const [restore, setRestore] = React.useState(false)
  const [rulesOpen, setRulesOpen] = React.useState(false)
  const [editor, setEditor] = React.useState<{ rule: SanitizeRule; isNew: boolean } | null>(null)
  const busy = snap.busy === 'sanitize'
  const ready = state.status === 'ready'
  const editable = ready && state.editable
  const disabled = !editable || busy

  React.useEffect(() => {
    setOn(state.on)
    setRules(state.rules)
    setProviders(state.providers)
    setScope(state.providers.length ? 'selected' : 'all')
    setRestore(false)
    setEditor(null)
  }, [state])

  const selected = scope === 'all' ? [] : providers
  const sameProviders = selected.length === state.providers.length
    && selected.every(id => state.providers.includes(id))
  const dirty = ready && (on !== state.on || !sameRules(rules, state.rules)
    || !sameProviders || (restore && state.customRules))
  const defaultLabel = restore || (!state.customRules && sameRules(rules, state.defaults))
  const saveStatus = <span className='hint mr-auto'>
    {dirty ? t('有未保存的修改') : t('当前设置已保存')}{!state.on ? t('；开关关闭时词表不执行') : ''}
  </span>

  function changeRules(next: SanitizeRule[]): void {
    setRules(next)
    setRestore(false)
  }

  function addRule(): void {
    setEditor({ isNew: true, rule: {
      id: `rule-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 12)}`,
      name: '', enabled: true, matchType: 'literal', pattern: '', replacement: '', repeat: false, roles: [],
    } })
  }

  async function save(): Promise<void> {
    const patch: SanitizePatch = {}
    if (on !== state.on) patch.sanitizeBlacklistFingerprints = on
    if (state.editable) {
      if (restore) patch.sanitizeFingerprintRules = null
      else if (!sameRules(rules, state.rules)) {
        patch.sanitizeFingerprintRules = rules.map(({ roles, ...rule }) =>
          roles.length ? { ...rule, roles } : rule)
      }
      if (!sameProviders) patch.sanitizeFingerprintProviders = selected
    }
    await saveSanitizeConfig(patch)
  }

  return (
    <Card render={<section />} className='panel'>
      <CardHeader className='panel-head'>
        <h2>{t('关键词替换')}</h2>
        <span className='tip-q' data-tip={TIPS.sanitize}></span>
        <Badge variant={!ready ? (state.status === 'loading' ? 'outline' : 'destructive') : state.on ? 'success' : 'outline'}>
          {!ready ? (state.status === 'loading' ? t('检测中…') : t('不可用')) : state.on ? t('已开启') : t('已关闭')}
        </Badge>
        <div className='head-actions'>
          <Button id='btn-sanitize-refresh' variant='outline' size='sm' disabled={busy}
            onClick={() => void refreshSanitize()}>{t('刷新')}</Button>
        </div>
      </CardHeader>
      <CardContent className='panel-body flex flex-col gap-3'>
        <div className='settings-switches'>
          <Label htmlFor='settings-sanitize' className='flex items-center gap-2'>
            <Switch id='settings-sanitize' checked={on} disabled={!ready || busy} onCheckedChange={setOn} />
            {t('按词表替换或删除出站请求中的指定文本与表头')}
          </Label>
        </div>
        <div className='settings-state'>
          {!ready ? (state.status === 'loading' ? STATES.appLoading : STATES.sanitizeUnavailable)
            : !state.on ? STATES.sanitizeOff
              : t('已启用 {count} 条规则，应用于{scope}。', {
                  count: state.rules.filter(rule => rule.enabled).length,
                  scope: state.providers.length
                    ? state.providers.map(id => state.options.find(option => option.id === id)?.label ?? id).join(t('、'))
                    : t('全部提供商'),
                })}
        </div>
        {!state.saved ? <Alert variant='destructive' role='alert' className='text-xs text-destructive'>
          {t('关键词替换设置仅本次运行生效，写入数据库失败，请检查配置目录')}
        </Alert> : null}
        {editable ? <>
          <div className='retention-list'>
            <div className='retention-row'>
              <Label>{t('应用提供商')}</Label>
              <SegmentedControl aria-label={t('应用提供商')} value={scope}
                options={[{ value: 'all', label: t('全部提供商') },
                  { value: 'selected', label: t('指定提供商'), disabled: !state.options.length }]}
                disabled={disabled} onValueChange={value => {
                  if (value === 'all') { setScope('all'); return }
                  if (!providers.length) setProviders(state.options.map(option => option.id))
                  setScope('selected')
                }} />
            </div>
            {scope === 'selected' ? <div className='flex flex-col gap-2'>
              <Label htmlFor='sanitize-provider-select'>{t('选择提供商（可多选）')}</Label>
              <MultiSelect id='sanitize-provider-select' value={providers}
                options={state.options.map(option => ({ value: option.id, label: option.label }))}
                disabled={disabled} onValueChange={value => {
                  setProviders(value)
                  if (!value.length) setScope('all')
                }} className='w-full'
                placeholder={t('请至少选择一个提供商')} />
            </div> : null}
            <div className='hint'>{t('默认应用全部提供商（包含之后新增的插件与自定义提供商）。指定范围后，未选中的提供商不做关键词替换；清空选择自动回到全部提供商。')}</div>
          </div>
          <Dialog open={rulesOpen} onOpenChange={open => {
            setRulesOpen(open)
            if (!open) setEditor(null)
          }}>
            <div className='flex flex-wrap items-center gap-2'>
              <h3 className='text-[12.5px] font-semibold'>{t('替换词表')}</h3>
              <Badge variant='outline'>{defaultLabel ? t('内置默认') : t('自定义')}</Badge>
              <span className='hint'>{t('{count} 条规则，按列表顺序执行', { count: rules.length })}</span>
              <DialogTrigger id='btn-sanitize-rules-open' disabled={disabled}
                render={<Button variant='outline' size='sm' className='ml-auto' />}>
                {t('编辑')}
              </DialogTrigger>
            </div>
            <DialogContent className='w-[min(1120px,calc(100vw-48px))]'>
              <DialogHeader>
                <DialogTitle>{t('替换词表')}</DialogTitle>
                <Badge variant='outline'>{defaultLabel ? t('内置默认') : t('自定义')}</Badge>
              </DialogHeader>
              <DialogBody>
                <div className='flex flex-wrap items-center gap-2'>
                  <span className='hint'>{t('{count} 条规则，按列表顺序执行', { count: rules.length })}</span>
                  <Button id='btn-sanitize-rule-add' variant='dashed' size='sm' className='ml-auto'
                    disabled={disabled || rules.length >= state.limits.maxRules} onClick={addRule}>{t('新增规则')}</Button>
                  <Button id='btn-sanitize-rules-reset' variant='outline' size='sm' disabled={disabled}
                    onClick={() => { setRules(state.defaults.map(rule => ({ ...rule }))); setRestore(true) }}>
                    {t('恢复默认词表')}
                  </Button>
                </div>
                <SanitizeRulesTable rules={rules} defaults={state.defaults} disabled={disabled} onChange={changeRules}
                  onEdit={rule => setEditor({ rule, isNew: false })} />
                <div className='hint'>{t('规则可以增删改、启停和调整顺序；替换内容留空表示删除匹配内容。恢复默认只重置词表，不改开关和提供商范围。')}</div>
                {!state.saved ? <Alert variant='destructive' role='alert' className='text-xs text-destructive'>
                  {t('关键词替换设置仅本次运行生效，写入数据库失败，请检查配置目录')}
                </Alert> : null}
              </DialogBody>
              <DialogFooter className='flex-wrap justify-end'>
                {saveStatus}
                <DialogClose render={<Button variant='outline' />}>{t('关闭')}</DialogClose>
                <Button id='btn-sanitize-rules-save' disabled={disabled || !dirty} onClick={() => void save()}>
                  {busy ? t('保存中…') : t('保存设置')}
                </Button>
              </DialogFooter>
              {editor ? <SanitizeRuleEditor rule={editor.rule} isNew={editor.isNew} limits={state.limits}
                roleScopeEditable={state.roleScopeEditable} onClose={() => setEditor(null)} onApply={rule => {
                  changeRules(editor.isNew ? [...rules, rule] : rules.map(item => item.id === rule.id ? rule : item))
                  setEditor(null)
                }} /> : null}
            </DialogContent>
          </Dialog>
        </> : ready ? <div className='hint'>{t('当前后端不支持词表编辑与提供商范围，请更新后端后重试。')}</div> : null}
        {ready ? <div className='flex flex-wrap items-center justify-end gap-2'>
          {saveStatus}
          <Button id='btn-sanitize-save' disabled={busy || !dirty} onClick={() => void save()}>
            {busy ? t('保存中…') : t('保存设置')}
          </Button>
        </div> : null}
        <div className='hint retention-note'>{NOTES.sanitize}</div>
      </CardContent>
    </Card>
  )
}
