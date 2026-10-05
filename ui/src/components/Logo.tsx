import { motion } from 'motion/react'

/** The IDMX app icon. It is a self-contained dark tile, so it reads well on both light and dark pages. */
export default function Logo({ size = 28, draw = false }: { size?: number; draw?: boolean }) {
  return (
    <motion.img
      src={`${import.meta.env.BASE_URL}idmx-logo.png`}
      width={size}
      height={size}
      alt="IDMX"
      draggable={false}
      style={{ borderRadius: size * 0.23, display: 'block' }}
      {...(draw ? { initial: { scale: 0.6, opacity: 0 }, animate: { scale: 1, opacity: 1 }, transition: { type: 'spring', stiffness: 260, damping: 18 } } : {})}
    />
  )
}
