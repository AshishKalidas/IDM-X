import { AnimatePresence, motion } from 'motion/react'
import { Pause, Play, Plus, Search, Sparkles, Trash2, X } from 'lucide-react'
import { useEffect, useMemo, useRef, useState } from 'react'
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

  // Several at once: tick rows, then act on them together.
  const toast = useStore((s) => s.toast)
  const [checked, setChecked] = useState<Set<string>>(new Set())
  const lastTicked = useRef<string | null>(null)
  const picked = useMemo(() => list.filter((j) => checked.has(j.id)), [list, checked])
  const selecting = picked.length > 0

  // Forget rows that were removed or filtered away.
  useEffect(() => {
    setChecked((c) => {
      const keep = new Set([...c].filter((id) => jobsMap[id]))
      return keep.size === c.size ? c : keep
    })
  }, [jobsMap])

  const tick = (id: string, shift: boolean) => {
    setChecked((c) => {
      const n = new Set(c)
      const from = lastTicked.current ? list.findIndex((j) => j.id === lastTicked.current) : -1
      const to = list.findIndex((j) => j.id === id)
      if (shift && from >= 0 && to >= 0) {
        const [a, b] = from < to ? [from, to] : [to, from]
        for (const j of list.slice(a, b + 1)) n.add(j.id)
      } else if (n.has(id)) n.delete(id)
      else n.add(id)
      return n
    })
    lastTicked.current = id
  }
  const clearPicked = () => setChecked(new Set())
  const allPicked = list.length > 0 && picked.length === list.length

  useEffect(() => {
    const k = (e: KeyboardEvent) => {
      const typing = e.target instanceof Element && e.target.closest('input, textarea, [contenteditable]')
      if (e.key === 'Escape' && checked.size) clearPicked()
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'a' && !typing && list.length) {
        e.preventDefault()
        setChecked(new Set(list.map((j) => j.id)))
      }
    }
    window.addEventListener('keydown', k)
    return () => window.removeEventListener('keydown', k)
  })

  const canPause = picked.some((j) => RUNNING.includes(j.status) || j.status === 'queued')
  const canResume = picked.some((j) => j.status === 'paused' || j.status === 'failed' || j.status === 'cancelled')
  const hasFiles = picked.some((j) => j.status === 'completed')

  async function bulk(label: string, jobs: typeof picked, fn: (j: (typeof picked)[number]) => Promise<unknown>, keep = false) {
    const results = await Promise.allSettled(jobs.map(fn))
    const failed = results.filter((r) => r.status === 'rejected').length
    if (failed) toast({ kind: 'error', title: `${label}: ${failed} of ${jobs.length} didn’t work`, body: String((results.find((r) => r.status === 'rejected') as PromiseRejectedResult).reason?.message ?? '') })
    if (!keep) clearPicked()
  }
  const confirm = useStore((s) => s.confirm)
  const pausable = picked.filter((j) => RUNNING.includes(j.status) || j.status === 'queued')
  const resumable = picked.filter((j) => j.status === 'paused' || j.status === 'failed' || j.status === 'cancelled')
  async function deleteWithFiles() {
    const n = picked.length
    const ok = await confirm({
      title: `Delete ${n} download${n === 1 ? '' : 's'}?`,
      body: 'They will be removed from the list. Finished files are deleted from your disk, and what has been downloaded so far of unfinished ones is deleted too. You can’t undo this.',
      confirmLabel: 'Delete',
      cancelLabel: 'Keep them',
      danger: true,
    })
    if (!ok) return
    await bulk('Delete', picked, async (j) => {
      if (j.status !== 'completed' && j.status !== 'cancelled') await api.cancel(j.id)
      await api.remove(j.id, j.status === 'completed')
    })
  }

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
                <DownloadCard key={j.id} job={j} checked={checked.has(j.id)} selecting={selecting} onCheck={(shift) => tick(j.id, shift)} />
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
        <AnimatePresence>
          {selecting && (
            <motion.div className="bulk-bar" initial={{ opacity: 0, y: 18 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: 18, transition: { duration: 0.14 } }} transition={{ type: 'spring', stiffness: 520, damping: 40 }}>
              <button className="bulk-count" onClick={() => (allPicked ? clearPicked() : setChecked(new Set(list.map((j) => j.id))))} title={allPicked ? 'Deselect all' : 'Select all'}>
                <b>{picked.length}</b> selected <span>{allPicked ? 'Deselect all' : `Select all ${list.length}`}</span>
              </button>
              <i className="bulk-sep" />
              {canPause && (
                <Button size="sm" icon={<Pause size={14} />} onClick={() => void bulk('Pause', pausable, (j) => api.pause(j.id))}>
                  Pause
                </Button>
              )}
              {canResume && (
                <Button size="sm" icon={<Play size={14} />} onClick={() => void bulk('Resume', resumable, (j) => api.resume(j.id))}>
                  Resume
                </Button>
              )}
              <Button size="sm" icon={<X size={14} />} onClick={() => void bulk('Remove', picked, (j) => api.remove(j.id, false))}>
                Remove from list
              </Button>
              <Button size="sm" variant="danger" icon={<Trash2 size={14} />} onClick={() => void deleteWithFiles()}>
                {hasFiles ? 'Delete with files' : 'Cancel & delete'}
              </Button>
              <i className="bulk-sep" />
              <IconButton label="Clear selection (Esc)" onClick={clearPicked}>
                <X />
              </IconButton>
            </motion.div>
          )}
        </AnimatePresence>
      </div>
      <AnimatePresence>{sel && <Inspector key={sel.id} job={sel} />}</AnimatePresence>
    </div>
  )
}
