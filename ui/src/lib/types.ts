// Mirrors crates/core/src/types.rs. Keep the two in step.

export type IfKind = 'wifi' | 'ethernet' | 'cellular' | 'tether' | 'vpn' | 'bridge' | 'virtual' | 'loopback' | 'unknown'
export type NetState = 'idle' | 'active' | 'unreachable' | 'failed' | 'capped' | 'offline' | 'disabled'
export type JobStatus =
  | 'queued'
  | 'probing'
  | 'downloading'
  | 'paused'
  | 'verifying'
  | 'processing'
  | 'seeding'
  | 'completed'
  | 'failed'
  | 'cancelled'

export interface NetStat {
  id: string
  label: string
  kind: IfKind
  bytes: number
  speed: number
  streams: number
  active: number
  latencyMs: number
  health: number
  errors: number
  state: NetState
  error?: string | null
  enabled: boolean
}

export interface StreamStat {
  id: number
  net: string
  state: 'idle' | 'downloading' | 'racing' | 'retrying' | 'parked'
  block: number | null
  bytes: number
  speed: number
}

export interface UpdateInfo {
  version: string
  url: string
  dismissed: boolean
}

export interface ChecksumResult {
  algo: string
  actual: string | null
  expected: string | null
  verified: boolean | null
  source: string | null
}

export interface BasicAuth {
  user: string
  password: string
}

export interface JobOptions {
  interfaces?: string[] | null
  streamsPerNetwork?: number | null
  speedLimit?: number | null
  headers?: [string, string][]
  cookies?: string | null
  auth?: BasicAuth | null
  bearer?: string | null
  referer?: string | null
  userAgent?: string | null
  proxy?: string | null
  checksum?: string | null
  fileName?: string | null
  scheduledWindow?: boolean
  skipPost?: boolean
  torrentFiles?: number[] | null
  media?: boolean | null
  mediaFormat?: string | null
  forceHttp?: boolean
  restartIfChanged?: boolean
}

export type JobKind = 'http' | 'torrent' | 'media'

export interface TorrentFile {
  index: number
  path: string
  size: number
  downloaded: number
  selected: boolean
}

export interface TorrentStatus {
  infoHash: string
  peers: number
  knownPeers: number
  uploaded: number
  uploadSpeed: number
  ratio: number
  state: string
  files: TorrentFile[]
  fetchingMetadata: boolean
}

export interface MediaStatus {
  title: string
  uploader: string | null
  thumbnail: string | null
  duration: number | null
  extractor: string | null
  format: string
  strategy: 'direct' | 'ytdlp'
  parts: number
}

export interface ToolInfo {
  path: string
  version: string
  managed: boolean
}

export interface InstallProgress {
  tool: string
  downloaded: number
  total: number | null
  error: string | null
}

export interface ToolsStatus {
  ytdlp: ToolInfo | null
  ffmpeg: ToolInfo | null
  js: ToolInfo | null
  installing: InstallProgress | null
}

export interface TorrentMeta {
  name: string
  infoHash: string
  total: number
  files: { index: number; path: string; size: number }[]
}

export interface MediaProbe {
  title: string
  uploader: string | null
  thumbnail: string | null
  duration: number | null
  extractor: string | null
  heights: number[]
  format: string
  size: number | null
  direct: boolean
  playlist: number | null
}

export interface ExtensionInfo {
  version: string
  browser: string
  seenAt: number
}

export interface BrowserEntry {
  id: string
  name: string
  installed: boolean
  hostRegistered: boolean
}

export interface BrowserState {
  browsers: BrowserEntry[]
  extensionDir: string | null
  extensionId: string
  extension: ExtensionInfo | null
  registered?: string[]
}

export interface Job {
  id: string
  url: string
  finalUrl: string | null
  fileName: string
  destDir: string
  path: string | null
  status: JobStatus
  size: number | null
  downloaded: number
  speed: number
  eta: number | null
  error: string | null
  resumable: boolean
  priority: number
  startAt: number | null
  createdAt: number
  startedAt: number | null
  completedAt: number | null
  options: JobOptions
  networks: NetStat[]
  blocksTotal: number
  blocksDone: number
  checksum: ChecksumResult | null
  etag: string | null
  lastModified: string | null
  log: string[]
  mime: string | null
  peakStreams: number
  speedHistory: number[]
  kind?: JobKind
  torrent?: TorrentStatus | null
  media?: MediaStatus | null
  parent?: string | null
}

export interface JobTick {
  id: string
  status: JobStatus | null
  downloaded: number
  speed: number
  eta: number | null
  blocksDone: number
  networks: NetStat[]
}

export interface BlockMap {
  blockSize: number
  networks: string[]
  state: number[]
  owner: number[]
  fill: number[]
  /** Blocks several networks each delivered part of: [block, [[network, percent], …]]. */
  split?: [number, [number, number][]][]
}

export interface InterfaceRule {
  enabled?: boolean | null
  label?: string | null
  color?: string | null
  speedLimit?: number | null
  metered: boolean
  dataCap?: number | null
  capPeriod: 'daily' | 'monthly'
  minFileSize?: number | null
  streams?: number | null
}

export interface NetInterface {
  id: string
  label: string
  kind: IfKind
  ipv4: string[]
  ipv6: string[]
  gateway: string | null
  subnets?: string[]
  mac: string | null
  up: boolean
  linkMbps: number | null
  usable: boolean
}

