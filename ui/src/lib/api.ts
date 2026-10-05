// One transport for every shell: the desktop app, a browser on this machine, and the LAN
// dashboard all talk to the same HTTP API. Only how we *find* it differs.

import type { BrowserState, MediaProbe, TorrentMeta, ToolsStatus, AddRequest, BlockMap, InterfaceInfo, Job, ProbeInfo, ServerEvent, Settings, Snapshot, InterfaceRule, StreamStat, UpdateInfo } from './types'

let base = ''
let token = ''

export const isTauri = (): boolean => typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window

export type Bootstrap = { ok: true } | { ok: false; reason: 'need-token' | 'unreachable'; detail?: string }

/** Work out where the API lives and how to authenticate. */
export async function bootstrap(): Promise<Bootstrap> {
  const q = new URLSearchParams(location.search)
  if (isTauri()) {
    const { invoke } = await import('@tauri-apps/api/core')
    const r = await invoke<{ port: number; token: string }>('bootstrap')
    base = `http://127.0.0.1:${r.port}`
    token = r.token
    return { ok: true }
  }
  base = q.get('api') || (location.port === '5173' ? 'http://127.0.0.1:7733' : location.origin)
  // `#token=…` never reaches a server's logs; keep it in memory and local storage only.
  const hash = new URLSearchParams(location.hash.replace(/^#/, ''))
  const fromHash = hash.get('token')
  if (fromHash) {
    localStorage.setItem('idmx.token', fromHash)
    history.replaceState(null, '', location.pathname + location.search)
  }
  token = fromHash || localStorage.getItem('idmx.token') || ''
  if (!token) return { ok: false, reason: 'need-token' }
  try {
    const r = await fetch(`${base}/api/state`, { headers: { Authorization: `Bearer ${token}` } })
    if (r.status === 401) return { ok: false, reason: 'need-token', detail: 'That token was not accepted.' }
    if (!r.ok) return { ok: false, reason: 'unreachable', detail: `HTTP ${r.status}` }
  } catch (e) {
    return { ok: false, reason: 'unreachable', detail: String(e) }
  }
  return { ok: true }
}

export function setToken(t: string) {
  token = t.trim()
  localStorage.setItem('idmx.token', token)
}

/** Open a web page in the user's browser — from the app or from a dashboard in a tab. */
export async function openExternal(url: string) {
  if (!/^https?:\/\//i.test(url)) return
  if (isTauri()) {
    const { invoke } = await import('@tauri-apps/api/core')
    await invoke('open_external', { url })
  } else window.open(url, '_blank', 'noopener')
}

export function connectionInfo() {
  return { base, token }
}

export class ApiError extends Error {
  constructor(
    message: string,
    public status: number,
  ) {
    super(message)
  }
}

async function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const r = await fetch(`${base}/api${path}`, {
    method,
    headers: { Authorization: `Bearer ${token}`, ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}) },
    body: body !== undefined ? JSON.stringify(body) : undefined,
  })
  const data = await r.json().catch(() => ({}))
  if (!r.ok) throw new ApiError(data.error || r.statusText, r.status)
  return data as T
}

