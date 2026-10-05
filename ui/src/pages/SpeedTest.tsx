import { motion } from 'motion/react'
import { AlertTriangle, Sparkles, Zap } from 'lucide-react'
import { useState } from 'react'
import { api } from '../lib/api'
import { kindMeta, netColor } from '../lib/kinds'
import { useStore } from '../lib/store'
import NoConnections, { useOffline } from '../components/NoConnections'
import { Button, Num, TextField } from '../components/ui'

export default function SpeedTest() {
  const report = useStore((s) => s.speed)
  const interfaces = useStore((s) => s.interfaces)
  const toast = useStore((s) => s.toast)
  const [url, setUrl] = useState('')
  const [adv, setAdv] = useState(false)
  const running = !!report && !report.done
  const offline = useOffline()

  const start = () => api.speedtest(url || undefined).catch((e) => toast({ kind: 'error', title: "Couldn't start the test", body: String(e.message ?? e) }))
  const max = Math.max(1, ...(report?.nets.map((n) => n.mbps) ?? []), report?.combinedMbps ?? 0)
  const colorOf = (id: string, kind: Parameters<typeof netColor>[0]) => {
    const i = interfaces.find((x) => x.id === id)
    return i?.rule.color || netColor(kind)
  }
  const failed = report?.phase === 'failed'

  return (
    <div className="page-pad">
      <header className="page-head" data-tauri-drag-region>
        <h1>Speed test</h1>
      </header>
      <div className="scroll">
        <p className="lede">Measures every network on its own, then all together, so you can see what combining them is actually worth — before you start a big download.</p>

        {offline && <NoConnections compact />}
        <div className="st-stage" style={offline ? { opacity: 0.4, pointerEvents: 'none' } : undefined}>
          <div className="st-orb">
            <div className={`orb ${running ? 'run' : ''}`}>
              <i /> <i /> <i />
              <div className="orb-core">
                {report?.gain && report.done ? (
                  <>
                    <b className="mono">
                      <Num value={report.gain} format={(v) => v.toFixed(2)} />×
                    </b>
                    <small>vs. fastest alone</small>
                  </>
                ) : (
                  <Zap size={36} strokeWidth={1.4} />
                )}
              </div>
            </div>
          </div>

          <div className="st-side">
            <div className="st-phase mono">{running ? report?.phase : report?.done && !failed ? 'Complete' : failed ? 'Couldn’t finish' : 'Ready'}</div>
            <Button variant="primary" size="lg" disabled={running} icon={<Sparkles size={16} />} onClick={start}>
              {running ? 'Measuring…' : report?.done ? 'Run again' : 'Start test'}
            </Button>
            <button className="more-toggle" onClick={() => setAdv(!adv)}>
              Test file
            </button>
            {adv && <TextField value={url} onChange={setUrl} placeholder="https://…  (defaults to a public speed-test file)" mono width="100%" />}
          </div>
        </div>

        {report && !failed && (
          <div className="st-bars">
            {report.nets.map((n, i) => {
              const Icon = kindMeta[n.kind].icon
              const c = colorOf(n.id, n.kind)
              return (
                <motion.div className="st-row" key={n.id} initial={{ opacity: 0, x: -10 }} animate={{ opacity: 1, x: 0 }} transition={{ delay: i * 0.05 }}>
                  <div className="st-name">
                    <Icon size={15} style={{ color: c }} /> {n.label}
                  </div>
                  <div className="st-track">
                    <motion.div className="st-fill" style={{ background: `linear-gradient(90deg, ${c}, color-mix(in srgb, ${c} 55%, white))`, boxShadow: `0 0 18px ${c}55` }} animate={{ width: `${(n.mbps / max) * 100}%` }} transition={{ type: 'spring', stiffness: 120, damping: 22 }} />
                  </div>
                  <div className="st-val mono">{n.error ? <span className="err">failed</span> : n.mbps > 0 ? `${n.mbps.toFixed(1)} Mbit/s` : '…'}</div>
                  <div className="st-lat mono">{n.latencyMs ? `${Math.round(n.latencyMs)} ms` : ''}</div>
                </motion.div>
              )
            })}
            {report.combinedMbps != null && (
              <motion.div className="st-row combined" initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }}>
                <div className="st-name">
                  <Zap size={15} /> Combined
                </div>
                <div className="st-track">
                  <motion.div className="st-fill all" animate={{ width: `${(report.combinedMbps / max) * 100}%` }} transition={{ type: 'spring', stiffness: 120, damping: 22 }} />
                </div>
                <div className="st-val mono">{report.combinedMbps.toFixed(1)} Mbit/s</div>
                <div className="st-lat" />
              </motion.div>
            )}
          </div>
        )}

        {report?.warnings.map((w, i) => (
          <motion.div key={i} className={`tip ${failed ? 'bad' : 'warn'}`} initial={{ opacity: 0, y: 6 }} animate={{ opacity: 1, y: 0 }}>
            <AlertTriangle size={16} />
            <div>{w}</div>
          </motion.div>
        ))}

        {report?.done && !failed && report.gain != null && report.gain >= 1.25 && (
          <motion.div className="tip ok" initial={{ opacity: 0, y: 6 }} animate={{ opacity: 1, y: 0 }}>
            <Zap size={16} />
            <div>
              Together these networks reach <b>{report.combinedMbps?.toFixed(0)} Mbit/s</b> — {report.gain.toFixed(1)}× the best one alone. Real downloads depend on the server too; see the <b>Downloads</b> page for live numbers. (Added up separately they measure {report.sumMbps.toFixed(0)} Mbit/s.)
            </div>
          </motion.div>
        )}
      </div>
    </div>
  )
}
