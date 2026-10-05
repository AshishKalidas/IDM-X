import { motion } from 'motion/react'
import { useId, useState, type ReactNode } from 'react'
import { useTween } from '../lib/hooks'

type BtnProps = React.ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: 'primary' | 'tinted' | 'plain' | 'danger' | 'glass'
  size?: 'sm' | 'md' | 'lg'
  icon?: ReactNode
}

export function Button({ variant = 'tinted', size = 'md', icon, children, className = '', ...p }: BtnProps) {
  return (
    <motion.button whileTap={{ scale: 0.96 }} transition={{ type: 'spring', stiffness: 600, damping: 30 }} className={`btn ${variant} ${size} ${className}`} {...(p as object)}>
      {icon}
      {children}
    </motion.button>
  )
}

export function IconButton({ label, children, active, ...p }: React.ButtonHTMLAttributes<HTMLButtonElement> & { label: string; active?: boolean }) {
  return (
    <motion.button whileTap={{ scale: 0.9 }} className={`icon-btn ${active ? 'active' : ''}`} aria-label={label} title={label} {...(p as object)}>
      {children}
    </motion.button>
  )
}

export function Segmented<T extends string>({ value, onChange, options, size = 'md' }: { value: T; onChange: (v: T) => void; options: { value: T; label: ReactNode }[]; size?: 'sm' | 'md' }) {
  const id = useId()
  return (
    <div className={`segmented ${size}`} role="tablist">
      {options.map((o) => (
        <button key={o.value} role="tab" aria-selected={value === o.value} className={value === o.value ? 'on' : ''} onClick={() => onChange(o.value)}>
          {value === o.value && <motion.span layoutId={`seg-${id}`} className="seg-pill" transition={{ type: 'spring', stiffness: 520, damping: 38 }} />}
          <span className="seg-label">{o.label}</span>
        </button>
      ))}
    </div>
  )
}

export function Switch({ checked, onChange, disabled }: { checked: boolean; onChange: (v: boolean) => void; disabled?: boolean }) {
  return (
    <button role="switch" aria-checked={checked} disabled={disabled} className={`switch ${checked ? 'on' : ''}`} onClick={() => onChange(!checked)}>
      <motion.span className="knob" layout transition={{ type: 'spring', stiffness: 700, damping: 34 }} />
    </button>
  )
}

export function Slider({ value, min, max, step = 1, onChange, format }: { value: number; min: number; max: number; step?: number; onChange: (v: number) => void; format?: (v: number) => string }) {
  const pct = ((value - min) / (max - min)) * 100
  return (
    <div className="slider">
      <input type="range" min={min} max={max} step={step} value={value} onChange={(e) => onChange(Number(e.target.value))} style={{ ['--p' as string]: `${pct}%` }} />
      <span className="slider-val mono">{format ? format(value) : value}</span>
    </div>
  )
}

export function Row({ title, hint, children, stack }: { title: ReactNode; hint?: ReactNode; children?: ReactNode; stack?: boolean }) {
  return (
    <div className={`row ${stack ? 'stack' : ''}`}>
      <div className="row-text">
        <div className="row-title">{title}</div>
        {hint && <div className="row-hint">{hint}</div>}
      </div>
      {children && <div className="row-control">{children}</div>}
    </div>
  )
}

export function Group({ title, footer, children }: { title?: ReactNode; footer?: ReactNode; children: ReactNode }) {
  return (
    <section className="group">
      {title && <h3 className="group-title">{title}</h3>}
      <div className="group-body">{children}</div>
      {footer && <p className="group-footer">{footer}</p>}
    </section>
  )
}

export function TextField({ value, onChange, placeholder, mono, type = 'text', width, onBlur, invalid }: { value: string; onChange: (v: string) => void; placeholder?: string; mono?: boolean; type?: string; width?: number | string; onBlur?: () => void; invalid?: boolean }) {
  return <input className={`field ${mono ? 'mono' : ''} ${invalid ? 'invalid' : ''}`} style={{ width }} type={type} value={value} placeholder={placeholder} onChange={(e) => onChange(e.target.value)} onBlur={onBlur} spellCheck={false} autoCapitalize="off" autoComplete="off" />
}

