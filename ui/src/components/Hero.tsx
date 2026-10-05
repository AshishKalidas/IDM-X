import { motion } from 'motion/react'
import { useMemo } from 'react'
import { useStore, RUNNING } from '../lib/store'
import { bytes, mbit, rate, rateParts } from '../lib/format'
import { infoColor } from '../lib/kinds'
import ThroughputChart from './ThroughputChart'
import { Num } from './ui'

/** The headline: how fast everything is going right now, and which network is carrying it. */
export default function Hero() {
  const stats = useStore((s) => s.stats)
  const history = useStore((s) => s.history)
  const interfaces = useStore((s) => s.interfaces)
  const jobs = useStore((s) => s.jobs)
  const nets = useMemo(() => interfaces.filter((i) => i.usable && i.enabled).map((i) => ({ id: i.id, color: infoColor(i), label: i.rule.label || i.label })), [interfaces])
  const active = Object.values(jobs).filter((j) => RUNNING.includes(j.status)).length
  const parts = rateParts(stats.speed)
  const idle = stats.speed < 1 && active === 0
  const live = interfaces.filter((i) => i.speed > 1)

  return (
    <div className={`hero ${idle ? 'idle' : ''}`}>
      <div className="hero-grid" />
      <div className="hero-top">
        <div>
          <div className="eyebrow">{idle ? 'All quiet' : 'Combined throughput'}</div>
          <div className="hero-num">
            {idle ? (
              <span className="hero-ready">Ready</span>
            ) : (
              <>
                <Num value={stats.speed} format={(v) => rateParts(v).value} />
                <span className="unit">{parts.unit}</span>
              </>
            )}
          </div>
          <div className="hero-sub mono">
            {idle ? (
              <>
                {nets.length} network{nets.length === 1 ? '' : 's'} standing by · {bytes(stats.sessionBytes)} this session
              </>
            ) : (
              <>
                {mbit(stats.speed)} · {active} active · {bytes(stats.sessionBytes)} this session
              </>
            )}
          </div>
        </div>
        <div className="hero-nets">
          {(live.length ? live : interfaces.filter((i) => i.usable && i.enabled)).slice(0, 5).map((i) => (
            <motion.div key={i.id} layout className={`hero-net ${i.speed > 1 ? 'live' : ''}`} style={{ ['--c' as string]: infoColor(i) }}>
              <i />
              <span className="hn-name">{i.rule.label || i.label}</span>
              <span className="hn-speed mono">{i.speed > 1 ? rate(i.speed) : 'idle'}</span>
            </motion.div>
          ))}
        </div>
      </div>
      <div className="hero-chart">
        <ThroughputChart samples={history} nets={nets} height={118} labels={false} />
      </div>
    </div>
  )
}
