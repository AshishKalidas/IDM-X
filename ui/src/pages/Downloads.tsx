import { AnimatePresence, motion } from 'motion/react'
import { Pause, Play, Plus, Search, Sparkles, Trash2 } from 'lucide-react'
import { useMemo } from 'react'
import { api } from '../lib/api'
import { matches, RUNNING, sortJobs, useStore, type Filter } from '../lib/store'
import { infoColor } from '../lib/kinds'
import Constellation from '../components/Constellation'
import DownloadCard from '../components/DownloadCard'
import Hero from '../components/Hero'
import NoConnections, { useOffline } from '../components/NoConnections'
import Inspector from '../components/Inspector'
import { Button, IconButton, Kbd, Segmented } from '../components/ui'

export default function Downloads({ only }: { only?: 'torrent' }) {
  const jobsMap = useStore((s) => s.jobs)
  const filter = useStore((s) => s.filter)
  const setFilter = useStore((s) => s.setFilter)
  const query = useStore((s) => s.query)
  const setQuery = useStore((s) => s.setQuery)
  const selected = useStore((s) => s.selected)
  const openAdd = useStore((s) => s.openAdd)
  const interfaces = useStore((s) => s.interfaces)
  const offline = useOffline()

  const all = useMemo(() => Object.values(jobsMap).filter((j) => !only || j.kind === only), [jobsMap, only])
  const q = query.trim().toLowerCase()
  const list = useMemo(() => sortJobs(all.filter((j) => matches(j, filter, q))), [all, filter, q])
  const count = (f: Filter) => all.filter((j) => matches(j, f, '')).length
  const sel = selected ? jobsMap[selected] : undefined
  const anyActive = all.some((j) => RUNNING.includes(j.status))
  const anyPaused = all.some((j) => j.status === 'paused')
  const anyDone = all.some((j) => j.status === 'completed' || j.status === 'cancelled')

  return (
    <div className="downloads">
      <div className="dl-col">
        <header className="page-head" data-tauri-drag-region>
          <h1>{only ? 'Torrents' : 'Downloads'}</h1>
          <div className="head-actions">
            <label className="search">
              <Search size={14} />
              <input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Filter" spellCheck={false} />
            </label>
            <Button variant="primary" icon={<Plus size={16} strokeWidth={2.4} />} onClick={() => openAdd()}>
              {only ? 'Add torrent' : 'Add'}
            </Button>
          </div>
        </header>

        <div className="dl-scroll">
          {offline ? <NoConnections /> : <Hero />}

          <div className="toolbar">
            <Segmented<Filter>
              value={filter}
              onChange={setFilter}
              options={[
                { value: 'all', label: `All ${all.length || ''}`.trim() },
                { value: 'active', label: `Active ${count('active') || ''}`.trim() },
                { value: 'queued', label: `Queued ${count('queued') || ''}`.trim() },
                { value: 'done', label: `Finished ${count('done') || ''}`.trim() },
              ]}
            />
            <span style={{ flex: 1 }} />
            {anyActive && (
              <IconButton label="Pause all" onClick={() => void api.pauseAll()}>
                <Pause />
              </IconButton>
            )}
            {anyPaused && (
              <IconButton label="Resume all" onClick={() => void api.resumeAll()}>
                <Play />
              </IconButton>
            )}
            {anyDone && (
              <IconButton label="Clear finished" onClick={() => void api.clearFinished()}>
                <Trash2 />
              </IconButton>
            )}
          </div>

          <div className="list">
            <AnimatePresence initial={false} mode="popLayout">
              {list.map((j) => (
                <DownloadCard key={j.id} job={j} />
              ))}
            </AnimatePresence>
            {list.length === 0 && (
              <motion.div className="empty" initial={{ opacity: 0, y: 10 }} animate={{ opacity: 1, y: 0 }}>
                <Constellation
                  height={210}
                  progress={0}
                  speed={0}
                  running={false}
                  nets={interfaces.filter((i) => i.usable && i.enabled).slice(0, 5).map((i) => ({ id: i.id, color: infoColor(i), label: i.rule.label || i.label, speed: 0, active: false }))}
                />
                <h3>{q || filter !== 'all' ? 'No matches' : only ? 'No torrents yet' : 'Nothing downloading'}</h3>
                <p>{q || filter !== 'all' ? 'Try a different filter.' : only ? 'Paste or drop a magnet link, or drop a .torrent file anywhere in this window.' : 'Paste a link and every network you have will pull it together.'}</p>
                {!q && filter === 'all' && (
                  <Button variant="primary" size="lg" icon={<Sparkles size={16} />} onClick={() => openAdd()}>
                    Add a download <Kbd>⌘N</Kbd>
                  </Button>
                )}
              </motion.div>
            )}
          </div>
        </div>
      </div>
      <AnimatePresence>{sel && <Inspector key={sel.id} job={sel} />}</AnimatePresence>
    </div>
  )
}
