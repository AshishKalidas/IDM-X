import { AnimatePresence, motion } from 'motion/react'
import { UploadCloud } from 'lucide-react'
import { useEffect, useState } from 'react'
import { api, isTauri } from './lib/api'
import { useKey } from './lib/hooks'
import { useStore } from './lib/store'
import AddSheet from './components/AddSheet'
import Onboarding from './components/Onboarding'
import CommandPalette from './components/CommandPalette'
import ConfirmDialog from './components/ConfirmDialog'
import Island from './components/Island'
import Sidebar from './components/Sidebar'
import Splash from './components/Splash'
import UpdateBanner from './components/UpdateBanner'
import Gate from './pages/Gate'
import Downloads from './pages/Downloads'
import Networks from './pages/Networks'
import Automation from './pages/Automation'
import SpeedTest from './pages/SpeedTest'
import SettingsPage from './pages/Settings'

export default function App() {
  const phase = useStore((s) => s.phase)
  const detail = useStore((s) => s.detail)
  const page = useStore((s) => s.page)
  const platform = useStore((s) => s.platform)
  const init = useStore((s) => s.init)
  const openAdd = useStore((s) => s.openAdd)
  const setPalette = useStore((s) => s.setPalette)
  const setPage = useStore((s) => s.setPage)
  const clipboardWatch = useStore((s) => s.settings?.clipboardWatch)
  const [dragging, setDragging] = useState(false)

  useEffect(() => void init(), [init])
  useEffect(() => {
    document.documentElement.classList.toggle('tauri', isTauri())
    document.documentElement.classList.toggle('mac', platform === 'macos')
  }, [platform])

  useKey((e) => {
    const mod = e.metaKey || e.ctrlKey
    if (!mod) return
    if (e.key === 'k') (e.preventDefault(), setPalette(true))
    else if (e.key === 'n') (e.preventDefault(), openAdd())
    else if (e.key === ',') (e.preventDefault(), setPage('settings'))
  })

  // Drop a link (or a text file of links) anywhere to add it.
  useEffect(() => {
    let depth = 0
    const over = (e: DragEvent) => e.preventDefault()
    const enter = (e: DragEvent) => ((depth += 1), e.preventDefault(), setDragging(true))
    const leave = () => ((depth = Math.max(0, depth - 1)), depth === 0 && setDragging(false))
    const drop = async (e: DragEvent) => {
      e.preventDefault()
      depth = 0
      setDragging(false)
      let text = e.dataTransfer?.getData('text/uri-list') || e.dataTransfer?.getData('text/plain') || ''
      const f = e.dataTransfer?.files?.[0]
      if (f && /\.torrent$/i.test(f.name)) {
        // The browser can't tell us the file's path, so hand the bytes to IDMX and open it from there.
        try {
          openAdd((await api.stageTorrent(f, useStore.getState().settings?.downloadDir)).path)
        } catch (err) {
          useStore.getState().toast({ kind: 'error', title: "Couldn't read that torrent", body: String((err as Error).message ?? err) })
        }
        return
      }
      if (!text && f && f.size < 2_000_000) text = await f.text()
      if (text.trim()) openAdd(text.trim())
    }
    window.addEventListener('dragover', over)
    window.addEventListener('dragenter', enter)
    window.addEventListener('dragleave', leave)
    window.addEventListener('drop', drop)
    return () => {
      window.removeEventListener('dragover', over)
      window.removeEventListener('dragenter', enter)
      window.removeEventListener('dragleave', leave)
      window.removeEventListener('drop', drop)
    }
  }, [openAdd])

  // Paste a magnet link anywhere (outside a text box) and it snaps into the Add sheet.
  useEffect(() => {
    const paste = (e: ClipboardEvent) => {
      const el = e.target as HTMLElement | null
      if (el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable)) return
      const t = e.clipboardData?.getData('text')?.trim() ?? ''
      if (/^magnet:\?/i.test(t)) {
        e.preventDefault()
        openAdd(t)
      }
    }
    window.addEventListener('paste', paste)
    return () => window.removeEventListener('paste', paste)
  }, [openAdd])

  // Offer a link that's on the clipboard when the window comes to the front.
  useEffect(() => {
    if (!clipboardWatch) return
    let last = ''
    const check = async () => {
      try {
        const t = (await navigator.clipboard.readText()).trim()
        if (t && t !== last && /^https?:\/\/\S+$/i.test(t)) {
          last = t
          openAdd(t)
        }
      } catch {
        /* no permission — fine */
      }
    }
    window.addEventListener('focus', check)
    return () => window.removeEventListener('focus', check)
  }, [clipboardWatch, openAdd])

  if (phase === 'need-token' || phase === 'unreachable') return <Gate kind={phase} detail={detail} />

  return (
    <div className="app">
      <Sidebar />
      <main className="main">
        <UpdateBanner />
        <motion.div key={page} className="page" initial={{ opacity: 0, y: 10 }} animate={{ opacity: 1, y: 0 }} transition={{ duration: 0.26, ease: [0.2, 0.8, 0.2, 1] }}>
          {page === 'downloads' && <Downloads />}
          {page === 'torrents' && <Downloads only="torrent" />}
          {page === 'networks' && <Networks />}
          {page === 'automation' && <Automation />}
          {page === 'speed' && <SpeedTest />}
          {page === 'settings' && <SettingsPage />}
        </motion.div>
      </main>
      <Island />
      <AddSheet />
      <Onboarding />
      <CommandPalette />
      <ConfirmDialog />
      <AnimatePresence>
        {dragging && (
          <motion.div className="dropzone" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }}>
            <motion.div initial={{ scale: 0.9 }} animate={{ scale: 1 }} className="dropzone-in">
              <UploadCloud size={44} strokeWidth={1.5} />
              <h2>Drop to download</h2>
              <p>Links, or a text file full of them</p>
            </motion.div>
          </motion.div>
        )}
      </AnimatePresence>
      <Splash ready={phase === 'ready'} />
    </div>
  )
}
