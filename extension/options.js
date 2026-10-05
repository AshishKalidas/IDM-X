const $ = (id) => document.getElementById(id)
const DEFAULTS = { server: 'http://127.0.0.1:7733', token: '', takeOver: true, minSize: 2 * 1024 * 1024 }

chrome.storage.local.get(Object.keys(DEFAULTS)).then((s) => {
  const c = { ...DEFAULTS, ...s }
  $('server').value = c.server
  $('token').value = c.token
  $('takeOver').checked = c.takeOver
  $('minSize').value = Math.round(c.minSize / 1048576)
})

$('save').onclick = async () => {
  const server = $('server').value.trim().replace(/\/$/, '')
  const token = $('token').value.trim()
  await chrome.storage.local.set({ server, token, takeOver: $('takeOver').checked, minSize: Number($('minSize').value || 0) * 1048576 })
  const st = $('status')
  try {
    const r = await fetch(`${server}/api/state`, { headers: { Authorization: `Bearer ${token}` } })
    st.textContent = r.ok ? '✓ Connected' : r.status === 401 ? '✗ That token was not accepted' : `✗ HTTP ${r.status}`
  } catch {
    st.textContent = '✗ Can’t reach IDMX — is it running?'
  }
}
