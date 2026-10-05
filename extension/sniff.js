// Pure helpers (no chrome.* calls) so they can be tested under Node.
const IDMX = {
  /** What kind of thing is this response? Returns null for anything not worth offering. */
  classify(url, contentType = '', length = 0) {
    let path = ''
    try {
      path = new URL(url).pathname.toLowerCase()
    } catch {
      return null
    }
    const ct = contentType.toLowerCase()
    if (/\.(ts|m4s|aac|vtt|key)$/.test(path)) return null // pieces of a stream, not the stream
    if (/\.m3u8$/.test(path) || ct.includes('mpegurl')) return { kind: 'hls', label: 'HLS stream' }
    if (/\.mpd$/.test(path) || ct.includes('dash+xml')) return { kind: 'dash', label: 'DASH stream' }
    const media = /\.(mp4|webm|mkv|mov|m4v|mp3|m4a|flac|ogg|opus|wav)$/.test(path) || /^(video|audio)\//.test(ct)
    if (media) {
      if (length && length < 200 * 1024) return null // ads, previews, sound effects
      return { kind: ct.startsWith('audio') || /\.(mp3|m4a|flac|ogg|opus|wav)$/.test(path) ? 'audio' : 'video', label: 'Media file' }
    }
    return null
  },

  isTorrentDownload(url, mime = '') {
    return /\.torrent(\?|#|$)/i.test(url) || mime === 'application/x-bittorrent'
  },

  isMagnet(href) {
    return /^magnet:\?/i.test(href || '')
  },

  /** Sites where the page URL itself (not a sniffed stream) is the right thing to hand to yt-dlp. */
  isVideoPage(url) {
    try {
      const h = new URL(url).hostname.replace(/^www\./, '')
      return /(^|\.)(youtube\.com|youtu\.be|vimeo\.com|twitch\.tv|dailymotion\.com|twitter\.com|x\.com|reddit\.com|tiktok\.com|instagram\.com|facebook\.com|soundcloud\.com|bilibili\.com)$/.test(h)
    } catch {
      return false
    }
  },

  /** The choices offered on hover and in the popup. `value` is what IDMX understands. */
  QUALITIES: [
    { label: 'Low · 480p', value: '480' },
    { label: 'Med · 720p', value: '720' },
    { label: 'High · 1080p', value: '1080' },
    { label: 'Audio · 216 kbps', value: 'audio216' },
    { label: 'Audio · 320 kbps', value: 'audio320' },
  ],

  /** Add a sniffed item to a per-tab list: de-duplicated, newest first, capped. */
  remember(list, item, cap = 20) {
    const without = list.filter((x) => x.url !== item.url)
    return [item, ...without].slice(0, cap)
  },
}
if (typeof module !== 'undefined') module.exports = IDMX
