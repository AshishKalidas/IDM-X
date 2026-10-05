import { motion } from 'motion/react'
import { useStore } from '../lib/store'
import { infoColor, netColor } from '../lib/kinds'
import { pct } from '../lib/format'
import type { Job } from '../lib/types'

/** Progress bar divided between the networks that did the work, with a light sweeping the live edge. */
export default function StackBar({ job, height = 6 }: { job: Job; height?: number }) {
  const interfaces = useStore((s) => s.interfaces)
  const frac = job.status === 'completed' ? 1 : pct(job.downloaded, job.size)
  const total = job.networks.reduce((a, n) => a + n.bytes, 0)
  const running = job.status === 'downloading'
  const indeterminate = job.size == null && (job.status === 'downloading' || job.status === 'probing')

  const colorFor = (id: string, kind: Job['networks'][number]['kind']) => {
    const i = interfaces.find((x) => x.id === id)
    return i ? infoColor(i) : netColor(kind)
  }

  return (
    <div className={`stackbar ${running ? 'running' : ''} ${indeterminate ? 'indet' : ''}`} style={{ height }}>
      <div className="stackbar-fill" style={{ width: `${frac * 100}%` }}>
        {total > 0 ? (
          job.networks
            .filter((n) => n.bytes > 0)
            .map((n) => (
              <motion.span key={n.id} layout="position" className="seg" style={{ flexGrow: n.bytes, background: colorFor(n.id, n.kind) }} />
            ))
        ) : (
          <span className="seg" style={{ flexGrow: 1, background: 'var(--accent)' }} />
        )}
      </div>
      {running && <span className="stackbar-shine" style={{ left: `${frac * 100}%` }} />}
    </div>
  )
}
