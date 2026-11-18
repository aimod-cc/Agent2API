import { useMemo } from 'react'
import type { PluginRequest } from '../../../../ui-shared/plugins-batch-runner'
import { apiCall } from '../../api/client'
import { serverIdentity } from '../../lib/servers'

export function usePluginRequest(identity: string): PluginRequest {
  return useMemo(() => async <T,>(method: string, path: string, body?: unknown, options?: { retry: 'never' }): Promise<T> => {
    if (serverIdentity() !== identity) throw new DOMException('服务器已切换', 'AbortError')
    const result = await apiCall<T>(path, { method: method as 'GET' | 'POST' | 'PATCH', body, noAuthReplay: options?.retry === 'never' })
    if (serverIdentity() !== identity) throw new DOMException('服务器已切换', 'AbortError')
    return result
  }, [identity])
}
