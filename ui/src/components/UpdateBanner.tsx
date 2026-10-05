import { motion } from 'motion/react'
import { ArrowUpCircle, X } from 'lucide-react'
import { api, openExternal } from '../lib/api'
import { useStore } from '../lib/store'
import { Button, IconButton } from './ui'

/** A new release is out. Dismissing it hides the banner for that version only; a quiet icon stays. */
export default function UpdateBanner() {
  const update = useStore((s) => s.update)
  if (!update || update.dismissed) return null
  return (
    <motion.div className="update-banner" initial={{ opacity: 0, y: -14, height: 0 }} animate={{ opacity: 1, y: 0, height: 'auto' }} exit={{ opacity: 0, height: 0 }} transition={{ type: 'spring', stiffness: 460, damping: 36 }}>
      <ArrowUpCircle size={18} />
      <span>
        <b>IDMX {update.version}</b> is available.
      </span>
      <span style={{ flex: 1 }} />
      <Button size="sm" variant="primary" onClick={() => void openExternal(update.url)}>
        View release
      </Button>
      <Button size="sm" variant="plain" onClick={() => void api.dismissUpdate()}>
        Not now
      </Button>
      <IconButton label="Dismiss" onClick={() => void api.dismissUpdate()}>
        <X />
      </IconButton>
    </motion.div>
  )
}
