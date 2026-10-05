import { useRef } from 'react'
import { useCanvasLoop } from '../lib/hooks'
import { hexA } from '../lib/kinds'
import { rate } from '../lib/format'
import type { Sample } from '../lib/store'

export interface SeriesNet {
  id: string
  color: string
  label: string
}

const MAX_WINDOW = 60_000
const MIN_WINDOW = 15_000

/** Stacked area of per-network throughput over the last minute, scrolling continuously. */
export default function ThroughputChart({ samples, nets, height = 150, labels = true, glow = true }: { samples: Sample[]; nets: SeriesNet[]; height?: number; labels?: boolean; glow?: boolean }) {
  const hover = useRef<{ x: number } | null>(null)
  const yMax = useRef(1e6)
  const win = useRef(MIN_WINDOW)
  const fg = useRef({ fg2: '#999', fg3: '#666' })

  const ref = useCanvasLoop(
    (ctx, w, h, dt, t) => {
      const dark = document.documentElement.dataset.theme === 'dark'
      if (Math.floor(t * 2) % 2 === 0) {
        const cs = getComputedStyle(document.documentElement)
        fg.current = { fg2: cs.getPropertyValue('--fg-2').trim(), fg3: cs.getPropertyValue('--fg-3').trim() }
      }
      const padT = 10
      const padB = labels ? 18 : 4
      const plotH = h - padT - padB
      const last = samples[samples.length - 1]
      const now = last ? last.t + Math.min(500, Date.now() - last.t) : Date.now()
      // Start zoomed in on what we have, and widen towards a full minute as history builds up.
      const have = samples.length ? now - samples[0].t + 1500 : MIN_WINDOW
      win.current += (Math.min(MAX_WINDOW, Math.max(MIN_WINDOW, have)) - win.current) * Math.min(1, dt * 2)
      const WINDOW_MS = win.current
      const x0 = now - WINDOW_MS

      const vis = samples.filter((s) => s.t >= x0 - 1500)
      const peak = Math.max(1e5, ...vis.map((s) => s.total))
      yMax.current += (peak * 1.18 - yMax.current) * Math.min(1, dt * 2.5)
      const Y = (v: number) => padT + plotH - (v / yMax.current) * plotH
      const X = (ts: number) => ((ts - x0) / WINDOW_MS) * w

      // gridlines
      ctx.lineWidth = 1
      ctx.font = '500 10px "SF Mono", ui-monospace, Menlo, monospace'
      ctx.textAlign = 'right'
      for (let i = 1; i <= 3; i++) {
        const v = (yMax.current * i) / 3.6
        const y = Y(v)
        ctx.strokeStyle = dark ? 'rgba(255,255,255,0.06)' : 'rgba(0,0,0,0.06)'
        ctx.setLineDash([2, 5])
        ctx.beginPath()
        ctx.moveTo(0, y)
        ctx.lineTo(w, y)
        ctx.stroke()
        ctx.setLineDash([])
        if (labels) {
          ctx.fillStyle = fg.current.fg3
          ctx.fillText(rate(v), w - 4, y - 4)
        }
      }
      if (labels) {
        ctx.fillStyle = fg.current.fg3
        ctx.textAlign = 'left'
        ctx.fillText(`${Math.round(WINDOW_MS / 1000)} s ago`, 4, h - 4)
        ctx.textAlign = 'right'
        ctx.fillText('now', w - 4, h - 4)
      }

      if (vis.length < 2) {
        ctx.fillStyle = fg.current.fg3
        ctx.textAlign = 'center'
        ctx.font = '500 12px -apple-system, system-ui, sans-serif'
        ctx.fillText('Waiting for traffic…', w / 2, h / 2)
        return
      }

      // points, extended to the right edge so the head never "gaps"
      const pts = [...vis, { ...vis[vis.length - 1], t: now }]
      const layers: { net: SeriesNet; lo: number[]; hi: number[] }[] = []
      const cum = pts.map(() => 0)
      for (const net of nets) {
        const lo = [...cum]
        const hi = pts.map((p, i) => (cum[i] += p.nets[net.id] ?? 0))
        layers.push({ net, lo, hi })
      }
      const smooth = (xs: number[], ys: number[]): [number, number, number, number, number, number][] => {
        // Catmull-Rom → cubic Bézier segments
        const segs: [number, number, number, number, number, number][] = []
        for (let i = 0; i < xs.length - 1; i++) {
          const p0 = [xs[Math.max(0, i - 1)], ys[Math.max(0, i - 1)]]
          const p1 = [xs[i], ys[i]]
          const p2 = [xs[i + 1], ys[i + 1]]
          const p3 = [xs[Math.min(xs.length - 1, i + 2)], ys[Math.min(xs.length - 1, i + 2)]]
          segs.push([p1[0] + (p2[0] - p0[0]) / 6, p1[1] + (p2[1] - p0[1]) / 6, p2[0] - (p3[0] - p1[0]) / 6, p2[1] - (p3[1] - p1[1]) / 6, p2[0], p2[1]])
        }
        return segs
      }
      const xs = pts.map((p) => X(p.t))

      for (const L of layers.reverse()) {
        const top = L.hi.map(Y)
        const bot = L.lo.map(Y)
        const topSegs = smooth(xs, top)
        const botSegs = smooth([...xs].reverse(), [...bot].reverse())
        ctx.beginPath()
        ctx.moveTo(xs[0], top[0])
        for (const s of topSegs) ctx.bezierCurveTo(...s)
        ctx.lineTo(xs[xs.length - 1], bot[bot.length - 1])
        for (const s of botSegs) ctx.bezierCurveTo(...s)
        ctx.closePath()
        const g = ctx.createLinearGradient(0, padT, 0, padT + plotH)
        g.addColorStop(0, hexA(L.net.color, dark ? 0.55 : 0.4))
        g.addColorStop(1, hexA(L.net.color, dark ? 0.04 : 0.03))
        ctx.fillStyle = g
        ctx.fill()

        ctx.beginPath()
        ctx.moveTo(xs[0], top[0])
        for (const s of topSegs) ctx.bezierCurveTo(...s)
        ctx.strokeStyle = L.net.color
        ctx.lineWidth = 1.8
        ctx.lineJoin = 'round'
        if (glow && dark) {
          ctx.shadowColor = L.net.color
          ctx.shadowBlur = 10
        }
        ctx.stroke()
        ctx.shadowBlur = 0
      }

      // Soften the left edge so a short history fades in instead of starting with a cliff.
      ctx.save()
      ctx.globalCompositeOperation = 'destination-out'
      const fade = ctx.createLinearGradient(0, 0, Math.min(90, w * 0.14), 0)
      fade.addColorStop(0, 'rgba(0,0,0,1)')
      fade.addColorStop(1, 'rgba(0,0,0,0)')
      ctx.fillStyle = fade
      ctx.fillRect(0, 0, Math.min(90, w * 0.14), h)
      ctx.restore()

      // hover readout
      const hv = hover.current
      if (hv) {
        let bi = 0
        let bd = Infinity
        pts.forEach((p, i) => {
          const d = Math.abs(X(p.t) - hv.x)
          if (d < bd) (bd = d), (bi = i)
        })
        const p = pts[bi]
        const px = X(p.t)
        ctx.strokeStyle = fg.current.fg3
        ctx.lineWidth = 1
        ctx.beginPath()
        ctx.moveTo(px, padT)
        ctx.lineTo(px, padT + plotH)
        ctx.stroke()
        const rows = nets.filter((n) => (p.nets[n.id] ?? 0) > 0).map((n) => ({ n, v: p.nets[n.id] }))
        const bw = 150
        const bh = 22 + Math.max(1, rows.length) * 16
        const bx = Math.min(w - bw - 4, Math.max(4, px + 10))
        ctx.fillStyle = dark ? 'rgba(30,30,34,0.92)' : 'rgba(255,255,255,0.94)'
        ctx.strokeStyle = dark ? 'rgba(255,255,255,0.12)' : 'rgba(0,0,0,0.1)'
        ctx.beginPath()
        ctx.roundRect(bx, padT + 4, bw, bh, 10)
        ctx.fill()
        ctx.stroke()
        ctx.textAlign = 'left'
        ctx.font = '600 11px -apple-system, system-ui, sans-serif'
        ctx.fillStyle = dark ? '#f5f5f7' : '#1d1d1f'
        ctx.fillText(`${rate(p.total)}`, bx + 10, padT + 20)
        ctx.font = '500 10.5px -apple-system, system-ui, sans-serif'
        rows.forEach((r, i) => {
          ctx.fillStyle = r.n.color
          ctx.beginPath()
          ctx.arc(bx + 14, padT + 33 + i * 16, 3.5, 0, Math.PI * 2)
          ctx.fill()
          ctx.fillStyle = fg.current.fg2
          ctx.fillText(`${r.n.label}  ${rate(r.v)}`, bx + 24, padT + 37 + i * 16)
        })
      }
    },
    [samples, nets, labels, glow],
  )

  return (
    <canvas
      ref={ref}
      style={{ width: '100%', height, display: 'block' }}
      onPointerMove={(e) => {
        const r = (e.target as HTMLCanvasElement).getBoundingClientRect()
        hover.current = { x: e.clientX - r.left }
      }}
      onPointerLeave={() => (hover.current = null)}
    />
  )
}
