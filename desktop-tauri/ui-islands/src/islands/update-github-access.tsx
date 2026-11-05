import { GithubAccessEditor } from '../../../ui-shared/github-access-editor'
import type { GithubAccessPatch } from '../../../ui-shared/github-access'
import { shared, toast } from './update-shared'
import { t } from '../i18n'

const load = async () => shared().workbuddyDesktop?.getGithubAccess()
const persist = async (patch: GithubAccessPatch) => shared().workbuddyDesktop?.setGithubAccess(patch)
const notify = (message: string, failed: boolean) => toast(message, failed ? 'err' : 'ok')

export function GithubAccessSettingsPanel() {
  return <GithubAccessEditor load={load} persist={persist} notify={notify} t={t} />
}
