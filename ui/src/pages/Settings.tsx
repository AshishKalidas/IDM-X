import { FolderOpen, Download } from 'lucide-react'
import { useEffect } from 'react'
import BrowserSetup from '../components/BrowserSetup'
import { useState } from 'react'
import { api, isTauri } from '../lib/api'
import { bytes, parseSize } from '../lib/format'
import { useStore, type ThemePref } from '../lib/store'
import { Button, Group, Row, Segmented, Slider, Switch, TextField } from '../components/ui'

export default function SettingsPage() {
  const s = useStore((st) => st.settings)
  const save = useStore((st) => st.updateSettings)
  const theme = useStore((st) => st.theme)
  const setTheme = useStore((st) => st.setTheme)
  const stats = useStore((st) => st.stats)
  const version = useStore((st) => st.version)
  const local = useStore((st) => st.local)
  const [limit, setLimit] = useState(s?.globalSpeedLimit ? bytes(s.globalSpeedLimit, 0).replace(' ', '').replace('B', '') : '')
  const [proxy, setProxy] = useState(s?.proxy ?? '')
  const [ua, setUa] = useState(s?.userAgent ?? '')
  const [dir, setDir] = useState(s?.downloadDir ?? '')
  const [repo, setRepo] = useState(s?.updateRepo ?? '')
  const [checking, setChecking] = useState(false)
  const update = useStore((st) => st.update)
  const toast = useStore((st) => st.toast)
  const tools = useStore((st) => st.tools)
  const setTools = useStore((st) => st.setTools)
  useEffect(() => {
    void api.tools().then(setTools).catch(() => {})
  }, [setTools])
  const [cache, setCache] = useState<number | null>(null)
  useEffect(() => {
    void api.cache().then((c) => setCache(c.bytes)).catch(() => {})
  }, [])
  if (!s) return null
  const toolRow = (title: string, which: 'ytdlp' | 'ffmpeg' | 'js', info: { version: string; managed: boolean } | null | undefined, hint: string) => (
    <Row title={title} hint={hint}>
      <span className="mono muted-note">{info ? `${info.version}${info.managed ? ' · managed' : ''}` : 'not installed'}</span>
      {local && (!info || info.managed) && (
        <Button variant="glass" size="sm" icon={<Download size={14} />} disabled={!!tools?.installing} onClick={() => void api.installTool(which).catch(() => {})}>
          {tools?.installing?.tool === which ? 'Installing…' : info ? 'Update' : 'Install'}
        </Button>
      )}
    </Row>
  )

  async function pick() {
    if (!isTauri()) return
    const { open } = await import('@tauri-apps/plugin-dialog')
    const d = await open({ directory: true, multiple: false, defaultPath: s?.downloadDir })
    if (typeof d === 'string') {
      setDir(d)
      void save({ downloadDir: d })
    }
  }

  return (
    <div className="page-pad">
      <header className="page-head" data-tauri-drag-region>
        <h1>Settings</h1>
      </header>
      <div className="scroll narrow">
        <Group title="Appearance" footer="Dark mode is true black, made for OLED. Light mode is true white.">
          <Row title="Theme">
            <Segmented<ThemePref> value={theme} onChange={setTheme} options={[{ value: 'system', label: 'Automatic' }, { value: 'light', label: 'Light' }, { value: 'dark', label: 'Dark' }]} />
          </Row>
        </Group>

        <Group title="Downloads">
          <Row title="Save to">
            <TextField value={dir} onChange={setDir} mono width={260} onBlur={() => dir !== s.downloadDir && void save({ downloadDir: dir })} />
            {isTauri() && local && (
              <Button variant="glass" size="sm" icon={<FolderOpen size={14} />} onClick={pick}>
                Choose
              </Button>
            )}
          </Row>
          <Row title="At the same time" hint="How many files download concurrently; the rest wait in the queue.">
            <Slider value={s.maxConcurrentJobs} min={1} max={10} onChange={(v) => save({ maxConcurrentJobs: v })} />
          </Row>
          <Row title="Connections per network" hint="Where each network starts. IDMX ramps up on its own while it helps.">
            <Slider value={s.defaultStreams} min={1} max={32} onChange={(v) => save({ defaultStreams: v })} />
          </Row>
          <Row title="Most connections per network" hint="A ceiling, so a polite server never gets flooded.">
            <Slider value={s.maxStreams} min={4} max={64} onChange={(v) => save({ maxStreams: v })} />
          </Row>
          <Row title="Overall speed limit" hint="Across everything, on every network.">
            <TextField value={limit} onChange={setLimit} placeholder="Unlimited" mono width={120} invalid={!!limit && parseSize(limit) == null} onBlur={() => save({ globalSpeedLimit: limit ? parseSize(limit) : null })} />
          </Row>
        </Group>

        <Group title="Browser extension" footer="Grabs downloads, videos and magnet links from Chrome, Edge, Brave and other Chromium browsers.">
          <div style={{ padding: '4px 16px 12px' }}>
            <BrowserSetup />
          </div>
        </Group>

        <Group title="Video" footer="yt-dlp runs on this computer. Videos that are plain files use every network at once; streams (HLS/DASH) are fetched by yt-dlp.">
          <Row title="Download video from supported sites">
            <Switch checked={s.media.enabled} onChange={(v) => save({ media: { enabled: v } })} />
          </Row>
          <Row title="Default quality">
            <Segmented<string> value={s.media.defaultQuality} onChange={(v) => save({ media: { defaultQuality: v } })} options={[{ value: 'best', label: 'Best' }, { value: '1080', label: '1080p' }, { value: '720', label: '720p' }, { value: 'audio', label: 'Audio' }]} />
          </Row>
          {toolRow('yt-dlp', 'ytdlp', tools?.ytdlp, 'Finds the video behind a page.')}
          {toolRow('ffmpeg', 'ffmpeg', tools?.ffmpeg, 'Joins separate video and audio into one file.')}
          {toolRow('JavaScript runtime', 'js', tools?.js, 'Needed by YouTube for full-quality formats.')}
        </Group>

        <Group title="Torrents">
          <Row title="Enable torrents">
            <Switch checked={s.torrent.enabled} onChange={(v) => save({ torrent: { enabled: v } })} />
          </Row>
          <Row title="Use every network" hint="Multi-file torrents are divided between your networks, each fetching its own files. A single-file torrent can’t be divided and uses one network.">
            <Switch checked={s.torrent.multiNetwork} onChange={(v) => save({ torrent: { multiNetwork: v } })} />
          </Row>
          <Row title="Keep sharing after download" hint="Seeding helps the swarm; it stops at the ratio below.">
            <Switch checked={s.torrent.seed} onChange={(v) => save({ torrent: { seed: v } })} />
          </Row>
          <Row title="Stop seeding at ratio" hint="0 = keep going.">
            <Slider value={Math.round(s.torrent.seedRatio * 10)} min={0} max={50} onChange={(v) => save({ torrent: { seedRatio: v / 10 } })} />
          </Row>
          <Row title="Find peers with DHT">
            <Switch checked={s.torrent.dht} onChange={(v) => save({ torrent: { dht: v } })} />
          </Row>
        </Group>

        <Group title="Command line" footer="The same engine from any terminal, with or without this window open. Run idmx --help for everything it can do.">
          <Row title="Install the idmx command" hint="From the IDMX project folder:">
            <code className="mono">./scripts/install-cli.sh</code>
            <Button variant="glass" size="sm" onClick={() => void navigator.clipboard.writeText('./scripts/install-cli.sh').then(() => toast({ kind: 'info', title: 'Copied' }))}>
              Copy
            </Button>
          </Row>
          <Row title="Try it">
            <code className="mono">idmx nets</code>
            <code className="mono">idmx get &lt;link&gt;</code>
          </Row>
        </Group>

        <Group title="Storage">
          <Row title="Cache" hint="Dropped .torrent files and scratch space, kept in a hidden .idmx-cache folder inside your downloads folder. Finished downloads are never touched.">
            <span className="mono muted-note">{cache == null ? '…' : bytes(cache)}</span>
            {local && (
              <Button
                variant="glass"
                size="sm"
                disabled={!cache}
                onClick={async () => {
                  const r = await api.clearCache().catch(() => null)
                  if (r) {
                    setCache(r.bytes)
                    toast({ kind: 'info', title: `Cleared ${bytes(r.freed)}` })
                  }
                }}
              >
                Clear cache
              </Button>
            )}
          </Row>
        </Group>

        <Group title="Reliability">
          <Row title="Race the slow tail" hint="Near the end, a free connection can duplicate a block that’s dragging so one slow link never holds everything back.">
            <Switch checked={s.hedging} onChange={(v) => save({ hedging: v })} />
          </Row>
          <Row title="Don’t resume a changed file" hint="If a file is replaced on the server while a download is paused, stop instead of starting over. Starting over is always one click away.">
            <Switch checked={s.refuseChangedResume} onChange={(v) => save({ refuseChangedResume: v })} />
          </Row>
          <Row title="Fingerprint finished files" hint="Compute a SHA-256 of every download (up to 4 GB) and check it against the server’s checksum when it publishes one.">
            <Switch checked={s.hashOnComplete} onChange={(v) => save({ hashOnComplete: v })} />
          </Row>
        </Group>

        <Group title="Network">
          <Row title="Proxy" hint="http://, https:// or socks5:// — leave empty to use your system settings.">
            <TextField value={proxy} onChange={setProxy} placeholder="None" mono width={260} onBlur={() => save({ proxy: proxy || null })} />
          </Row>
          <Row title="User agent">
            <TextField value={ua} onChange={setUa} placeholder="IDMX default" mono width={260} onBlur={() => save({ userAgent: ua || null })} />
          </Row>
        </Group>

        <Group title="Notifications">
          <Row title="Tell me when downloads finish or fail">
            <Switch checked={s.notifications} onChange={(v) => save({ notifications: v })} />
          </Row>
          <Row title="Offer links from the clipboard" hint="When you come back to IDMX with a link copied, offer to download it.">
            <Switch checked={s.clipboardWatch} onChange={(v) => save({ clipboardWatch: v })} />
          </Row>
        </Group>

        <Group title="Updates" footer="IDMX asks GitHub for the latest release once a day. Nothing is downloaded or installed without you.">
          <Row title="Look for new releases in" hint="A GitHub repository as owner/name. Leave empty to never check.">
            <TextField value={repo} onChange={setRepo} placeholder="owner/name" mono width={200} onBlur={() => repo !== s.updateRepo && void save({ updateRepo: repo })} />
            <Button
              variant="glass"
              size="sm"
              disabled={checking || !repo.trim()}
              onClick={async () => {
                setChecking(true)
                try {
                  if (repo !== s.updateRepo) await save({ updateRepo: repo })
                  const r = await api.checkUpdate()
                  toast({ kind: 'info', title: r ? `IDMX ${r.version} is available` : 'You’re up to date' })
                } catch (e) {
                  toast({ kind: 'error', title: 'Couldn’t check', body: String((e as Error).message ?? e) })
                } finally {
                  setChecking(false)
                }
              }}
            >
              {checking ? 'Checking…' : 'Check now'}
            </Button>
          </Row>
          {update && (
            <Row title={`IDMX ${update.version} is available`} hint="Open the release page to download it.">
              <Button variant="primary" size="sm" onClick={() => void import('../lib/api').then((m) => m.openExternal(update.url))}>
                View release
              </Button>
            </Row>
          )}
        </Group>

        <Group title="About">
          <Row title="IDMX" hint={`Version ${version || '—'}`}>
            <span className="mono muted">{bytes(stats.totalBytes)} downloaded, all time</span>
          </Row>
        </Group>
      </div>
    </div>
  )
}
