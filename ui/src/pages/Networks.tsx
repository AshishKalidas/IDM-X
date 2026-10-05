import { AnimatePresence, motion } from 'motion/react'
import { Activity, AlertTriangle, ChevronDown, Gauge as GaugeIcon, SlidersHorizontal, WifiOff } from 'lucide-react'
import { useMemo, useState } from 'react'
import { api } from '../lib/api'
import { bytes, parseSize, rate } from '../lib/format'
import { infoColor, kindMeta, PALETTE } from '../lib/kinds'
import { useStore } from '../lib/store'
import type { InterfaceInfo, InterfaceRule } from '../lib/types'
import PhoneGuide from '../components/PhoneGuide'
import NoConnections, { useOffline } from '../components/NoConnections'
import { Button, Ring, Segmented, Slider, Spark, Switch, TextField } from '../components/ui'

const DEFAULT_RULE: InterfaceRule = { metered: false, capPeriod: 'monthly' }

export default function Networks() {
  const interfaces = useStore((s) => s.interfaces)
  const history = useStore((s) => s.history)
  const [showOther, setShowOther] = useState(false)
  const usable = interfaces.filter((i) => i.usable)
  const other = interfaces.filter((i) => !i.usable && i.kind !== 'loopback')
  const live = usable.filter((i) => i.enabled).length
  const toast = useStore((s) => s.toast)
  const local = useStore((s) => s.local)
  const [pinging, setPinging] = useState(false)
  const offline = useOffline()
  const phone = other.find((i) => i.kind === 'tether')

  return (
    <div className="page-pad">
      <header className="page-head" data-tauri-drag-region>
        <h1>Networks</h1>
        <div className="head-actions">
          <Button
            variant="glass"
            size="sm"
            icon={pinging ? <i className="spin" /> : <Activity size={14} />}
            disabled={pinging}
            onClick={() => {
              setPinging(true)
              api.ping().catch((e) => toast({ kind: 'error', title: 'Ping failed', body: String(e.message ?? e) })).finally(() => setPinging(false))
            }}
          >
            Ping all
          </Button>
          {local && (
            <Button variant="glass" size="sm" icon={<SlidersHorizontal size={14} />} onClick={() => void api.openNetworkSettings().catch(() => {})}>
              Network settings
            </Button>
          )}
        </div>
      </header>
      <div className="scroll">
        <p className="lede">
          IDMX binds each connection to one specific network, so Wi-Fi, Ethernet, a tethered phone and a cellular modem can all pull the same file at the same time. <b>{live}</b> {live === 1 ? 'network is' : 'networks are'} ready.
        </p>

        {offline && <NoConnections compact />}

        <PhoneGuide startOpen={usable.length <= 1 || !!phone} />

        <div className="net-grid">
          {usable.map((i, idx) => (
            <NetCard key={i.id} info={i} history={history} index={idx} />
          ))}
        </div>

        {other.length > 0 && (
          <div className="others">
            <button className="more-toggle" onClick={() => setShowOther(!showOther)}>
              <ChevronDown size={14} style={{ transform: showOther ? 'rotate(180deg)' : 'none' }} />
              {other.length} other interface{other.length === 1 ? '' : 's'} (no connection, or plumbing)
            </button>
            <AnimatePresence initial={false}>
              {showOther && (
                <motion.div initial={{ height: 0, opacity: 0 }} animate={{ height: 'auto', opacity: 1 }} exit={{ height: 0, opacity: 0 }} className="other-list">
                  {other.map((i) => {
                    const Icon = kindMeta[i.kind].icon
                    return (
                      <div key={i.id} className="other-row">
                        <Icon size={14} />
                        <span className="mono">{i.id}</span>
                        <span>{i.label}</span>
                        <span className="badge">{i.up ? 'no address' : 'down'}</span>
                      </div>
                    )
                  })}
                </motion.div>
              )}
            </AnimatePresence>
          </div>
        )}
      </div>
    </div>
  )
}

