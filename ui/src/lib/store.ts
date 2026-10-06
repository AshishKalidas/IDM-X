import { create } from 'zustand'
import { api, bootstrap, openEvents, setToken } from './api'
import type { AddRequest, BlockMap, GlobalStats, InterfaceInfo, Job, JobTick, ServerEvent, Settings, Snapshot, SpeedTestReport, EnduranceStatus, StreamStat, UpdateInfo, ToolsStatus, ExtensionInfo } from './types'

export type Page = 'downloads' | 'torrents' | 'networks' | 'automation' | 'speed' | 'settings'
export type Filter = 'all' | 'active' | 'queued' | 'done'
export type ThemePref = 'system' | 'light' | 'dark'

export interface Sample {
  t: number
  total: number
  nets: Record<string, number>
}

export interface DialogOptions {
  title: string
  body: string
  confirmLabel: string
  cancelLabel?: string
  danger?: boolean
}

export interface Toast {
  id: number
  kind: 'complete' | 'error' | 'info' | 'queue-done'
  title: string
  body?: string
}

const HISTORY = 240
const emptyStats: GlobalStats = { speed: 0, active: 0, queued: 0, completed: 0, failed: 0, totalBytes: 0, sessionBytes: 0 }

interface State {
  phase: 'booting' | 'need-token' | 'unreachable' | 'ready'
  detail?: string
  connected: boolean
  local: boolean
  platform: string
  arch: string
  version: string
  jobs: Record<string, Job>
  interfaces: InterfaceInfo[]
  settings: Settings | null
  stats: GlobalStats
  history: Sample[]
  jobHistory: Record<string, Sample[]>
  maps: Record<string, BlockMap>
  streams: Record<string, StreamStat[]>
  update: UpdateInfo | null
  setTools: (t: ToolsStatus) => void
  tools: ToolsStatus | null
  extension: ExtensionInfo | null
  selected: string | null
  page: Page
  filter: Filter
  query: string
  addOpen: boolean
  addPrefill: string
  paletteOpen: boolean
  toasts: Toast[]
  speed: SpeedTestReport | null
  endurance: EnduranceStatus | null
  theme: ThemePref
  dialog: (DialogOptions & { resolve: (ok: boolean) => void }) | null

  init: () => Promise<void>
  submitToken: (t: string) => Promise<void>
  setPage: (p: Page) => void
  select: (id: string | null) => void
  setFilter: (f: Filter) => void
  setQuery: (q: string) => void
  openAdd: (prefill?: string) => void
  closeAdd: () => void
  setPalette: (open: boolean) => void
  toast: (t: Omit<Toast, 'id'>) => void
  dismissToast: (id: number) => void
  setTheme: (t: ThemePref) => void
  addDownloads: (req: AddRequest) => Promise<Job[]>
  fetchMap: (id: string) => Promise<void>
  fetchStreams: (id: string) => Promise<void>
  updateSettings: (patch: Record<string, unknown>) => Promise<void>
  confirm: (o: DialogOptions) => Promise<boolean>
  answerDialog: (ok: boolean) => void
}

let toastId = 1
let stopEvents: (() => void) | null = null

