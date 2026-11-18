export type PermissionRequest = <T>(method: 'GET' | 'PATCH', path: string, body?: unknown) => Promise<T>

export function createPermissionReads(request: PermissionRequest): { request: PermissionRequest; invalidate(): void } {
  const pending = new Map<string, Promise<unknown>>()
  const invalidate = () => pending.clear()
  const run: PermissionRequest = <T,>(method: 'GET' | 'PATCH', path: string, body?: unknown): Promise<T> => {
    if (method !== 'GET') {
      invalidate()
      return request<T>(method, path, body).finally(invalidate)
    }
    const current = pending.get(path)
    if (current) return current as Promise<T>
    const task = Promise.resolve().then(() => request<T>(method, path, body)).finally(() => {
      if (pending.get(path) === task) pending.delete(path)
    })
    pending.set(path, task)
    return task
  }
  return { request: run, invalidate }
}
