import { motion } from 'motion/react'
import { ArrowUpToLine, Copy, Folder, Pause, Play, RotateCcw, Trash2, X, MoreHorizontal, Clock } from 'lucide-react'
import { useState } from 'react'
import { api } from '../lib/api'
import { bytes, eta, host, rate, ago, pct, clock } from '../lib/format'
import { RUNNING, useStore } from '../lib/store'
import { useScramble } from '../lib/hooks'
import { infoColor, netColor } from '../lib/kinds'
import type { Job } from '../lib/types'
import FileGlyph from './FileGlyph'
import { useOffline } from './NoConnections'
import StackBar from './StackBar'
import { IconButton, Menu, Num } from './ui'

export function StatusBadge({ job }: { job: Job }) {
  const offline = useOffline()
  const s = job.status
  const map: Record<string, { label: string; cls: string }> = {
    queued: { label: offline ? 'Waiting for network' : job.startAt ? `Starts ${clock(job.startAt)}` : 'Queued', cls: offline ? 'warn' : '' },
    probing: { label: 'Connecting', cls: 'accent' },
    downloading: { label: 'Downloading', cls: 'accent' },
    paused: { label: 'Paused', cls: 'warn' },
    verifying: { label: 'Verifying', cls: 'accent' },
    processing: { label: job.kind === 'media' ? 'Merging' : 'Processing', cls: 'accent' },
    seeding: { label: 'Seeding', cls: 'ok' },
    completed: { label: 'Done', cls: 'ok' },
    failed: { label: 'Failed', cls: 'bad' },
    cancelled: { label: 'Cancelled', cls: '' },
  }
  const m = map[s]
  const text = useScramble(m.label, 360)
  return (
    <span className={`badge ${m.cls}`}>
      {(s === 'probing' || s === 'verifying' || s === 'processing') && <i className="spin" />}
      {s === 'queued' && job.startAt && <Clock size={11} />}
      <span className="mono">{text}</span>
    </span>
  )
}

