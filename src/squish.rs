//! Enemies Mario lands on get flattened for a moment, like a stomped Goomba that survives.

use std::sync::Mutex;
use std::time::Instant;

use eldenring::cs::{FieldInsHandle, WorldChrMan};
use fromsoftware_shared::FromStatic;

/// Squash and spring back (s)
const DOWN_FOR: f32 = 0.07;
const BACK_FOR: f32 = 0.5;

/// (who, since, how flat 0..1, how long it stays flat (s), the scale to go back to)
static LIVE: Mutex<Vec<(FieldInsHandle, Instant, f32, f32, [f32; 3])>> = Mutex::new(Vec::new());

fn key(h: &FieldInsHandle) -> u64 {
    unsafe { std::mem::transmute_copy::<FieldInsHandle, u64>(h) }
}

/// Mario landed on `h`. `flat` 0..1: a stomp dents, a ground pound flattens and holds.
pub fn start(h: &FieldInsHandle, flat: f32, hold: f32) {
    let Some(chr) = unsafe { WorldChrMan::instance() }.ok().and_then(|w| w.chr_ins_by_handle(h)) else { return };
    let c = &chr.chr_ctrl;
    let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    // hit again mid squish: keep the scale it started from
    let rest = match live.iter().position(|e| key(&e.0) == key(h)) {
        Some(i) => live.remove(i).4,
        None => [c.scale_size_x, c.scale_size_y, c.scale_size_z],
    };
    live.push((*h, Instant::now(), flat, hold, rest));
}

/// Every frame.
pub fn tick() {
    let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    if live.is_empty() {
        return;
    }
    let Ok(wcm) = (unsafe { WorldChrMan::instance_mut() }) else { return };
    live.retain(|(h, since, flat, hold, rest)| {
        let Some(chr) = wcm.chr_ins_by_handle_mut(h) else { return false };
        let t = since.elapsed().as_secs_f32();
        let done = t >= DOWN_FOR + hold + BACK_FOR;
        let amount = if done {
            0.0
        } else if t < DOWN_FOR {
            t / DOWN_FOR
        } else if t < DOWN_FOR + hold {
            1.0
        } else {
            // springs back past its height once
            let u = (t - DOWN_FOR - hold) / BACK_FOR;
            (1.0 - u) * (u * std::f32::consts::PI * 2.5).cos()
        };
        let y = 1.0 - flat * amount;
        // wider as it gets flatter, about the same volume
        let xz = 1.0 / y.max(0.3).sqrt();
        let c = &mut chr.chr_ctrl;
        c.scale_size_x = rest[0] * xz;
        c.scale_size_y = rest[1] * y;
        c.scale_size_z = rest[2] * xz;
        !done
    });
}
