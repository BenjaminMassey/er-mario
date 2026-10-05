//! Yoshi at full speed runs over what's in his way: a share of the enemy's health, and anything
//! short of a boss is sent flying and lies there a moment.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use eldenring::cs::{FieldInsHandle, WorldChrMan};
use eldenring::position::HavokPosition;
use fromsoftware_shared::FromStatic;
use glam::Vec3;

use crate::combat::{self, Combat};
use crate::log;

/// Share of its full health (%): ordinary enemies, and bosses and other strong ones
const HIT: f32 = 30.0;
const HIT_STRONG: f32 = 5.0;
/// How near his path (m, on top of the enemy's own width)
const REACH: f32 = 0.9;
/// The same enemy isn't hit again this soon (s); longer than it lies there, since the game
/// takes a hit on its ragdoll for a kill
const AGAIN: f32 = 5.0;
/// Pushed along for this long (s) before it goes limp: the ragdoll takes over the speed it has
const PUSHED: f32 = 0.1;
const DOWN: f32 = 4.0;
/// Thrown ahead at this share of his speed, and up (m/s)
const CARRY: f32 = 0.7;
const LIFT: f32 = 5.0;

struct Flung {
    handle: FieldInsHandle,
    vel: Vec3,
    since: Instant,
    limp: bool,
}

static FLUNG: Mutex<Vec<Flung>> = Mutex::new(Vec::new());
static RECENT: Mutex<Option<HashMap<u64, Instant>>> = Mutex::new(None);

/// Every frame. `charge`: where Yoshi is and how fast he's going, while he's at full speed.
pub fn update(combat: &mut Combat, tick: u32, dt: f32, charge: Option<(Vec3, Vec3)>) {
    let mut flung = FLUNG.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, vel)) = charge {
        let mut recent = RECENT.lock().unwrap_or_else(|e| e.into_inner());
        let recent = recent.get_or_insert_with(HashMap::new);
        recent.retain(|_, t| t.elapsed().as_secs_f32() < AGAIN);
        // (a little ahead of him: at this speed he's past them a frame later)
        let ahead = at + vel.normalize_or_zero() * 0.8;
        for (handle, key, strong) in combat::in_the_way(ahead, REACH) {
            if recent.contains_key(&key) {
                continue;
            }
            recent.insert(key, Instant::now());
            combat::impact(combat, &handle, if strong { HIT_STRONG } else { HIT }, tick);
            crate::worker::call("trample sound", |_| unsafe { crate::sm64::sm64_play_sound_global(crate::swing::SOUND_IMPACT) });
            log(format!("trample: ran into {} at {:.1} m/s", if strong { "a strong enemy" } else { "an enemy" }, vel.length()));
            if !strong {
                flung.push(Flung { handle, vel: vel * CARRY + Vec3::Y * LIFT, since: Instant::now(), limp: false });
            }
        }
    }
    if flung.is_empty() {
        return;
    }
    let Ok(wcm) = (unsafe { WorldChrMan::instance_mut() }) else { return };
    flung.retain_mut(|f| {
        let Some(chr) = wcm.chr_ins_by_handle_mut(&f.handle) else { return false };
        let t = f.since.elapsed().as_secs_f32();
        // (the game's own fall damage would finish it off)
        chr.modules.fall.fall_timer = 0.0;
        if !f.limp {
            if t < PUSHED {
                let p = chr.modules.physics.position;
                let next = Vec3::new(p.0, p.1, p.2) + f.vel * dt;
                chr.modules.physics.position = HavokPosition(next.x, next.y, next.z, 0.0);
                chr.modules.physics.chr_proxy_pos_update_requested = true;
                chr.modules.physics.gravity_disabled = true;
            } else {
                chr.modules.physics.gravity_disabled = false;
                if chr.chr_ctrl.ragdoll_ins == 0 {
                    return false;
                }
                chr.chr_ctrl.chr_ragdoll_state = 2;
                f.limp = true;
            }
            return true;
        }
        if t < DOWN {
            return true;
        }
        // back on its feet, unless that was the end of it
        if chr.modules.data.hp > 0 {
            chr.chr_ctrl.chr_ragdoll_state = 0;
            chr.chr_ctrl.ragdoll_revive_time = 1.0;
        }
        false
    });
}
