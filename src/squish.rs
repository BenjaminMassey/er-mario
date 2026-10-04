//! Enemies Mario lands on get flattened for a moment, like a stomped Goomba that survives.
//!
//! Done on the skeleton (the model pose the animation job writes, see engine_mario.rs), not with
//! the character's model scale: cloth hangs off the bones and never sees that scale, so a
//! flattened knight kept a full-length cape.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use eldenring::cs::{FieldInsHandle, WorldChrMan};
use fromsoftware_shared::FromStatic;
use glam::{Quat, Vec3};

use crate::explore;

/// Squash and spring back (s)
const DOWN_FOR: f32 = 0.07;
const BACK_FOR: f32 = 0.5;

struct Squish {
    who: FieldInsHandle,
    since: Instant,
    /// how flat 0..1, and how long it stays flat (s)
    flat: f32,
    hold: f32,
    /// height and width factors right now
    now: (f32, f32),
    chr: usize,
    imp: usize,
    model: usize,
    count: usize,
    /// what we last wrote per bone: anything else there is a fresh pose from the animation
    wrote: Vec<[f32; 12]>,
}

static LIVE: Mutex<Vec<Squish>> = Mutex::new(Vec::new());
/// (the animation hook runs for every character, many times a frame)
static ANY: AtomicBool = AtomicBool::new(false);

fn key(h: &FieldInsHandle) -> u64 {
    unsafe { std::mem::transmute_copy::<FieldInsHandle, u64>(h) }
}

/// ChrIns +0x398 pose importer: +0x60 model pose (hkQsTransform, 0x30 each), +0x68 bone count.
fn pose_of(chr: usize) -> Option<(usize, usize, usize)> {
    let imp = explore::read_u64(chr + 0x398)? as usize;
    let model = explore::read_u64(imp + 0x60)? as usize;
    let count = explore::read_u64(imp + 0x68)? as u32 as usize;
    (count > 0 && count <= 1024 && explore::readable(model, count * 0x30)).then_some((imp, model, count))
}

/// Mario landed on `h`. `flat` 0..1: a stomp dents, a ground pound flattens and holds.
pub fn start(h: &FieldInsHandle, flat: f32, hold: f32) {
    let Some(chr) = unsafe { WorldChrMan::instance() }.ok().and_then(|w| w.chr_ins_by_handle(h)) else { return };
    let chr = chr as *const _ as usize;
    let Some((imp, model, count)) = pose_of(chr) else { return };
    let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    live.retain(|s| key(&s.who) != key(h));
    live.push(Squish { who: *h, since: Instant::now(), flat, hold, now: (1.0, 1.0), chr, imp, model, count, wrote: vec![[0.0; 12]; count] });
    ANY.store(true, Ordering::Relaxed);
}

/// Every frame: how flat each one is now, and who's done or gone.
pub fn tick() {
    let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    if live.is_empty() {
        return;
    }
    let wcm = unsafe { WorldChrMan::instance() }.ok();
    live.retain_mut(|s| {
        let here = wcm.as_ref().and_then(|w| w.chr_ins_by_handle(&s.who)).map(|c| c as *const _ as usize);
        let t = s.since.elapsed().as_secs_f32();
        if here != Some(s.chr) || t >= DOWN_FOR + s.hold + BACK_FOR {
            return false;
        }
        let amount = if t < DOWN_FOR {
            t / DOWN_FOR
        } else if t < DOWN_FOR + s.hold {
            1.0
        } else {
            // springs back past its height once
            let u = (t - DOWN_FOR - s.hold) / BACK_FOR;
            (1.0 - u) * (u * std::f32::consts::PI * 2.5).cos()
        };
        let y = 1.0 - s.flat * amount;
        // wider as it gets flatter, about the same volume
        s.now = (y, 1.0 / y.max(0.3).sqrt());
        true
    });
    ANY.store(!live.is_empty(), Ordering::Relaxed);
}

/// Flattens the poses the animation has written since the last call. Runs after the animation
/// job and in the task groups up to drawing, like Mario's own pose.
pub fn apply() {
    if !ANY.load(Ordering::Relaxed) {
        return;
    }
    let raw = |a: usize| unsafe { *(a as *const usize) };
    let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    for s in live.iter_mut() {
        // (no memory checks here: tick drops a character that's gone within a frame)
        if raw(s.chr + 0x398) != s.imp || raw(s.imp + 0x60) != s.model {
            continue;
        }
        let (y, xz) = s.now;
        let squash = Vec3::new(xz, y, xz);
        for b in 0..s.count {
            let at = (s.model + b * 0x30) as *mut [f32; 12];
            let v = unsafe { *at };
            if v == s.wrote[b] {
                continue;
            }
            let rot = Quat::from_xyzw(v[4], v[5], v[6], v[7]);
            // the bone's own axes, squashed in model space: how long each one is now
            let grow = |axis: Vec3| (rot * axis * squash).length();
            let out = [v[0] * xz, v[1] * y, v[2] * xz, v[3], v[4], v[5], v[6], v[7], v[8] * grow(Vec3::X), v[9] * grow(Vec3::Y), v[10] * grow(Vec3::Z), v[11]];
            unsafe { *at = out };
            s.wrote[b] = out;
        }
    }
}
