//! The block table: the single source of truth for what is downloaded, who is fetching what,
//! and what a free stream should do next. Pure data — no I/O, no clock of its own — so every
//! scheduling rule can be tested against exact situations.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

pub const MIN_BLOCK: u64 = 1 << 20;
pub const MAX_BLOCK: u64 = 8 << 20;
/// How many bytes of a half-finished block are re-fetched on resume, in case the OS never
/// flushed the tail of what we thought we had written.
const RESUME_REWIND: u64 = 256 << 10;
const MAX_HEDGES_PER_BLOCK: u8 = 3;
pub const NO_OWNER: u8 = 255;

/// Aim for at least two blocks per stream so the last blocks are small enough to balance.
pub fn plan_block_size(size: u64, total_streams: u32) -> u64 {
    let streams = total_streams.max(1) as u64;
    let ideal = size / (streams * 2).max(1);
    let clamped = ideal.clamp(MIN_BLOCK, MAX_BLOCK);
    // 64 KiB aligned, which keeps writes disk-friendly.
    (clamped + 0xFFFF) & !0xFFFF
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum St {
    Pending,
    Active,
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Primary,
    Hedge,
}

pub struct Attempt {
    pub id: u64,
    pub net: usize,
    pub kind: Kind,
    pub started: Instant,
    /// Where in the block this request began.
    pub from: u64,
    /// Bytes this request has put on disk.
    pub bytes: u64,
    pub token: CancellationToken,
}

pub struct Block {
    pub start: u64,
    pub len: u64,
    /// Contiguous prefix on disk.
    pub done: u64,
    pub st: St,
    pub owner: u8,
    pub hedges: u8,
    /// A network whose last attempt here delivered nothing: let someone else try first.
    pub avoid: Option<usize>,
    pub attempts: Vec<Attempt>,
    /// Bytes of this block each network delivered: `(network index, bytes)`. A block started on
    /// one network and finished on another (after a retry, a pause, a race) credits both.
    pub contrib: Vec<(u8, u64)>,
}

pub struct Claim {
    pub idx: usize,
    pub kind: Kind,
    pub attempt: u64,
    pub start: u64,
    pub end: u64,
    pub from: u64,
    pub token: CancellationToken,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SavedBlocks {
    pub block_size: u64,
    pub done: Vec<u64>,
    pub owner: Vec<u8>,
    pub nets: Vec<String>,
    #[serde(default)]
    pub contrib: Vec<Vec<(u8, u64)>>,
}

pub struct BlockTable {
    pub block_size: u64,
    pub blocks: Vec<Block>,
    queue: VecDeque<usize>,
    done_count: usize,
    next_attempt: u64,
}

impl BlockTable {
    pub fn new(size: u64, block_size: u64) -> Self {
        let block_size = block_size.max(1);
        let n = size.div_ceil(block_size).max(if size == 0 { 0 } else { 1 }) as usize;
        let blocks = (0..n)
            .map(|i| {
                let start = i as u64 * block_size;
                Block {
                    start,
                    len: block_size.min(size - start),
                    done: 0,
                    st: St::Pending,
                    owner: NO_OWNER,
                    hedges: 0,
                    avoid: None,
                    attempts: Vec::new(),
                    contrib: Vec::new(),
                }
            })
            .collect();
        Self { block_size, blocks, queue: (0..n).collect(), done_count: 0, next_attempt: 1 }
    }

    /// Rebuild from a saved table. Returns the bytes already on disk.
    pub fn restore(size: u64, saved: &SavedBlocks) -> Option<(Self, u64)> {
        let mut t = Self::new(size, saved.block_size);
        if saved.done.len() != t.blocks.len() {
            return None;
        }
        t.queue.clear();
        let mut total = 0;
        for (i, b) in t.blocks.iter_mut().enumerate() {
            let mut d = saved.done[i].min(b.len);
            if d < b.len && d > 0 {
                d = d.saturating_sub(RESUME_REWIND);
            }
            b.done = d;
            b.owner = saved.owner.get(i).copied().unwrap_or(NO_OWNER);
            b.contrib = saved.contrib.get(i).cloned().unwrap_or_default();
            // Credit can't exceed what is still trusted on disk after the rewind.
            let mut over = b.contrib.iter().map(|c| c.1).sum::<u64>().saturating_sub(d);
            for c in b.contrib.iter_mut().rev() {
                let cut = over.min(c.1);
                c.1 -= cut;
                over -= cut;
            }
            b.contrib.retain(|c| c.1 > 0);
            if d == b.len {
                b.st = St::Done;
                t.done_count += 1;
            } else {
                t.queue.push_back(i);
            }
            total += d;
        }
        Some((t, total))
    }

    pub fn export(&self, nets: &[String]) -> SavedBlocks {
        SavedBlocks {
            block_size: self.block_size,
            done: self.blocks.iter().map(|b| b.done).collect(),
            owner: self.blocks.iter().map(|b| b.owner).collect(),
            nets: nets.to_vec(),
            contrib: self.blocks.iter().map(|b| b.contrib.clone()).collect(),
        }
    }

    /// Network indices in a saved table refer to the networks of *that* run. Translate them to
    /// this run's, dropping credit for a network that is no longer here.
    pub fn remap_networks(&mut self, saved: &[String], now: &[String]) {
        let map: Vec<Option<u8>> = saved.iter().map(|id| now.iter().position(|n| n == id).map(|i| i.min(254) as u8)).collect();
        let tr = |i: u8| map.get(i as usize).copied().flatten();
        for b in &mut self.blocks {
            b.owner = tr(b.owner).unwrap_or(NO_OWNER);
            b.contrib = b.contrib.iter().filter_map(|(n, v)| tr(*n).map(|m| (m, *v))).collect();
            if b.owner == NO_OWNER {
                b.owner = dominant(&b.contrib).unwrap_or(NO_OWNER);
            }
        }
    }

    pub fn all_done(&self) -> bool {
        self.done_count == self.blocks.len()
    }
    pub fn done_blocks(&self) -> usize {
        self.done_count
    }
    pub fn pending_blocks(&self) -> usize {
        self.queue.len()
    }
    pub fn remaining_blocks(&self) -> usize {
        self.blocks.len() - self.done_count
    }
    pub fn bytes_done(&self) -> u64 {
        self.blocks.iter().map(|b| b.done).sum()
    }

    /// Give a free stream on `net` something to do. `my_speed` is what a stream on that network
    /// typically manages (bytes/s); it decides whether racing a slow block is worth it.
    pub fn claim(&mut self, net: usize, other_net_free: bool, hedge: Option<HedgePolicy>, my_speed: f64, now: Instant) -> Option<Claim> {
        // 1) A waiting block — skipping one that this network just failed to deliver, if
        //    someone else is free to take it (otherwise a dead network would be handed the
        //    same block again and again while healthy ones sat idle beside it).
        let pos = self
            .queue
            .iter()
            .position(|&i| !(other_net_free && self.blocks[i].avoid == Some(net)))
            .or_else(|| if self.queue.is_empty() { None } else { Some(0) });
        if let Some(p) = pos {
            let idx = self.queue.remove(p).unwrap();
            return Some(self.begin(idx, net, Kind::Primary, now));
        }
        // 2) Nothing waiting: race the slowest block, if it clearly pays.
        let policy = hedge?;
        let idx = self.pick_hedge(net, my_speed, now, policy)?;
        self.blocks[idx].hedges += 1;
        Some(self.begin(idx, net, Kind::Hedge, now))
    }

    fn begin(&mut self, idx: usize, net: usize, kind: Kind, now: Instant) -> Claim {
        let id = self.next_attempt;
        self.next_attempt += 1;
        let b = &mut self.blocks[idx];
        b.st = St::Active;
        let token = CancellationToken::new();
        b.attempts.push(Attempt { id, net, kind, started: now, from: b.done, bytes: 0, token: token.clone() });
        Claim { idx, kind, attempt: id, start: b.start + b.done, end: b.start + b.len - 1, from: b.done, token }
    }

    fn pick_hedge(&self, net: usize, my_speed: f64, now: Instant, p: HedgePolicy) -> Option<usize> {
        let mut best: Option<(usize, f64)> = None;
        for (i, b) in self.blocks.iter().enumerate() {
            if b.st != St::Active || b.attempts.is_empty() || b.hedges >= MAX_HEDGES_PER_BLOCK || b.attempts.len() >= 3 {
                continue;
            }
            // This network already came back empty-handed on this block; leave the retry to another one.
            if b.avoid == Some(net) {
                continue;
            }
            // Measured speeds are noise until each attempt has been running for a little while.
            if b.attempts.iter().any(|a| now.duration_since(a.started) < p.after) {
                continue;
            }
            let remaining = (b.len - b.done) as f64;
            if remaining <= 0.0 {
                continue;
            }
            // A block is as slow as its fastest attempt. A holder that has gone quiet has no ETA.
            let eta = b
                .attempts
                .iter()
                .map(|a| {
                    let secs = now.duration_since(a.started).as_secs_f64().max(0.001);
                    let speed = a.bytes as f64 / secs;
                    if speed > 1.0 { remaining / speed } else { f64::INFINITY }
                })
                .fold(f64::INFINITY, f64::min);
            let mine = p.startup.as_secs_f64() + if my_speed > 1.0 { remaining / my_speed } else { f64::INFINITY };
            let worth = if my_speed > 1.0 { eta >= 2.0 * mine } else { eta >= p.after.as_secs_f64() * 2.0 };
            if worth && best.is_none_or(|(_, e)| eta > e) {
                best = Some((i, eta));
            }
        }
        best.map(|(i, _)| i)
    }

    /// `wrote` more bytes of this attempt's request are now on disk. Returns how many were new
    /// to the block (a hedge writing bytes another attempt already placed adds nothing).
    pub fn progress(&mut self, idx: usize, attempt: u64, wrote: u64) -> u64 {
        let b = &mut self.blocks[idx];
        let Some(a) = b.attempts.iter_mut().find(|a| a.id == attempt) else { return 0 };
        a.bytes += wrote;
        let net = a.net.min(254) as u8;
        let prefix = (a.from + a.bytes).min(b.len);
        if prefix > b.done {
            let d = prefix - b.done;
            b.done = prefix;
            // Only bytes that were new to the block count: a racing attempt rewriting what
            // another already delivered earns no credit.
            match b.contrib.iter_mut().find(|c| c.0 == net) {
                Some(c) => c.1 += d,
                None => b.contrib.push((net, d)),
            }
            d
        } else {
            0
        }
    }

    /// An attempt got to the end of its range. Returns the attempts that should now stand down.
    pub fn finish(&mut self, idx: usize, attempt: u64, net: usize) -> bool {
        let b = &mut self.blocks[idx];
        b.attempts.retain(|a| a.id != attempt);
        if b.st == St::Done {
            return false;
        }
        b.done = b.len;
        b.st = St::Done;
        // Whoever delivered most of it owns it; the one that happened to finish may have
        // delivered only the last few bytes.
        b.owner = dominant(&b.contrib).unwrap_or(net.min(254) as u8);
        for a in b.attempts.drain(..) {
            a.token.cancel();
        }
        self.done_count += 1;
        true
    }

    /// An attempt ended without finishing. `delivered` is whether it got any bytes at all.
    pub fn release(&mut self, idx: usize, attempt: u64, delivered: bool) {
        let b = &mut self.blocks[idx];
        let net = b.attempts.iter().find(|a| a.id == attempt).map(|a| a.net);
        b.attempts.retain(|a| a.id != attempt);
        if b.st == St::Done {
            return;
        }
        if !delivered {
            b.avoid = net;
        } else {
            b.avoid = None;
        }
        if b.attempts.is_empty() {
            b.st = St::Pending;
            self.queue.push_front(idx);
        }
    }

    /// Abort every in-flight attempt on a network (it changed or went away).
    pub fn cancel_net(&self, net: usize) {
        for b in &self.blocks {
            for a in b.attempts.iter().filter(|a| a.net == net) {
                a.token.cancel();
            }
        }
    }

    /// Blocks that more than one network contributed to, as percentages of what was delivered.
    pub fn split(&self) -> Vec<(u32, Vec<(u8, u8)>)> {
        self.blocks
            .iter()
            .enumerate()
            .filter(|(_, b)| b.contrib.len() > 1)
            .map(|(i, b)| {
                let total: u64 = b.contrib.iter().map(|c| c.1).sum::<u64>().max(1);
                let parts: Vec<(u8, u8)> = b.contrib.iter().map(|(n, v)| (*n, ((*v as f64 / total as f64) * 100.0).round() as u8)).collect();
                (i as u32, parts)
            })
            .collect()
    }

    /// For the UI: state, owner and fill per block.
    pub fn map(&self) -> (Vec<u8>, Vec<u8>, Vec<f32>) {
        let mut st = Vec::with_capacity(self.blocks.len());
        let mut ow = Vec::with_capacity(self.blocks.len());
        let mut fill = Vec::with_capacity(self.blocks.len());
        for b in &self.blocks {
            st.push(match b.st {
                St::Pending if b.done == 0 => 0,
                St::Pending | St::Active => 1,
                St::Done => 2,
            });
            ow.push(match b.st {
                St::Active => b.attempts.first().map(|a| a.net.min(254) as u8).unwrap_or(b.owner),
                _ => b.owner,
            });
            fill.push(if b.len == 0 { 1.0 } else { b.done as f32 / b.len as f32 });
        }
        (st, ow, fill)
    }
}

/// The network that delivered the most bytes.
fn dominant(contrib: &[(u8, u64)]) -> Option<u8> {
    contrib.iter().max_by_key(|c| c.1).map(|c| c.0)
}

#[derive(Clone, Copy, Debug)]
pub struct HedgePolicy {
    /// Don't race a block until every attempt has had this long to show its speed.
    pub after: Duration,
    /// What a fresh request costs before its first byte.
    pub startup: Duration,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> Option<HedgePolicy> {
        Some(HedgePolicy { after: Duration::from_secs(2), startup: Duration::from_millis(200) })
    }

    #[test]
    fn block_size_bounds() {
        assert_eq!(plan_block_size(10 << 20, 16), MIN_BLOCK);
        assert_eq!(plan_block_size(100 << 30, 16), MAX_BLOCK);
        let mid = plan_block_size(256 << 20, 16);
        assert!(mid > MIN_BLOCK && mid <= MAX_BLOCK);
        assert_eq!(mid % 0x10000, 0);
    }

    #[test]
    fn table_covers_file_exactly() {
        let t = BlockTable::new(10_500_000, 4 << 20);
        assert_eq!(t.blocks.len(), 3);
        assert_eq!(t.blocks.iter().map(|b| b.len).sum::<u64>(), 10_500_000);
        assert_eq!(t.blocks[2].start + t.blocks[2].len, 10_500_000);
        assert_eq!(BlockTable::new(0, 1 << 20).blocks.len(), 0);
        assert_eq!(BlockTable::new(1, 1 << 20).blocks.len(), 1);
    }

    #[test]
    fn claims_in_order_and_completes() {
        let now = Instant::now();
        let mut t = BlockTable::new(3 << 20, 1 << 20);
        let a = t.claim(0, false, None, 0.0, now).unwrap();
        let b = t.claim(1, false, None, 0.0, now).unwrap();
        assert_eq!((a.idx, b.idx), (0, 1));
        assert_eq!(t.progress(0, a.attempt, 1 << 20), 1 << 20);
        assert!(t.finish(0, a.attempt, 0));
        let c = t.claim(0, false, None, 0.0, now).unwrap();
        assert_eq!(c.idx, 2);
        assert!(!t.all_done());
        t.finish(1, b.attempt, 1);
        t.finish(2, c.attempt, 0);
        assert!(t.all_done());
        assert_eq!(t.blocks[1].owner, 1);
    }

    #[test]
    fn failed_attempt_requeues_with_progress_kept() {
        let now = Instant::now();
        let mut t = BlockTable::new(2 << 20, 1 << 20);
        let a = t.claim(0, false, None, 0.0, now).unwrap();
        t.progress(0, a.attempt, 300_000);
        t.release(0, a.attempt, true);
        let again = t.claim(0, false, None, 0.0, now).unwrap();
        assert_eq!(again.idx, 0);
        assert_eq!(again.from, 300_000);
        assert_eq!(again.start, 300_000);
    }

    #[test]
    fn dead_network_is_not_handed_the_same_block() {
        let now = Instant::now();
        let mut t = BlockTable::new(2 << 20, 1 << 20);
        let a = t.claim(0, true, None, 0.0, now).unwrap();
        t.release(0, a.attempt, false); // net 0 delivered nothing
        // Net 0 asks again while net 1 is free: it gets block 1, not block 0.
        let b = t.claim(0, true, None, 0.0, now).unwrap();
        assert_eq!(b.idx, 1);
        // If nobody else is free it is handed the block anyway — never stranded.
        let c = t.claim(0, false, None, 0.0, now).unwrap();
        assert_eq!(c.idx, 0);
    }

    #[test]
    fn hedges_only_when_queue_is_empty_and_it_pays() {
        let t0 = Instant::now();
        let mut t = BlockTable::new(2 << 20, 1 << 20);
        let slow = t.claim(0, false, policy(), 0.0, t0).unwrap();
        let fast = t.claim(1, false, policy(), 0.0, t0).unwrap();
        t.progress(1, fast.attempt, 1 << 20);
        t.finish(1, fast.attempt, 1);
        // Slow one has delivered 10 KB in 5 s; the free stream manages 10 MB/s.
        t.progress(0, slow.attempt, 10_000);
        let later = t0 + Duration::from_secs(5);
        let h = t.claim(1, false, policy(), 10e6, later).unwrap();
        assert_eq!((h.idx, h.kind), (0, Kind::Hedge));
        // A third network may race it too, but a stream on a network that holds it already
        // is still allowed (different attempts), bounded by the hedge cap.
        assert!(t.blocks[0].hedges == 1);
    }

    #[test]
    fn does_not_hedge_when_slow_block_is_nearly_done_or_requester_is_slow() {
        let t0 = Instant::now();
        let mut t = BlockTable::new(1 << 20, 1 << 20);
        let a = t.claim(0, false, policy(), 0.0, t0).unwrap();
        let later = t0 + Duration::from_secs(3);
        // The holder has delivered nothing: infinite ETA, so a decent requester should race it…
        assert!(t.clone_policy_check(1, 5e6, later));
        // …now it has moved half the block in 3 s (≈175 KB/s, 3 s left). A 100 B/s requester
        // cannot win by 2×, so it must not bother.
        t.progress(0, a.attempt, (1 << 20) / 2);
        assert!(!t.clone_policy_check(1, 100.0, later));
        // And a requester only slightly faster than the holder doesn't clear the 2× bar either.
        assert!(!t.clone_policy_check(1, 200_000.0, later));
    }

    #[test]
    fn winner_stands_down_the_rest() {
        let t0 = Instant::now();
        let mut t = BlockTable::new(1 << 20, 1 << 20);
        let a = t.claim(0, false, policy(), 0.0, t0).unwrap();
        let h = t.claim(1, false, policy(), 20e6, t0 + Duration::from_secs(5)).unwrap();
        assert_eq!(h.kind, Kind::Hedge);
        assert!(t.finish(0, h.attempt, 1));
        assert!(a.token.is_cancelled());
        assert_eq!(t.blocks[0].owner, 1);
        // The loser reporting in afterwards changes nothing.
        t.release(0, a.attempt, true);
        assert!(t.all_done());
    }

    #[test]
    fn a_block_split_across_networks_credits_each_and_is_owned_by_the_larger() {
        let now = Instant::now();
        let mut t = BlockTable::new(1 << 20, 1 << 20);
        // Network 0 gets 100 KB in, then fails; network 1 picks up and does the other 900 KB.
        let a = t.claim(0, false, None, 0.0, now).unwrap();
        t.progress(0, a.attempt, 100_000);
        t.release(0, a.attempt, true);
        let b = t.claim(1, false, None, 0.0, now).unwrap();
        t.progress(0, b.attempt, (1 << 20) - 100_000);
        t.finish(0, b.attempt, 1);
        assert_eq!(t.blocks[0].owner, 1);
        let split = t.split();
        assert_eq!(split.len(), 1);
        let parts = &split[0].1;
        assert_eq!(parts.iter().map(|p| p.1 as u32).sum::<u32>(), 100);
        assert_eq!(parts.iter().find(|p| p.0 == 0).unwrap().1, 10);
    }

    #[test]
    fn a_racing_attempt_rewriting_delivered_bytes_earns_nothing() {
        let t0 = Instant::now();
        let mut t = BlockTable::new(1 << 20, 1 << 20);
        let slow = t.claim(0, false, policy(), 0.0, t0).unwrap();
        t.progress(0, slow.attempt, 500_000);
        let h = t.claim(1, false, policy(), 20e6, t0 + Duration::from_secs(5)).unwrap();
        // The hedge starts at the prefix, so its first 0 bytes are all new — but if the slow
        // attempt delivers more meanwhile, the hedge's overlap is not credited twice.
        t.progress(0, slow.attempt, 100_000);
        let before: u64 = t.blocks[0].contrib.iter().map(|c| c.1).sum();
        t.progress(0, h.attempt, 50_000); // still behind the prefix the slow one reached
        let after: u64 = t.blocks[0].contrib.iter().map(|c| c.1).sum();
        assert_eq!(before, after);
        assert!(t.blocks[0].contrib.iter().all(|c| c.1 <= 600_000));
    }

    #[test]
    fn contrib_survives_save_restore_and_remaps_when_networks_change() {
        let now = Instant::now();
        let mut t = BlockTable::new(2 << 20, 1 << 20);
        let a = t.claim(1, false, None, 0.0, now).unwrap(); // network index 1 = "usb"
        t.progress(0, a.attempt, 1 << 20);
        t.finish(0, a.attempt, 1);
        let saved = t.export(&["wifi".into(), "usb".into()]);
        let (mut r, _) = BlockTable::restore(2 << 20, &saved).unwrap();
        assert_eq!(r.blocks[0].contrib, vec![(1, 1 << 20)]);
        // Next run only has usb (now index 0) and a new network.
        r.remap_networks(&saved.nets, &["usb".into(), "cell".into()]);
        assert_eq!(r.blocks[0].owner, 0);
        assert_eq!(r.blocks[0].contrib, vec![(0, 1 << 20)]);
        // And if the network is gone entirely, the credit is dropped, not misattributed.
        let (mut r2, _) = BlockTable::restore(2 << 20, &saved).unwrap();
        r2.remap_networks(&saved.nets, &["cell".into()]);
        assert_eq!(r2.blocks[0].owner, NO_OWNER);
    }

    #[test]
    fn resume_rewinds_partial_blocks_only() {
        let mut t = BlockTable::new(3 << 20, 1 << 20);
        let now = Instant::now();
        let a = t.claim(0, false, None, 0.0, now).unwrap();
        t.progress(0, a.attempt, 1 << 20);
        t.finish(0, a.attempt, 0);
        let b = t.claim(0, false, None, 0.0, now).unwrap();
        t.progress(1, b.attempt, 700_000);
        let saved = t.export(&["en0".into()]);
        let (r, bytes) = BlockTable::restore(3 << 20, &saved).unwrap();
        assert_eq!(r.blocks[0].st, St::Done);
        assert_eq!(r.blocks[1].done, 700_000 - RESUME_REWIND);
        assert_eq!(bytes, (1 << 20) + 700_000 - RESUME_REWIND);
        assert_eq!(r.pending_blocks(), 2);
    }
}

#[cfg(test)]
impl BlockTable {
    fn clone_policy_check(&self, net: usize, my_speed: f64, now: Instant) -> bool {
        self.pick_hedge(net, my_speed, now, HedgePolicy { after: Duration::from_secs(2), startup: Duration::from_millis(200) }).is_some()
    }
}