export const useStore = create<State>((set, get) => ({
  phase: 'booting',
  connected: false,
  local: true,
  platform: '',
  arch: '',
  version: '',
  jobs: {},
  interfaces: [],
  settings: null,
  stats: emptyStats,
  history: [],
  jobHistory: {},
  maps: {},
  streams: {},
  update: null,
  tools: null,
  extension: null,
  selected: null,
  page: 'downloads',
  filter: 'all',
  query: '',
  addOpen: false,
  addPrefill: '',
  paletteOpen: false,
  toasts: [],
  speed: null,
  endurance: null,
  theme: (localStorage.getItem('idmx.theme') as ThemePref) || 'system',
  dialog: null,

  async init() {
    applyTheme(get().theme)
    const b = await bootstrap()
    if (!b.ok) {
      set({ phase: b.reason === 'need-token' ? 'need-token' : 'unreachable', detail: b.detail })
      return
    }
    stopEvents?.()
    stopEvents = openEvents({
      status: (up) => set({ connected: up }),
      snapshot: (s) => {
        applySnapshot(set, s)
        void collectPending(get)
      },
      event: (e) => applyEvent(set, get, e),
    })
  },

  async submitToken(t) {
    setToken(t)
    set({ phase: 'booting' })
    await get().init()
  },

  setPage: (page) => set({ page }),
  select: (selected) => set({ selected }),
  setFilter: (filter) => set({ filter }),
  setQuery: (query) => set({ query }),
  openAdd: (prefill = '') => set({ addOpen: true, addPrefill: prefill, paletteOpen: false }),
  closeAdd: () => set({ addOpen: false, addPrefill: '' }),
  setTools: (t) => set({ tools: t }),
  setPalette: (paletteOpen) => set({ paletteOpen }),

  toast(t) {
    const id = toastId++
    set((s) => ({ toasts: [...s.toasts.slice(-2), { ...t, id }] }))
    setTimeout(() => get().dismissToast(id), t.kind === 'error' ? 7000 : 4200)
  },
  dismissToast: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),

  setTheme(theme) {
    localStorage.setItem('idmx.theme', theme)
    applyTheme(theme)
    set({ theme })
  },

  async addDownloads(req) {
    const jobs = await api.add(req)
    if (jobs.length === 1) set({ selected: jobs[0].id })
    return jobs
  },

  async fetchMap(id) {
    try {
      const m = await api.map(id)
      if (m) set((s) => ({ maps: { ...s.maps, [id]: m } }))
    } catch {
      /* the job may have just been removed */
    }
  },

  async fetchStreams(id) {
    try {
      const list = await api.streams(id)
      set((s) => ({ streams: { ...s.streams, [id]: list } }))
    } catch {
      /* the job may have just been removed */
    }
  },

  confirm: (o) => new Promise<boolean>((resolve) => set({ dialog: { ...o, resolve } })),
  answerDialog(ok) {
    const d = get().dialog
    set({ dialog: null })
    d?.resolve(ok)
  },

  async updateSettings(patch) {
    const s = await api.settings(patch)
    set({ settings: s })
  },
}))

type Set = (fn: Partial<State> | ((s: State) => Partial<State>)) => void

function applySnapshot(set: Set, s: Snapshot) {
  const jobs: Record<string, Job> = {}
  for (const j of s.jobs) jobs[j.id] = j
  set({
    phase: 'ready',
    local: s.local,
    platform: s.platform,
    arch: s.arch ?? '',
    version: s.version,
    jobs,
    interfaces: s.interfaces,
    settings: s.settings,
    stats: s.stats,
    update: s.update ?? null,
    extension: s.extension ?? null,
    connected: true,
  })
}

/** Open the Add sheet for links the operating system handed to the app (a magnet link, a double-clicked .torrent). */
export async function collectPending(get: () => State) {
  try {
    const { items } = await api.pendingAdd()
    if (items.length) get().openAdd(items.join('\n'))
  } catch {
    /* not connected yet — the next snapshot asks again */
  }
}

