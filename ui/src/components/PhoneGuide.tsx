import { AnimatePresence, motion } from 'motion/react'
import { CheckCircle2, ChevronDown, Smartphone } from 'lucide-react'
import { useEffect, useState } from 'react'
import { api, openExternal } from '../lib/api'
import { useStore } from '../lib/store'
import { Button, Segmented } from './ui'

type Phone = 'iphone' | 'android'

// Direct download (no account or website to navigate). Only an Apple-silicon build is published.
const TETHERKIT_ZIP = 'https://github.com/XiaoMiku01/TetherKit/releases/download/v0.1.5/TetherKit-v0.1.5-macos-arm64.zip'
const TETHERKIT_PAGE = 'https://github.com/XiaoMiku01/TetherKit/releases/latest'

/** Plain-language help for adding a phone's mobile data as another connection, with live status. */
export default function PhoneGuide({ startOpen }: { startOpen: boolean }) {
  const interfaces = useStore((s) => s.interfaces)
  const platform = useStore((s) => s.platform)
  const arch = useStore((s) => s.arch)
  const local = useStore((s) => s.local)
  const [open, setOpen] = useState(startOpen)
  const [phone, setPhone] = useState<Phone>('iphone')
  useEffect(() => setOpen(startOpen), [startOpen])

  const tether = interfaces.filter((i) => i.kind === 'tether')
  const ready = tether.some((i) => i.usable && i.enabled)
  const seen = tether.length > 0
  const status = ready ? { tone: 'ok', text: 'Phone connected and ready' } : seen ? { tone: 'warn', text: 'Phone found — waiting for its internet' } : { tone: '', text: 'No phone detected yet' }

  const mac = platform === 'macos'
  const win = platform === 'windows'
  const steps =
    phone === 'iphone'
      ? [
          <>Plug your iPhone into this computer with its cable. Unlock it and tap <b>Trust</b> if it asks.</>,
          <>On the iPhone open <b>Settings → Personal Hotspot</b> and switch on <b>Allow Others to Join</b>. Mobile data must be on.</>,
          ...(win ? [<>Windows needs Apple’s <b>Apple Devices</b> app (free, from the Microsoft Store) to see an iPhone.</>] : []),
          <>That’s it — the phone shows up below within a few seconds.</>,
        ]
      : mac
        ? [
            <>macOS can’t share an Android phone’s data by itself, so you need a small free helper called <b>TetherKit</b>. Download it, open the file, then drag TetherKit into your Applications folder.{' '}
              {arch === 'x86_64' ? (
                <>
                  <span className="muted-note">(Its ready-made download is for newer Macs with Apple silicon; yours is an Intel Mac.)</span>{' '}
                  <button className="linklike" onClick={() => void openExternal(TETHERKIT_PAGE)}>See other downloads</button>
                </>
              ) : (
                <Button variant="primary" size="sm" onClick={() => void openExternal(TETHERKIT_ZIP)}>
                  Download TetherKit
                </Button>
              )}
            </>,
            <>Plug your Android phone in with its cable.</>,
            <>On the phone open <b>Settings → Network &amp; internet → Hotspot &amp; tethering</b> and switch on <b>USB tethering</b>.</>,
            <>Open TetherKit (the first time, right-click it and choose <b>Open</b>), pick your phone and click <b>Connect</b>.</>,
            <>Once macOS shows it as a network connection, it appears below and IDMX uses it automatically.</>,
          ]
        : [
            <>Plug your Android phone in with its cable.</>,
            <>On the phone open <b>Settings → Network &amp; internet → Hotspot &amp; tethering</b> and switch on <b>USB tethering</b>.</>,
            <>{win ? 'Windows' : 'Your computer'} sets it up by itself. The phone shows up below within a few seconds.</>,
          ]

  return (
    <div className="phoneguide">
      <button className="pg-head" onClick={() => setOpen(!open)} aria-expanded={open}>
        <Smartphone size={18} />
        <span className="pg-title">
          <b>Add your phone’s mobile data</b>
          <small>Plug it in and download over Wi-Fi and mobile data at once</small>
        </span>
        <span className={`pg-status ${status.tone}`}>{ready && <CheckCircle2 size={13} />} {status.text}</span>
        <ChevronDown size={16} style={{ transform: open ? 'rotate(180deg)' : 'none', transition: 'transform .25s' }} />
      </button>
      <AnimatePresence initial={false}>
        {open && (
          <motion.div initial={{ height: 0, opacity: 0 }} animate={{ height: 'auto', opacity: 1 }} exit={{ height: 0, opacity: 0 }} className="pg-body">
            <Segmented<Phone> value={phone} onChange={setPhone} options={[{ value: 'iphone', label: 'iPhone' }, { value: 'android', label: 'Android' }]} />
            <ol className="pg-steps">
              {steps.map((s, i) => (
                <li key={i}>{s}</li>
              ))}
            </ol>
            <div className="pg-foot">
              <span className="muted-note">Your phone’s data counts against your mobile plan. You can set a limit for it on its card below.</span>
              {local && (
                <Button variant="glass" size="sm" onClick={() => void api.refreshNetworks().catch(() => {})}>
                  Check again
                </Button>
              )}
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  )
}
