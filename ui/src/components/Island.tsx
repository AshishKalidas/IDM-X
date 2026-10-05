import { AnimatePresence, motion } from 'motion/react'
import { AlertTriangle, CheckCircle2, Info, PartyPopper } from 'lucide-react'
import { useStore } from '../lib/store'

/** Notifications that morph out of the top of the window, like the iPhone's Dynamic Island. */
export default function Island() {
  const toasts = useStore((s) => s.toasts)
  const dismiss = useStore((s) => s.dismissToast)
  const t = toasts[toasts.length - 1]
  return (
    <div className="island-wrap">
      <AnimatePresence mode="popLayout">
        {t && (
          <motion.div
            key={t.id}
            layout
            className={`island ${t.kind}`}
            initial={{ opacity: 0, y: -26, scale: 0.7, borderRadius: 40 }}
            animate={{ opacity: 1, y: 0, scale: 1, borderRadius: 26 }}
            exit={{ opacity: 0, y: -18, scale: 0.8, transition: { duration: 0.18 } }}
            transition={{ type: 'spring', stiffness: 520, damping: 30, mass: 0.9 }}
            onClick={() => dismiss(t.id)}
          >
            <span className="island-ic">{t.kind === 'complete' ? <CheckCircle2 size={20} /> : t.kind === 'error' ? <AlertTriangle size={20} /> : t.kind === 'queue-done' ? <PartyPopper size={20} /> : <Info size={20} />}</span>
            <div className="island-text">
              <b>{t.title}</b>
              {t.body && <span>{t.body}</span>}
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  )
}