function NetCard({ info, history, index }: { info: InterfaceInfo; history: ReturnType<typeof useStore.getState>['history']; index: number }) {
  const [open, setOpen] = useState(false)
  const color = infoColor(info)
  const Icon = kindMeta[info.kind].icon
  const rule = { ...DEFAULT_RULE, ...info.rule }
  const series = useMemo(() => history.slice(-60).map((h) => h.nets[info.id] ?? 0), [history, info.id])
  const toast = useStore((s) => s.toast)
  const [label, setLabel] = useState(rule.label ?? '')
  const [limit, setLimit] = useState(rule.speedLimit ? bytes(rule.speedLimit, 0).replace(' ', '').replace('B', '') : '')
  const [cap, setCap] = useState(rule.dataCap ? bytes(rule.dataCap, 0).replace(' ', '').replace('B', '') : '')

  const save = (patch: Partial<InterfaceRule>) => api.setRule(info.id, { ...rule, ...patch }).catch((e) => toast({ kind: 'error', title: 'Could not save', body: String(e.message ?? e) }))
  const capFrac = rule.dataCap ? Math.min(1, info.usedBytes / rule.dataCap) : 0
  const problem: Record<string, { label: string; cls: string } | undefined> = {
    unreachable: { label: info.reachable === false ? 'No internet' : 'Unreachable', cls: 'bad' },
    failed: { label: 'Refused', cls: 'bad' },
    offline: { label: 'Offline', cls: 'bad' },
    capped: { label: 'Data cap reached', cls: 'warn' },
  }
  const badge = info.enabled ? problem[info.state] : undefined

  return (
    <motion.div className={`net-card ${info.enabled ? '' : 'off'}`} style={{ ['--c' as string]: color }} initial={{ opacity: 0, y: 16 }} animate={{ opacity: 1, y: 0 }} transition={{ delay: index * 0.05, type: 'spring', stiffness: 420, damping: 36 }} layout>
      <div className="nc-glow" />
      <div className="nc-head">
        <div className="nc-icon">
          <Icon size={20} />
        </div>
        <div className="nc-title">
          <b>{rule.label || info.label}</b>
          <span className="mono">
            {info.id} · {info.ipv4[0] ?? info.ipv6[0] ?? '—'}
          </span>
        </div>
        {badge && (
          <span className={`badge ${badge.cls}`}>
            {badge.label === 'No internet' && <WifiOff size={11} />}
            {badge.label}
          </span>
        )}
        <Switch checked={info.enabled} onChange={(v) => save({ enabled: v })} />
      </div>

      <div className="nc-live">
        <div>
          <div className="eyebrow">Right now</div>
          <div className="nc-speed mono">{info.speed > 1 ? rate(info.speed) : 'idle'}</div>
        </div>
        <Spark values={series} color={color} width={130} height={44} />
      </div>

      <div className="nc-stats">
        <div>
          <span className="eyebrow">Health</span>
          <div className="nc-ring">
            <Ring value={info.health / 100} size={30} stroke={4} color={info.health > 70 ? 'var(--ok)' : info.health > 40 ? 'var(--warn)' : 'var(--bad)'} />
            <b className="mono">{info.health}</b>
          </div>
        </div>
        <div>
          <span className="eyebrow">Latency</span>
          <b className="mono">{info.latencyMs ? `${Math.round(info.latencyMs)} ms` : '—'}</b>
        </div>
        <div>
          <span className="eyebrow">Gateway</span>
          <b className="mono">{info.gateway ?? '—'}</b>
        </div>
        <div>
          <span className="eyebrow">Link</span>
          <b className="mono">{info.linkMbps ? `${info.linkMbps} Mbit` : '—'}</b>
        </div>
      </div>

      {info.sharesSubnetWith.length > 0 ? (
        <div className="nc-warn">
          <AlertTriangle size={14} />
          <span>
            On the same subnet as <b>{info.sharesSubnetWith.join(', ')}</b> — they sit on one link, so combining them won’t add speed.
          </span>
        </div>
      ) : (
        info.sharesGatewayWith.length > 0 && (
          <div className="nc-warn">
            <AlertTriangle size={14} />
            <span>
              Same gateway as <b>{info.sharesGatewayWith.join(', ')}</b> — they share one internet connection, so combining them won’t add speed.
            </span>
          </div>
        )
      )}

      {rule.dataCap ? (
        <div className="nc-cap">
          <div className="nc-cap-top mono">
            <span>
              {bytes(info.usedBytes)} of {bytes(rule.dataCap)} {rule.capPeriod}
            </span>
            <span>{Math.round(capFrac * 100)}%</span>
          </div>
          <div className="stackbar" style={{ height: 5 }}>
            <div className="stackbar-fill" style={{ width: `${capFrac * 100}%` }}>
              <span className="seg" style={{ flexGrow: 1, background: capFrac > 0.9 ? 'var(--bad)' : capFrac > 0.7 ? 'var(--warn)' : color }} />
            </div>
          </div>
        </div>
      ) : (
        <div className="nc-cap-top mono muted">{bytes(info.usedBytes)} used this period</div>
      )}

      <button className="more-toggle" onClick={() => setOpen(!open)}>
        <ChevronDown size={14} style={{ transform: open ? 'rotate(180deg)' : 'none' }} />
        Rules
      </button>
      <AnimatePresence initial={false}>
        {open && (
          <motion.div className="nc-rules" initial={{ height: 0, opacity: 0 }} animate={{ height: 'auto', opacity: 1 }} exit={{ height: 0, opacity: 0 }} transition={{ type: 'spring', stiffness: 400, damping: 40 }}>
            <div className="nc-rule">
              <label>Name</label>
              <TextField value={label} onChange={setLabel} placeholder={info.label} onBlur={() => save({ label: label || null })} width="100%" />
            </div>
            <div className="nc-rule">
              <label>Colour</label>
              <div className="swatches">
                <button className={!rule.color ? 'on auto' : 'auto'} onClick={() => save({ color: null })} title="Automatic">
                  A
                </button>
                {PALETTE.map((c) => (
                  <button key={c} className={rule.color === c ? 'on' : ''} style={{ background: c }} onClick={() => save({ color: c })} aria-label={c} />
                ))}
              </div>
            </div>
            <div className="nc-rule">
              <label>Speed limit</label>
              <TextField value={limit} onChange={setLimit} placeholder="Unlimited" mono width={120} invalid={!!limit && parseSize(limit) == null} onBlur={() => save({ speedLimit: limit ? parseSize(limit) : null })} />
            </div>
            <div className="nc-rule">
              <label>Connections</label>
              <Slider value={rule.streams ?? 8} min={2} max={32} onChange={(v) => save({ streams: v })} />
            </div>
            <div className="nc-rule inline">
              <div>
                <label>Metered</label>
                <small>Pay-per-GB link. Only joins big files, and respects its data cap.</small>
              </div>
              <Switch checked={rule.metered} onChange={(v) => save({ metered: v })} />
            </div>
            <div className="nc-rule">
              <label>Data cap</label>
              <div className="cap-row">
                <TextField value={cap} onChange={setCap} placeholder="None" mono width={110} invalid={!!cap && parseSize(cap) == null} onBlur={() => save({ dataCap: cap ? parseSize(cap) : null })} />
                <Segmented size="sm" value={rule.capPeriod} onChange={(v) => save({ capPeriod: v })} options={[{ value: 'daily', label: 'Day' }, { value: 'monthly', label: 'Month' }]} />
              </div>
            </div>
            <div className="nc-rule hint">
              <GaugeIcon size={13} /> Files smaller than {rule.metered && !rule.minFileSize ? '256 MB' : bytes(rule.minFileSize ?? 0)} skip this network.
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </motion.div>
  )
}
