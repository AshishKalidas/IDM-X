importScripts('sniff.js')

const HOST = 'app.idmx.host'
const DEFAULTS = { server: 'http://127.0.0.1:7733', token: '', takeOver: true, minSize: 2 * 1024 * 1024, grabMagnets: true }

const cfg = async () => ({ ...DEFAULTS, ...(await chrome.storage.local.get(Object.keys(DEFAULTS))) })

/** Ask the native host where the running app is. No token copying: this is the whole pairing step. */
async function pair() {
  try {
    const r = await chrome.runtime.sendNativeMessage(HOST, { type: 'pair' })
    if (r?.ok) {
      await chrome.storage.local.set({ server: `http://127.0.0.1:${r.port}`, token: r.token })
      return true
    }
  } catch {
    /* host not registered yet, or the app isn't running */
  }
  return false
}

async function api(path, init = {}, retry = true) {
  const c = await cfg()
  let res
  try {
    res = await fetch(`${c.server}/api${path}`, {
      ...init,
      headers: { Authorization: `Bearer ${c.token}`, 'Content-Type': 'application/json', ...(init.headers || {}) },
    })
  } catch (e) {
    if (retry && (await pair())) return api(path, init, false)
    throw new Error('IDMX isn’t running')
  }
  if (res.status === 401 && retry && (await pair())) return api(path, init, false)
  if (!res.ok) throw new Error((await res.json().catch(() => ({}))).error || res.statusText)
  return res.json()
}

async function cookieHeader(url) {
  try {
    return (await chrome.cookies.getAll({ url })).map((c) => `${c.name}=${c.value}`).join('; ')
  } catch {
    return ''
  }
}

function notify(title, message) {
  chrome.notifications.create({ type: 'basic', iconUrl: 'icon.png', title, message })
}

/** Hand something to IDMX. `media` makes it resolve the page/stream with yt-dlp. */
async function send(url, { referer, media, quiet, mediaFormat } = {}) {
  try {
    const jobs = await api('/jobs', {
      method: 'POST',
      body: JSON.stringify({
        urls: [url],
        options: { cookies: (await cookieHeader(url)) || null, referer: referer || null, media: media || null, mediaFormat: mediaFormat || null },
      }),
    })
    if (!quiet) notify('Sent to IDMX', jobs[0]?.fileName || url)
    return true
  } catch (e) {
    notify('Couldn’t send to IDMX', String(e.message || e))
    return false
  }
}

// ---- heartbeat: lets the app show "connected" ----
async function hello() {
  try {
    await api('/extension/hello', { method: 'POST', body: JSON.stringify({ version: chrome.runtime.getManifest().version, browser: 'chrome' }) })
  } catch {
    /* app closed */
  }
}
chrome.alarms?.create('idmx-hello', { periodInMinutes: 0.5 })
chrome.alarms?.onAlarm.addListener((a) => a.name === 'idmx-hello' && hello())

// ---- media sniffing ----
async function tabItems(tabId) {
  const k = `tab${tabId}`
  return (await chrome.storage.session.get(k))[k] || []
}

chrome.webRequest.onResponseStarted.addListener(
  async (d) => {
    if (d.tabId < 0) return
    const h = (n) => d.responseHeaders?.find((x) => x.name.toLowerCase() === n)?.value || ''
    const hit = IDMX.classify(d.url, h('content-type'), Number(h('content-length')) || 0)
    if (!hit) return
    const items = IDMX.remember(await tabItems(d.tabId), { url: d.url, ...hit, size: Number(h('content-length')) || 0 })
    await chrome.storage.session.set({ [`tab${d.tabId}`]: items })
    chrome.action.setBadgeBackgroundColor({ tabId: d.tabId, color: '#0a84ff' })
    chrome.action.setBadgeText({ tabId: d.tabId, text: String(items.length) })
  },
  { urls: ['<all_urls>'], types: ['media', 'xmlhttprequest', 'other'] },
  ['responseHeaders'],
)

// A new page load clears what we saw on the old one.
chrome.webNavigation?.onCommitted.addListener((d) => {
  if (d.frameId === 0) {
    chrome.storage.session.remove(`tab${d.tabId}`)
    chrome.action.setBadgeText({ tabId: d.tabId, text: '' })
  }
})
chrome.tabs.onRemoved.addListener((id) => chrome.storage.session.remove(`tab${id}`))

// ---- messages from the popup and the page script ----
chrome.runtime.onMessage.addListener((m, sender, reply) => {
  ;(async () => {
    if (m.type === 'send') reply({ ok: await send(m.url, { referer: m.referer || sender.tab?.url, media: m.media }) })
    else if (m.type === 'items') reply({ items: await tabItems(m.tabId) })
    else if (m.type === 'status') {
      try {
        const s = await api('/state')
        reply({ ok: true, jobs: s.jobs?.length ?? 0 })
      } catch (e) {
        reply({ ok: false, error: String(e.message) })
      }
    } else if (m.type === 'pair') reply({ ok: await pair() })
  })()
  return true
})

// ---- menus, downloads, startup ----
chrome.runtime.onInstalled.addListener(async () => {
  chrome.contextMenus.create({ id: 'idmx-link', title: 'Download with IDMX', contexts: ['link', 'video', 'audio', 'image'] })
  chrome.contextMenus.create({ id: 'idmx-page', title: 'Download this page’s video with IDMX', contexts: ['page'] })
  if (await pair()) hello()
})
chrome.runtime.onStartup.addListener(async () => {
  await pair()
  hello()
})

chrome.contextMenus.onClicked.addListener((info, tab) => {
  if (info.menuItemId === 'idmx-page') send(tab.url, { media: true })
  else {
    const url = info.linkUrl || info.srcUrl
    if (url) send(url, { referer: tab?.url, media: IDMX.isVideoPage(url) || null })
  }
})

// Take over browser downloads (torrent files always; others above a size).
chrome.downloads.onCreated.addListener(async (item) => {
  const c = await cfg()
  const url = item.finalUrl || item.url
  if (!/^https?:/i.test(url) || item.byExtensionId) return
  const torrent = IDMX.isTorrentDownload(url, item.mime)
  if (!torrent && (!c.takeOver || (item.fileSize > 0 && item.fileSize < c.minSize))) return
  if (await send(url, { referer: item.referrer })) {
    chrome.downloads.cancel(item.id)
    chrome.downloads.erase({ id: item.id })
  }
})
