import { t } from '../i18n'
import type { PromptApi, PromptView, PromptPreview } from '../../../ui-shared/settings-prompt-types'

type Bridge = {
  getPromptRules(): Promise<PromptView>
  savePromptRules: PromptApi['save']
  previewPromptRules: PromptApi['preview']
  savePrompt: PromptApi['saveGateway']
}

function bridge(): Bridge {
  const value = (window as unknown as { workbuddyDesktop?: Bridge }).workbuddyDesktop
  if (!value?.getPromptRules || !value.savePromptRules || !value.previewPromptRules || !value.savePrompt) {
    throw new Error(t('系统提示词入口尚未就绪，请刷新页面后重试'))
  }
  return value
}

export const promptRulesApi: PromptApi = {
  load: () => bridge().getPromptRules(),
  save: payload => bridge().savePromptRules(payload),
  preview: (payload): Promise<PromptPreview> => bridge().previewPromptRules(payload),
  saveGateway: payload => bridge().savePrompt(payload),
}
