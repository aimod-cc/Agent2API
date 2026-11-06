import { callJson } from './assistant-api'
import { VisionCompatibilityPanel } from '../../../ui-shared/settings-vision-panel'
import type { VisionCompatApi } from '../../../ui-shared/settings-vision-types'
import { t } from '../i18n'

const api: VisionCompatApi = {
  load: () => callJson('GET', '/api/vision-compatibility'),
  save: payload => callJson('PUT', '/api/vision-compatibility', payload),
}

export function VisionCompatibilitySettings({ active }: { active: boolean }) {
  return <VisionCompatibilityPanel api={api} t={t} active={active} />
}
