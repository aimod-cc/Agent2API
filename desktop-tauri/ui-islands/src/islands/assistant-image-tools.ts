import { t } from '../i18n'
import { callJson } from './assistant-api'
import { assistantImageMeta, createAssistantImageTools } from '../../../ui-shared/assistant-image-tools'

export const ASSISTANT_IMAGE_TOOLS = createAssistantImageTools(
  (path, body, signal, apiKey) => callJson('POST', path, body, signal, apiKey), t,
)
export const ASSISTANT_IMAGE_META = assistantImageMeta(t)
