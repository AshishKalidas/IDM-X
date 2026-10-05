/* IDMX website behaviour. No dependencies. */
;(() => {
  const $ = (s, r = document) => r.querySelector(s)
  const $$ = (s, r = document) => [...r.querySelectorAll(s)]
  const root = document.documentElement
  const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches
  const css = (v) => getComputedStyle(root).getPropertyValue(v).trim()
  const DATA = JSON.parse($('#release-data').textContent)

  /* ───────── theme ───────── */
  const theme = () => root.dataset.theme
  $('#theme').addEventListener('click', () => {
    const next = theme() === 'dark' ? 'light' : 'dark'
    root.dataset.theme = next
    try { localStorage.setItem('idmx.site.theme', next) } catch {}
    swapShots()
  })

  /* ───────── screenshots follow the theme ───────── */
  function shotSrc(name) { return `assets/shots/${name}-${theme()}.png` }
  function swapShots() {
    $$('img[data-shot]').forEach((img) => {
      const src = shotSrc(img.dataset.shot)
      if (img.getAttribute('src') !== src) img.src = src
    })
  }
  swapShots()

  /* ───────── scroll progress, nav, reveal ───────── */
  const bar = $('#progress'), nav = $('#nav')
  let lastY = 0
  addEventListener('scroll', () => {
    const h = document.documentElement
    bar.style.transform = `scaleX(${h.scrollTop / Math.max(1, h.scrollHeight - h.clientHeight)})`
    const y = h.scrollTop
    nav.style.transform = y > 400 && y > lastY + 4 ? 'translateY(-90px)' : 'none'
    lastY = y
  }, { passive: true })

  const io = new IntersectionObserver((es) => es.forEach((e) => { if (e.isIntersecting) { e.target.classList.add('in'); io.unobserve(e.target) } }), { threshold: 0.12, rootMargin: '0px 0px -6% 0px' })
  $$('.reveal').forEach((el) => io.observe(el))

  const links = $$('.links a')
  $$('main section[id]').forEach((s) => new IntersectionObserver((es) => es.forEach((e) => e.isIntersecting && links.forEach((a) => a.classList.toggle('on', a.getAttribute('href') === '#' + s.id))), { rootMargin: '-45% 0px -50% 0px' }).observe(s))

  /* ───────── counters ───────── */
  const countIO = new IntersectionObserver((es) => es.forEach((e) => {
    if (!e.isIntersecting) return
    countIO.unobserve(e.target)
    const el = e.target
    if (el.dataset.text) { el.textContent = el.dataset.text; return }
    const to = +el.dataset.count, suf = el.dataset.suffix || ''
    const t0 = performance.now()
    const tick = (t) => {
      const p = Math.min(1, (t - t0) / 1100)
      el.textContent = Math.round(to * (1 - Math.pow(1 - p, 3))) + suf
      if (p < 1) requestAnimationFrame(tick)
    }
    reduced ? (el.textContent = to + suf) : requestAnimationFrame(tick)
  }), { threshold: 0.6 })
  $$('[data-count]').forEach((el) => countIO.observe(el))

  /* ───────── hero tilt + feature spotlight ───────── */
  const tilt = $('[data-tilt] .window')
  if (tilt && !reduced) {
    const host = tilt.parentElement
    host.addEventListener('pointermove', (e) => {
      const r = host.getBoundingClientRect()
      tilt.style.setProperty('--ry', `${((e.clientX - r.left) / r.width - 0.5) * 7}deg`)
      tilt.style.setProperty('--rx', `${6 - ((e.clientY - r.top) / r.height - 0.5) * 7}deg`)
    })
    host.addEventListener('pointerleave', () => { tilt.style.removeProperty('--ry'); tilt.style.removeProperty('--rx') })
  }
  $$('.feat').forEach((f) => f.addEventListener('pointermove', (e) => {
    const r = f.getBoundingClientRect()
    f.style.setProperty('--mx', `${e.clientX - r.left}px`)
    f.style.setProperty('--my', `${e.clientY - r.top}px`)
  }))

  /* ───────── canvas helper ───────── */
  function fit(c) {
    const dpr = Math.min(2, devicePixelRatio || 1)
    const r = c.getBoundingClientRect()
    c.width = Math.max(1, r.width * dpr); c.height = Math.max(1, r.height * dpr)
    const g = c.getContext('2d'); g.setTransform(dpr, 0, 0, dpr, 0, 0)
    return { g, w: r.width, h: r.height }
  }
  function loop(c, draw) {
    let vis = true, size = fit(c)
    new IntersectionObserver((e) => (vis = e[0].isIntersecting)).observe(c)
    new ResizeObserver(() => (size = fit(c))).observe(c)
    const frame = (t) => { if (vis) draw(size.g, size.w, size.h, t / 1000); requestAnimationFrame(frame) }
    requestAnimationFrame(frame)
  }
  const rgba = (hex, a) => { const n = parseInt(hex.replace('#', ''), 16); return `rgba(${n >> 16},${(n >> 8) & 255},${n & 255},${a})` }

  /* ───────── hero: networks converging ───────── */
  const hc = $('#hero-canvas')
  if (hc) {
    const nets = [['--net-wifi', 0.18], ['--net-ethernet', 0.34], ['--net-tether', 0.5], ['--net-cellular', 0.66]]
    const parts = Array.from({ length: 70 }, (_, i) => ({ n: i % 4, t: Math.random(), s: 0.12 + Math.random() * 0.16 }))
    loop(hc, (g, w, h, t) => {
      g.clearRect(0, 0, w, h)
      const hub = { x: w * 0.5, y: h * 0.34 }
      const dark = theme() === 'dark'
      nets.forEach(([v, yy], i) => {
        const col = css(v)
        const a = { x: w * (0.04 + i * 0.015), y: h * (0.06 + yy * 0.5) }
        const c1 = { x: w * 0.3, y: a.y }, c2 = { x: w * 0.38, y: hub.y }
        g.beginPath(); g.moveTo(a.x, a.y); g.bezierCurveTo(c1.x, c1.y, c2.x, c2.y, hub.x, hub.y)
        g.strokeStyle = rgba(col, dark ? 0.22 : 0.3); g.lineWidth = 1.5; g.stroke()
        g.beginPath(); g.arc(a.x, a.y, 5 + Math.sin(t * 2 + i) * 1.2, 0, 7); g.fillStyle = col; g.shadowColor = col; g.shadowBlur = dark ? 18 : 0; g.fill(); g.shadowBlur = 0
        // right-hand mirror
        const b = { x: w * (0.96 - i * 0.015), y: a.y }
        g.beginPath(); g.moveTo(hub.x, hub.y); g.bezierCurveTo(w - c2.x, c2.y, w - c1.x, c1.y, b.x, b.y)
        g.strokeStyle = rgba(col, dark ? 0.1 : 0.16); g.stroke()
      })
      // particles ride the curves into the hub
      parts.forEach((p) => {
        p.t += p.s * 0.012 * (reduced ? 0 : 1)
        if (p.t > 1) p.t = 0
        const [v, yy] = nets[p.n]
        const a = { x: w * (0.04 + p.n * 0.015), y: h * (0.06 + yy * 0.5) }
        const u = p.t, k = 1 - u
        const x = k * k * k * a.x + 3 * k * k * u * (w * 0.3) + 3 * k * u * u * (w * 0.38) + u * u * u * hub.x
        const y = k * k * k * a.y + 3 * k * k * u * a.y + 3 * k * u * u * hub.y + u * u * u * hub.y
        g.beginPath(); g.arc(x, y, 2.2, 0, 7); g.fillStyle = rgba(css(v), 0.4 + 0.6 * Math.sin(u * Math.PI)); g.fill()
      })
      const pulse = 1 + Math.sin(t * 2.4) * 0.12
      const grd = g.createRadialGradient(hub.x, hub.y, 0, hub.x, hub.y, 90 * pulse)
      grd.addColorStop(0, rgba(css('--accent'), dark ? 0.5 : 0.22)); grd.addColorStop(1, 'transparent')
      g.fillStyle = grd; g.fillRect(hub.x - 100, hub.y - 100, 200, 200)
    })
  }

  /* ───────── race demo ───────── */
  const NETS = [
    { id: 'wifi', name: 'Wi-Fi', mbps: 60, v: '--net-wifi', on: true },
    { id: 'eth', name: 'Ethernet', mbps: 90, v: '--net-ethernet', on: false },
    { id: 'tether', name: 'Phone hotspot', mbps: 35, v: '--net-tether', on: true },
    { id: 'cell', name: 'Cellular', mbps: 50, v: '--net-cellular', on: true },
  ]
  const chips = $('#race-chips'), size = $('#race-size'), sizeOut = $('#race-size-out')
  const fmtTime = (s) => (s >= 3600 ? `${Math.floor(s / 3600)}h ${Math.round((s % 3600) / 60)}m` : s >= 60 ? `${Math.floor(s / 60)}m ${String(Math.round(s % 60)).padStart(2, '0')}s` : `${Math.round(s)}s`)
  let raf = 0
  function runRace() {
    cancelAnimationFrame(raf)
    const gb = +size.value
    sizeOut.textContent = `${gb} GB`
    const on = NETS.filter((n) => n.on)
    const single = NETS[0].on ? NETS[0] : on[0]
    const sumM = on.reduce((a, n) => a + n.mbps, 0)
    const tA = single ? (gb * 8000) / single.mbps : Infinity
    const tB = sumM ? (gb * 8000) / sumM : Infinity
    $('#race-a-time').textContent = single ? fmtTime(tA) : '—'
    $('#race-b-time').textContent = sumM ? fmtTime(tB) : '—'
    $('#race-speed').textContent = sumM ? `${single ? single.mbps : 0} Mbit/s  →  ${sumM} Mbit/s combined${single && sumM > single.mbps ? `  ·  ${(sumM / single.mbps).toFixed(1)}× faster` : ''}` : 'Switch a connection on'
    const A = $('#race-a'), B = $('#race-b')
    B.innerHTML = ''
    const segs = on.map((n) => { const i = document.createElement('i'); i.style.setProperty('--c', css(n.v)); B.appendChild(i); return { i, share: n.mbps / sumM } })
    const total = 6000 // slow lane takes six real seconds on screen
    const t0 = performance.now()
    const step = (now) => {
      const el = (now - t0) / total
      const pa = single ? Math.min(1, el) : 0
      const pb = sumM ? Math.min(1, el * (tA / tB)) : 0
      A.style.width = pa * 100 + '%'
      segs.forEach((s) => (s.i.style.width = s.share * pb * 100 + '%'))
      if (pa < 1 || pb < 1) raf = requestAnimationFrame(step)
    }
    reduced ? step(t0 + total * 3) : (raf = requestAnimationFrame(step))
  }
  NETS.forEach((n) => {
    const b = document.createElement('button')
    b.className = 'rchip' + (n.on ? ' on' : '')
    b.style.setProperty('--c', css(n.v))
    b.innerHTML = `<i></i>${n.name} <small>${n.mbps} Mbit/s</small>`
    b.addEventListener('click', () => { n.on = !n.on; b.classList.toggle('on', n.on); runRace() })
    chips.appendChild(b)
  })
  size.addEventListener('input', runRace)
  $('#race-go').addEventListener('click', runRace)
  new IntersectionObserver((e, o) => { if (e[0].isIntersecting) { runRace(); o.disconnect() } }, { threshold: 0.4 }).observe($('#race'))
  new MutationObserver(() => NETS.forEach((n, i) => chips.children[i].style.setProperty('--c', css(n.v)))).observe(root, { attributes: true, attributeFilter: ['data-theme'] })

  /* ───────── block map animation ───────── */
  const bc = $('#blocks')
  if (bc) {
    const COLS = 26, ROWS = 18
    let owner = new Int8Array(COLS * ROWS).fill(-1), done = 0, hold = 0
    const order = [...Array(COLS * ROWS).keys()]
    const cols = ['--net-wifi', '--net-tether', '--net-cellular']
    const speeds = [7, 4, 5.5]
    let carry = [0, 0, 0], last = 0, cursor = [0, 0, 0]
    // each network claims a random contiguous run of blocks, like real work-stealing
    const reset = () => { owner.fill(-1); done = 0; hold = 0; carry = [0, 0, 0]; order.sort(() => Math.random() - 0.5); cursor = [0, 0, 0] }
    reset()
    loop(bc, (g, w, h, t) => {
      const dt = Math.min(0.05, t - last); last = t
      if (done < owner.length) {
        for (let n = 0; n < 3; n++) {
          carry[n] += speeds[n] * dt * 14
          while (carry[n] >= 1 && done < owner.length) {
            carry[n]--
            let k = order.findIndex((idx) => owner[idx] === -1)
            if (k < 0) break
            const idx = order[k]; owner[idx] = n; order.splice(k, 1); done++
          }
        }
      } else if ((hold += dt) > 2.2) reset()
      g.clearRect(0, 0, w, h)
      const cw = w / COLS, ch = h / ROWS, gap = 2.5
      for (let i = 0; i < owner.length; i++) {
        const x = (i % COLS) * cw, y = Math.floor(i / COLS) * ch
        g.fillStyle = owner[i] < 0 ? rgba(theme() === 'dark' ? '#ffffff' : '#000000', 0.07) : css(cols[owner[i]])
        g.beginPath(); g.roundRect(x + gap / 2, y + gap / 2, cw - gap, ch - gap, 3); g.fill()
      }
      g.fillStyle = css('--fg-2'); g.font = '600 12px ui-monospace, monospace'
      g.fillText(`${Math.round((done / owner.length) * 100)}%`, 8, h - 6)
    })
  }

  /* ───────── feature showcase ───────── */
  const TABS = {
    downloads: ['Downloads', '<b>One combined speed.</b> Every active download, with a live view of how much each network is contributing and which block each connection is on.'],
    networks: ['Networks', '<b>Every road, rated.</b> Live health, latency and throughput per network — switch any of them on or off, set caps and limits, and get a warning when two networks share one line.'],
    torrents: ['Torrents', '<b>Magnet links and .torrent files.</b> Pick the files you want, watch peers and ratio, seed to a limit — and multi-file torrents are split across your networks.'],
    speedtest: ['Speed test', '<b>Find out before you download.</b> Measures each connection, then all of them together, and tells you honestly how much combining will help.'],
    automation: ['Automation', '<b>Set it and forget it.</b> Schedules, bandwidth limits, rules that move or extract files when a download finishes, and sleep when the queue is empty.'],
    add: ['New download', '<b>Paste anything.</b> It checks the link, shows what it is, expands patterns like [01-20], and tells you whether it can use every network.'],
    settings: ['Settings', '<b>Yours to tune.</b> Video quality, torrent behaviour, the browser extension, the command line and storage — all in one place.'],
  }
  const img = $('#showcase-img')
  function setTab(k) {
    $$('#tabs button').forEach((b) => b.classList.toggle('on', b.dataset.tab === k))
    $('#tab-title').textContent = TABS[k][0]
    $('#tab-copy').innerHTML = TABS[k][1]
    img.classList.add('swap')
    setTimeout(() => { img.dataset.shot = k; img.alt = `IDMX ${TABS[k][0]} screen`; img.src = shotSrc(k); img.classList.remove('swap') }, 180)
  }
  $('#tabs').addEventListener('click', (e) => { const b = e.target.closest('button'); if (b) setTab(b.dataset.tab) })
  $('#tab-copy').innerHTML = TABS.downloads[1]
  const order = Object.keys(TABS)
  let auto = setInterval(() => { const cur = $('#tabs .on').dataset.tab; setTab(order[(order.indexOf(cur) + 1) % order.length]) }, 6500)
  $('.showcase').addEventListener('pointerdown', () => clearInterval(auto))
  new IntersectionObserver((e) => { if (!e[0].isIntersecting) clearInterval(auto) }, { threshold: 0 }).observe($('.showcase'))

  /* ───────── video chip demo ───────── */
  const mv = $('.mock-video')
  if (mv) new IntersectionObserver((e) => { mv.classList.toggle('demo', e[0].isIntersecting) }, { threshold: 0.7 }).observe(mv)

  /* ───────── CLI ───────── */
  const CMDS = {
    install: './scripts/install-cli.sh',
    get: 'idmx get https://example.com/big.iso',
    video: 'idmx get --quality 720 "https://www.youtube.com/watch?v=…"',
    torrent: 'idmx get "magnet:?xt=urn:btih:…"',
    daemon: 'idmx daemon --lan   # web dashboard for your whole network',
  }
  const cmd = $('#cmd')
  const setCmd = (k) => { cmd.textContent = CMDS[k]; $$('#cmdtabs button').forEach((b) => b.classList.toggle('on', b.dataset.c === k)) }
  setCmd('install')
  $('#cmdtabs').addEventListener('click', (e) => { const b = e.target.closest('button'); if (b) setCmd(b.dataset.c) })
  $('#copy').addEventListener('click', async (e) => {
    try { await navigator.clipboard.writeText(cmd.textContent.replace(/\s+#.*$/, '')); e.target.textContent = 'Copied ✓'; setTimeout(() => (e.target.textContent = 'Copy'), 1400) } catch { e.target.textContent = 'Select & copy' }
  })

  const SCRIPT = [
    ['<span class="p">$</span> <span class="c">idmx nets</span>', 0],
    ['<span class="d">ID       NAME                    KIND      PING</span>', 120],
    ['<span class="b">en0</span>      Wi-Fi                   wifi      18 ms', 120],
    ['<span class="o">en5</span>      iPhone USB              tether    41 ms', 120],
    ['<span class="v">pdp_ip0</span>  Cellular                cellular  63 ms', 380],
    ['', 0],
    ['<span class="p">$</span> <span class="c">idmx get https://example.com/big.iso</span>', 0],
    ['<span class="d">big.iso   6.2 GB   3 networks · 24 connections</span>', 500],
    ['<span class="b">Wi-Fi</span>   ████████████░░░░░░  38%   <span class="c">9.4 MB/s</span>', 300],
    ['<span class="o">iPhone</span>  ████████░░░░░░░░░░  27%   <span class="c">6.1 MB/s</span>', 100],
    ['<span class="v">Cell</span>    ███████░░░░░░░░░░░  23%   <span class="c">5.2 MB/s</span>', 100],
    ['<span class="d">─────────────────────────────────────────</span>', 100],
    ['combined  <span class="c">20.7 MB/s</span>   ETA 4m 55s', 500],
    ['<span class="p">✔</span> big.iso  6.2 GB in 5:02  <span class="d">(2.2× faster than Wi-Fi alone)</span>', 900],
    ['', 0],
    ['<span class="p">$</span> <span class="c">idmx daemon</span> <span class="cur"></span>', 0],
  ]
  const term = $('#term')
  let ran = false
  const playTerm = async () => {
    if (ran) return; ran = true
    if (reduced) { term.innerHTML = SCRIPT.map((l) => l[0]).join('\n'); return }
    let html = ''
    for (const [line, wait] of SCRIPT) {
      await new Promise((r) => setTimeout(r, wait + 60))
      html += line + '\n'
      term.innerHTML = html
    }
  }
  new IntersectionObserver((e, o) => { if (e[0].isIntersecting) { playTerm(); o.disconnect() } }, { threshold: 0.4 }).observe($('.term'))
  term.textContent = ''

  /* ───────── downloads ───────── */
  const ua = navigator.userAgent
  const isMac = /Mac/i.test(navigator.platform || ua)
  const isWin = /Win/i.test(navigator.platform || ua)
  const dls = $('#dls')
  const card = (k, o, rec) => `<article class="dl reveal${rec ? ' rec' : ''}" data-k="${k}">${rec ? '<span class="tag">Recommended</span>' : ''}<h3>${o.label}</h3><p class="s">${o.sub}</p><a class="btn ${rec ? 'primary' : 'glass'}" data-file="${o.file}" href="${o.file}" download>Download</a><p class="st" aria-live="polite">Checking…</p></article>`
  const D = DATA.downloads
  dls.innerHTML = [
    card('macos-arm64', D['macos-arm64'], isMac),
    card('macos-x64', D['macos-x64'], false),
    card('windows', D.windows, isWin),
    card('linux', D.linux, /Linux/.test(navigator.userAgent) && !/Android/.test(navigator.userAgent)),
    `<h4>Command line</h4>`,
    card('cli-mac', D['cli-mac']), card('cli-mac-x64', D['cli-mac-x64']), card('cli-win', D['cli-win']), card('cli-linux', D['cli-linux']),
  ].join('')
  $$('.dl').forEach((el) => io.observe(el))
  $$('.dl').forEach(async (c) => {
    const a = $('a', c), st = $('.st', c)
    const off = () => {
      a.removeAttribute('href'); a.removeAttribute('download'); a.setAttribute('aria-disabled', 'true'); a.style.opacity = '.5'; a.style.pointerEvents = 'none'; a.textContent = 'Not published yet'
      st.textContent = c.dataset.k.startsWith('cli') ? 'Build it yourself: ./scripts/install-cli.sh' : 'This build hasn’t been uploaded to this site yet.'
    }
    try {
      // A link to another website can't be probed from here (browsers block it); trust it.
      if (/^https?:\/\//i.test(a.dataset.file) && new URL(a.dataset.file).origin !== location.origin) { st.textContent = `v${DATA.latest}`; st.classList.add('ok'); return }
      const r = await fetch(a.dataset.file, { method: 'HEAD' })
      if (!r.ok) throw 0
      const len = +r.headers.get('content-length')
      st.textContent = len ? `${(len / 1048576).toFixed(0)} MB · v${DATA.latest}` : `v${DATA.latest}`
      st.classList.add('ok')
    } catch { off() }
  })
  $$('[data-latest]').forEach((el) => (el.textContent = DATA.latest))
  const label = $('[data-dl-label]')
  if (label) label.textContent = isMac ? 'Download for macOS' : isWin ? 'Download for Windows' : 'Download IDMX'
  const fine = $('#hero-fine')
  if (fine && isMac) fine.textContent = 'Free · Apple silicon & Intel · No account · Nothing leaves your computer'
  else if (fine && isWin) fine.textContent = 'Free · Windows 10 and 11 · No account · Nothing leaves your computer'

  /* ───────── release notes ───────── */
  $('#timeline').innerHTML = DATA.releases.map((r, i) => `<article class="rel reveal"><div class="rel-h"><b>v${r.version}</b>${i === 0 ? '<span class="tagl">Latest</span>' : ''}<span>${new Date(r.date + 'T12:00:00').toLocaleDateString(undefined, { year: 'numeric', month: 'long', day: 'numeric' })}</span></div><h3>${r.title}</h3><p class="blurb">${r.blurb}</p>${r.groups.map((g) => `<h5>${g.name}</h5><ul>${g.items.map((x) => `<li>${x}</li>`).join('')}</ul>`).join('')}</article>`).join('')
  $$('.rel').forEach((el) => io.observe(el))
})()
