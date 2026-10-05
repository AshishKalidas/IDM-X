import { useId } from 'react'
import { rate } from '../lib/format'

/** The whole download's speed over time, as a still picture (the live chart only keeps a minute). */
export default function HistoryChart({ values, color = 'var(--accent)', seconds }: { values: number[]; color?: string; seconds?: number }) {
  const id = useId()
  if (values.length < 2) return <div className="muted-note">Too short to draw — the download finished within a couple of seconds.</div>
  const W = 400
  const H = 110
  const max = Math.max(...values)
  const avg = values.reduce((a, b) => a + b, 0) / values.length
  const x = (i: number) => (i / (values.length - 1)) * W
  const y = (v: number) => H - 6 - (v / max) * (H - 18)
  // Smooth the line through the samples.
  let d = `M${x(0)} ${y(values[0])}`
  for (let i = 1; i < values.length; i++) {
    const cx = (x(i - 1) + x(i)) / 2
    d += ` C${cx} ${y(values[i - 1])} ${cx} ${y(values[i])} ${x(i)} ${y(values[i])}`
  }
  return (
    <div className="hist">
      <svg viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" width="100%" height={H}>
        <defs>
          <linearGradient id={id} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0" stopColor={color} stopOpacity=".4" />
            <stop offset="1" stopColor={color} stopOpacity="0" />
          </linearGradient>
        </defs>
        <line x1="0" x2={W} y1={y(avg)} y2={y(avg)} stroke="var(--fg-3)" strokeDasharray="3 5" strokeWidth="1" vectorEffect="non-scaling-stroke" />
        <path d={`${d} L${W} ${H} L0 ${H} Z`} fill={`url(#${id})`} />
        <path d={d} fill="none" stroke={color} strokeWidth="2" strokeLinecap="round" vectorEffect="non-scaling-stroke" />
      </svg>
      <div className="hist-legend mono">
        <span>peak {rate(max)}</span>
        <span>average {rate(avg)}</span>
        {seconds != null && <span>{Math.round(seconds)} s</span>}
      </div>
    </div>
  )
}
