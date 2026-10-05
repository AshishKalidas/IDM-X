import { motion } from 'motion/react'
import { Check, ChevronDown, Copy, ExternalLink, Folder, Pause, Play, RotateCcw, Trash2, X, ShieldCheck, ShieldAlert } from 'lucide-react'
import { useEffect, useMemo, useState } from 'react'
import { api } from '../lib/api'
import { bytes, duration, eta, rate, pct, host } from '../lib/format'
import { RUNNING, useStore } from '../lib/store'
import { infoColor, netColor, kindMeta } from '../lib/kinds'
import type { Job, NetState, StreamStat } from '../lib/types'
import BlockMap from './BlockMap'
import Constellation from './Constellation'
import FileGlyph from './FileGlyph'
import { StatusBadge } from './DownloadCard'
import HistoryChart from './HistoryChart'
import ThroughputChart from './ThroughputChart'
import { Button, IconButton, Num, Ring, Spark, Switch } from './ui'

export default function Inspector({ job }: { job: Job }) {
  const select = useStore((s) => s.select)
  const interfaces = useStore((s) => s.interfaces)
  const history = useStore((s) => s.jobHistory[job.id])
  const map = useStore((s) => s.maps[job.id] ?? null)
  const fetchMap = useStore((s) => s.fetchMap)
  const toast = useStore((s) => s.toast)
  const streams = useStore((s) => s.streams[job.id])
  const fetchStreams = useStore((s) => s.fetchStreams)
  const [showConns, setShowConns] = useState(false)
  const [refIdx, setRefIdx] = useState(0)
  const running = RUNNING.includes(job.status)
  const done = job.status === 'completed'

  useEffect(() => {
    void fetchMap(job.id)
    if (!running) return
    const t = setInterval(() => void fetchMap(job.id), 650)
    return () => clearInterval(t)
  }, [job.id, running, fetchMap])

  // The connection table is only worth polling while it's open.
  useEffect(() => {
    if (!showConns || !running) return
    void fetchStreams(job.id)
    const t = setInterval(() => void fetchStreams(job.id), 900)
    return () => clearInterval(t)
  }, [showConns, running, job.id, fetchStreams])

  const colorOf = (id: string, kind: Job['networks'][number]['kind']) => {
    const i = interfaces.find((x) => x.id === id)
    return i ? infoColor(i) : netColor(kind)
  }
  const colors = useMemo(() => Object.fromEntries([...interfaces.map((i) => [i.id, infoColor(i)]), ...job.networks.map((n) => [n.id, colorOf(n.id, n.kind)])]), [interfaces, job.networks]) // eslint-disable-line react-hooks/exhaustive-deps
  const labels = useMemo(() => Object.fromEntries([...interfaces.map((i) => [i.id, i.rule.label || i.label]), ...job.networks.map((n) => [n.id, n.label])]), [interfaces, job.networks])

  const used = job.networks.filter((n) => n.enabled || n.bytes > 0)
  const conNets = used.length
    ? used.map((n) => ({ id: n.id, color: colors[n.id], label: labels[n.id] ?? n.label, speed: n.speed, active: n.active > 0, muted: !n.enabled || n.state === 'offline' || n.state === 'unreachable' || n.state === 'failed' }))
    : interfaces.filter((i) => i.usable && i.enabled).map((i) => ({ id: i.id, color: infoColor(i), label: i.rule.label || i.label, speed: 0, active: false, muted: true }))
  const chartNets = used.map((n) => ({ id: n.id, color: colors[n.id], label: labels[n.id] ?? n.label }))

  const frac = done ? 1 : pct(job.downloaded, job.size)

  // "2.3× Wi-Fi alone": what combining bought, measured against each network on its own. By
  // current speed while bytes are moving, by bytes delivered once it's over; click to cycle.
  const refs = job.networks.filter((n) => (done || job.status !== 'downloading' ? n.bytes > 0 : n.speed > 1))
  const sortedRefs = [...refs].sort((a, b) => (done ? b.bytes - a.bytes : b.speed - a.speed))
  const ref = sortedRefs.length >= 2 ? sortedRefs[refIdx % sortedRefs.length] : undefined
  const gain = ref ? (done || job.status !== 'downloading' ? (ref.bytes > 0 ? job.networks.reduce((a, n) => a + n.bytes, 0) / ref.bytes : 0) : ref.speed > 0 ? job.speed / ref.speed : 0) : 0
  const single = !job.resumable && job.size != null && job.networks.length > 0 && !done
  // In single-connection mode only the first network that is on and up carries the file.
  const chosen = single ? job.networks.find((n) => n.enabled && n.state !== 'offline' && n.state !== 'capped')?.id : undefined
  const elapsed = job.startedAt ? (job.completedAt ?? Date.now() / 1000) - job.startedAt : 0
  const totalBytes = job.networks.reduce((a, n) => a + n.bytes, 0) || 1
  /** In single-connection mode the switches act as a choice: turning one on turns the rest off. */
  const pickSingle = async (id: string, on: boolean) => {
    if (!on) return void (await api.jobNetwork(job.id, id, false))
    await api.jobNetwork(job.id, id, true)
    for (const o of job.networks.filter((x) => x.id !== id && x.enabled)) await api.jobNetwork(job.id, o.id, false)
  }
  const act = (fn: () => Promise<unknown>) => fn().catch((e) => toast({ kind: 'error', title: 'Something went wrong', body: String(e.message ?? e) }))

  return (
    <motion.aside className="inspector" initial={{ x: 40, opacity: 0 }} animate={{ x: 0, opacity: 1 }} exit={{ x: 40, opacity: 0, transition: { duration: 0.18 } }} transition={{ type: 'spring', stiffness: 420, damping: 38 }}>
      <header className="insp-head">
        <FileGlyph name={job.fileName} size={38} />
        <div className="insp-title">
          <div className="insp-name" title={job.fileName}>
            {job.fileName}
          </div>
          <div className="insp-sub">
            <StatusBadge job={job} />
            <span className="mono">{job.kind === 'torrent' ? 'BitTorrent' : host(job.url)}</span>
          </div>
        </div>
        <IconButton label="Close" onClick={() => select(null)}>
          <X />
        </IconButton>
      </header>

      <div className="insp-scroll">
        <div className="insp-con">
          <div style={{ position: 'relative' }}>
          <Constellation nets={conNets} progress={frac} speed={job.speed} running={job.status === 'downloading'} height={250} />
          {ref && gain >= 1.05 && (
            <button className="gain-chip mono" onClick={() => setRefIdx((i) => i + 1)} title="Click to compare with another network">
              <b>{gain.toFixed(1)}×</b> {labels[ref.id] ?? ref.label} alone
            </button>
          )}
          </div>
          {job.torrent && (
            <Section title="Torrent" aside={job.torrent.state}>
              <dl className="kv">
              <Kv k="Peers" v={`${job.torrent.peers} connected · ${job.torrent.knownPeers} known`} />
              <Kv k="Uploaded" v={`${bytes(job.torrent.uploaded)} · ratio ${job.torrent.ratio.toFixed(2)}`} />
              <Kv k="Info hash" v={job.torrent.infoHash} mono copy />
              </dl>
              {job.torrent.files.length > 1 && (
                <div style={{ marginTop: 8 }}>
                  {job.torrent.files.map((f) => (
                    <label key={f.index} style={{ display: 'flex', gap: 8, padding: '3px 0', fontSize: 12 }}>
                      <input
                        type="checkbox"
                        checked={f.selected}
                        onChange={() => {
                          const cur = job.torrent!.files.filter((x) => x.selected).map((x) => x.index)
                          const next = f.selected ? cur.filter((i) => i !== f.index) : [...cur, f.index]
                          void api.setTorrentFiles(job.id, next)
                        }}
                      />
                      <span style={{ flex: 1, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{f.path}</span>
                      <span className="mono">{f.selected ? `${Math.round((f.downloaded / Math.max(1, f.size)) * 100)}%` : 'skipped'}</span>
                    </label>
                  ))}
                </div>
              )}
            </Section>
          )}
          {job.media && (
            <Section title="Video" aside={job.media.strategy === 'direct' ? 'multi-network' : 'yt-dlp'}>
              <dl className="kv">
              <Kv k="Title" v={job.media.title} />
              {job.media.uploader && <Kv k="Channel" v={job.media.uploader} />}
              <Kv k="Format" v={job.media.format} />
              </dl>
            </Section>
          )}
          <div className="con-center">
            <div className="con-pct mono">
              <Num value={frac * 100} format={(v) => `${v.toFixed(v >= 99.95 || v === 0 ? 0 : 1)}`} />
              <small>%</small>
            </div>
          </div>
        </div>

        {done ? (
          <div className="stat-grid">
            <Stat label="Took" value={duration(elapsed)} sub={bytes(job.size)} />
            <Stat label="Average" value={elapsed > 0 ? rate((job.size ?? job.downloaded) / elapsed) : '—'} />
            <Stat label="Peak connections" value={job.peakStreams ? String(job.peakStreams) : '—'} sub={job.networks.filter((n) => n.bytes > 0).length > 1 ? `over ${job.networks.filter((n) => n.bytes > 0).length} networks` : undefined} />
          </div>
        ) : (
          <div className="stat-grid">
            <Stat label="Downloaded" value={bytes(job.downloaded)} sub={job.size ? `of ${bytes(job.size)}` : undefined} />
            <Stat label="Speed" value={job.status === 'downloading' ? rate(job.speed) : '—'} />
            <Stat label="Time left" value={eta(job.eta)} sub={elapsed > 0 ? `${duration(elapsed)} elapsed` : undefined} />
          </div>
        )}

        {done ? (
          <>
            <Section title="Speed over the download">
              <HistoryChart values={job.speedHistory ?? []} seconds={(job.speedHistory?.length ?? 0) * 2} />
            </Section>
            {job.networks.some((n) => n.bytes > 0) && (
              <Section title="Contribution by network">
                <div className="contrib">
                  <div className="stackbar" style={{ height: 10 }}>
                    <div className="stackbar-fill" style={{ width: '100%' }}>
                      {job.networks
                        .filter((n) => n.bytes > 0)
                        .map((n) => (
                          <span key={n.id} className="seg" style={{ flexGrow: n.bytes, background: colors[n.id] }} />
                        ))}
                    </div>
                  </div>
                  <div className="contrib-rows">
                    {job.networks
                      .filter((n) => n.bytes > 0)
                      .sort((a, b) => b.bytes - a.bytes)
                      .map((n) => (
                        <div key={n.id}>
                          <i style={{ background: colors[n.id] }} />
                          <span>{labels[n.id] ?? n.label}</span>
                          <b className="mono">{Math.round((n.bytes / totalBytes) * 100)}%</b>
                          <span className="mono muted">{bytes(n.bytes)}</span>
                        </div>
                      ))}
                  </div>
                </div>
              </Section>
            )}
          </>
        ) : (
          <Section title="Throughput">
            <ThroughputChart samples={history ?? []} nets={chartNets} height={120} labels />
          </Section>
        )}

        <Section title="Chunk map" aside={job.blocksTotal ? `${job.blocksDone} / ${job.blocksTotal}` : undefined}>
          {map ? <BlockMap map={map} colors={colors} labels={labels} height={map.state.length > 400 ? 170 : 110} /> : <div className="muted-note">The map appears once the file size is known.</div>}
          <div className="legend">
            {job.networks.map((n) => (
              <span key={n.id}>
                <i style={{ background: colors[n.id] }} />
                {labels[n.id] ?? n.label}
              </span>
            ))}
          </div>
        </Section>

        {job.networks.length > 0 && (
          <Section title="Networks" aside={running && !single ? 'switch any on or off, live' : undefined}>
            {single && (
              <div className="single-note">
                <b>Single-connection mode.</b> {job.size ? 'This server can’t split the file into parts' : 'The server didn’t say how big this file is'}, so it downloads over one network at a time. Choose which one below.
              </div>
            )}
            <div className="net-rows">
              {job.networks.map((n) => {
                const KindIcon = kindMeta[n.kind].icon
                const hist = (history ?? []).slice(-40).map((s) => s.nets[n.id] ?? 0)
                const st = STATE_LABEL[n.state]
                return (
                  <div className={`net-row ${n.enabled ? '' : 'off'}`} key={n.id}>
                    <div className="nr-icon" style={{ ['--c' as string]: colors[n.id] }}>
                      <KindIcon size={15} />
                    </div>
                    <div className="nr-main">
                      <div className="nr-top">
                        <b>{labels[n.id] ?? n.label}</b>
                        {n.enabled && n.bytes > 0 && <span className="mono nr-share">{Math.round((n.bytes / totalBytes) * 100)}%</span>}
                        {st.badge && <span className={`badge ${st.cls}`}>{st.label}</span>}
                      </div>
                      <div className="nr-sub mono" title={n.error ?? undefined}>
                        {n.error && n.enabled ? (
                          <span className="err">{n.error}</span>
                        ) : (
                          <>
                            {bytes(n.bytes)} · {n.active}/{n.streams} streams · {n.latencyMs > 0 ? `${Math.round(n.latencyMs)} ms` : '—'}
                            {n.errors > 0 && ` · ${n.errors} retries`}
                          </>
                        )}
                      </div>
                    </div>
                    {n.enabled && <Spark values={hist} color={colors[n.id]} width={56} height={26} />}
                    <div className="nr-speed mono">{n.speed > 1 ? rate(n.speed) : '—'}</div>
                    {n.enabled && <Ring value={n.health / 100} size={24} stroke={3} color={n.health > 70 ? 'var(--ok)' : n.health > 40 ? 'var(--warn)' : 'var(--bad)'} />}
                    {!done && <Switch checked={single ? n.id === chosen : n.enabled} onChange={(v) => (single ? act(() => pickSingle(n.id, v)) : act(() => api.jobNetwork(job.id, n.id, v)))} />}
                  </div>
                )
              })}
            </div>
          </Section>
        )}

        {running && (
          <Section title="Connections" aside={streams && showConns ? `${streams.filter((x) => x.state === 'downloading' || x.state === 'racing').length} active of ${streams.length}` : undefined}>
            <button className="more-toggle tight" onClick={() => setShowConns(!showConns)}>
              <ChevronDown size={14} style={{ transform: showConns ? 'rotate(180deg)' : 'none' }} />
              {showConns ? 'Hide' : 'Show'} every connection
            </button>
            {showConns && <ConnTable streams={streams ?? []} colors={colors} labels={labels} />}
          </Section>
        )}

        <Section title="Details">
          <dl className="kv">
            <Kv k="Link" v={job.url} copy />
            {job.finalUrl && job.finalUrl !== job.url && <Kv k="Resolved to" v={job.finalUrl} copy />}
            <Kv k="Saved to" v={job.path ?? `${job.destDir}/${job.fileName}`} copy />
            <Kv k="Resume" v={job.resumable ? 'Supported — multiple connections' : 'Not supported — single connection'} ok={job.resumable} />
            {job.etag && <Kv k="ETag" v={job.etag} mono />}
            {job.lastModified && <Kv k="Modified" v={job.lastModified} />}
            {job.checksum && (
              <div className="kv-row">
                <dt>Checksum</dt>
                <dd>
                  <span className={`badge ${job.checksum.verified === true ? 'ok' : job.checksum.verified === false ? 'bad' : ''}`}>
                    {job.checksum.verified === true ? <ShieldCheck size={12} /> : job.checksum.verified === false ? <ShieldAlert size={12} /> : null}
                    {job.checksum.algo}
                    {job.checksum.verified === true ? ` verified (${job.checksum.source})` : job.checksum.verified === false ? ' MISMATCH' : ''}
                  </span>
                  <div className="mono hash">{job.checksum.actual}</div>
                </dd>
              </div>
            )}
          </dl>
        </Section>

        {job.log.length > 0 && (
          <Section title="Log">
            <div className="log mono">
              {job.log.map((l, i) => (
                <div key={i}>{l}</div>
              ))}
            </div>
          </Section>
        )}
        {job.error && <div className="insp-error">{job.error}</div>}
      </div>

      <footer className="insp-foot">
        {(running || job.status === 'queued') && (
          <Button variant="glass" icon={<Pause size={15} />} onClick={() => act(() => api.pause(job.id))}>
            Pause
          </Button>
        )}
        {(job.status === 'paused' || job.status === 'failed' || job.status === 'cancelled') && (
          <Button variant="primary" icon={job.status === 'paused' ? <Play size={15} /> : <RotateCcw size={15} />} onClick={() => act(() => api.resume(job.id))}>
            {job.status === 'paused' ? 'Resume' : 'Retry'}
          </Button>
        )}
        {done && (
          <>
            <Button variant="primary" icon={<Folder size={15} />} onClick={() => act(() => api.reveal(job.id))}>
              Show in folder
            </Button>
          </>
        )}
        {!running && (
          <Button variant="glass" icon={<RotateCcw size={15} />} onClick={() => act(() => api.restart(job.id))}>
            Start over
          </Button>
        )}
        <span style={{ flex: 1 }} />
        <IconButton label="Remove" onClick={() => act(() => api.remove(job.id, false))}>
          <Trash2 />
        </IconButton>
      </footer>
    </motion.aside>
  )
}

const STATE_LABEL: Record<NetState, { label: string; cls: string; badge: boolean }> = {
  active: { label: 'On', cls: '', badge: false },
  idle: { label: 'On', cls: '', badge: false },
  unreachable: { label: 'Unreachable', cls: 'bad', badge: true },
  failed: { label: 'Refused', cls: 'bad', badge: true },
  capped: { label: 'Data cap', cls: 'warn', badge: true },
  offline: { label: 'Offline', cls: 'bad', badge: true },
  disabled: { label: 'Off', cls: '', badge: true },
}

function ConnTable({ streams, colors, labels }: { streams: StreamStat[]; colors: Record<string, string>; labels: Record<string, string> }) {
  if (!streams.length) return <div className="muted-note">No open connections yet.</div>
  const order = { downloading: 0, racing: 0, retrying: 1, idle: 2, parked: 3 } as const
  const list = [...streams].sort((a, b) => order[a.state] - order[b.state] || a.id - b.id)
  return (
    <div className="conn-table mono">
      <div className="conn-head">
        <span>#</span>
        <span>Network</span>
        <span>State</span>
        <span>Chunk</span>
        <span>Speed</span>
        <span>Total</span>
      </div>
      <div className="conn-body">
        {list.map((c) => (
          <div className={`conn-row ${c.state}`} key={c.id}>
            <span>{c.id}</span>
            <span className="conn-net">
              <i style={{ background: colors[c.net] }} />
              {labels[c.net] ?? c.net}
            </span>
            <span>{c.state === 'racing' ? <b className="race">racing</b> : ({ downloading: 'active', retrying: 'retry', parked: 'off', idle: 'idle' } as const)[c.state]}</span>
            <span>{c.block ? `#${c.block}` : ''}</span>
            <span>{c.speed > 1 ? rate(c.speed) : ''}</span>
            <span>{bytes(c.bytes)}</span>
          </div>
        ))}
      </div>
    </div>
  )
}

function Stat({ label, value, sub }: { label: string; value: string; sub?: string }) {
  return (
    <div className="stat">
      <div className="stat-label">{label}</div>
      <div className="stat-value mono">{value}</div>
      {sub && <div className="stat-sub mono">{sub}</div>}
    </div>
  )
}

function Section({ title, aside, children }: { title: string; aside?: string; children: React.ReactNode }) {
  return (
    <section className="insp-sec">
      <div className="insp-sec-h">
        <h4>{title}</h4>
        {aside && <span className="mono">{aside}</span>}
      </div>
      {children}
    </section>
  )
}

function Kv({ k, v, copy, mono, ok }: { k: string; v: string; copy?: boolean; mono?: boolean; ok?: boolean }) {
  return (
    <div className="kv-row">
      <dt>{k}</dt>
      <dd className={mono ? 'mono' : ''}>
        <span className="kv-v" title={v}>
          {ok !== undefined && (ok ? <Check size={13} className="okc" /> : <X size={13} className="warnc" />)} {v}
        </span>
        {copy && (
          <button className="kv-copy" aria-label="Copy" onClick={() => void navigator.clipboard.writeText(v)}>
            <Copy size={12} />
          </button>
        )}
        {copy && v.startsWith('http') && (
          <a className="kv-copy" href={v} target="_blank" rel="noreferrer" aria-label="Open">
            <ExternalLink size={12} />
          </a>
        )}
      </dd>
    </div>
  )
}
