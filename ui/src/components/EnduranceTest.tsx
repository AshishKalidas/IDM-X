import { FolderOpen, Square, Play, Trash2 } from 'lucide-react'
import { useEffect, useState } from 'react'
import { api } from '../lib/api'
import { bytes, duration } from '../lib/format'
import { useStore } from '../lib/store'
import type { EnduranceNet, EndurancePoint, EnduranceSummary } from '../lib/types'
import { Button, TextField } from './ui'

const when = (ts: number) => new Date(ts * 1000).toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' })
const ms = (v: number | null) => (v == null ? '–' : `${v < 10 ? v.toFixed(1) : Math.round(v)} ms`)

/** A small line chart of the last couple of minutes; lost probes are marked along the bottom. */
function Chart({ points, pick, unit, color }: { points: EndurancePoint[]; pick: (p: EndurancePoint) => number | null; unit: string; color: string }) {
  const W = 560
  const H = 90
  const vals = points.map(pick)
  const max = Math.max(1, ...vals.map((v) => v ?? 0))
  const x = (i: number) => (points.length < 2 ? 0 : (i / (points.length - 1)) * W)
  const y = (v: number) => H - 6 - (v / max) * (H - 14)
  let d = ''
  vals.forEach((v, i) => {
    if (v != null) d += `${d && vals[i - 1] != null ? 'L' : 'M'}${x(i).toFixed(1)},${y(v).toFixed(1)}`
  })
  return (
    <svg viewBox={`0 0 ${W} ${H}`} width="100%" height={H} preserveAspectRatio="none" style={{ display: 'block' }}>
      <path d={d} fill="none" stroke={color} strokeWidth="1.8" vectorEffect="non-scaling-stroke" />
      {points.map((p, i) => p.lost && <rect key={i} x={x(i) - 1.5} y={H - 5} width="3" height="5" fill="#ff453a" />)}
      <text x="4" y="12" fontSize="10" fill="currentColor" opacity="0.55">
        {max.toFixed(max < 10 ? 1 : 0)} {unit}
      </text>
    </svg>
  )
}

