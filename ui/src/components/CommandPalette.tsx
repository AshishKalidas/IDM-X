import { AnimatePresence, motion } from 'motion/react'
import { Gauge, Moon, Network, Pause, Play, Plus, Search, Settings as Cog, Sparkles, Sun, Trash2, Workflow, Download } from 'lucide-react'
import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { api } from '../lib/api'
import { useStore } from '../lib/store'
import FileGlyph from './FileGlyph'
import { Kbd } from './ui'

interface Item {
  id: string
  label: string
  hint?: string
  icon: ReactNode
  group: string
  run: () => void
}

export default function CommandPalette() {
  const open = useStore((s) => s.paletteOpen)
  return <AnimatePresence>{open && <Palette />}</AnimatePresence>
}

function Palette() {
  const s = useStore()
  const [q, setQ] = useState('')
  const [i, setI] = useState(0)
  const input = useRef<HTMLInputElement>(null)
  useEffect(() => input.current?.focus(), [])
  // Escape should work wherever focus happens to be, not only inside the text field.
  useEffect(() => {
    const k = (e: KeyboardEvent) => e.key === 'Escape' && s.setPalette(false)
    window.addEventListener('keydown', k)
    return () => window.removeEventListener('keydown', k)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const items = useMemo<Item[]>(() => {
    const go = (p: typeof s.page) => () => (s.setPage(p), s.setPalette(false))
    const dark = document.documentElement.dataset.theme === 'dark'
    const base: Item[] = [
      { id: 'add', group: 'Actions', label: 'Add download…', hint: '⌘N', icon: <Plus size={16} />, run: () => s.openAdd() },
      { id: 'pause', group: 'Actions', label: 'Pause all downloads', icon: <Pause size={16} />, run: () => (void api.pauseAll(), s.setPalette(false)) },
      { id: 'resume', group: 'Actions', label: 'Resume all downloads', icon: <Play size={16} />, run: () => (void api.resumeAll(), s.setPalette(false)) },
      { id: 'clear', group: 'Actions', label: 'Clear finished downloads', icon: <Trash2 size={16} />, run: () => (void api.clearFinished(), s.setPalette(false)) },
      { id: 'speed', group: 'Actions', label: 'Run network speed test', icon: <Sparkles size={16} />, run: () => (s.setPage('speed'), s.setPalette(false), void api.speedtest().catch(() => {})) },
      { id: 'theme', group: 'Actions', label: dark ? 'Switch to light mode' : 'Switch to dark mode', icon: dark ? <Sun size={16} /> : <Moon size={16} />, run: () => (s.setTheme(dark ? 'light' : 'dark'), s.setPalette(false)) },
      { id: 'p1', group: 'Go to', label: 'Downloads', icon: <Download size={16} />, run: go('downloads') },
      { id: 'p2', group: 'Go to', label: 'Networks', icon: <Network size={16} />, run: go('networks') },
      { id: 'p3', group: 'Go to', label: 'Automation', icon: <Workflow size={16} />, run: go('automation') },
      { id: 'p4', group: 'Go to', label: 'Speed test', icon: <Gauge size={16} />, run: go('speed') },
      { id: 'p5', group: 'Go to', label: 'Settings', hint: '⌘,', icon: <Cog size={16} />, run: go('settings') },
    ]
    const jobs: Item[] = Object.values(s.jobs).map((j) => ({
      id: `j${j.id}`,
      group: 'Downloads',
      label: j.fileName,
      hint: j.status,
      icon: <FileGlyph name={j.fileName} size={22} />,
      run: () => (s.setPage('downloads'), s.select(j.id), s.setPalette(false)),
    }))
    const ql = q.trim().toLowerCase()
    return [...base, ...jobs].filter((x) => !ql || x.label.toLowerCase().includes(ql)).slice(0, 14)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [q, s.jobs])

  useEffect(() => setI(0), [q])

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === 'ArrowDown') (e.preventDefault(), setI((v) => Math.min(items.length - 1, v + 1)))
    else if (e.key === 'ArrowUp') (e.preventDefault(), setI((v) => Math.max(0, v - 1)))
    else if (e.key === 'Enter') items[i]?.run()
    else if (e.key === 'Escape') s.setPalette(false)
  }

  let lastGroup = ''
  return (
    <motion.div className="scrim top" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} onMouseDown={(e) => e.target === e.currentTarget && s.setPalette(false)}>
      <motion.div className="palette" initial={{ opacity: 0, y: -12, scale: 0.97 }} animate={{ opacity: 1, y: 0, scale: 1 }} exit={{ opacity: 0, y: -8, scale: 0.98, transition: { duration: 0.12 } }} transition={{ type: 'spring', stiffness: 520, damping: 38 }}>
        <div className="pal-input">
          <Search size={18} />
          <input ref={input} value={q} onChange={(e) => setQ(e.target.value)} onKeyDown={onKey} placeholder="Type a command or search downloads…" spellCheck={false} />
          <Kbd>esc</Kbd>
        </div>
        <div className="pal-list">
          {items.length === 0 && <div className="pal-empty">Nothing matches “{q}”</div>}
          {items.map((it, idx) => {
            const head = it.group !== lastGroup ? ((lastGroup = it.group), <div className="pal-group">{it.group}</div>) : null
            return (
              <div key={it.id}>
                {head}
                <button className={`pal-item ${idx === i ? 'on' : ''}`} onMouseMove={() => setI(idx)} onClick={it.run}>
                  <span className="pal-ic">{it.icon}</span>
                  <span className="pal-label">{it.label}</span>
                  {it.hint && <span className="pal-hint mono">{it.hint}</span>}
                </button>
              </div>
            )
          })}
        </div>
      </motion.div>
    </motion.div>
  )
}
