import { Disc3, File, FileArchive, FileAudio, FileCode, FileText, FileVideo, Image, Package, type LucideIcon } from 'lucide-react'
import { ext } from '../lib/format'

const KINDS: [string[], LucideIcon, string][] = [
  [['zip', 'gz', 'tgz', 'tar', '7z', 'rar', 'xz', 'bz2', 'zst'], FileArchive, '#ff9f0a'],
  [['mp4', 'mkv', 'mov', 'avi', 'webm', 'm4v'], FileVideo, '#ff375f'],
  [['mp3', 'flac', 'wav', 'm4a', 'aac', 'ogg'], FileAudio, '#bf5af2'],
  [['png', 'jpg', 'jpeg', 'gif', 'webp', 'heic', 'svg', 'tiff'], Image, '#30d158'],
  [['pdf', 'doc', 'docx', 'txt', 'epub', 'md', 'rtf'], FileText, '#64d2ff'],
  [['iso', 'dmg', 'img'], Disc3, '#0a84ff'],
  [['pkg', 'deb', 'rpm', 'exe', 'msi', 'appimage', 'apk'], Package, '#5e5ce6'],
  [['js', 'ts', 'py', 'rs', 'go', 'json', 'html', 'css', 'sh', 'c', 'cpp'], FileCode, '#ffd60a'],
]

export function fileKind(name: string): { Icon: LucideIcon; color: string; ext: string } {
  const e = ext(name)
  const hit = KINDS.find(([list]) => list.includes(e))
  return hit ? { Icon: hit[1], color: hit[2], ext: e } : { Icon: File, color: '#8e8e93', ext: e }
}

export default function FileGlyph({ name, size = 42 }: { name: string; size?: number }) {
  const { Icon, color, ext: e } = fileKind(name)
  return (
    <div className="glyph" style={{ width: size, height: size, ['--c' as string]: color }}>
      <Icon size={size * 0.46} strokeWidth={1.7} />
      {e && <span className="glyph-ext">{e.slice(0, 4)}</span>}
    </div>
  )
}
