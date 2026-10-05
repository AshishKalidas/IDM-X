const $ = (id) => document.getElementById(id)
const ask = (m) => chrome.runtime.sendMessage(m)
const mb = (n) => (n ? `${(n / 1048576).toFixed(n > 1e8 ? 0 : 1)} MB` : '')

async function main() {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true })
  let st = await ask({ type: 'status' })
  if (!st.ok && (await ask({ type: 'pair' })).ok) st = await ask({ type: 'status' })
  $('dot').className = `dot${st.ok ? ' on' : ''}`
  if (!st.ok) {
    $('msg').textContent = 'IDMX isn’t running. Open the app and this will connect by itself.'
    return
  }
  const isVideoSite = /(youtube\.com|youtu\.be|vimeo\.com|twitch\.tv|dailymotion\.com|tiktok\.com|x\.com|twitter\.com|soundcloud\.com)/.test(tab?.url || '')
  const { items } = await ask({ type: 'items', tabId: tab.id })
  if (isVideoSite) {
    $('page').hidden = false
    for (const q of IDMX.QUALITIES) $('q').add(new Option(q.label, q.value))
    $('q').value = '720'
    $('pageBtn').onclick = async (e) => {
      e.target.textContent = (await ask({ type: 'send', url: tab.url, media: true, mediaFormat: $('q').value })).ok ? 'Sent ✓' : 'Failed'
    }
  }
  if (!items.length && !isVideoSite) $('msg').textContent = 'No downloadable media found on this page yet. Play the video and check again.'
  for (const it of items) {
    const row = document.createElement('div')
    row.className = 'row'
    const name = decodeURIComponent(new URL(it.url).pathname.split('/').pop() || it.url)
    row.innerHTML = '<span></span><small></small><button>Get</button>'
    row.children[0].textContent = name
    row.children[0].title = it.url
    row.children[1].textContent = `${it.label}${it.size ? ' · ' + mb(it.size) : ''}`
    row.children[2].onclick = async (e) => {
      e.target.textContent = (await ask({ type: 'send', url: it.url, referer: tab.url, media: it.kind === 'hls' || it.kind === 'dash' || null })).ok ? '✓' : '✗'
    }
    $('list').appendChild(row)
  }
}
main()
