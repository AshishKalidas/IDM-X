import { useRef, useState } from 'react'
import { useCanvasLoop } from '../lib/hooks'
import { hexA } from '../lib/kinds'
import { bytes } from '../lib/format'
import type { BlockMap as BlockMapData } from '../lib/types'

/**
 * Every chunk of the file as one square. Squares fill with the colour of the network that
 * fetched them, and flash as they land — so you can watch the work-stealing happen.
 */
export default function BlockMap({ map, colors, labels, height = 150 }: { map: BlockMapData | null; colors: Record<string, string>; labels: Record<string, string>; height?: number }) {
  const prev = useRef<number[]>([])
  const flash = useRef<Float32Array>(new Float32Array(0))
  const geo = useRef({ cols: 1, cell: 8, gap: 2, bucket: 1, n: 0, ox: 0 })
  const splits = useRef<Map<number, [number, number][]>>(new Map())
  const [tip, setTip] = useState<{ x: number; y: number; text: string } | null>(null)

  const ref = useCanvasLoop(
    (ctx, w, h, dt, t) => {
      if (!map || map.state.length === 0) return
      const dark = document.documentElement.dataset.theme === 'dark'
      const total = map.state.length
      splits.current = new Map((map.split ?? []).map(([i, p]) => [i, p]))
      // Choose the largest cell size that fits; beyond ~6000 cells, merge blocks into buckets.
      const maxCells = 6000
      const bucket = Math.max(1, Math.ceil(total / maxCells))
      const n = Math.ceil(total / bucket)
      let cell = 3
      let cols = 1
      for (let s = 22; s >= 3; s--) {
        const g = s >= 8 ? 2 : 1
        const c = Math.max(1, Math.floor((w + g) / (s + g)))
        if (Math.ceil(n / c) * (s + g) - g <= h) {
          cell = s
          cols = c
          break
        }
      }
      const gap = cell >= 8 ? 2 : 1
      const used = cols * (cell + gap) - gap
      const ox = Math.max(0, (w - used) / 2)
      geo.current = { cols, cell, gap, bucket, n, ox }
      if (flash.current.length !== n) flash.current = new Float32Array(n)
      const r = cell >= 10 ? 3 : cell >= 6 ? 2 : 1

      const curState = new Array<number>(n)
      for (let i = 0; i < n; i++) {
        const a = i * bucket
        const b = Math.min(total, a + bucket)
        let done = 0
        let act = false
        let fill = 0
        for (let k = a; k < b; k++) {
          if (map.state[k] === 2) done++
          if (map.state[k] === 1) act = true
          fill += map.fill[k]
        }
        const full = done === b - a
        curState[i] = full ? 2 : act ? 1 : fill > 0 ? 1 : 0
        const ownerIdx = map.owner[Math.min(total - 1, a + ((b - a) >> 1))]
        const color = colors[map.networks[ownerIdx]] ?? (dark ? '#8e8e93' : '#8e8e93')
        const col = i % cols
        const row = Math.floor(i / cols)
        const x = ox + col * (cell + gap)
        const y = row * (cell + gap)
        if (prev.current.length === n && prev.current[i] !== 2 && curState[i] === 2) flash.current[i] = 1
        const f = flash.current[i]
        if (f > 0) flash.current[i] = Math.max(0, f - dt * 2.4)

        if (curState[i] === 2) {
          ctx.fillStyle = hexA(color, 0.92)
          ctx.beginPath()
          ctx.roundRect(x, y, cell, cell, r)
          ctx.fill()
          // A block that several networks each delivered part of is shown as bands, so the
          // work-stealing is visible: who started it, who finished it.
          const parts = bucket === 1 ? splits.current.get(a) : undefined
          if (parts && cell >= 6) {
            ctx.save()
            ctx.beginPath()
            ctx.roundRect(x, y, cell, cell, r)
            ctx.clip()
            let px = x
            for (const [net, pc] of parts) {
              const w2 = (cell * pc) / 100
              ctx.fillStyle = hexA(colors[map.networks[net]] ?? color, 0.95)
              ctx.fillRect(px, y, Math.ceil(w2), cell)
              px += w2
            }
            ctx.restore()
          }
          if (f > 0.01) {
            ctx.save()
            ctx.shadowColor = color
            ctx.shadowBlur = 18 * f
            ctx.fillStyle = `rgba(255,255,255,${0.9 * f})`
            const e = f * 2.5
            ctx.beginPath()
            ctx.roundRect(x - e, y - e, cell + e * 2, cell + e * 2, r + 1)
            ctx.fill()
            ctx.restore()
          }
        } else if (curState[i] === 1) {
          const fr = (fill / (b - a)) || 0
          ctx.fillStyle = dark ? 'rgba(255,255,255,0.08)' : 'rgba(0,0,0,0.07)'
          ctx.beginPath()
          ctx.roundRect(x, y, cell, cell, r)
          ctx.fill()
          const pulse = 0.55 + 0.45 * Math.sin(t * 5 + i * 0.7)
          ctx.fillStyle = hexA(color, 0.45 + 0.4 * pulse)
          const fh = Math.max(2, cell * Math.max(0.15, fr))
          ctx.beginPath()
          ctx.roundRect(x, y + cell - fh, cell, fh, Math.min(r, fh / 2))
          ctx.fill()
          if (cell >= 8) {
            ctx.strokeStyle = hexA(color, 0.4 + 0.5 * pulse)
            ctx.lineWidth = 1
            ctx.beginPath()
            ctx.roundRect(x - 0.5, y - 0.5, cell + 1, cell + 1, r + 0.5)
            ctx.stroke()
          }
        } else {
          ctx.fillStyle = dark ? 'rgba(255,255,255,0.07)' : 'rgba(0,0,0,0.06)'
          ctx.beginPath()
          ctx.roundRect(x, y, cell, cell, r)
          ctx.fill()
        }
      }
      prev.current = curState
    },
    [map, colors],
  )

  return (
    <div style={{ position: 'relative' }}>
      <canvas
        ref={ref}
        style={{ width: '100%', height, display: 'block' }}
        onPointerMove={(e) => {
          if (!map) return
          const r = (e.target as HTMLCanvasElement).getBoundingClientRect()
          const { cols, cell, gap, bucket, n, ox } = geo.current
          const col = Math.floor((e.clientX - r.left - ox) / (cell + gap))
          const row = Math.floor((e.clientY - r.top) / (cell + gap))
          const i = row * cols + col
          if (col < 0 || col >= cols || i < 0 || i >= n) return setTip(null)
          const bi = Math.min(map.state.length - 1, i * bucket)
          const st = map.state[bi]
          const owner = map.owner[bi]
          const who = owner < map.networks.length ? labels[map.networks[owner]] ?? map.networks[owner] : null
          const parts = bucket === 1 ? splits.current.get(bi) : undefined
          const shared = parts ? ` — shared: ${parts.map(([n, p]) => `${labels[map.networks[n]] ?? map.networks[n]} ${p}%`).join(', ')}` : ''
          const text = `Chunk #${bi + 1}${bucket > 1 ? `–${Math.min(map.state.length, bi + bucket)}` : ''} · ${bytes(map.blockSize * bucket, 0)} · ${st === 2 ? `done${parts ? shared : who ? ` via ${who}` : ''}` : st === 1 ? `downloading${who ? ` via ${who}` : ''}` : 'waiting'}`
          setTip({ x: e.clientX - r.left, y: e.clientY - r.top, text })
        }}
        onPointerLeave={() => setTip(null)}
      />
      {tip && (
        <div className="map-tip" style={{ left: Math.min(tip.x + 12, 9999), top: tip.y + 14 }}>
          {tip.text}
        </div>
      )}
    </div>
  )
}
