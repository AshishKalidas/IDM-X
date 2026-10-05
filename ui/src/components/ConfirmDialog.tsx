import { AnimatePresence, motion } from 'motion/react'
import { useEffect } from 'react'
import { useStore } from '../lib/store'
import { Button } from './ui'

/** Asks before anything that can't be undone. Rendered once; opened with `useStore.getState().confirm(...)`. */
export default function ConfirmDialog() {
  const dialog = useStore((s) => s.dialog)
  const answer = useStore((s) => s.answerDialog)
  useEffect(() => {
    if (!dialog) return
    const k = (e: KeyboardEvent) => {
      if (e.key === 'Escape') answer(false)
      if (e.key === 'Enter') answer(true)
    }
    window.addEventListener('keydown', k)
    return () => window.removeEventListener('keydown', k)
  }, [dialog, answer])
  return (
    <AnimatePresence>
      {dialog && (
        <motion.div className="scrim" style={{ zIndex: 200 }} initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} onMouseDown={(e) => e.target === e.currentTarget && answer(false)}>
          <motion.div className="confirm" role="alertdialog" initial={{ opacity: 0, scale: 0.94, y: 12 }} animate={{ opacity: 1, scale: 1, y: 0 }} exit={{ opacity: 0, scale: 0.97, transition: { duration: 0.12 } }} transition={{ type: 'spring', stiffness: 520, damping: 34 }}>
            <h3>{dialog.title}</h3>
            <p>{dialog.body}</p>
            <div className="confirm-actions">
              <Button variant="glass" onClick={() => answer(false)} autoFocus>
                {dialog.cancelLabel ?? 'Cancel'}
              </Button>
              <Button variant={dialog.danger ? 'danger' : 'primary'} onClick={() => answer(true)}>
                {dialog.confirmLabel}
              </Button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  )
}
