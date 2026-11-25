import { apiCall } from './client'
import type {
  SandboxOutcome,
  TaskJob,
  TaskJobRun,
  TaskJobsPayload,
  TaskPluginsPayload,
} from '../../../ui-shared/task-plugins-types'

export function fetchTaskPlugins(signal?: AbortSignal): Promise<TaskPluginsPayload> {
  return apiCall<TaskPluginsPayload>('/api/task-plugins', { signal })
}

export function fetchTaskJobs(signal?: AbortSignal): Promise<TaskJobsPayload> {
  return apiCall<TaskJobsPayload>('/api/task-jobs', { signal })
}

export function createTaskJob(payload: Record<string, unknown>): Promise<{ job: TaskJob }> {
  return apiCall<{ job: TaskJob }>('/api/task-jobs', { method: 'POST', body: payload })
}

export function updateTaskJob(id: string, payload: Record<string, unknown>): Promise<{ job: TaskJob }> {
  return apiCall<{ job: TaskJob }>(`/api/task-jobs/${encodeURIComponent(id)}`, {
    method: 'PUT',
    body: payload,
  })
}

export function patchTaskJob(id: string, payload: { enabled?: boolean; proxyId?: string }): Promise<{ job: TaskJob }> {
  return apiCall<{ job: TaskJob }>(`/api/task-jobs/${encodeURIComponent(id)}`, {
    method: 'PATCH',
    body: payload,
  })
}

export function deleteTaskJob(id: string): Promise<{ deleted: boolean }> {
  return apiCall<{ deleted: boolean }>(`/api/task-jobs/${encodeURIComponent(id)}`, {
    method: 'DELETE',
  })
}

export function runTaskJobNow(id: string): Promise<{ outcome: SandboxOutcome }> {
  return apiCall<{ outcome: SandboxOutcome }>(`/api/task-jobs/${encodeURIComponent(id)}/run`, {
    method: 'POST',
    body: {},
    noAuthReplay: true,
  })
}

export function stopTaskJob(id: string): Promise<{ stopped: boolean }> {
  return apiCall<{ stopped: boolean }>(`/api/task-jobs/${encodeURIComponent(id)}/stop`, {
    method: 'POST',
    body: {},
  })
}

export function triggerTaskJobAction(
  id: string,
  actionId: string,
  values: Record<string, unknown>,
): Promise<{ outcome: SandboxOutcome; job?: TaskJob }> {
  return apiCall<{ outcome: SandboxOutcome; job?: TaskJob }>(
    `/api/task-jobs/${encodeURIComponent(id)}/action`,
    {
      method: 'POST',
      body: { actionId, values },
      noAuthReplay: true,
    },
  )
}

export function fetchTaskJobRuns(id: string, limit = 50): Promise<{ runs: TaskJobRun[] }> {
  return apiCall<{ runs: TaskJobRun[] }>(
    `/api/task-jobs/${encodeURIComponent(id)}/runs?limit=${limit}`,
  )
}

export function clearTaskJobRuns(id: string): Promise<{ cleared: boolean }> {
  return apiCall<{ cleared: boolean }>(`/api/task-jobs/${encodeURIComponent(id)}/runs`, {
    method: 'DELETE',
  })
}

export function updateTaskPluginNotifyPermission(
  pluginId: string,
  notifyAllowed: boolean,
): Promise<unknown> {
  return apiCall(`/api/task-plugins/${encodeURIComponent(pluginId)}`, {
    method: 'PATCH',
    body: { notifyAllowed },
  })
}