export default function DownloadCard({ job }: { job: Job }) {
  const selected = useStore((s) => s.selected === job.id)
  const select = useStore((s) => s.select)
  const interfaces = useStore((s) => s.interfaces)
  const toast = useStore((s) => s.toast)
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null)
  const name = useScramble(job.fileName, 480)
  const running = RUNNING.includes(job.status)
  const done = job.status === 'completed'
  const frac = done ? 1 : pct(job.downloaded, job.size)

  const act = (fn: () => Promise<unknown>) => fn().catch((e) => toast({ kind: 'error', title: 'Something went wrong', body: String(e.message ?? e) }))

  const live = job.networks.filter((n) => n.speed > 1)

  return (
    <>
      <motion.div
        layout="position"
        initial={{ opacity: 0, y: 14, scale: 0.985 }}
        animate={{ opacity: 1, y: 0, scale: 1 }}
        exit={{ opacity: 0, scale: 0.96, transition: { duration: 0.16 } }}
        transition={{ type: 'spring', stiffness: 520, damping: 40 }}
        className={`dl ${selected ? 'sel' : ''} ${job.status}`}
        onClick={() => select(selected ? null : job.id)}
        onContextMenu={(e) => {
          e.preventDefault()
          setMenu({ x: e.clientX, y: e.clientY })
        }}
      >
        <FileGlyph name={job.fileName} />
        <div className="dl-main">
          <div className="dl-top">
            <span className="dl-name">{name || job.fileName}</span>
            <StatusBadge job={job} />
          </div>
          <StackBar job={job} />
          <div className="dl-meta">
            <span className="mono">
              {job.size ? (
                <>
                  {bytes(done ? job.size : job.downloaded)} <i>of</i> {bytes(job.size)}
                </>
              ) : (
                bytes(job.downloaded)
              )}
            </span>
            <span className="dot" />
            <span>{job.torrent ? (job.torrent.fetchingMetadata ? 'Finding peers…' : `${job.torrent.peers} peer${job.torrent.peers === 1 ? '' : 's'}${job.torrent.uploaded ? ` · ratio ${job.torrent.ratio.toFixed(2)}` : ''}`) : job.media ? [job.media.extractor, job.media.format].filter(Boolean).join(' · ') : host(job.url)}</span>
            {job.error && (
              <>
                <span className="dot" />
                <span className="err">{job.error}</span>
              </>
            )}
            {done && job.completedAt && (
              <>
                <span className="dot" />
                <span>{ago(job.completedAt)}</span>
              </>
            )}
            {live.length > 0 && (
              <span className="dl-nets">
                {live.map((n) => {
                  const i = interfaces.find((x) => x.id === n.id)
                  return <i key={n.id} title={`${n.label} · ${rate(n.speed)}`} style={{ background: i ? infoColor(i) : netColor(n.kind) }} />
                })}
              </span>
            )}
          </div>
        </div>
        <div className="dl-right">
          {job.status === 'downloading' ? (
            <>
              <div className="dl-speed mono">
                <Num value={job.speed} format={rate} />
              </div>
              <div className="dl-eta mono">{eta(job.eta)}</div>
            </>
          ) : done ? (
            <div className="dl-pct mono">{bytes(job.size)}</div>
          ) : (
            <div className="dl-pct mono">{job.size ? `${Math.round(frac * 100)}%` : ''}</div>
          )}
          <div className="dl-actions" onClick={(e) => e.stopPropagation()}>
            {(running || job.status === 'queued') && (
              <IconButton label="Pause" onClick={() => act(() => api.pause(job.id))}>
                <Pause />
              </IconButton>
            )}
            {(job.status === 'paused' || job.status === 'failed' || job.status === 'cancelled') && (
              <IconButton label={job.status === 'paused' ? 'Resume' : 'Retry'} onClick={() => act(() => api.resume(job.id))}>
                {job.status === 'paused' ? <Play /> : <RotateCcw />}
              </IconButton>
            )}
            {done && (
              <IconButton label="Show in folder" onClick={() => act(() => api.reveal(job.id))}>
                <Folder />
              </IconButton>
            )}
            <IconButton
              label="More"
              onClick={(e) => {
                const r = (e.currentTarget as HTMLElement).getBoundingClientRect()
                setMenu({ x: r.left - 150, y: r.bottom + 6 })
              }}
            >
              <MoreHorizontal />
            </IconButton>
          </div>
        </div>
      </motion.div>
      {menu && (
        <Menu
          x={menu.x}
          y={menu.y}
          onClose={() => setMenu(null)}
          items={[
            { label: 'Move to top of queue', icon: <ArrowUpToLine />, onClick: () => act(() => api.top(job.id)), hidden: done },
            { label: 'Show in folder', icon: <Folder />, onClick: () => act(() => api.reveal(job.id)), hidden: !done },
            { label: 'Copy link', icon: <Copy />, onClick: () => void navigator.clipboard.writeText(job.url) },
            { label: 'Start over', icon: <RotateCcw />, onClick: () => act(() => api.restart(job.id)), hidden: running },
            {
              label: 'Cancel and delete partial',
              icon: <X />,
              danger: true,
              onClick: async () => {
                const ok = await useStore.getState().confirm({ title: 'Cancel this download?', body: `“${job.fileName}” will stop and what has been downloaded so far (${bytes(job.downloaded)}) will be deleted. You can’t undo this.`, confirmLabel: 'Cancel download', cancelLabel: 'Keep downloading', danger: true })
                if (ok) void act(() => api.cancel(job.id))
              },
              hidden: done || job.status === 'cancelled',
            },
            { label: 'Remove from list', icon: <Trash2 />, danger: true, onClick: () => act(() => api.remove(job.id, false)) },
            {
              label: 'Remove and delete file',
              icon: <Trash2 />,
              danger: true,
              onClick: async () => {
                const ok = await useStore.getState().confirm({ title: 'Delete this file?', body: `“${job.fileName}” will be removed from the list and deleted from your disk.`, confirmLabel: 'Delete file', cancelLabel: 'Keep it', danger: true })
                if (ok) void act(() => api.remove(job.id, true))
              },
              hidden: !done,
            },
          ]}
        />
      )}
    </>
  )
}
