import { AnimatePresence, motion } from 'motion/react'
import { ArrowRight, Check, Copy, FolderInput, Lock, Pencil, PackageOpen, Plus, Terminal, Trash2, Bell, Eye, X, RefreshCw } from 'lucide-react'
import qrcode from 'qrcode-generator'
import { useMemo, useState } from 'react'
import { api } from '../lib/api'
import { bytes, minutesToClock, parseSize } from '../lib/format'
import { useStore } from '../lib/store'
import type { PostAction, PostRule, Settings } from '../lib/types'
import { Button, Chip, Group, IconButton, Row, Segmented, Switch, TextField } from '../components/ui'

export default function Automation() {
  const settings = useStore((s) => s.settings)
  const update = useStore((s) => s.updateSettings)
  const local = useStore((s) => s.local)
  const [editing, setEditing] = useState<PostRule | 'new' | null>(null)
  if (!settings) return null

  const saveRules = (rules: PostRule[]) => update({ postRules: rules })
  const sched = settings.schedule

  return (
    <div className="page-pad">
      <header className="page-head" data-tauri-drag-region>
        <h1>Automation</h1>
      </header>
      <div className="scroll narrow">
        <Group title="After a download finishes" footer={local ? 'Rules run top to bottom for every completed file that matches. Commands run on this computer with the file’s path in $IDMX_FILE.' : 'Post-download rules can only be edited from the computer running IDMX.'}>
          {settings.postRules.length === 0 && (
            <div className="empty-row">
              <PackageOpen size={20} strokeWidth={1.5} />
              <span>No rules yet. Sort ISOs into one folder, unpack archives, run a script…</span>
            </div>
          )}
          {settings.postRules.map((r, idx) => (
            <div className="rule" key={r.id}>
              <div className="rule-main">
                <div className="rule-name">
                  <b>{r.name}</b>
                </div>
                <div className="rule-flow">
                  <span className="rule-when">{describeMatch(r)}</span>
                  <ArrowRight size={13} />
                  {r.actions.map((a, i) => (
                    <span className="rule-act" key={i}>
                      {actionIcon(a)}
                      {actionLabel(a)}
                    </span>
                  ))}
                </div>
              </div>
              <Switch checked={r.enabled} disabled={!local} onChange={(v) => saveRules(settings.postRules.map((x, i) => (i === idx ? { ...x, enabled: v } : x)))} />
              <IconButton label="Edit" disabled={!local} onClick={() => setEditing(r)}>
                <Pencil />
              </IconButton>
              <IconButton label="Delete" disabled={!local} onClick={() => saveRules(settings.postRules.filter((_, i) => i !== idx))}>
                <Trash2 />
              </IconButton>
            </div>
          ))}
          <div className="row">
            <Button variant="tinted" icon={<Plus size={15} />} disabled={!local} onClick={() => setEditing('new')}>
              Add rule
            </Button>
          </div>
        </Group>

        <Group title="Schedule" footer="Downloads marked “only in schedule window” wait for it. Use it to leave the big stuff for overnight, when the network is idle.">
          <Row title="Run scheduled downloads between" hint={sched.enabled ? `${minutesToClock(sched.startMinute)} → ${minutesToClock(sched.endMinute)}${sched.startMinute > sched.endMinute ? ' (overnight)' : ''}` : 'Off'}>
            <Switch checked={sched.enabled} onChange={(v) => update({ schedule: { ...sched, enabled: v } })} />
          </Row>
          {sched.enabled && (
            <div className="row stack">
              <Dial start={sched.startMinute} end={sched.endMinute} />
              <div className="time-row">
                <label>
                  From <input className="field mono" type="time" value={minutesToClock(sched.startMinute)} onChange={(e) => update({ schedule: { ...sched, startMinute: toMin(e.target.value) } })} />
                </label>
                <label>
                  To <input className="field mono" type="time" value={minutesToClock(sched.endMinute)} onChange={(e) => update({ schedule: { ...sched, endMinute: toMin(e.target.value) } })} />
                </label>
              </div>
            </div>
          )}
        </Group>

        <Group title="When the queue is empty">
          <Row title="After the last download finishes">
            <Segmented size="sm" value={settings.onQueueEmpty} onChange={(v) => update({ onQueueEmpty: v })} options={[{ value: 'nothing', label: 'Do nothing' }, { value: 'sleep', label: 'Sleep Mac' }, { value: 'quit', label: 'Quit IDMX' }]} />
          </Row>
        </Group>

        <BrowserIntegration />
        <RemoteAccess settings={settings} />
      </div>

      <AnimatePresence>
        {editing && (
          <RuleEditor
            rule={editing === 'new' ? null : editing}
            onClose={() => setEditing(null)}
            onSave={(r) => {
              const list = editing === 'new' ? [...settings.postRules, r] : settings.postRules.map((x) => (x.id === r.id ? r : x))
              void saveRules(list)
              setEditing(null)
            }}
          />
        )}
      </AnimatePresence>
    </div>
  )
}