function NetTable({ nets }: { nets: EnduranceNet[] }) {
  const rows: [string, (n: EnduranceNet) => string][] = [
    ['Speed now', (n) => `${n.mbps.toFixed(1)} Mbit/s`],
    ['Average', (n) => `${n.avgMbps.toFixed(1)} Mbit/s`],
    ['Slowest 5% of seconds', (n) => `${n.lowMbps.toFixed(1)} Mbit/s`],
    ['Peak', (n) => `${n.maxMbps.toFixed(1)} Mbit/s`],
    ['Latency (avg / 95th / worst)', (n) => `${ms(n.avgRttMs)} / ${ms(n.p95RttMs)} / ${ms(n.maxRttMs)}`],
    ['Jitter', (n) => ms(n.jitterMs)],
    ['Probes lost', (n) => `${n.lost} of ${n.sent} (${n.lossPct.toFixed(2)}%)`],
    ['Longest gap in a row', (n) => `${n.longestLossRun} s`],
    ['Seconds with no data', (n) => String(n.stallSecs)],
    ['Request errors', (n) => String(n.errors)],
    ['500 MB files finished', (n) => String(n.cycles)],
    ['Transferred', (n) => bytes(n.bytes)],
  ]
  return (
    <div className="kv" style={{ overflowX: 'auto' }}>
      <table style={{ width: '100%', borderCollapse: 'collapse', fontSize: 12.5 }}>
        <thead>
          <tr>
            <th />
            {nets.map((n) => (
              <th key={n.id} style={{ textAlign: 'right', padding: '9px 13px', fontWeight: 650 }}>
                {n.label}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map(([k, f]) => (
            <tr key={k} style={{ borderTop: '0.5px solid var(--line)' }}>
              <td style={{ padding: '8px 13px', color: 'var(--fg-2)' }}>{k}</td>
              {nets.map((n) => (
                <td key={n.id} className="mono" style={{ textAlign: 'right', padding: '8px 13px' }}>
                  {f(n)}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

function SavedRun({ run, onDeleted }: { run: EnduranceSummary; onDeleted: () => void }) {
  const toast = useStore((s) => s.toast)
  const confirm = useStore((s) => s.confirm)
  const worst = Math.max(0, ...run.nets.map((n) => n.lossPct))
  return (
    <div className="kv" style={{ padding: '12px 14px', display: 'flex', gap: 12, alignItems: 'center', flexDirection: 'row' }}>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ fontWeight: 600 }}>{when(run.startedAt)}</div>
        <div className="mono" style={{ fontSize: 12, color: 'var(--fg-2)' }}>
          {duration(run.durationSecs)} · {bytes(run.totalBytes)} · {run.nets.map((n) => n.label).join(', ')} · worst loss {worst.toFixed(2)}% · log {bytes(run.csvBytes)}
        </div>
      </div>
      <Button size="sm" icon={<FolderOpen size={14} />} onClick={() => api.enduranceReveal(run.name).catch((e) => toast({ kind: 'error', title: "Couldn't open the folder", body: String(e.message ?? e) }))}>
        Show log
      </Button>
      <Button
        size="sm"
        variant="plain"
        icon={<Trash2 size={14} />}
        onClick={async () => {
          if (await confirm({ title: 'Delete this log?', body: 'The CSV and its summary will be removed from your computer.', confirmLabel: 'Delete', danger: true })) {
            await api.enduranceDelete(run.name).catch(() => {})
            onDeleted()
          }
        }}
      >
        Delete
      </Button>
    </div>
  )
}

export default function EnduranceTest() {
  const st = useStore((s) => s.endurance)
  const interfaces = useStore((s) => s.interfaces)
  const toast = useStore((s) => s.toast)
  const [url, setUrl] = useState('')
  const [picked, setPicked] = useState<string[]>([])
  const [logs, setLogs] = useState<EnduranceSummary[]>([])
  const running = !!st?.running
  const usable = interfaces.filter((i) => i.usable)

  const refresh = () => api.enduranceLogs().then((r) => setLogs(r.logs)).catch(() => {})
  useEffect(() => {
    void refresh()
    if (!useStore.getState().endurance) void api.enduranceStatus().then((r) => r.status && useStore.setState({ endurance: r.status })).catch(() => {})
  }, [])
  useEffect(() => {
    if (st && !st.running) void refresh()
  }, [st?.running])

  const start = () => api.enduranceStart(url || undefined, picked).catch((e) => toast({ kind: 'error', title: "Couldn't start the test", body: String(e.message ?? e) }))
  const live = running ? st : null
  const done = !running ? st?.summary : null

  return (
    <>
      <p className="lede">
        Runs a <b>500 MB</b> download over and over on each network until you press stop, logging speed, latency, jitter and lost probes every second. The downloaded data is counted and thrown away; only the log is kept, on disk, so you can leave it running for hours.
      </p>

      {!running && (
        <div style={{ display: 'grid', gap: 12, marginBottom: 18 }}>
          <div>
            <div style={{ fontSize: 12, color: 'var(--fg-2)', marginBottom: 6 }}>Networks to test {picked.length === 0 && '(all enabled)'}</div>
            <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
              {usable.map((i) => {
                const on = picked.includes(i.id)
                return (
                  <button key={i.id} style={{ padding: '6px 12px', borderRadius: 999, boxShadow: `0 0 0 ${on ? 1.5 : 0.5}px ${on ? 'var(--accent)' : 'var(--line-strong)'}` }} onClick={() => setPicked(on ? picked.filter((x) => x !== i.id) : [...picked, i.id])}>
                    {i.label}
                  </button>
                )
              })}
            </div>
          </div>
          <TextField value={url} onChange={setUrl} placeholder="Test file (optional) — defaults to a public speed-test server" mono width="100%" />
        </div>
      )}

      <div style={{ display: 'flex', gap: 14, alignItems: 'center', marginBottom: 18 }}>
        {running ? (
          <Button variant="primary" size="lg" icon={<Square size={15} />} onClick={() => api.enduranceStop()}>
            End &amp; save log
          </Button>
        ) : (
          <Button variant="primary" size="lg" icon={<Play size={15} />} onClick={start}>
            {done ? 'Run again' : 'Start in-depth test'}
          </Button>
        )}
        {live && (
          <div className="mono" style={{ color: 'var(--fg-2)' }}>
            {duration(live.elapsedSecs)} · {bytes(live.totalBytes)} · file {Math.round(Math.max(0, ...live.nets.map((n) => n.cyclePct)))}% · log {bytes(live.logBytes)}
          </div>
        )}
      </div>

      {live && (
        <>
          <div className="kv" style={{ padding: 14, marginBottom: 14 }}>
            <div style={{ fontSize: 12, color: 'var(--fg-2)', marginBottom: 4 }}>Throughput — last {Math.min(150, live.recent.length)} s</div>
            <Chart points={live.recent} pick={(p) => p.mbps} unit="Mbit/s" color="var(--accent)" />
            <div style={{ fontSize: 12, color: 'var(--fg-2)', margin: '10px 0 4px' }}>Latency under load (red ticks = a probe that got no answer)</div>
            <Chart points={live.recent} pick={(p) => p.rttMs} unit="ms" color="#ff9f0a" />
          </div>
          <NetTable nets={live.nets} />
        </>
      )}

      {done && (
        <>
          <div className="tip ok" style={{ marginBottom: 12 }}>
            <div>
              <b>Saved.</b> {duration(done.durationSecs)} tested, {bytes(done.totalBytes)} transferred. The live data has been cleared from the app; the log is on your computer.
              {done.notes.map((n, i) => (
                <div key={i}>• {n}</div>
              ))}
            </div>
          </div>
          <NetTable nets={done.nets} />
        </>
      )}

      {st?.phase === 'failed' && st.warnings.map((w, i) => <div key={i} className="tip bad">{w}</div>)}

      {logs.length > 0 && (
        <div style={{ marginTop: 26 }}>
          <div style={{ fontSize: 12, color: 'var(--fg-2)', margin: '0 0 8px', textTransform: 'uppercase', letterSpacing: '0.06em' }}>Saved logs</div>
          <div style={{ display: 'grid', gap: 8 }}>
            {logs.map((l) => (
              <SavedRun key={l.name} run={l} onDeleted={refresh} />
            ))}
          </div>
          <p className="lede" style={{ marginTop: 10, fontSize: 12.5 }}>
            Probe loss is measured with a small connection attempt to 1.1.1.1 / 8.8.8.8 out of each network once a second, while it is downloading flat out. It is not ICMP ping, but a connection that can't be made, or takes seconds, is what you feel as loss.
          </p>
        </div>
      )}
    </>
  )
}
