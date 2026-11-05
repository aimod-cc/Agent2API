export type GithubAccessMode = 'auto' | 'official' | 'accelerated'

export type GithubMirror = {
  url: string
  enabled: boolean
  supportsApi: boolean
}

export type GithubSourceObservation = {
  source: string
  label: string
  resource: 'api' | 'index' | 'download'
  state: 'ok' | 'failed' | 'timeout' | 'invalid' | 'unsupported' | 'rateLimited'
  elapsedMs?: number | null
  checkedAt: number
  selected: boolean
}

export type GithubAccessSettings = {
  mode: GithubAccessMode
  mirrorUrl: string
  mirrors?: GithubMirror[]
  observations?: GithubSourceObservation[]
  saved?: boolean
}

export type GithubAccessPatch = Partial<Pick<GithubAccessSettings, 'mode' | 'mirrorUrl' | 'mirrors'>>

export const DEFAULT_GITHUB_MIRRORS: GithubMirror[] = [
  { url: 'https://gh-proxy.com/', enabled: true, supportsApi: true },
  { url: 'https://cdn.gh-proxy.org/', enabled: true, supportsApi: true },
  { url: 'https://ghproxy.net/', enabled: true, supportsApi: false },
]

export function githubMirrors(settings: GithubAccessSettings): GithubMirror[] {
  return settings.mirrors ?? [{ url: settings.mirrorUrl, enabled: true, supportsApi: true }]
}

export function githubSourceLabel(url: string): string {
  try {
    const parsed = new URL(url)
    return `${parsed.host}${parsed.pathname === '/' ? '' : parsed.pathname}`
  } catch { return url }
}
