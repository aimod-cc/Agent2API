import * as React from 'react'
import {
  Alert, Badge, Button, Card, CardContent, CardHeader, Checkbox, Label,
  MultiSelect, SegmentedControl, Select, SelectContent, SelectItem,
  SelectTrigger, SelectValue, Switch,
} from '@ui'
import { t } from '../i18n'
import { STATES } from './settings-model'
import {
  PRIVACY_NOTES, PRIVACY_TIPS, samePrivacyDraft, type PrivacyAction,
  type PrivacyPatch, type PrivacyState,
} from './settings-privacy-model'
import {
  PrivacyCustomRulesSection, PrivacyTermsSection, PrivacyTestSection,
} from './settings-privacy-rules'
import {
  refreshPrivacy, savePrivacyConfig, testPrivacyPreview, type SettingsSnapshot,
} from './settings-state'

function toPatch(draft: PrivacyState, revision?: string): PrivacyPatch {
  const patch: PrivacyPatch = {
    enabled: draft.enabled,
    reversible: draft.reversible,
    ephemeralPlaceholders: draft.ephemeralPlaceholders,
    action: draft.action,
    secrets: draft.secrets,
    personal: draft.personal,
    categories: draft.categories,
    kindActions: draft.kindActions,
    words: draft.words,
    wordsIgnoreCase: draft.wordsIgnoreCase,
    wordsWholeWord: draft.wordsWholeWord,
    customRules: draft.customRules,
    allowlist: draft.allowlist,
    providerMode: draft.providerMode,
    providers: draft.providers,
  }
  if (revision) patch.revision = revision
  return patch
}