function applyEvent(set: Set, get: () => State, e: ServerEvent) {
  switch (e.type) {
    case 'tick':
      return applyTick(set, e.data.stats, e.data.jobs)
    case 'jobAdded':
    case 'jobUpdated': {
      const j = e.data
      const prev = get().jobs[j.id]
      set((s) => ({ jobs: { ...s.jobs, [j.id]: j } }))
      if (prev && prev.status !== 'failed' && j.status === 'failed') get().toast({ kind: 'error', title: j.fileName, body: j.error ?? 'Download failed' })
      return
    }
    case 'jobRemoved':
      return set((s) => {
        const { [e.data]: _gone, ...rest } = s.jobs
        const { [e.data]: _h, ...hist } = s.jobHistory
        return { jobs: rest, jobHistory: hist, selected: s.selected === e.data ? null : s.selected }
      })
    case 'interfaces':
      return set({ interfaces: e.data })
    case 'settings':
      return set((s) => ({ settings: s.settings ? { ...e.data, remote: { ...e.data.remote, token: s.settings.remote.token } } : e.data }))
    case 'endurance':
      return set({ endurance: e.data })
    case 'speedTest':
      return set({ speed: e.data })
    case 'action':
      if (e.data.action === 'add') void collectPending(get)
      return
    case 'update':
      return set({ update: e.data })
    case 'tools':
      return set({ tools: e.data })
    case 'extension':
      return set({ extension: e.data })
    case 'notice': {
      const { level, message } = e.data
      if (level === 'complete') get().toast({ kind: 'complete', title: 'Download complete', body: message })
      else if (level === 'queue-done') get().toast({ kind: 'queue-done', title: message })
      else if (level === 'error') get().toast({ kind: 'error', title: 'Download failed', body: message })
      else get().toast({ kind: 'info', title: message })
    }
  }
}

function applyTick(set: Set, stats: GlobalStats, ticks: JobTick[]) {
  const now = Date.now()
  set((s) => {
    const jobs = { ...s.jobs }
    const jobHistory = { ...s.jobHistory }
    const total: Record<string, number> = {}
    for (const t of ticks) {
      const j = jobs[t.id]
      if (!j) continue
      jobs[t.id] = {
        ...j,
        status: t.status ?? j.status,
        downloaded: t.downloaded,
        speed: t.speed,
        eta: t.eta,
        blocksDone: t.blocksDone,
        networks: t.networks,
      }
      const nets: Record<string, number> = {}
      for (const n of t.networks) {
        nets[n.id] = n.speed
        total[n.id] = (total[n.id] ?? 0) + n.speed
      }
      const h = jobHistory[t.id] ?? []
      jobHistory[t.id] = [...h.slice(-(HISTORY - 1)), { t: now, total: t.speed, nets }]
    }
    const sample: Sample = { t: now, total: stats.speed, nets: total }
    return { jobs, jobHistory, stats, history: [...s.history.slice(-(HISTORY - 1)), sample] }
  })
}

function applyTheme(pref: ThemePref) {
  const dark = pref === 'dark' || (pref === 'system' && matchMedia('(prefers-color-scheme: dark)').matches)
  document.documentElement.dataset.theme = dark ? 'dark' : 'light'
  document.documentElement.style.background = dark ? '#000' : '#fff'
}

matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => {
  if (useStore.getState().theme === 'system') applyTheme('system')
})

// ───────────── selectors ─────────────

export const RUNNING = ['probing', 'downloading', 'verifying', 'processing', 'seeding']

export function sortJobs(jobs: Job[]): Job[] {
  const rank = (j: Job) =>
    RUNNING.includes(j.status) ? 0 : j.status === 'queued' ? 1 : j.status === 'paused' ? 2 : j.status === 'failed' ? 3 : 4
  return [...jobs].sort((a, b) => {
    const r = rank(a) - rank(b)
    if (r) return r
    if (rank(a) === 4) return (b.completedAt ?? 0) - (a.completedAt ?? 0)
    return b.priority - a.priority || a.createdAt - b.createdAt
  })
}

export function matches(j: Job, f: Filter, q: string): boolean {
  if (q && !(j.fileName.toLowerCase().includes(q) || j.url.toLowerCase().includes(q))) return false
  switch (f) {
    case 'active':
      return RUNNING.includes(j.status) || j.status === 'paused'
    case 'queued':
      return j.status === 'queued'
    case 'done':
      return j.status === 'completed' || j.status === 'failed' || j.status === 'cancelled'
    default:
      return true
  }
}
