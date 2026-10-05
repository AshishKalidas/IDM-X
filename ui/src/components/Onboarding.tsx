import { AnimatePresence, motion } from 'motion/react'
import { Download, CheckCircle2, Magnet, Film, Globe } from 'lucide-react'
import { useEffect, useState } from 'react'
import { api } from '../lib/api'
import { bytes } from '../lib/format'
import { useStore } from '../lib/store'
import BrowserSetup from './BrowserSetup'
import { Button } from './ui'

/** Shown once, the first time IDMX runs on this computer. */
export default function Onboarding() {
  const settings = useStore((s) => s.settings)
  const local = useStore((s) => s.local)
  const save = useStore((s) => s.updateSettings)
  const tools = useStore((s) => s.tools)
  const setTools = useStore((s) => s.setTools)
  const ext = useStore((s) => s.extension)
  const [step, setStep] = useState(0)
  const show = !!settings && local && !settings.onboarded

  useEffect(() => {
    if (show) void api.tools().then(setTools).catch(() => {})
  }, [show, setTools])

  const finish = () => void save({ onboarded: true })
  const ready = !!tools?.ytdlp
  const inst = tools?.installing

  const steps = [
    {
      icon: <Globe size={26} />,
      title: 'Grab downloads from your browser',
      body: (
        <>
          <p>Install the IDMX extension and links, videos and magnet links go straight to IDMX — no copying and pasting. Videos on the page get a download button.</p>
          <BrowserSetup />
        </>
      ),
    },
    {
      icon: <Film size={26} />,
      title: 'Video from thousands of sites',
      body: (
        <>
          <p>IDMX uses yt-dlp, running on this computer, to download video and audio from YouTube and most other sites. It installs into IDMX’s own folder and is checked against a published checksum.</p>
          {ready ? (
            <p style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
              <CheckCircle2 size={16} color="#30d158" /> yt-dlp {tools?.ytdlp?.version} is ready
            </p>
          ) : (
            <Button variant="primary" icon={<Download size={14} />} disabled={!!inst} onClick={() => void api.installTool('ytdlp').catch(() => {})}>
              {inst ? `Installing ${inst.tool}… ${bytes(inst.downloaded)}` : 'Install yt-dlp'}
            </Button>
          )}
          {inst?.error && <p className="err">{inst.error}</p>}
          <p className="muted-note">The first full-quality YouTube download also fetches a small JavaScript runtime and ffmpeg; IDMX asks before it does.</p>
        </>
      ),
    },
    {
      icon: <Magnet size={26} />,
      title: 'Torrents too',
      body: <p>Paste a magnet link or a .torrent link into New download, choose which files you want, and IDMX downloads and seeds it. Magnet links clicked in your browser are caught by the extension.</p>,
    },
  ]
  const cur = steps[step]

  return (
    <AnimatePresence>
      {show && (
        <motion.div className="scrim" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }}>
          <motion.div className="sheet" style={{ maxWidth: 520 }} initial={{ opacity: 0, y: 30, scale: 0.96 }} animate={{ opacity: 1, y: 0, scale: 1 }} exit={{ opacity: 0, y: 16 }}>
            <div className="sheet-head" style={{ gap: 12 }}>
              {cur.icon}
              <h2 style={{ flex: 1 }}>{cur.title}</h2>
              <span className="mono muted-note">
                {step + 1}/{steps.length}
              </span>
            </div>
            <div key={step} style={{ lineHeight: 1.5 }}>
              {cur.body}
            </div>
            <div className="sheet-foot">
              <Button variant="plain" onClick={finish}>
                Skip setup
              </Button>
              <span style={{ flex: 1 }} />
              {step > 0 && (
                <Button variant="glass" onClick={() => setStep(step - 1)}>
                  Back
                </Button>
              )}
              <Button variant="primary" onClick={() => (step === steps.length - 1 ? finish() : setStep(step + 1))}>
                {step === steps.length - 1 ? 'Done' : ext && step === 0 ? 'Next' : 'Next'}
              </Button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  )
}