export interface InterfaceInfo extends NetInterface {
  rule: InterfaceRule
  enabled: boolean
  health: number
  latencyMs: number | null
  usedBytes: number
  speed: number
  sharesGatewayWith: string[]
  sharesSubnetWith: string[]
  reachable: boolean | null
  state: NetState
}

export type PostAction =
  | { type: 'moveTo'; dir: string }
  | { type: 'extract'; dir: string | null; deleteArchive: boolean }
  | { type: 'run'; command: string }
  | { type: 'reveal' }
  | { type: 'notify'; message: string | null }

export interface RuleMatch {
  extensions: string[]
  nameRegex: string | null
  host: string | null
  minSize: number | null
  maxSize: number | null
}

export interface PostRule {
  id: string
  name: string
  enabled: boolean
  when: RuleMatch
  actions: PostAction[]
}

export interface Settings {
  downloadDir: string
  maxConcurrentJobs: number
  defaultStreams: number
  maxStreams: number
  globalSpeedLimit: number | null
  interfaces: Record<string, InterfaceRule>
  proxy: string | null
  userAgent: string | null
  schedule: { enabled: boolean; startMinute: number; endMinute: number }
  postRules: PostRule[]
  onQueueEmpty: 'nothing' | 'sleep' | 'quit'
  remote: { enabled: boolean; port: number; lan: boolean; token?: string }
  notifications: boolean
  hedging: boolean
  hashOnComplete: boolean
  hashLimit: number
  clipboardWatch: boolean
  updateRepo: string
  dismissedUpdate: string | null
  refuseChangedResume: boolean
  onboarded: boolean
  torrent: {
    enabled: boolean
    network: string | null
    multiNetwork: boolean
    seed: boolean
    seedRatio: number
    seedMinutes: number
    dht: boolean
    listenPort: number | null
    uploadLimit: number | null
    peerLimit: number | null
  }
  media: {
    enabled: boolean
    ytdlpPath: string
    ffmpegPath: string
    jsRuntimePath: string
    defaultQuality: string
    embedExtras: boolean
    cookiesFromBrowser: string
  }
}

export interface GlobalStats {
  speed: number
  active: number
  queued: number
  completed: number
  failed: number
  totalBytes: number
  sessionBytes: number
}

export interface ProbeInfo {
  url: string
  finalUrl: string
  fileName: string
  size: number | null
  ranges: boolean
  mime: string | null
  etag: string | null
  lastModified: string | null
  status: number
  ttfbMs: number
}

export interface SpeedTestNet {
  id: string
  label: string
  kind: IfKind
  mbps: number
  latencyMs: number | null
  error: string | null
}

export interface SpeedTestReport {
  phase: string
  done: boolean
  nets: SpeedTestNet[]
  combinedMbps: number | null
  sumMbps: number
  gain: number | null
  warnings: string[]
}

export interface EnduranceNet {
  id: string
  label: string
  kind: IfKind | null
  mbps: number
  avgMbps: number
  minMbps: number
  maxMbps: number
  lowMbps: number
  rttMs: number | null
  minRttMs: number | null
  avgRttMs: number | null
  p95RttMs: number | null
  maxRttMs: number | null
  jitterMs: number | null
  sent: number
  lost: number
  lossPct: number
  longestLossRun: number
  stallSecs: number
  errors: number
  bytes: number
  cycles: number
  cyclePct: number
}

export interface EndurancePoint {
  t: number
  mbps: number
  rttMs: number | null
  lost: boolean
}

export interface EnduranceSummary {
  name: string
  startedAt: number
  endedAt: number
  durationSecs: number
  url: string
  totalBytes: number
  nets: EnduranceNet[]
  notes: string[]
  csvPath: string
  csvBytes: number
}

export interface EnduranceStatus {
  running: boolean
  phase: string
  startedAt: number
  elapsedSecs: number
  cycleBytes: number
  totalBytes: number
  logBytes: number
  nets: EnduranceNet[]
  recent: EndurancePoint[]
  summary: EnduranceSummary | null
  warnings: string[]
}

export interface AddRequest {
  urls: string[]
  destDir?: string | null
  options?: JobOptions
  paused?: boolean
  priority?: number
  startAt?: number | null
}

export type ServerEvent =
  | { type: 'tick'; data: { stats: GlobalStats; jobs: JobTick[] } }
  | { type: 'jobAdded'; data: Job }
  | { type: 'jobUpdated'; data: Job }
  | { type: 'jobRemoved'; data: string }
  | { type: 'interfaces'; data: InterfaceInfo[] }
  | { type: 'settings'; data: Settings }
  | { type: 'speedTest'; data: SpeedTestReport }
  | { type: 'endurance'; data: EnduranceStatus }
  | { type: 'update'; data: UpdateInfo | null }
  | { type: 'tools'; data: ToolsStatus }
  | { type: 'extension'; data: ExtensionInfo | null }
  | { type: 'notice'; data: { level: string; message: string } }
  | { type: 'action'; data: { action: string; path: string | null; jobId: string | null } }

export interface Snapshot {
  version: string
  local: boolean
  platform: string
  arch?: string
  jobs: Job[]
  interfaces: InterfaceInfo[]
  settings: Settings
  stats: GlobalStats
  update: UpdateInfo | null
  extension?: ExtensionInfo | null
}
