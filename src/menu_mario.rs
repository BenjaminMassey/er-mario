//! Mario in character creation: a Mario of his own on a flat floor, idling, whose pose goes on
//! the preview's skeleton (engine_mario's pose sync hook). libsm64 only runs in the world
//! otherwise.

use std::sync::Mutex;
use std::time::Instant;

use crate::{engine_mario, sm64, worker};

type Parts = [engine_mario::PartPose; engine_mario::PARTS];

struct Menu {
    id: Option<i32>,
    last: Option<Instant>,
    /// time not ticked yet (s)
    owed: f32,
    /// the last two ticks' poses: frames in between blend them, like in the world
    prev: Option<Parts>,
    now: Option<Parts>,
}

static MARIO: Mutex<Menu> = Mutex::new(Menu { id: None, last: None, owed: 0.0, prev: None, now: None });
/// SM64 runs at 30 Hz
const TICK: f32 = 1.0 / 30.0;

/// Every frame. `on`: the character on screen outside the world is Mario.
pub fn tick(on: bool) {
    let mut m = MARIO.lock().unwrap_or_else(|e| e.into_inner());
    if !on {
        if let Some(id) = m.id.take() {
            worker::call("menu delete", move |_| unsafe { sm64::sm64_mario_delete(id) });
        }
        *m = Menu { id: None, last: None, owed: 0.0, prev: None, now: None };
        return;
    }
    if !crate::SM64_READY.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let dt = m.last.map(|t| t.elapsed().as_secs_f32()).unwrap_or(TICK);
    m.last = Some(Instant::now());
    // (after a hitch: carry on from here, no fast-forward)
    m.owed = (m.owed + dt).min(TICK * 3.0);
    if m.id.is_none() {
        m.id = worker::call("menu create", |_| {
            let e = 8000;
            let floor = [
                sm64::SM64Surface::grass([[-e, 0, -e], [e, 0, e], [e, 0, -e]]),
                sm64::SM64Surface::grass([[-e, 0, -e], [-e, 0, e], [e, 0, e]]),
            ];
            unsafe {
                sm64::sm64_static_surfaces_load(floor.as_ptr(), floor.len() as u32);
                sm64::sm64_mario_create(0.0, 0.0, 0.0)
            }
        })
        .filter(|id| *id >= 0);
    }
    let Some(id) = m.id else { return };
    while m.owed >= TICK {
        m.owed -= TICK;
        let parts = worker::call("menu tick", move |ctx| {
            let inputs = sm64::SM64MarioInputs::default();
            let mut state = sm64::SM64MarioState::default();
            {
                let mut b = ctx.geo.buffers();
                unsafe { sm64::sm64_mario_tick(id, &inputs, &mut state, &mut *b) };
            }
            let mut mats = vec![0f32; 64 * 16];
            let mut tri_part = vec![0i32; sm64::GEO_MAX_TRIANGLES];
            let count = unsafe { sm64::sm64_er_get_parts(mats.as_mut_ptr(), tri_part.as_mut_ptr(), std::ptr::null_mut(), std::ptr::null_mut()) };
            engine_mario::relative_parts(&mats, count, state.position, crate::eye_cell(&ctx.geo.uv, ctx.geo.used()), false)
        })
        .flatten();
        if let Some(parts) = parts {
            m.prev = m.now.replace(parts);
        }
    }
    match (&m.prev, &m.now) {
        (Some(a), Some(b)) => engine_mario::set_standing(&engine_mario::blend(a, b, m.owed / TICK)),
        (None, Some(b)) => engine_mario::set_standing(b),
        _ => {}
    }
}
