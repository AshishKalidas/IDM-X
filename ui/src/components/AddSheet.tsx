import { AnimatePresence, motion } from 'motion/react'
import { ChevronDown, ClipboardPaste, FolderOpen, Link2, Zap, AlertCircle, Layers, CheckCircle2 } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { api, isTauri } from '../lib/api'
import { bytes, parseSize } from '../lib/format'
import { useStore } from '../lib/store'
import { infoColor } from '../lib/kinds'
import type { AddRequest, JobOptions, MediaProbe, ProbeInfo, TorrentMeta } from '../lib/types'
import FileGlyph from './FileGlyph'
import { Download, Magnet, Film } from 'lucide-react'
import { Button, Chip, Segmented, Slider, Switch, TextField } from './ui'

type Start = 'now' | 'paused' | 'later'

const isTorrentLink = (t: string) => /^magnet:\?/i.test(t) || /\.torrent(\?|#|$)/i.test(t)
const isVideoSite = (t: string) => !/^https?:\/\/[^/]+\/?$/.test(t) && /^https?:\/\/(www\.|m\.)?(youtube\.com|youtu\.be|vimeo\.com|twitch\.tv|dailymotion\.com|tiktok\.com|x\.com|twitter\.com|soundcloud\.com|reddit\.com|bilibili\.com)\//i.test(t)
const QUALITIES = [
  { value: 'best', label: 'Best' },
  { value: '1080', label: '1080p' },
  { value: '720', label: '720p' },
  { value: '480', label: '480p' },
  { value: 'audio', label: 'Audio' },
  { value: 'audio216', label: 'MP3 216k' },
  { value: 'audio320', label: 'MP3 320k' },
]

export default function AddSheet() {
  const open = useStore((s) => s.addOpen)
  return <AnimatePresence>{open && <Sheet />}</AnimatePresence>
}

function Sheet() {
  const close = useStore((s) => s.closeAdd)
  const prefill = useStore((s) => s.addPrefill)
  const settings = useStore((s) => s.settings)
  const interfaces = useStore((s) => s.interfaces)
  const add = useStore((s) => s.addDownloads)
  const toast = useStore((s) => s.toast)
  const local = useStore((s) => s.local)

  const [text, setText] = useState(prefill)
  const [dest, setDest] = useState(settings?.downloadDir ?? '')
  const [more, setMore] = useState(false)
  const [probe, setProbe] = useState<{ info?: ProbeInfo; error?: string; loading: boolean } | null>(null)
  const [batch, setBatch] = useState<{ total: number; items: { url: string }[] } | null>(null)
  const [nets, setNets] = useState<string[] | null>(null)
  const [streams, setStreams] = useState(settings?.defaultStreams ?? 8)
  const [limit, setLimit] = useState('')
  const [headers, setHeaders] = useState('')
  const [cookies, setCookies] = useState('')
  const [user, setUser] = useState('')
  const [pass, setPass] = useState('')
  const [bearer, setBearer] = useState('')
  const [referer, setReferer] = useState('')
  const [proxy, setProxy] = useState('')
  const [checksum, setChecksum] = useState('')
  const [fileName, setFileName] = useState('')
  const [start, setStart] = useState<Start>('now')
  const [when, setWhen] = useState('')
  const [high, setHigh] = useState(false)
  const [win, setWin] = useState(false)
  const [busy, setBusy] = useState(false)
  const [tor, setTor] = useState<{ meta?: TorrentMeta; error?: string; loading: boolean } | null>(null)
  const [picked, setPicked] = useState<Set<number>>(new Set())
  const [vid, setVid] = useState<{ info?: MediaProbe; error?: string; loading: boolean } | null>(null)
  const [quality, setQuality] = useState(settings?.media.defaultQuality ?? 'best')
  const tools = useStore((s) => s.tools)
  const setTools = useStore((s) => s.setTools)
  const area = useRef<HTMLTextAreaElement>(null)

  useEffect(() => area.current?.focus(), [])
  useEffect(() => {
    const k = (e: KeyboardEvent) => {
      if (e.key === 'Escape') close()
      if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') void submit()
    }
    window.addEventListener('keydown', k)
    return () => window.removeEventListener('keydown', k)
  })

  // Debounced look at what was pasted: expand patterns, then probe the first link.
  useEffect(() => {
    const t = text.trim()
    if (!t) {
      setProbe(null)
      setBatch(null)
      return
    }
    let dead = false
    const first = t.split('\n')[0].trim()
    if (isTorrentLink(first)) {
      setProbe(null)
      setBatch(null)
      setVid(null)
      setTor({ loading: true })
      const id = setTimeout(async () => {
        try {
          const meta = await api.torrentInspect(first)
          if (dead) return
          setTor({ meta, loading: false })
          setPicked(new Set(meta.files.map((f) => f.index)))
        } catch (e) {
          if (!dead) setTor({ error: String((e as Error).message ?? e), loading: false })
        }
      }, 300)
      return () => {
        dead = true
        clearTimeout(id)
      }
    }
    setTor(null)
    if (isVideoSite(first) && t.split('\n').length === 1) {
      setProbe(null)
      setBatch({ total: 1, items: [{ url: first }] })
      setVid({ loading: true })
      void api.tools().then(setTools).catch(() => {})
      const id = setTimeout(async () => {
        try {
          const info = await api.mediaProbe(first, { referer: referer || undefined, cookies: cookies || undefined })
          if (!dead) setVid({ info, loading: false })
        } catch (e) {
          if (!dead) setVid({ error: String((e as Error).message ?? e), loading: false })
        }
      }, 300)
      return () => {
        dead = true
        clearTimeout(id)
      }
    }
    setVid(null)
    const id = setTimeout(async () => {
      try {
        const ex = await api.expand(t)
        if (dead) return
        setBatch(ex)
        if (!ex.items.length) return setProbe(null)
        setProbe({ loading: true })
        const info = await api.probe(ex.items[0].url, { referer: referer || undefined, cookies: cookies || undefined, headers: parseHeaders(headers) })
        if (!dead) setProbe({ info, loading: false })
      } catch (e) {
        if (!dead) setProbe({ error: String((e as Error).message ?? e), loading: false })
      }
    }, 450)
    return () => {
      dead = true
      clearTimeout(id)
    }
  }, [text, referer, cookies, headers])

  const usable = useMemo(() => interfaces.filter((i) => i.usable), [interfaces])
  const total = batch?.total ?? 0

  const options = (): JobOptions => ({
    interfaces: nets,
    streamsPerNetwork: streams !== settings?.defaultStreams ? streams : null,
    speedLimit: limit ? parseSize(limit) : null,
    headers: parseHeaders(headers),
    cookies: cookies || null,
    auth: user ? { user, password: pass } : null,
    bearer: bearer || null,
    referer: referer || null,
    proxy: proxy || null,
    checksum: checksum || null,
    fileName: total <= 1 && fileName ? fileName : null,
    scheduledWindow: win,
    ...(tor?.meta && picked.size < tor.meta.files.length ? { torrentFiles: [...picked].sort((a, b) => a - b) } : {}),
    ...(vid ? { media: true, mediaFormat: quality } : {}),
  })

  async function submit() {
    if (!text.trim() || busy || (tor?.meta && picked.size === 0)) return
    setBusy(true)
    try {
      const req: AddRequest = {
        urls: [text],
        destDir: dest && dest !== settings?.downloadDir ? dest : null,
        options: options(),
        paused: start === 'paused',
        priority: high ? 10 : 0,
        startAt: start === 'later' && when ? Math.floor(new Date(when).getTime() / 1000) : null,
      }
      const jobs = await add(req)
      toast({ kind: 'info', title: jobs.length === 0 ? 'Already in your list' : jobs.length === 1 ? 'Download added' : `${jobs.length} downloads added` })
      close()
    } catch (e) {
      toast({ kind: 'error', title: "Couldn't add that", body: String((e as Error).message ?? e) })
    } finally {
      setBusy(false)
    }
  }

  async function pickFolder() {
    if (!isTauri()) return
    const { open } = await import('@tauri-apps/plugin-dialog')
    const d = await open({ directory: true, multiple: false, defaultPath: dest || undefined })
    if (typeof d === 'string') setDest(d)
  }

  async function paste() {
    try {
      setText(await navigator.clipboard.readText())
    } catch {
      area.current?.focus()
    }
  }

  return (
    <motion.div className="scrim" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} onMouseDown={(e) => e.target === e.currentTarget && close()}>
      <motion.div className="sheet" initial={{ opacity: 0, y: 30, scale: 0.96 }} animate={{ opacity: 1, y: 0, scale: 1 }} exit={{ opacity: 0, y: 16, scale: 0.97, transition: { duration: 0.14 } }} transition={{ type: 'spring', stiffness: 460, damping: 36 }}>
        <div className="sheet-head">
          <h2>New download</h2>
          <Button variant="plain" size="sm" icon={<ClipboardPaste size={14} />} onClick={paste}>
            Paste
          </Button>
        </div>

        <textarea ref={area} className="field add-text mono" rows={4} value={text} onChange={(e) => setText(e.target.value)} placeholder={'https://example.com/file.iso\nhttps://cdn.example.com/photo[001-120].jpg\n…or paste a whole list'} spellCheck={false} />

        <AnimatePresence initial={false} mode="wait">
          {probe && (
            <motion.div key={probe.loading ? 'l' : probe.error ? 'e' : 'i'} className={`probe ${probe.error ? 'bad' : ''}`} initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: 'auto' }} exit={{ opacity: 0, height: 0 }}>
              {probe.loading ? (
                <>
                  <i className="spin" /> <span>Checking the link…</span>
                </>
              ) : probe.error ? (
                <>
                  <AlertCircle size={16} /> <span>{probe.error}</span>
                </>
              ) : probe.info ? (
                <>
                  <FileGlyph name={probe.info.fileName} size={34} />
                  <div className="probe-main">
                    <b>{total > 1 ? `${total} files` : probe.info.fileName}</b>
                    <span className="mono">
                      {total > 1 ? `first: ${probe.info.fileName} · ` : ''}
                      {bytes(probe.info.size)} · {probe.info.ttfbMs} ms
                    </span>
                  </div>
                  {probe.info.ranges ? (
                    <span className="badge ok">
                      <Zap size={11} /> Multi-network
                    </span>
                  ) : (
                    <span className="badge warn">Single connection</span>
                  )}
                </>
              ) : null}
            </motion.div>
          )}
        </AnimatePresence>
        {tor && (
          <div className="probe">
            {tor.loading ? (
              <>
                <i className="spin" /> <span>Reading the torrent… magnet links may take a moment to find peers</span>
              </>
            ) : tor.error ? (
              <>
                <AlertCircle size={16} /> <span>{tor.error}</span>
              </>
            ) : tor.meta ? (
              <div style={{ width: '100%' }}>
                <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                  <Magnet size={18} />
                  <div className="probe-main">
                    <b>{tor.meta.name}</b>
                    <span className="mono">
                      {tor.meta.files.length} file{tor.meta.files.length === 1 ? '' : 's'} · {bytes(tor.meta.total)} · {picked.size} selected
                    </span>
                  </div>
                  <span className="badge ok">Torrent</span>
                </div>
                {tor.meta.files.length > 1 && (
                  <div style={{ maxHeight: 160, overflow: 'auto', marginTop: 8 }}>
                    {tor.meta.files.map((f) => (
                      <label key={f.index} style={{ display: 'flex', gap: 8, padding: '3px 0', fontSize: 12 }}>
                        <input
                          type="checkbox"
                          checked={picked.has(f.index)}
                          onChange={() => setPicked((p) => { const n = new Set(p); if (n.has(f.index)) n.delete(f.index); else n.add(f.index); return n })}
                        />
                        <span style={{ flex: 1, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{f.path}</span>
                        <span className="mono">{bytes(f.size)}</span>
                      </label>
                    ))}
                  </div>
                )}
              </div>
            ) : null}
          </div>
        )}
        {vid && (
          <div className={`probe ${vid.error ? 'bad' : ''}`}>
            {vid.loading ? (
              <>
                <i className="spin" /> <span>Looking up the video…</span>
              </>
            ) : vid.error ? (
              <>
                <AlertCircle size={16} /> <span>{vid.error}</span>
                {!tools?.ytdlp && (
                  <Button size="sm" variant="primary" icon={<Download size={13} />} onClick={() => void api.installTool('ytdlp').catch(() => {})}>
                    {tools?.installing ? 'Installing…' : 'Install video support'}
                  </Button>
                )}
              </>
            ) : vid.info ? (
              <div style={{ width: '100%' }}>
                <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                  <Film size={18} />
                  <div className="probe-main">
                    <b>{vid.info.title}</b>
                    <span className="mono">
                      {[vid.info.uploader, vid.info.extractor, vid.info.duration ? `${Math.floor(vid.info.duration / 60)}:${String(vid.info.duration % 60).padStart(2, '0')}` : null].filter(Boolean).join(' · ')}
                    </span>
                  </div>
                  <span className={`badge ${vid.info.direct ? 'ok' : 'warn'}`}>{vid.info.direct ? 'Multi-network' : 'Stream'}</span>
                </div>
                <div className="chips" style={{ marginTop: 8 }}>
                  {QUALITIES.filter((q) => q.value === 'best' || q.value.startsWith('audio') || vid.info!.heights.some((h) => h >= Number(q.value))).map((q) => (
                    <Chip key={q.value} on={quality === q.value} onClick={() => setQuality(q.value)}>
                      {q.label}
                    </Chip>
                  ))}
                </div>
              </div>
            ) : null}
          </div>
        )}
        {total > 1 && (
          <div className="batch">
            <Layers size={14} />
            <span>
              <b>{total}</b> downloads will be added
              {total > 200 && ' (list truncated in preview)'}
            </span>
          </div>
        )}

        <div className="sheet-row">
          <label>Save to</label>
          <div className="dest">
            <TextField value={dest} onChange={setDest} mono placeholder="Downloads folder" width="100%" />
            {isTauri() && local && (
              <Button variant="glass" size="sm" icon={<FolderOpen size={14} />} onClick={pickFolder}>
                Choose
              </Button>
            )}
          </div>
        </div>

        <div className="sheet-row">
          <label>Networks</label>
          <div className="chips">
            <Chip on={nets === null} onClick={() => setNets(null)}>
              All enabled
            </Chip>
            {usable.map((i) => (
              <Chip
                key={i.id}
                color={infoColor(i)}
                on={nets?.includes(i.id) ?? false}
                onClick={() => setNets((cur) => (cur ?? []).includes(i.id) ? ((cur ?? []).filter((x) => x !== i.id).length ? (cur ?? []).filter((x) => x !== i.id) : null) : [...(cur ?? []), i.id])}
              >
                {i.rule.label || i.label}
              </Chip>
            ))}
          </div>
        </div>

        <div className="sheet-row">
          <label>Start</label>
          <div className="start">
            <Segmented<Start>
              size="sm"
              value={start}
              onChange={setStart}
              options={[
                { value: 'now', label: 'Now' },
                { value: 'paused', label: 'Paused' },
                { value: 'later', label: 'Later' },
              ]}
            />
            {start === 'later' && <input className="field" type="datetime-local" value={when} onChange={(e) => setWhen(e.target.value)} />}
          </div>
        </div>

        <button className="more-toggle" onClick={() => setMore(!more)}>
          <ChevronDown size={14} style={{ transform: more ? 'rotate(180deg)' : 'none' }} />
          Advanced
        </button>
        <AnimatePresence initial={false}>
          {more && (
            <motion.div className="advanced" initial={{ height: 0, opacity: 0 }} animate={{ height: 'auto', opacity: 1 }} exit={{ height: 0, opacity: 0 }} transition={{ type: 'spring', stiffness: 400, damping: 40 }}>
              <div className="adv-grid">
                <div className="adv-field wide">
                  <label>Connections per network</label>
                  <Slider value={streams} min={1} max={settings?.maxStreams ?? 32} onChange={setStreams} />
                </div>
                <div className="adv-field">
                  <label>Speed limit</label>
                  <TextField value={limit} onChange={setLimit} placeholder="e.g. 5M" mono width="100%" invalid={!!limit && parseSize(limit) == null} />
                </div>
                {total <= 1 && (
                  <div className="adv-field">
                    <label>Save as</label>
                    <TextField value={fileName} onChange={setFileName} placeholder="file name" width="100%" />
                  </div>
                )}
                <div className="adv-field wide">
                  <label>Verify checksum</label>
                  <TextField value={checksum} onChange={setChecksum} placeholder="sha256:…  or  auto" mono width="100%" />
                </div>
                <div className="adv-field wide">
                  <label>Headers</label>
                  <textarea className="field mono" rows={2} value={headers} onChange={(e) => setHeaders(e.target.value)} placeholder={'Name: value'} />
                </div>
                <div className="adv-field">
                  <label>Cookie</label>
                  <TextField value={cookies} onChange={setCookies} placeholder="a=1; b=2" mono width="100%" />
                </div>
                <div className="adv-field">
                  <label>Referer</label>
                  <TextField value={referer} onChange={setReferer} placeholder="https://…" mono width="100%" />
                </div>
                <div className="adv-field">
                  <label>Username</label>
                  <TextField value={user} onChange={setUser} width="100%" />
                </div>
                <div className="adv-field">
                  <label>Password</label>
                  <TextField value={pass} onChange={setPass} type="password" width="100%" />
                </div>
                <div className="adv-field">
                  <label>Bearer token</label>
                  <TextField value={bearer} onChange={setBearer} type="password" mono width="100%" />
                </div>
                <div className="adv-field">
                  <label>Proxy</label>
                  <TextField value={proxy} onChange={setProxy} placeholder="socks5://…" mono width="100%" />
                </div>
                <div className="adv-field inline">
                  <label>Jump the queue</label>
                  <Switch checked={high} onChange={setHigh} />
                </div>
                <div className="adv-field inline">
                  <label>Only in schedule window</label>
                  <Switch checked={win} onChange={setWin} />
                </div>
              </div>
            </motion.div>
          )}
        </AnimatePresence>

        <div className="sheet-foot">
          <span className="hint">
            <Link2 size={13} /> Patterns like <code>[01-20]</code> and <code>{'{a,b}'}</code> expand to many files
          </span>
          <span style={{ flex: 1 }} />
          <Button variant="glass" onClick={close}>
            Cancel
          </Button>
          <Button variant="primary" disabled={!text.trim() || busy || (probe?.error != null && total === 0 && !tor && !vid) || (tor?.loading ?? false) || (tor?.error != null) || (vid?.error != null)} onClick={submit} icon={busy ? <i className="spin" /> : <CheckCircle2 size={15} />}>
            {total > 1 ? `Add ${total}` : start === 'paused' ? 'Add paused' : 'Download'}
          </Button>
        </div>
      </motion.div>
    </motion.div>
  )
}

function parseHeaders(s: string): [string, string][] {
  return s
    .split('\n')
    .map((l) => l.trim())
    .filter((l) => l.includes(':'))
    .map((l) => {
      const i = l.indexOf(':')
      return [l.slice(0, i).trim(), l.slice(i + 1).trim()] as [string, string]
    })
}