export function PrivacyPanel({ snap }: { snap: SettingsSnapshot }) {
  const state = snap.privacy
  const [draft, setDraft] = React.useState<PrivacyState>(state)
  const [saveError, setSaveError] = React.useState('')
  const busy = snap.busy === 'privacy'
  const ready = state.status === 'ready'
  const disabled = !ready || busy

  React.useEffect(() => {
    setDraft(state)
    setSaveError('')
  }, [state])

  const actionOptions: { value: PrivacyAction; label: string }[] = [
    { value: 'mask', label: t('遮罩替换（请求前脱敏）') },
    { value: 'warn', label: t('仅提醒（记录命中，原文放行）') },
    { value: 'block', label: t('拦截请求（命中即拒绝转发）') },
  ]
  const currentActionLabel = actionOptions.find(item => item.value === draft.action)?.label ?? actionOptions[0].label
  const dirty = ready && !samePrivacyDraft(draft, state)

  function updateDraft(patch: Partial<PrivacyState>): void {
    setSaveError('')
    setDraft(current => ({ ...current, ...patch }))
  }

  async function save(): Promise<void> {
    setSaveError('')
    const result = await savePrivacyConfig(toPatch(draft, state.revision || undefined))
    if (!result.ok && result.error) {
      setSaveError(result.error)
    }
  }

  const stateSummary = !ready
    ? (state.status === 'loading' ? STATES.appLoading : t('未能读取隐私脱敏设置，请稍后重试'))
    : !state.enabled
      ? t('已关闭；敏感信息不会自动遮罩。建议在使用公开或第三方模型时开启。')
      : state.ephemeralPlaceholders
        ? t('已开启（单次随机隐藏类型占位符；映射随请求结束自动销毁）')
        : state.vaultEntries > 0
          ? t('已开启（内存映射 {count} 条；仅内存保留，不落盘）', { count: state.vaultEntries })
          : t('已开启；出站请求将按规则识别并保护敏感信息。')

  return (
    <Card render={<section />} className='panel'>
      <CardHeader className='panel-head'>
        <h2>{t('隐私脱敏')}</h2>
        <span className='tip-q' data-tip={PRIVACY_TIPS.panel}></span>
        <Badge variant={!ready ? (state.status === 'loading' ? 'outline' : 'destructive') : state.enabled ? 'success' : 'outline'}>
          {!ready ? (state.status === 'loading' ? t('检测中…') : t('不可用')) : state.enabled ? t('已开启') : t('已关闭')}
        </Badge>
        <div className='head-actions'>
          <Button id='btn-privacy-refresh' variant='outline' size='sm' disabled={busy}
            onClick={() => void refreshPrivacy()}>{t('刷新')}</Button>
        </div>
      </CardHeader>
      <CardContent className='panel-body flex flex-col gap-3'>
        <div className='settings-switches'>
          <Label htmlFor='settings-privacy-enabled' className='flex items-center gap-2'>
            <Switch id='settings-privacy-enabled' checked={draft.enabled} disabled={disabled}
              onCheckedChange={enabled => updateDraft({ enabled })} />
            {t('开启隐私脱敏（请求离开本机前识别并保护密钥、隐私信息与保密词）')}
          </Label>
          <Label htmlFor='settings-privacy-reversible' className='flex items-center gap-2'>
            <Switch id='settings-privacy-reversible' checked={draft.reversible} disabled={disabled}
              onCheckedChange={reversible => updateDraft({ reversible })} />
            {t('自动还原响应中的占位符（支持流式输出、JSON 输出与工具调用参数）')}
          </Label>
          <Label htmlFor='settings-privacy-ephemeral' className='flex items-center gap-2'>
            <Switch id='settings-privacy-ephemeral' checked={draft.ephemeralPlaceholders}
              disabled={disabled || !draft.reversible}
              onCheckedChange={ephemeralPlaceholders => updateDraft({ ephemeralPlaceholders })} />
            {t('每次请求使用随机且隐藏类型的占位符（隐私更强，会降低提示词缓存命中，且不支持引用上游历史会话）')}
          </Label>
        </div>

        <div className='settings-state'>{stateSummary}</div>

        {!state.saved ? (
          <Alert variant='destructive' role='alert' className='text-xs text-destructive'>
            {t('隐私脱敏设置仅本次运行生效，写入数据库失败，请检查配置目录')}
          </Alert>
        ) : null}

        {saveError ? (
          <Alert variant='destructive' role='alert' className='flex flex-wrap items-center justify-between gap-2 text-xs text-destructive'>
            <span>{saveError}</span>
            <Button variant='outline' size='xs' disabled={busy} onClick={() => void refreshPrivacy()}>
              {t('刷新最新配置')}
            </Button>
          </Alert>
        ) : null}

        {ready ? (
          <>
            <div className='flex flex-wrap items-center gap-3'>
              <Label htmlFor='privacy-action-select'>{t('默认处理方式')}</Label>
              <Select value={draft.action} disabled={disabled} onValueChange={value => {
                if (value === 'mask' || value === 'warn' || value === 'block') updateDraft({ action: value })
              }}>
                <SelectTrigger id='privacy-action-select' className='w-[260px]'>
                  <SelectValue>{currentActionLabel}</SelectValue>
                </SelectTrigger>
                <SelectContent>
                  {actionOptions.map(item => (
                    <SelectItem key={item.value} value={item.value}>{item.label}</SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <div className='flex flex-col gap-2 rounded-md border border-border p-3'>
              <Label htmlFor='privacy-detect-secrets' className='flex items-center gap-2'>
                <Switch id='privacy-detect-secrets' checked={draft.secrets} disabled={disabled}
                  onCheckedChange={secrets => updateDraft({ secrets })} />
                {t('自动识别密钥、钱包与凭据（API Key、私钥、助记词、JWT、密码字段与工具 JSON 敏感字段）')}
              </Label>
              <div className='flex flex-wrap items-center gap-2'>
                <Label htmlFor='privacy-detect-personal' className='flex items-center gap-2'>
                  <Switch id='privacy-detect-personal' checked={draft.personal} disabled={disabled}
                    onCheckedChange={personal => updateDraft({ personal })} />
                  {t('识别个人隐私信息（PII）')}
                </Label>
                <span className='tip-q' data-tip={PRIVACY_TIPS.personal}></span>
              </div>
              {draft.personal ? (
                <div className='mt-1 grid grid-cols-2 gap-2 sm:grid-cols-3'>
                  {draft.categoryOptions.map(cat => (
                    <Label key={cat.id} htmlFor={`privacy-cat-${cat.id}`} className='flex items-center gap-2 text-xs font-normal'>
                      <Checkbox
                        id={`privacy-cat-${cat.id}`}
                        checked={draft.categories[cat.id] ?? cat.defaultOn}
                        disabled={disabled}
                        onCheckedChange={checked => updateDraft({
                          categories: { ...draft.categories, [cat.id]: checked === true },
                        })}
                      />
                      {cat.label}
                    </Label>
                  ))}
                </div>
              ) : null}
            </div>

            <div className='retention-list'>
              <div className='retention-row'>
                <Label>{t('作用范围')}</Label>
                <SegmentedControl aria-label={t('作用范围')} value={draft.providerMode}
                  disabled={disabled}
                  options={[
                    { value: 'exclude', label: t('全部提供商（排除选中）') },
                    { value: 'include', label: t('仅指定提供商'), disabled: !draft.providerOptions.length },
                  ]}
                  onValueChange={value => {
                    if (value === 'exclude' || value === 'include') {
                      updateDraft({ providerMode: value })
                    }
                  }} />
              </div>
              {draft.providerOptions.length ? (
                <div className='flex flex-col gap-1.5'>
                  <Label htmlFor='privacy-provider-select'>
                    {draft.providerMode === 'exclude' ? t('排除的提供商（留空表示保护全部）') : t('启用脱敏的提供商')}
                  </Label>
                  <MultiSelect id='privacy-provider-select' value={draft.providers}
                    options={draft.providerOptions.map(item => ({ value: item.id, label: item.label }))}
                    disabled={disabled}
                    onValueChange={providers => updateDraft({ providers })}
                    className='w-full'
                    placeholder={draft.providerMode === 'exclude'
                      ? t('可选：选择信任而不脱敏的提供商（如本地模型）')
                      : t('请至少选择一个提供商')} />
                </div>
              ) : null}
            </div>

            <PrivacyCustomRulesSection
              rules={draft.customRules}
              limits={draft.limits}
              disabled={disabled}
              onChange={customRules => updateDraft({ customRules })}
            />

            <PrivacyTermsSection
              words={draft.words}
              wordsIgnoreCase={draft.wordsIgnoreCase}
              wordsWholeWord={draft.wordsWholeWord}
              allowlist={draft.allowlist}
              limits={draft.limits}
              disabled={disabled}
              onChangeWords={words => updateDraft({ words })}
              onChangeIgnoreCase={wordsIgnoreCase => updateDraft({ wordsIgnoreCase })}
              onChangeWholeWord={wordsWholeWord => updateDraft({ wordsWholeWord })}
              onChangeAllowlist={allowlist => updateDraft({ allowlist })}
            />

            <PrivacyTestSection
              draft={draft}
              disabled={disabled}
              onRunTest={(text, nextDraft) => testPrivacyPreview(text, toPatch(nextDraft))}
            />

            <div className='flex flex-wrap items-center justify-end gap-2'>
              <span className='hint mr-auto'>
                {dirty ? t('有未保存的修改') : t('当前设置已保存')}
              </span>
              {dirty ? (
                <Button variant='outline' disabled={busy} onClick={() => { setDraft(state); setSaveError('') }}>
                  {t('放弃修改')}
                </Button>
              ) : null}
              <Button id='btn-privacy-save' disabled={busy || !dirty} onClick={() => void save()}>
                {busy ? t('保存中…') : t('保存设置')}
              </Button>
            </div>
          </>
        ) : null}

        <div className='hint retention-note'>{PRIVACY_NOTES.panel}</div>
      </CardContent>
    </Card>
  )
}