export const api = {
  state: () => call<Snapshot>('GET', '/state'),
  add: (req: AddRequest) => call<Job[]>('POST', '/jobs', req),
  patch: (id: string, patch: Partial<Pick<Job, 'priority' | 'startAt' | 'options'>>) => call<Job>('PATCH', `/jobs/${id}`, patch),
  pause: (id: string) => call('POST', `/jobs/${id}/pause`, {}),
  resume: (id: string) => call('POST', `/jobs/${id}/resume`, {}),
  cancel: (id: string) => call('POST', `/jobs/${id}/cancel`, {}),
  restart: (id: string) => call('POST', `/jobs/${id}/restart`, {}),
  top: (id: string) => call('POST', `/jobs/${id}/top`, {}),
  reveal: (id: string) => call('POST', `/jobs/${id}/reveal`, {}),
  remove: (id: string, deleteFile = false) => call('DELETE', `/jobs/${id}?deleteFile=${deleteFile}`),
  map: (id: string) => call<BlockMap | null>('GET', `/jobs/${id}/map`),
  streams: (id: string) => call<StreamStat[]>('GET', `/jobs/${id}/streams`),
  jobNetwork: (id: string, net: string, enabled: boolean) => call('POST', `/jobs/${id}/networks/${encodeURIComponent(net)}`, { enabled }),
  ping: () => call<InterfaceInfo[]>('POST', '/ping', {}),
  refreshNetworks: () => call<InterfaceInfo[]>('POST', '/networks/refresh', {}),
  openNetworkSettings: () => call('POST', '/open-network-settings', {}),
  checkUpdate: () => call<UpdateInfo | null>('POST', '/update/check', {}),
  dismissUpdate: () => call('POST', '/update/dismiss', {}),
  pauseAll: () => call('POST', '/pause-all', {}),
  resumeAll: () => call('POST', '/resume-all', {}),
  clearFinished: () => call('POST', '/clear-finished', {}),
  settings: (patch: Partial<Settings> | Record<string, unknown>) => call<Settings>('PUT', '/settings', patch),
  newToken: () => call<{ token: string; note: string }>('POST', '/settings/token', {}),
  setRule: (id: string, rule: InterfaceRule) => call<Settings>('PUT', `/interfaces/${encodeURIComponent(id)}/rule`, rule),
  clearRule: (id: string) => call<Settings>('DELETE', `/interfaces/${encodeURIComponent(id)}/rule`),
  probe: (url: string, options = {}) => call<ProbeInfo>('POST', '/probe', { url, options }),
  expand: (text: string) => call<{ total: number; items: { url: string; name: string | null; checksum: string | null }[] }>('POST', '/expand', { text }),
  tools: () => call<ToolsStatus>('GET', '/tools'),
  installTool: (which: 'ytdlp' | 'ffmpeg' | 'js') => call<ToolsStatus>('POST', `/tools/install/${which}`, {}),
  mediaProbe: (url: string, options = {}) => call<MediaProbe>('POST', '/media/probe', { url, options }),
  torrentInspect: (source: string) => call<TorrentMeta>('POST', '/torrent/inspect', { url: source }),
  setTorrentFiles: (id: string, files: number[]) => call('POST', `/jobs/${id}/torrent-files`, { files }),
  browser: () => call<BrowserState>('GET', '/browser'),
  browserSetup: () => call<BrowserState>('POST', '/browser/setup', {}),
  browserOpen: (id: string) => call('POST', `/browser/open/${id}`, {}),
  browserReveal: () => call<{ path: string }>('POST', '/browser/reveal', {}),
  stageTorrent: async (file: File, dir?: string) => {
    const r = await fetch(`${base}/api/torrent/stage${dir ? `?dir=${encodeURIComponent(dir)}` : ''}`, { method: 'POST', headers: { Authorization: `Bearer ${token}` }, body: file })
    const data = await r.json().catch(() => ({}))
    if (!r.ok) throw new ApiError(data.error || r.statusText, r.status)
    return data as { path: string }
  },
  pendingAdd: () => call<{ items: string[] }>('GET', '/pending-add'),
  cache: () => call<{ bytes: number }>('GET', '/cache'),
  clearCache: () => call<{ freed: number; bytes: number }>('POST', '/cache/clear', {}),
  speedtest: (url?: string) => call('POST', '/speedtest', { url }),
}

/** Live events with automatic reconnection. */
export function openEvents(on: { event: (e: ServerEvent) => void; snapshot: (s: Snapshot) => void; status: (up: boolean) => void }): () => void {
  let es: EventSource | null = null
  let closed = false
  let retry = 500

  const connect = () => {
    if (closed) return
    es = new EventSource(`${base}/api/events?token=${encodeURIComponent(token)}`)
    es.addEventListener('snapshot', (m) => {
      retry = 500
      on.status(true)
      on.snapshot(JSON.parse((m as MessageEvent).data))
    })
    es.addEventListener('resync', () => api.state().then(on.snapshot).catch(() => {}))
    es.onmessage = (m) => {
      try {
        on.event(JSON.parse(m.data))
      } catch {
        /* a malformed frame is not worth dying for */
      }
    }
    es.onerror = () => {
      on.status(false)
      es?.close()
      if (!closed) setTimeout(connect, (retry = Math.min(retry * 1.6, 5000)))
    }
  }
  connect()
  return () => {
    closed = true
    es?.close()
  }
}
