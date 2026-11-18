export type CapacitySettings = {
  mode: 'auto' | 'manual'
  maxConversations: number
  maxInteractive: number
  maxBackground: number
  backgroundPerProvider: number
  maxImages: number
  maxConnections: number
  memoryMib: number
  queueRequests: number
  queueMemoryMib: number
  queueWaitSeconds: number
  providerLimits: Record<string, number>
}

export type CapacitySnapshot = {
  settings: CapacitySettings
  revision: string
  error?: string | null
  effective: {
    conversations: number
    interactive: number
    background: number
    backgroundPerProvider: number
    images: number
    connections: number
    memoryBytes: number
    queueRequests: number
    queueBytes: number
    queueWaitSeconds: number
    cpuPressure: boolean
    memoryPressure: boolean
  }
  resources: {
    totalBytes: number
    availableBytes: number
    processBytes: number
    containerBytes?: number | null
    cpuPercent: number
    cores: number
    connectionCeiling: number
  }
  usage: {
    conversations: number
    interactive: number
    background: number
    images: number
    connections: number
    memoryBytes: number
    queued: number
    queueBytes: number
    requests: number
    imageRequests: number
  }
  rejected: number
}

export type CapacityApi = <T>(method: 'GET' | 'PATCH', path: string, body?: unknown) => Promise<T>
