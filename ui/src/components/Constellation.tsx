import { useRef } from 'react'
import { useCanvasLoop } from '../lib/hooks'
import { hexA } from '../lib/kinds'
import { rate } from '../lib/format'

export interface ConNet {
  id: string
  color: string
  label: string
  speed: number
  active: boolean
  muted?: boolean
}

interface Particle {
  t: number
  v: number
  size: number
  off: number
}

/**
 * Networks orbit a file; packets stream along curves from each one into the centre at a rate
 * that follows its real throughput. It is decoration that is also data: a dead network goes
 * quiet, a fast one floods.
 */
export default function Constellation({ nets, progress, speed, running, height = 250 }: { nets: ConNet[]; progress: number; speed: number; running: boolean; height?: number }) {
  const parts = useRef<Record<string, Particle[]>>({})
  const spawn = useRef<Record<string, number>>({})
  const shown = useRef({ progress: 0, speed: 0 })
  const mouse = useRef({ x: 0, y: 0, in: false })
  const fg2 = useRef('#999')

  const ref = useCanvasLoop(
    (ctx, w, h, dt, t) => {
      const dark = document.documentElement.dataset.theme === 'dark'
      if (Math.floor(t * 2) % 2 === 0) fg2.current = getComputedStyle(document.documentElement).getPropertyValue('--fg-2').trim() || '#999'
      const accent = getComputedStyle(document.documentElement).getPropertyValue('--accent').trim() || '#0a84ff'
      const s = shown.current
      s.progress += (progress - s.progress) * Math.min(1, dt * 5)
      s.speed += (speed - s.speed) * Math.min(1, dt * 3)

      const cx = w / 2 + (mouse.current.in ? (mouse.current.x - w / 2) * 0.02 : 0)
      const cy = h / 2 + (mouse.current.in ? (mouse.current.y - h / 2) * 0.02 : 0)
      const R = Math.min(w * 0.3, h * 0.35)
      const r0 = Math.min(w, h) * 0.135
      ctx.globalCompositeOperation = dark ? 'lighter' : 'source-over'

      // orbit
      ctx.save()
      ctx.strokeStyle = dark ? 'rgba(255,255,255,0.07)' : 'rgba(0,0,0,0.07)'
      ctx.lineWidth = 1
      ctx.setLineDash([2, 7])
      ctx.lineDashOffset = -t * 6
      ctx.beginPath()
      ctx.arc(cx, cy, R, 0, Math.PI * 2)
      ctx.stroke()
      ctx.restore()

      const n = nets.length
      const totalSpeed = nets.reduce((a, b) => a + b.speed, 0) || 1

      nets.forEach((net, i) => {
        const ang = n === 1 ? Math.PI : -Math.PI / 2 + (i * 2 * Math.PI) / n
        const nx = cx + Math.cos(ang) * R
        const ny = cy + Math.sin(ang) * R
        const ex = cx + Math.cos(ang) * (r0 + 6)
        const ey = cy + Math.sin(ang) * (r0 + 6)
        // curve control point: pushed sideways so flows swirl rather than run straight
        const mx = (nx + ex) / 2
        const my = (ny + ey) / 2
        const bend = (i % 2 ? 1 : -1) * R * 0.28
        const qx = mx + -Math.sin(ang) * bend
        const qy = my + Math.cos(ang) * bend

        const share = net.speed / totalSpeed
        const live = running && net.speed > 1

        // path
        ctx.beginPath()
        ctx.moveTo(nx, ny)
        ctx.quadraticCurveTo(qx, qy, ex, ey)
        ctx.strokeStyle = hexA(net.color, live ? 0.22 : 0.08)
        ctx.lineWidth = live ? 1.4 : 1
        ctx.stroke()

        // particles
        const list = (parts.current[net.id] ??= [])
        if (live) {
          const rateHz = 3 + 22 * Math.min(1, Math.log10(1 + net.speed / 2e5) / 2.2)
          spawn.current[net.id] = (spawn.current[net.id] ?? 0) + dt * rateHz
          while (spawn.current[net.id] >= 1) {
            spawn.current[net.id] -= 1
            list.push({ t: 0, v: 0.45 + Math.random() * 0.5, size: 1.4 + Math.random() * 1.8, off: (Math.random() - 0.5) * 7 })
          }
        }
        for (let k = list.length - 1; k >= 0; k--) {
          const p = list[k]
          p.t += dt * p.v
          if (p.t >= 1) {
            list.splice(k, 1)
            continue
          }
          for (let tr = 0; tr < 4; tr++) {
            const tt = Math.max(0, p.t - tr * 0.022)
            const u = 1 - tt
            const px = u * u * nx + 2 * u * tt * qx + tt * tt * ex + -Math.sin(ang) * p.off * Math.sin(tt * Math.PI)
            const py = u * u * ny + 2 * u * tt * qy + tt * tt * ey + Math.cos(ang) * p.off * Math.sin(tt * Math.PI)
            const a = (1 - tr / 4) * Math.sin(Math.min(1, p.t * 1.4) * Math.PI * 0.5) * (p.t > 0.85 ? (1 - p.t) / 0.15 : 1)
            ctx.fillStyle = hexA(net.color, a * (dark ? 0.9 : 0.75))
            ctx.beginPath()
            ctx.arc(px, py, p.size * (1 - tr * 0.18), 0, Math.PI * 2)
            ctx.fill()
          }
        }

        // node
        const pulse = live ? 0.5 + 0.5 * Math.sin(t * 3 + i) : 0
        const gr = 13 + share * 16 + pulse * 3
        const g = ctx.createRadialGradient(nx, ny, 0, nx, ny, gr * 2.4)
        g.addColorStop(0, hexA(net.color, live ? (dark ? 0.55 : 0.35) : 0.16))
        g.addColorStop(1, hexA(net.color, 0))
        ctx.fillStyle = g
        ctx.beginPath()
        ctx.arc(nx, ny, gr * 2.4, 0, Math.PI * 2)
        ctx.fill()
        ctx.globalCompositeOperation = 'source-over'
        ctx.fillStyle = net.muted ? hexA(net.color, 0.35) : net.color
        ctx.beginPath()
        ctx.arc(nx, ny, 6.5 + share * 3, 0, Math.PI * 2)
        ctx.fill()
        ctx.strokeStyle = dark ? '#000' : '#fff'
        ctx.lineWidth = 2
        ctx.stroke()
        ctx.globalCompositeOperation = dark ? 'lighter' : 'source-over'

        // label
        ctx.save()
        ctx.globalCompositeOperation = 'source-over'
        ctx.fillStyle = fg2.current
        ctx.font = '600 11px -apple-system, "SF Pro Text", system-ui, sans-serif'
        const above = Math.sin(ang) < -0.3
        const left = Math.cos(ang) < -0.3
        const right = Math.cos(ang) > 0.3
        ctx.textAlign = left ? 'right' : right ? 'left' : 'center'
        const lx = nx + (left ? -16 : right ? 16 : 0)
        const ly = ny + (above ? -18 : left || right ? 4 : 22)
        // Trim the label to the room it actually has so it never runs off the canvas.
        const room = left ? lx - 6 : right ? w - lx - 6 : Math.min(lx, w - lx) * 2 - 12
        let text = net.label
        while (text.length > 3 && ctx.measureText(text).width > room) text = text.slice(0, -2).trimEnd() + '…'
        ctx.fillText(text, lx, ly)
        if (live) {
          ctx.font = '500 10.5px "SF Mono", ui-monospace, Menlo, monospace'
          ctx.fillStyle = hexA(net.color, 1)
          ctx.fillText(rate(net.speed), lx, ly + 13)
        }
        ctx.restore()
      })

      // core: progress ring + glow
      const glow = 0.12 + Math.min(0.5, Math.log10(1 + s.speed / 1e5) / 6) + (running ? 0.05 * Math.sin(t * 2.2) : 0)
      const cg = ctx.createRadialGradient(cx, cy, r0 * 0.3, cx, cy, r0 * 2.6)
      cg.addColorStop(0, hexA(accent.startsWith('#') ? accent : '#0a84ff', glow * (dark ? 1 : 0.6)))
      cg.addColorStop(1, hexA(accent.startsWith('#') ? accent : '#0a84ff', 0))
      ctx.fillStyle = cg
      ctx.beginPath()
      ctx.arc(cx, cy, r0 * 2.6, 0, Math.PI * 2)
      ctx.fill()

      ctx.globalCompositeOperation = 'source-over'
      ctx.lineWidth = 5
      ctx.lineCap = 'round'
      ctx.strokeStyle = dark ? 'rgba(255,255,255,0.1)' : 'rgba(0,0,0,0.08)'
      ctx.beginPath()
      ctx.arc(cx, cy, r0, 0, Math.PI * 2)
      ctx.stroke()
      if (s.progress > 0.002) {
        const start = -Math.PI / 2
        const end = start + Math.PI * 2 * Math.min(1, s.progress)
        let stroke: string | CanvasGradient = accent
        if ('createConicGradient' in ctx) {
          const cg2 = (ctx as CanvasRenderingContext2D).createConicGradient(start, cx, cy)
          const colors = nets.length ? nets.map((x) => x.color) : [accent]
          colors.forEach((c, i) => cg2.addColorStop(Math.min(1, (i / Math.max(1, colors.length)) * Math.min(1, s.progress)), c))
          cg2.addColorStop(Math.min(1, s.progress), colors[colors.length - 1])
          stroke = cg2
        }
        ctx.strokeStyle = stroke
        ctx.shadowColor = dark ? 'rgba(10,132,255,0.6)' : 'transparent'
        ctx.shadowBlur = dark ? 14 : 0
        ctx.beginPath()
        ctx.arc(cx, cy, r0, start, end)
        ctx.stroke()
        ctx.shadowBlur = 0
      }
      // ticks around the ring
      ctx.lineWidth = 1
      for (let k = 0; k < 60; k++) {
        const a = (k / 60) * Math.PI * 2 - Math.PI / 2
        const on = k / 60 <= s.progress
        ctx.strokeStyle = on ? hexA(accent.startsWith('#') ? accent : '#0a84ff', 0.7) : dark ? 'rgba(255,255,255,0.1)' : 'rgba(0,0,0,0.1)'
        const r1 = r0 + 9
        const r2 = r0 + (k % 5 === 0 ? 15 : 12)
        ctx.beginPath()
        ctx.moveTo(cx + Math.cos(a) * r1, cy + Math.sin(a) * r1)
        ctx.lineTo(cx + Math.cos(a) * r2, cy + Math.sin(a) * r2)
        ctx.stroke()
      }
    },
    [nets, progress, speed, running],
  )

  return (
    <canvas
      ref={ref}
      style={{ width: '100%', height }}
      onPointerMove={(e) => {
        const r = (e.target as HTMLCanvasElement).getBoundingClientRect()
        mouse.current = { x: e.clientX - r.left, y: e.clientY - r.top, in: true }
      }}
      onPointerLeave={() => (mouse.current.in = false)}
    />
  )
}
