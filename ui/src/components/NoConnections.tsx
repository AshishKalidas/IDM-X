import { motion } from 'motion/react'
import { RefreshCw, SlidersHorizontal, WifiOff } from 'lucide-react'
import { useState } from 'react'
import { api } from '../lib/api'
import { RUNNING, useStore } from '../lib/store'
import { infoColor } from '../lib/kinds'
import Constellation from './Constellation'
import { Button } from './ui'

/** True once the engine has told us about the machine's networks and none of them can carry traffic. */
export function useOffline(): boolean {
  const phase = useStore((s) => s.phase)
  const interfaces = useStore((s) => s.interfaces)
  return phase === 'ready' && interfaces.length >= 0 && !interfaces.some((i) => i.usable)
}

/** Shown in place of the live readout when there is nothing to download over. */
export default function NoConnections({ compact = false }: { compact?: boolean }) {
  const interfaces = useStore((s) => s.interfaces)
  const jobs = useStore((s) => s.jobs)
  const local = useStore((s) => s.local)
  const toast = useStore((s) => s.toast)
  const [busy, setBusy] = useState(false)
  const waiting = Object.values(jobs).filter((j) => j.status === 'queued' || RUNNING.includes(j.status)).length

  // The hardware that *could* carry downloads, shown dim: it's the thing to go and plug in.
  const physical = interfaces.filter((i) => ['wifi', 'ethernet', 'cellular', 'tether'].includes(i.kind)).slice(0, 4)
  const hasWifi = physical.some((i) => i.kind === 'wifi')
  const hint = hasWifi ? 'Wi-Fi is off or not connected to a network.' : 'No Wi-Fi or Ethernet connection was found.'

  const check = async () => {
    setBusy(true)
    try {
      const list = await api.refreshNetworks()
      if (!list.some((i) => i.usable)) toast({ kind: 'info', title: 'Still no connection', body: 'Connect to Wi-Fi or plug in a cable, then try again.' })
    } catch (e) {
      toast({ kind: 'error', title: "Couldn't check", body: String((e as Error).message ?? e) })
    } finally {
      setBusy(false)
    }
  }

  return (
    <motion.div className={`no-conn ${compact ? 'compact' : ''}`} initial={{ opacity: 0, y: 10 }} animate={{ opacity: 1, y: 0 }}>
      <div className="no-conn-art">
        <Constellation height={compact ? 150 : 220} progress={0} speed={0} running={false} nets={physical.map((i) => ({ id: i.id, color: infoColor(i), label: i.rule.label || i.label, speed: 0, active: false, muted: true }))} />
        <div className="con-center">
          <motion.div className="no-conn-icon" animate={{ opacity: [0.5, 1, 0.5] }} transition={{ duration: 2.4, repeat: Infinity }}>
            <WifiOff size={compact ? 22 : 30} strokeWidth={1.6} />
          </motion.div>
        </div>
      </div>
      <h3>No network connection</h3>
      <p>
        {hint} {waiting > 0 ? `${waiting} download${waiting === 1 ? ' is' : 's are'} waiting and will start by themselves the moment you're back online.` : 'IDMX will be ready as soon as you connect.'}
      </p>
      <ul className="no-conn-tips">
        <li>Is Airplane Mode on?</li>
        <li>Is Wi-Fi switched on, and joined to a network?</li>
        <li>Is the Ethernet cable (or the tethered phone) plugged in?</li>
      </ul>
      <div className="no-conn-actions">
        <Button variant="primary" icon={busy ? <i className="spin" /> : <RefreshCw size={15} />} disabled={busy} onClick={check}>
          Check again
        </Button>
        {local && (
          <Button variant="glass" icon={<SlidersHorizontal size={15} />} onClick={() => void api.openNetworkSettings().catch(() => {})}>
            Open network settings
          </Button>
        )}
      </div>
    </motion.div>
  )
}
