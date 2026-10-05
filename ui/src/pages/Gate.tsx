import { motion } from 'motion/react'
import { KeyRound, PlugZap } from 'lucide-react'
import { useState } from 'react'
import { useStore } from '../lib/store'
import Logo from '../components/Logo'
import { Button, TextField } from '../components/ui'

/** Shown instead of the app when it can't (yet) talk to an engine. */
export default function Gate({ kind, detail }: { kind: 'need-token' | 'unreachable'; detail?: string }) {
  const submit = useStore((s) => s.submitToken)
  const init = useStore((s) => s.init)
  const [t, setT] = useState('')
  return (
    <div className="gate">
      <motion.div className="gate-card" initial={{ opacity: 0, y: 20, scale: 0.97 }} animate={{ opacity: 1, y: 0, scale: 1 }} transition={{ type: 'spring', stiffness: 400, damping: 34 }}>
        <Logo size={44} />
        {kind === 'need-token' ? (
          <>
            <h1>Connect to IDMX</h1>
            <p>Enter the access token shown in the desktop app under Settings → Remote access, or open the full dashboard link it gives you.</p>
            <div className="gate-row">
              <KeyRound size={16} />
              <TextField value={t} onChange={setT} placeholder="Access token" mono type="password" width="100%" />
            </div>
            {detail && <div className="gate-err">{detail}</div>}
            <Button variant="primary" size="lg" disabled={t.trim().length < 8} onClick={() => submit(t)}>
              Connect
            </Button>
          </>
        ) : (
          <>
            <h1>Can’t reach IDMX</h1>
            <p>The engine isn’t answering. Start the app, or run <code>idmx daemon</code> on the machine that’s downloading.</p>
            {detail && <div className="gate-err">{detail}</div>}
            <Button variant="primary" size="lg" icon={<PlugZap size={16} />} onClick={() => init()}>
              Try again
            </Button>
          </>
        )}
      </motion.div>
    </div>
  )
}
