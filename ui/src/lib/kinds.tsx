import { Cable, CircleHelp, Network, Radio, ShieldCheck, Smartphone, Boxes, Wifi, type LucideIcon } from 'lucide-react'
import type { IfKind, InterfaceInfo } from './types'

export const kindMeta: Record<IfKind, { label: string; icon: LucideIcon; css: string }> = {
  wifi: { label: 'Wi-Fi', icon: Wifi, css: '--net-wifi' },
  ethernet: { label: 'Ethernet', icon: Cable, css: '--net-ethernet' },
  cellular: { label: 'Cellular', icon: Radio, css: '--net-cellular' },
  tether: { label: 'USB tether', icon: Smartphone, css: '--net-tether' },
  vpn: { label: 'VPN', icon: ShieldCheck, css: '--net-vpn' },
  bridge: { label: 'Bridge', icon: Network, css: '--net-other' },
  virtual: { label: 'Virtual', icon: Boxes, css: '--net-other' },
  loopback: { label: 'Loopback', icon: Network, css: '--net-other' },
  unknown: { label: 'Network', icon: CircleHelp, css: '--net-other' },
}

/** Resolved colour for canvas drawing (which can't read `var()`), honouring the user's override. */
export function netColor(kind: IfKind, override?: string | null): string {
  if (override) return override
  const v = getComputedStyle(document.documentElement).getPropertyValue(kindMeta[kind].css).trim()
  return v || '#8e8e93'
}

export function infoColor(i: Pick<InterfaceInfo, 'kind' | 'rule'>): string {
  return netColor(i.kind, i.rule?.color)
}

export const PALETTE = ['#0a84ff', '#30d158', '#ff9f0a', '#bf5af2', '#ff375f', '#64d2ff', '#ffd60a', '#5e5ce6']

export function hexA(hex: string, a: number): string {
  const m = hex.replace('#', '')
  if (m.length < 6) return hex
  const n = parseInt(m.slice(0, 6), 16)
  return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${a})`
}
