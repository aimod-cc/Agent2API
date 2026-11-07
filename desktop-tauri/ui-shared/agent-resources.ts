export type ResourceKind = 'prompt' | 'skills' | 'mcp'
export type ResourceCapabilities = { prompt: boolean; skills: boolean; mcp: boolean; note: string }
export type PromptResource = { path: string; exists: boolean; content: string; revision: string }
export type SkillResource = { id: string; name: string; description?: string; path: string; revision: string; linked: boolean }
export type SkillsResource = { roots: string[]; items: SkillResource[] }
export type McpResource = { path: string; revision: string; servers: Array<{ name: string; endpoint: string; config: Record<string, unknown>; editable?: boolean; deletable?: boolean }> }
export type ResourcePreview = { token: string; hasChanges: boolean; warnings: string[]; files: Array<{ path: string; before: string; text: string; diff: Array<{ kind: string; text: string }> }> }
export type ResourceRequest = <T>(method: 'GET' | 'POST' | 'PUT' | 'DELETE' | 'PATCH', path: string, body?: unknown) => Promise<T>

export function resourceBase(id: string) { return `/api/agents/${encodeURIComponent(id)}/resources` }
