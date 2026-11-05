import { GithubAccessEditor } from '../../../ui-shared/github-access-editor'
import type { GithubAccessPatch, GithubAccessSettings } from '../../../ui-shared/github-access'
import { apiCall } from '../api/client'
import { store } from '../state/store'
import { t } from '../i18n'

const load = () => apiCall<GithubAccessSettings>('/api/update/github')
const persist = (patch: GithubAccessPatch) => apiCall<GithubAccessSettings>('/api/update/github', { method: 'POST', body: patch })
const notify = (message: string) => store.toast(message)

export function GithubAccessSettingsPanel() {
  return <div className='px-3 py-3' style={{ borderTop: '1px solid var(--m-line)' }}>
    <GithubAccessEditor load={load} persist={persist} notify={notify} t={t} mobile />
  </div>
}