const toMin = (s: string) => {
  const [h, m] = s.split(':').map(Number)
  return (h || 0) * 60 + (m || 0)
}

function describeMatch(r: PostRule): string {
  const w = r.when
  const parts: string[] = []
  if (w.extensions.length) parts.push(w.extensions.map((e) => `.${e}`).join(' '))
  if (w.host) parts.push(`from ${w.host}`)
  if (w.nameRegex) parts.push(`/${w.nameRegex}/`)
  if (w.minSize) parts.push(`≥ ${bytes(w.minSize, 0)}`)
  if (w.maxSize) parts.push(`≤ ${bytes(w.maxSize, 0)}`)
  return parts.length ? parts.join(' · ') : 'Every download'
}

function actionIcon(a: PostAction) {
  const p = { size: 12 }
  return a.type === 'moveTo' ? <FolderInput {...p} /> : a.type === 'extract' ? <PackageOpen {...p} /> : a.type === 'run' ? <Terminal {...p} /> : a.type === 'reveal' ? <Eye {...p} /> : <Bell {...p} />
}
function actionLabel(a: PostAction): string {
  switch (a.type) {
    case 'moveTo':
      return `Move to ${a.dir}`
    case 'extract':
      return a.deleteArchive ? 'Extract & delete archive' : 'Extract'
    case 'run':
      return a.command.length > 30 ? a.command.slice(0, 30) + '…' : a.command
    case 'reveal':
      return 'Show in folder'
    case 'notify':
      return 'Notify'
  }
}

function Dial({ start, end }: { start: number; end: number }) {
  const R = 54
  const pt = (m: number, r = R) => {
    const a = (m / 1440) * Math.PI * 2 - Math.PI / 2
    return [70 + Math.cos(a) * r, 70 + Math.sin(a) * r]
  }
  const [x1, y1] = pt(start)
  const [x2, y2] = pt(end)
  const span = (end - start + 1440) % 1440 || 1440
  return (
    <svg className="dial" viewBox="0 0 140 140" width="150" height="150">
      <circle cx="70" cy="70" r={R} fill="none" stroke="var(--fill-2)" strokeWidth="10" />
      <path d={`M${x1} ${y1} A${R} ${R} 0 ${span > 720 ? 1 : 0} 1 ${x2} ${y2}`} fill="none" stroke="var(--accent)" strokeWidth="10" strokeLinecap="round" />
      {[0, 6, 12, 18].map((h) => {
        const [tx, ty] = pt(h * 60, 36)
        return (
          <text key={h} x={tx} y={ty + 3} textAnchor="middle" className="mono" fontSize="9" fill="var(--fg-3)">
            {h}
          </text>
        )
      })}
      <text x="70" y="75" textAnchor="middle" className="mono" fontSize="12" fontWeight="600" fill="var(--fg)">
        {Math.round(span / 60)}h
      </text>
    </svg>
  )
}

