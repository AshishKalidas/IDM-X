import { motion } from 'motion/react'
import { ArrowUpCircle, Download, Magnet, Gauge, Network, Search, Settings as Cog, Workflow, Plus } from 'lucide-react'
import { useStore, RUNNING, type Page } from '../lib/store'
import { openExternal } from '../lib/api'
import { bytes } from '../lib/format'
import Logo from './Logo'
import { Kbd, Spark } from './ui'

const NAV: { page: Page; label: string; icon: typeof Download }[] = [
  { page: 'downloads', label: 'Downloads', icon: Download },
  { page: 'torrents', label: 'Torrents', icon: Magnet },
  { page: 'networks', label: 'Networks', icon: Network },
  { page: 'automation', label: 'Automation', icon: Workflow },
  { page: 'speed', label: 'Speed test', icon: Gauge },
  { page: 'settings', label: 'Settings', icon: Cog },
]

export default function Sidebar() {
  const page = useStore((s) => s.page)
  const setPage = useStore((s) => s.setPage)
  const jobs = useStore((s) => s.jobs)
  const interfaces = useStore((s) => s.interfaces)
  const stats = useStore((s) => s.stats)
  const history = useStore((s) => s.history)
  const connected = useStore((s) => s.connected)
  const local = useStore((s) => s.local)
  const openAdd = useStore((s) => s.openAdd)
  const setPalette = useStore((s) => s.setPalette)
  const update = useStore((s) => s.update)

  const active = Object.values(jobs).filter((j) => RUNNING.includes(j.status)).length
  const usable = interfaces.filter((i) => i.usable && i.enabled).length
  const badge: Partial<Record<Page, number>> = { downloads: active, networks: usable, torrents: Object.values(jobs).filter((j) => j.kind === 'torrent' && RUNNING.includes(j.status)).length }

  return (
    <aside className="sidebar">
      <div className="side-head" data-tauri-drag-region>
        <Logo size={26} />
        <span className="wordmark">IDMX</span>
      </div>

      <button className="side-search" onClick={() => setPalette(true)}>
        <Search size={14} />
        <span>Search</span>
        <Kbd>⌘K</Kbd>
      </button>

      <nav className="side-nav">
        {NAV.map(({ page: p, label, icon: Icon }) => (
          <button key={p} className={`side-item ${page === p ? 'on' : ''}`} onClick={() => setPage(p)}>
            {page === p && <motion.span layoutId="side-pill" className="side-pill" transition={{ type: 'spring', stiffness: 500, damping: 40 }} />}
            <Icon size={17} strokeWidth={1.9} />
            <span>{label}</span>
            {!!badge[p] && <em className="side-badge">{badge[p]}</em>}
          </button>
        ))}
      </nav>

      <button className="side-add" onClick={() => openAdd()}>
        <Plus size={16} strokeWidth={2.4} /> New download
        <Kbd>⌘N</Kbd>
      </button>

      <div className="side-foot">
        <div className="side-stat">
          <div>
            <div className="eyebrow">This session</div>
            <div className="mono side-total">{bytes(stats.sessionBytes)}</div>
          </div>
          <Spark values={history.slice(-60).map((h) => h.total)} color="var(--accent)" width={74} height={30} />
        </div>
        {update?.dismissed && (
          <button className="side-update" onClick={() => void openExternal(update.url)} title={`IDMX ${update.version} is available`}>
            <ArrowUpCircle size={14} /> Update {update.version}
          </button>
        )}
        <div className="conn">
          <i className={connected ? 'up' : ''} />
          <span>{connected ? (local ? 'Engine running' : 'Remote connected') : 'Reconnecting…'}</span>
        </div>
        <div className="devby">Developed By XSHLABS</div>
      </div>
    </aside>
  )
}