export function Chip({ on, onClick, children, color }: { on?: boolean; onClick?: () => void; children: ReactNode; color?: string }) {
  return (
    <button className={`chip ${on ? 'on' : ''}`} onClick={onClick} style={color ? ({ ['--c' as string]: color } as React.CSSProperties) : undefined}>
      {color && <i className="chip-dot" />}
      {children}
    </button>
  )
}

export function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="kbd">{children}</kbd>
}

/** A number that glides to its new value. */
export function Num({ value, format }: { value: number; format: (v: number) => string }) {
  const v = useTween(value, 380)
  return <span className="num">{format(v)}</span>
}

export function Spark({ values, color, height = 28, width = 90, fill = true }: { values: number[]; color: string; height?: number; width?: number; fill?: boolean }) {
  const id = useId()
  if (values.length < 2) return <svg width={width} height={height} />
  const max = Math.max(...values, 1)
  const pts = values.map((v, i) => [(i / (values.length - 1)) * width, height - 2 - (v / max) * (height - 4)] as const)
  const d = pts.map(([x, y], i) => (i ? `L${x.toFixed(1)} ${y.toFixed(1)}` : `M${x.toFixed(1)} ${y.toFixed(1)}`)).join(' ')
  return (
    <svg width={width} height={height} viewBox={`0 0 ${width} ${height}`} className="spark">
      <defs>
        <linearGradient id={id} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0" stopColor={color} stopOpacity=".35" />
          <stop offset="1" stopColor={color} stopOpacity="0" />
        </linearGradient>
      </defs>
      {fill && <path d={`${d} L${width} ${height} L0 ${height} Z`} fill={`url(#${id})`} />}
      <path d={d} fill="none" stroke={color} strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  )
}

export function Ring({ value, size = 34, stroke = 4, color, children }: { value: number; size?: number; stroke?: number; color: string; children?: ReactNode }) {
  const r = (size - stroke) / 2
  const c = 2 * Math.PI * r
  const v = useTween(value, 600)
  return (
    <div className="ring" style={{ width: size, height: size }}>
      <svg width={size} height={size} style={{ transform: 'rotate(-90deg)' }}>
        <circle cx={size / 2} cy={size / 2} r={r} fill="none" stroke="var(--fill-2)" strokeWidth={stroke} />
        <circle cx={size / 2} cy={size / 2} r={r} fill="none" stroke={color} strokeWidth={stroke} strokeLinecap="round" strokeDasharray={`${c * Math.max(0, Math.min(1, v))} ${c}`} />
      </svg>
      <div className="ring-in">{children}</div>
    </div>
  )
}

export function Menu({ items, onClose, x, y }: { items: { label: string; onClick: () => void; danger?: boolean; icon?: ReactNode; hidden?: boolean }[]; onClose: () => void; x: number; y: number }) {
  const [vis] = useState(true)
  if (!vis) return null
  const left = Math.min(x, window.innerWidth - 220)
  const top = Math.min(y, window.innerHeight - items.length * 36 - 24)
  return (
    <>
      <div className="menu-scrim" onClick={onClose} onContextMenu={(e) => (e.preventDefault(), onClose())} />
      <motion.div className="menu" style={{ left, top }} initial={{ opacity: 0, scale: 0.94, y: -4 }} animate={{ opacity: 1, scale: 1, y: 0 }} transition={{ type: 'spring', stiffness: 700, damping: 38 }}>
        {items
          .filter((i) => !i.hidden)
          .map((i) => (
            <button
              key={i.label}
              className={i.danger ? 'danger' : ''}
              onClick={() => {
                i.onClick()
                onClose()
              }}
            >
              {i.icon}
              {i.label}
            </button>
          ))}
      </motion.div>
    </>
  )
}
