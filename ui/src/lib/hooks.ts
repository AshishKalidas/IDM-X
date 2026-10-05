import { useEffect, useLayoutEffect, useRef, useState } from 'react'

/** Smoothly chase a changing number — for counters that should glide, not jump. */
export function useTween(target: number, ms = 450): number {
  const [v, setV] = useState(target)
  const from = useRef(target)
  const cur = useRef(target)
  cur.current = v
  useEffect(() => {
    from.current = cur.current
    const start = performance.now()
    let raf = 0
    const step = (now: number) => {
      const t = Math.min(1, (now - start) / ms)
      const e = 1 - Math.pow(1 - t, 3)
      setV(from.current + (target - from.current) * e)
      if (t < 1) raf = requestAnimationFrame(step)
    }
    raf = requestAnimationFrame(step)
    return () => cancelAnimationFrame(raf)
  }, [target, ms])
  return v
}

const GLYPHS = '▓▒░<>/\\|{}[]#$%&*+=01'

/** Text that resolves from noise, left to right — the "decoding" look. */
export function useScramble(text: string, ms = 520, enabled = true): string {
  const [out, setOut] = useState(enabled ? '' : text)
  useEffect(() => {
    if (!enabled || matchMedia('(prefers-reduced-motion: reduce)').matches) {
      setOut(text)
      return
    }
    const start = performance.now()
    let raf = 0
    const step = (now: number) => {
      const t = Math.min(1, (now - start) / ms)
      const reveal = Math.floor(t * text.length)
      let s = ''
      for (let i = 0; i < text.length; i++) {
        const c = text[i]
        s += i < reveal || c === ' ' || c === '.' ? c : GLYPHS[Math.floor(Math.random() * GLYPHS.length)]
      }
      setOut(s)
      if (t < 1) raf = requestAnimationFrame(step)
      else setOut(text)
    }
    raf = requestAnimationFrame(step)
    return () => cancelAnimationFrame(raf)
  }, [text, ms, enabled])
  return out
}

export function useSize<T extends HTMLElement>(): [React.RefObject<T | null>, { w: number; h: number }] {
  const ref = useRef<T>(null)
  const [size, setSize] = useState({ w: 0, h: 0 })
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const ro = new ResizeObserver(([e]) => setSize({ w: Math.round(e.contentRect.width), h: Math.round(e.contentRect.height) }))
    ro.observe(el)
    return () => ro.disconnect()
  }, [])
  return [ref, size]
}

/** A canvas that handles device-pixel-ratio and runs a draw loop only while visible. */
export function useCanvasLoop(draw: (ctx: CanvasRenderingContext2D, w: number, h: number, dt: number, t: number) => void, deps: unknown[] = []) {
  const canvas = useRef<HTMLCanvasElement>(null)
  const drawRef = useRef(draw)
  drawRef.current = draw
  useEffect(() => {
    const c = canvas.current
    if (!c) return
    const ctx = c.getContext('2d')
    if (!ctx) return
    let raf = 0
    let last = performance.now()
    let visible = true
    const io = new IntersectionObserver(([e]) => (visible = e.isIntersecting))
    io.observe(c)
    const loop = (now: number) => {
      raf = requestAnimationFrame(loop)
      if (!visible || document.hidden) {
        last = now
        return
      }
      const dpr = Math.min(window.devicePixelRatio || 1, 2)
      const w = c.clientWidth
      const h = c.clientHeight
      if (c.width !== Math.round(w * dpr) || c.height !== Math.round(h * dpr)) {
        c.width = Math.round(w * dpr)
        c.height = Math.round(h * dpr)
      }
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
      ctx.clearRect(0, 0, w, h)
      const dt = Math.min(0.05, (now - last) / 1000)
      last = now
      drawRef.current(ctx, w, h, dt, now / 1000)
    }
    raf = requestAnimationFrame(loop)
    return () => {
      cancelAnimationFrame(raf)
      io.disconnect()
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps)
  return canvas
}

export function useKey(handler: (e: KeyboardEvent) => void) {
  const ref = useRef(handler)
  ref.current = handler
  useEffect(() => {
    const f = (e: KeyboardEvent) => ref.current(e)
    window.addEventListener('keydown', f)
    return () => window.removeEventListener('keydown', f)
  }, [])
}
