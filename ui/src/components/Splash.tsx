import { AnimatePresence, motion } from 'motion/react'
import { useEffect, useState } from 'react'
import { useScramble } from '../lib/hooks'
import Logo from './Logo'

/** A short boot sequence: the mark draws itself, the name decodes, then it dissolves into the app. */
export default function Splash({ ready }: { ready: boolean }) {
  const [minDone, setMinDone] = useState(false)
  useEffect(() => {
    const t = setTimeout(() => setMinDone(true), 1500)
    return () => clearTimeout(t)
  }, [])
  const word = useScramble('IDMX', 700)
  const sub = useScramble('every network. one download.', 1100)
  const show = !(ready && minDone)
  return (
    <AnimatePresence>
      {show && (
        <motion.div className="splash" initial={{ opacity: 1 }} exit={{ opacity: 0, scale: 1.04, filter: 'blur(8px)', transition: { duration: 0.55, ease: [0.2, 0.8, 0.2, 1] } }}>
          <div className="splash-grid" />
          <Logo size={92} draw />
          <div className="splash-word mono">{word}</div>
          <div className="splash-sub mono">{sub}</div>
          <div className="splash-bar">
            <motion.i initial={{ x: '-100%' }} animate={{ x: '0%' }} transition={{ duration: 1.4, ease: [0.2, 0.8, 0.2, 1] }} />
          </div>
        </motion.div>
      )}
    </AnimatePresence>
  )
}
