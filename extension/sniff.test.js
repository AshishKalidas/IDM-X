const test = require('node:test')
const assert = require('node:assert')
const X = require('./sniff.js')

test('classifies streams and media', () => {
  assert.equal(X.classify('https://c.net/v/master.m3u8?x=1').kind, 'hls')
  assert.equal(X.classify('https://c.net/a.mpd').kind, 'dash')
  assert.equal(X.classify('https://c.net/a', 'application/vnd.apple.mpegurl').kind, 'hls')
  assert.equal(X.classify('https://c.net/a.mp4', '', 5_000_000).kind, 'video')
  assert.equal(X.classify('https://c.net/a.mp3').kind, 'audio')
})
test('ignores segments and tiny files', () => {
  assert.equal(X.classify('https://c.net/seg1.ts'), null)
  assert.equal(X.classify('https://c.net/chunk.m4s'), null)
  assert.equal(X.classify('https://c.net/ad.mp4', 'video/mp4', 50_000), null)
  assert.equal(X.classify('https://c.net/app.js', 'text/javascript'), null)
  assert.equal(X.classify('not a url'), null)
})
test('torrents, magnets, video pages', () => {
  assert.ok(X.isTorrentDownload('https://x.org/a.torrent?dl=1'))
  assert.ok(X.isTorrentDownload('https://x.org/dl', 'application/x-bittorrent'))
  assert.ok(!X.isTorrentDownload('https://x.org/a.zip'))
  assert.ok(X.isMagnet('magnet:?xt=urn:btih:abc'))
  assert.ok(!X.isMagnet('https://x.org'))
  assert.ok(X.isVideoPage('https://www.youtube.com/watch?v=1'))
  assert.ok(!X.isVideoPage('https://example.com'))
})
test('remember dedupes and caps', () => {
  let l = []
  for (let i = 0; i < 30; i++) l = X.remember(l, { url: `u${i % 25}` })
  assert.equal(l.length, 20)
  assert.equal(new Set(l.map((x) => x.url)).size, 20)
  assert.equal(X.remember([{ url: 'a' }, { url: 'b' }], { url: 'b' })[0].url, 'b')
})
