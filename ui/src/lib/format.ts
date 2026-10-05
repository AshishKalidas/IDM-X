const UNITS = ['B', 'KB', 'MB', 'GB', 'TB']

export function bytes(n: number | null | undefined, digits = 1): string {
  if (n == null) return '—'
  let v = n
  let i = 0
  while (v >= 1024 && i < UNITS.length - 1) {
    v /= 1024
    i++
  }
  return i === 0 ? `${Math.round(v)} B` : `${v.toFixed(v >= 100 ? 0 : digits)} ${UNITS[i]}`
}

/** Split a rate into a big number and its unit, for the hero readout. */
export function rateParts(bps: number): { value: string; unit: string } {
  if (bps < 1) return { value: '0', unit: 'KB/s' }
  let v = bps
  let i = 0
  while (v >= 1024 && i < UNITS.length - 1) {
    v /= 1024
    i++
  }
  const unit = `${UNITS[Math.max(i, 1)]}/s`
  if (i === 0) v /= 1024
  return { value: v >= 100 ? v.toFixed(0) : v >= 10 ? v.toFixed(1) : v.toFixed(2), unit }
}

export function rate(bps: number): string {
  if (bps < 1) return '—'
  const { value, unit } = rateParts(bps)
  return `${value} ${unit}`
}

export function mbit(bps: number): string {
  return `${((bps * 8) / 1e6).toFixed(bps * 8 >= 1e8 ? 0 : 1)} Mbit/s`
}

export function eta(s: number | null | undefined): string {
  if (s == null) return '—'
  if (s >= 86400) return `${Math.floor(s / 86400)}d ${Math.floor((s % 86400) / 3600)}h`
  if (s >= 3600) return `${Math.floor(s / 3600)}h ${String(Math.floor((s % 3600) / 60)).padStart(2, '0')}m`
  if (s >= 60) return `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, '0')}s`
  return `${s}s`
}

export function duration(seconds: number): string {
  const s = Math.max(0, Math.round(seconds))
  const h = Math.floor(s / 3600)
  const m = Math.floor((s % 3600) / 60)
  const sec = s % 60
  return h ? `${h}:${String(m).padStart(2, '0')}:${String(sec).padStart(2, '0')}` : `${m}:${String(sec).padStart(2, '0')}`
}

export function pct(done: number, total: number | null): number {
  return total && total > 0 ? Math.min(1, done / total) : 0
}

export function ago(ts: number | null): string {
  if (!ts) return ''
  const d = Math.max(0, Date.now() / 1000 - ts)
  if (d < 45) return 'just now'
  if (d < 3600) return `${Math.round(d / 60)} min ago`
  if (d < 86400) return `${Math.round(d / 3600)} h ago`
  return new Date(ts * 1000).toLocaleDateString(undefined, { month: 'short', day: 'numeric' })
}

export function clock(ts: number): string {
  return new Date(ts * 1000).toLocaleString(undefined, { weekday: 'short', hour: 'numeric', minute: '2-digit' })
}

export function host(url: string): string {
  try {
    return new URL(url).host
  } catch {
    return url
  }
}

export function ext(name: string): string {
  const i = name.lastIndexOf('.')
  return i > 0 && name.length - i <= 8 ? name.slice(i + 1).toLowerCase() : ''
}

export function minutesToClock(m: number): string {
  return `${String(Math.floor(m / 60) % 24).padStart(2, '0')}:${String(m % 60).padStart(2, '0')}`
}

export function parseSize(s: string): number | null {
  const m = s.trim().toLowerCase().match(/^([\d.]+)\s*([kmgt]?)i?b?(?:\/s)?$/)
  if (!m) return null
  const mult = { '': 1, k: 1 << 10, m: 1 << 20, g: 2 ** 30, t: 2 ** 40 }[m[2] as '' | 'k' | 'm' | 'g' | 't']
  const v = parseFloat(m[1]) * mult
  return Number.isFinite(v) ? Math.round(v) : null
}