function RuleEditor({ rule, onSave, onClose }: { rule: PostRule | null; onSave: (r: PostRule) => void; onClose: () => void }) {
  const [name, setName] = useState(rule?.name ?? 'New rule')
  const [exts, setExts] = useState(rule?.when.extensions.join(', ') ?? '')
  const [regex, setRegex] = useState(rule?.when.nameRegex ?? '')
  const [host, setHost] = useState(rule?.when.host ?? '')
  const [min, setMin] = useState(rule?.when.minSize ? bytes(rule.when.minSize, 0).replace(' ', '') : '')
  const [max, setMax] = useState(rule?.when.maxSize ? bytes(rule.when.maxSize, 0).replace(' ', '') : '')
  const [actions, setActions] = useState<PostAction[]>(rule?.actions ?? [])

  const setAct = (i: number, a: PostAction) => setActions((l) => l.map((x, k) => (k === i ? a : x)))
  const valid = name.trim() && actions.length > 0

  return (
    <motion.div className="scrim" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <motion.div className="sheet" initial={{ opacity: 0, y: 30, scale: 0.96 }} animate={{ opacity: 1, y: 0, scale: 1 }} exit={{ opacity: 0, y: 16, scale: 0.97 }} transition={{ type: 'spring', stiffness: 460, damping: 36 }}>
        <div className="sheet-head">
          <h2>{rule ? 'Edit rule' : 'New rule'}</h2>
        </div>
        <div className="sheet-row">
          <label>Name</label>
          <TextField value={name} onChange={setName} width="100%" />
        </div>
        <div className="sheet-sec">When a download…</div>
        <div className="adv-grid">
          <div className="adv-field">
            <label>Has extension</label>
            <TextField value={exts} onChange={setExts} placeholder="iso, dmg, zip" mono width="100%" />
          </div>
          <div className="adv-field">
            <label>Comes from</label>
            <TextField value={host} onChange={setHost} placeholder="example.com" mono width="100%" />
          </div>
          <div className="adv-field">
            <label>Name matches (regex)</label>
            <TextField value={regex} onChange={setRegex} placeholder="^ubuntu-\d+" mono width="100%" />
          </div>
          <div className="adv-field two">
            <div>
              <label>Min size</label>
              <TextField value={min} onChange={setMin} placeholder="1G" mono width="100%" invalid={!!min && parseSize(min) == null} />
            </div>
            <div>
              <label>Max size</label>
              <TextField value={max} onChange={setMax} placeholder="" mono width="100%" invalid={!!max && parseSize(max) == null} />
            </div>
          </div>
        </div>

        <div className="sheet-sec">Then…</div>
        <div className="act-list">
          {actions.map((a, i) => (
            <div className="act" key={i}>
              <div className="act-ic">{actionIcon(a)}</div>
              <div className="act-body">
                {a.type === 'moveTo' && <TextField value={a.dir} onChange={(v) => setAct(i, { ...a, dir: v })} placeholder="~/Movies/{year}  ({ext} {host} {date} {name})" mono width="100%" />}
                {a.type === 'extract' && (
                  <div className="act-inline">
                    <TextField value={a.dir ?? ''} onChange={(v) => setAct(i, { ...a, dir: v || null })} placeholder="Next to the file" mono width="100%" />
                    <label className="mini-sw">
                      Delete archive <Switch checked={a.deleteArchive} onChange={(v) => setAct(i, { ...a, deleteArchive: v })} />
                    </label>
                  </div>
                )}
                {a.type === 'run' && <TextField value={a.command} onChange={(v) => setAct(i, { ...a, command: v })} placeholder={'open "$IDMX_FILE"'} mono width="100%" />}
                {a.type === 'reveal' && <span className="muted">Show the file in Finder / Explorer</span>}
                {a.type === 'notify' && <TextField value={a.message ?? ''} onChange={(v) => setAct(i, { ...a, message: v || null })} placeholder="Message (optional)" width="100%" />}
              </div>
              <IconButton label="Remove" onClick={() => setActions((l) => l.filter((_, k) => k !== i))}>
                <X />
              </IconButton>
            </div>
          ))}
          <div className="chips">
            <Chip onClick={() => setActions((l) => [...l, { type: 'moveTo', dir: '' }])}>
              <FolderInput size={13} /> Move
            </Chip>
            <Chip onClick={() => setActions((l) => [...l, { type: 'extract', dir: null, deleteArchive: false }])}>
              <PackageOpen size={13} /> Extract
            </Chip>
            <Chip onClick={() => setActions((l) => [...l, { type: 'run', command: '' }])}>
              <Terminal size={13} /> Run command
            </Chip>
            <Chip onClick={() => setActions((l) => [...l, { type: 'reveal' }])}>
              <Eye size={13} /> Reveal
            </Chip>
            <Chip onClick={() => setActions((l) => [...l, { type: 'notify', message: null }])}>
              <Bell size={13} /> Notify
            </Chip>
          </div>
        </div>

        <div className="sheet-foot">
          <span style={{ flex: 1 }} />
          <Button variant="glass" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            disabled={!valid}
            icon={<Check size={15} />}
            onClick={() =>
              onSave({
                id: rule?.id ?? Math.random().toString(36).slice(2, 10),
                name: name.trim(),
                enabled: rule?.enabled ?? true,
                when: {
                  extensions: exts.split(/[,\s]+/).map((e) => e.replace(/^\./, '').toLowerCase()).filter(Boolean),
                  nameRegex: regex || null,
                  host: host || null,
                  minSize: min ? parseSize(min) : null,
                  maxSize: max ? parseSize(max) : null,
                },
                actions: actions.filter((a) => (a.type === 'moveTo' ? a.dir.trim() : a.type === 'run' ? a.command.trim() : true)),
              })
            }
          >
            Save rule
          </Button>
        </div>
      </motion.div>
    </motion.div>
  )
}

