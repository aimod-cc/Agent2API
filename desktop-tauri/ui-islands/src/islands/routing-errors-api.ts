import { t } from '../i18n'
import type { ErrorPolicyApi, ErrorPolicyView } from '../../../ui-shared/routing-errors-types'

type Bridge = { getRoutingErrors(): Promise<ErrorPolicyView>; saveRoutingErrors: ErrorPolicyApi['save'] }
function bridge(): Bridge {
  const api = (window as unknown as { workbuddyDesktop?: Bridge }).workbuddyDesktop
  if (!api?.getRoutingErrors || !api.saveRoutingErrors) throw new Error(t('错误处理入口尚未就绪，请刷新页面后重试'))
  return api
}
export const routingErrorsApi: ErrorPolicyApi = { load: () => bridge().getRoutingErrors(), save: payload => bridge().saveRoutingErrors(payload) }
