// Runs on every page: captures magnet links and offers a "Download" chip over playing videos.
// Hovering the chip lists quality choices (Low 480p / Med 720p / High 1080p / Audio 216 or 320 kbps).
;(() => {
  const send = (msg) => chrome.runtime.sendMessage(msg).catch(() => {})
  const QUALITIES = IDMX.QUALITIES

  document.addEventListener(
    'click',
    (e) => {
      const a = e.target.closest?.('a[href^="magnet:"]')
      if (!a) return
      e.preventDefault()
      e.stopPropagation()
      send({ type: 'send', url: a.href })
    },
    true,
  )

  const box = document.createElement('div')
  box.setAttribute('style', 'all:initial;position:fixed;z-index:2147483647;display:none;font:600 12px -apple-system,system-ui,sans-serif')
  const chip = document.createElement('div')
  chip.textContent = '⬇ IDMX'
  chip.setAttribute('style', 'all:initial;display:inline-block;padding:6px 12px;border-radius:999px;background:#0a84ff;color:#fff;font:600 12px -apple-system,system-ui,sans-serif;cursor:pointer;box-shadow:0 4px 16px rgba(0,0,0,.4)')
  const menu = document.createElement('div')
  menu.setAttribute('style', 'all:initial;display:none;margin-top:6px;padding:4px;border-radius:12px;background:rgba(20,20,22,.96);box-shadow:0 8px 28px rgba(0,0,0,.5);min-width:170px')
  box.append(chip, menu)
  let current = null

  const directSrc = () => (current?.currentSrc && /^https?:/.test(current.currentSrc) ? current.currentSrc : null)
  const flash = (t) => {
    chip.textContent = t
    setTimeout(() => (chip.textContent = '⬇ IDMX'), 1500)
  }
  const grab = (mediaFormat) => {
    const src = directSrc()
    // Streaming sites use blob: URLs; the page address is what yt-dlp understands there.
    send({ type: 'send', url: src && !mediaFormat ? src : location.href, media: src && !mediaFormat ? null : true, mediaFormat })
    flash('✓ Sent')
  }
  for (const q of QUALITIES) {
    const row = document.createElement('div')
    row.textContent = q.label
    row.setAttribute('style', 'all:initial;display:block;padding:7px 12px;border-radius:8px;color:#fff;font:500 12.5px -apple-system,system-ui,sans-serif;cursor:pointer')
    row.onmouseenter = () => (row.style.background = '#0a84ff')
    row.onmouseleave = () => (row.style.background = 'transparent')
    row.onclick = (e) => (e.preventDefault(), e.stopPropagation(), grab(q.value))
    menu.appendChild(row)
  }
  chip.onclick = (e) => (e.preventDefault(), e.stopPropagation(), grab(null))
  box.onmouseenter = () => {
    clearTimeout(hideTimer)
    menu.style.display = 'block'
  }
  box.onmouseleave = () => {
    menu.style.display = 'none'
    hideTimer = setTimeout(() => (box.style.display = 'none'), 1200)
  }
  document.documentElement.appendChild(box)

  let hideTimer
  document.addEventListener(
    'mouseover',
    (e) => {
      const v = e.target instanceof HTMLVideoElement ? e.target : null
      if (!v) return
      const r = v.getBoundingClientRect()
      if (r.width < 240 || r.height < 135) return
      current = v
      box.style.left = `${Math.max(8, r.left + 10)}px`
      box.style.top = `${Math.max(8, r.top + 10)}px`
      box.style.display = 'block'
      clearTimeout(hideTimer)
      hideTimer = setTimeout(() => menu.style.display === 'none' && (box.style.display = 'none'), 3000)
    },
    true,
  )
})()