function CopyField({ label, value, secret }: { label: string; value: string; secret?: boolean }) {
  const [shown, setShown] = useState(!secret)
  const [done, setDone] = useState(false)
  return (
    <div className="copyfield">
      <label>{label}</label>
      <div className="cf-box mono">
        <span>{shown ? value : '•'.repeat(Math.min(28, value.length))}</span>
        {secret && (
          <button onClick={() => setShown(!shown)} aria-label="Reveal">
            <Eye size={14} />
          </button>
        )}
        <button
          aria-label="Copy"
          onClick={() => {
            void navigator.clipboard.writeText(value)
            setDone(true)
            setTimeout(() => setDone(false), 1200)
          }}
        >
          {done ? <Check size={14} /> : <Copy size={14} />}
        </button>
      </div>
    </div>
  )
}

function BrowserIntegration() {
  const settings = useStore((s) => s.settings)
  const local = useStore((s) => s.local)
  if (!settings) return null
  const url = `http://127.0.0.1:${settings.remote.port}`
  return (
    <Group title="Browser integration" footer="The IDMX browser extension (in the extension/ folder) sends links to this app, with your logged-in cookies, so downloads from sites that need a login just work. Load it with chrome://extensions → Developer mode → Load unpacked.">
      <div className="row stack">
        {local && settings.remote.token ? (
          <>
            <CopyField label="Server" value={url} />
            <CopyField label="Access token" value={settings.remote.token} secret />
          </>
        ) : (
          <span className="muted">Pairing details are only shown on the computer running IDMX.</span>
        )}
      </div>
    </Group>
  )
}

function RemoteAccess({ settings }: { settings: Settings }) {
  const update = useStore((s) => s.updateSettings)
  const interfaces = useStore((s) => s.interfaces)
  const local = useStore((s) => s.local)
  const toast = useStore((s) => s.toast)
  const [port, setPort] = useState(String(settings.remote.port))
  const ip = interfaces.find((i) => i.usable && i.ipv4[0] && i.kind !== 'vpn')?.ipv4[0]
  const token = settings.remote.token
  const link = ip && token ? `http://${ip}:${settings.remote.port}/#token=${token}` : ''

  const qr = useMemo(() => {
    if (!link) return null
    const q = qrcode(0, 'M')
    q.addData(link)
    q.make()
    const n = q.getModuleCount()
    const cells: [number, number][] = []
    for (let r = 0; r < n; r++) for (let c = 0; c < n; c++) if (q.isDark(r, c)) cells.push([c, r])
    return { n, cells }
  }, [link])

  return (
    <Group title="Remote dashboard" footer="Lets a phone or another computer on your network watch and control downloads. Anyone with the link can control IDMX, so only turn this on for networks you trust.">
      <Row title="Open to other devices on this network" hint={settings.remote.lan ? `Listening on every network interface, port ${settings.remote.port}` : 'Only this computer can connect'}>
        <Switch checked={settings.remote.lan} disabled={!local} onChange={(v) => update({ remote: { lan: v } })} />
      </Row>
      <Row title="Port">
        <TextField value={port} onChange={setPort} mono width={90} onBlur={() => {
          const p = Number(port)
          if (p >= 1024 && p <= 65535 && p !== settings.remote.port) {
            update({ remote: { port: p } }).then(() => toast({ kind: 'info', title: 'Port changed', body: 'Reloading…' })).then(() => setTimeout(() => location.reload(), 600))
          } else setPort(String(settings.remote.port))
        }} />
      </Row>
      {settings.remote.lan && local && link && (
        <div className="row stack">
          <div className="qr-wrap">
            {qr && (
              <svg className="qr" viewBox={`-2 -2 ${qr.n + 4} ${qr.n + 4}`} shapeRendering="crispEdges">
                <rect x="-2" y="-2" width={qr.n + 4} height={qr.n + 4} rx="2" fill="#fff" />
                {qr.cells.map(([x, y]) => (
                  <rect key={`${x}-${y}`} x={x} y={y} width="1" height="1" fill="#000" />
                ))}
              </svg>
            )}
            <div className="qr-side">
              <p>Scan with your phone, or open the link on any device on this network.</p>
              <CopyField label="Dashboard link" value={link} secret />
              <Button
                variant="glass"
                size="sm"
                icon={<RefreshCw size={13} />}
                onClick={() =>
                  api.newToken().then((r) => {
                    toast({ kind: 'info', title: 'New token created', body: r.note })
                  })
                }
              >
                New token
              </Button>
            </div>
          </div>
        </div>
      )}
      {settings.remote.lan && !local && (
        <div className="row">
          <span className="badge ok">
            <Lock size={11} /> Connected remotely
          </span>
        </div>
      )}
    </Group>
  )
}
