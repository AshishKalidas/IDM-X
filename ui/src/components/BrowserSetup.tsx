import { Chrome, CheckCircle2, FolderOpen } from 'lucide-react'
import { useEffect, useState } from 'react'
import { api } from '../lib/api'
import { useStore } from '../lib/store'
import type { BrowserState } from '../lib/types'
import { Button } from './ui'

/** Installs the browser extension: unpack it, register the pairing host, then open the browser's extensions page. */
export default function BrowserSetup() {
  const ext = useStore((s) => s.extension)
  const local = useStore((s) => s.local)
  const [st, setSt] = useState<BrowserState | null>(null)
  const [err, setErr] = useState('')

  useEffect(() => {
    if (local) void api.browser().then(setSt).catch(() => {})
  }, [local])

  async function setup(id: string) {
    setErr('')
    try {
      setSt(await api.browserSetup())
      await api.browserOpen(id)
    } catch (e) {
      setErr(String((e as Error).message ?? e))
    }
  }

  if (!local) return <p className="muted-note">Set this up from the computer IDMX runs on.</p>
  if (ext) {
    return (
      <p style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
        <CheckCircle2 size={16} color="#30d158" /> Connected to {ext.browser || 'your browser'} (extension {ext.version})
      </p>
    )
  }
  const installed = st?.browsers.filter((b) => b.installed) ?? []
  return (
    <div>
      <div className="chips" style={{ marginBottom: 10 }}>
        {installed.length === 0 && <span className="muted-note">No Chromium browser found.</span>}
        {installed.map((b) => (
          <Button key={b.id} variant="primary" size="sm" icon={<Chrome size={14} />} onClick={() => setup(b.id)}>
            Set up in {b.name}
          </Button>
        ))}
        <Button variant="glass" size="sm" icon={<FolderOpen size={14} />} onClick={() => void api.browserSetup().then(setSt).then(() => api.browserReveal())}>
          Show extension folder
        </Button>
      </div>
      <ol className="muted-note" style={{ paddingLeft: 18, lineHeight: 1.6 }}>
        <li>Click your browser above — its extensions page opens.</li>
        <li>Switch on <b>Developer mode</b> (top right).</li>
        <li>Click <b>Show extension folder</b>. The IDMX folder is highlighted in Finder / Explorer — <b>drag it onto the extensions page</b> (or choose <b>Load unpacked</b> and pick it).</li>
        <li>Done. The extension finds IDMX by itself, with no token to copy.</li>
      </ol>
      {err && <p className="err">{err}</p>}
    </div>
  )
}
