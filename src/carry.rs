//! Picking enemies up like SM64's Bob-ombs: Mario's punch from behind lifts a regular (not boss,
//! not too big) enemy, he carries it between his hands (SM64's own carrying: walk, jump), and B
//! throws it: a moment into the flight it goes limp (the game's death ragdoll) and whatever it
//! hits first kills it.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use eldenring::cs::{FieldInsHandle, WorldChrMan};
use eldenring::position::HavokPosition;
use fromsoftware_shared::FromStatic;
use glam::Vec3;

use crate::log;

/// SM64's throwing actions (on the ground, in the air)
const ACT_THROWING: u32 = 0x8000_0588;
const ACT_AIR_THROW: u32 = 0x8300_08AB;
/// Biggest enemy Mario can lift (m): about a soldier
const MAX_RADIUS: f32 = 0.8;
const MAX_HEIGHT: f32 = 2.4;
/// Throw: speed forward and up (m/s), gravity (m/s²)
const THROW_SPEED: f32 = 14.0;
const THROW_UP: f32 = 5.0;
const GRAVITY: f32 = 16.0;
/// Impact damage: all of it (the finishing blow is the game's, see combat::impact)
pub const IMPACT_PCT: f32 = 100.0;
/// Damage to an enemy the thrown one crashes into (share of its max HP)
pub const BOWLED_PCT: f32 = 50.0;
/// Guided flight before it goes limp (the ragdoll's bodies take over the speed it has then)
const LIMP_AFTER: f32 = 0.1;

enum Phase {
    Idle,
    /// Mario lifts / carries it: (enemy, its height in m)
    Held { mob: FieldInsHandle, height: f32, since: Instant },
    Flying { mob: FieldInsHandle, pos: Vec3, vel: Vec3, since: Instant, radius: f32 },
    /// flying limp (the ragdoll carries it): smoothed speed, frames since going limp
    Limp { mob: FieldInsHandle, last: Vec3, speed: f32, since: Instant, frames: u32 },
}

static PHASE: Mutex<Phase> = Mutex::new(Phase::Idle);
/// A pickup just started: the SM64 tick puts Mario into SM64's pickup
static START: AtomicBool = AtomicBool::new(false);
/// Mario must let go (the enemy is gone or died)
static DROP: AtomicBool = AtomicBool::new(false);

fn key(h: &FieldInsHandle) -> u64 {
    unsafe { std::mem::transmute_copy::<FieldInsHandle, u64>(h) }
}

/// Mario's punch landed on `h` (a regular enemy): lifts it if Mario is behind it and it isn't too
/// big. True: picked up (no normal damage).
pub fn try_pick_up(h: &FieldInsHandle, mario: Vec3, radius_m: f32, height_m: f32) -> bool {
    let mut phase = PHASE.lock().unwrap_or_else(|e| e.into_inner());
    if !matches!(*phase, Phase::Idle) || radius_m > MAX_RADIUS || height_m > MAX_HEIGHT {
        return false;
    }
    let Ok(wcm) = (unsafe { WorldChrMan::instance() }) else { return false };
    let Some(chr) = wcm.chr_ins_by_handle(h) else { return false };
    if chr.modules.data.hp <= 0 {
        return false;
    }
    let ph = &chr.modules.physics;
    let o = ph.orientation;
    let fwd = glam::Quat::from_xyzw(o.0, o.1, o.2, o.3).mul_vec3(Vec3::new(0.0, 0.0, -1.0));
    let fwd = Vec3::new(fwd.x, 0.0, fwd.z).normalize_or_zero();
    let to_mario = Vec3::new(mario.x - ph.position.0, 0.0, mario.z - ph.position.2).normalize_or_zero();
    // not in front of it: Mario anywhere outside its ~70° front cone (behind or beside it). Only
    // right behind (±60°) was too hard to hit, enemies turn to face Mario quickly.
    let behind = fwd.dot(to_mario);
    if behind > 0.35 {
        if crate::debug() {
            log(format!("carry: no pickup, Mario is in front of it (facing {behind:.2})"));
        }
        return false;
    }
    log(format!("carry: picked up an enemy from behind ({:.1} x {:.1} m, facing {behind:.2})", radius_m, height_m));
    *phase = Phase::Held { mob: *h, height: height_m, since: Instant::now() };
    START.store(true, Ordering::Relaxed);
    true
}

/// SM64 tick: whether to start SM64's pickup now.
pub fn take_start() -> bool {
    START.swap(false, Ordering::Relaxed)
}

/// SM64 tick: whether Mario must let go.
pub fn take_drop() -> bool {
    DROP.swap(false, Ordering::Relaxed)
}


/// The enemy Mario carries or threw last, and when it was last carried or flying.
static LAST_MOB: Mutex<Option<(u64, Instant)>> = Mutex::new(None);
/// How long after that it still can't hurt Mario (running after a throw ran him into its hitbox).
const SAFE_AFTER: f32 = 1.0;

/// The last carried enemy's handle key (diagnostics), 0 if none.
pub fn last_mob_key() -> u64 {
    LAST_MOB.lock().unwrap_or_else(|e| e.into_inner()).map_or(0, |(k, _)| k)
}

/// Whether a hit Mario (at `mario`, game coordinates) just took came from the enemy he holds, the
/// one in the air, or the one he let go of a moment ago, so it's ignored. The game doesn't say who
/// hit the player (its last-attacker field stays empty), so: that enemy is within reach and no
/// other enemy is. Everyone else still hurts him.
pub fn harmless(mario: Vec3) -> bool {
    let phase = PHASE.lock().unwrap_or_else(|e| e.into_inner());
    let mut last = LAST_MOB.lock().unwrap_or_else(|e| e.into_inner());
    if let Phase::Held { mob, .. } | Phase::Flying { mob, .. } | Phase::Limp { mob, .. } = &*phase {
        *last = Some((key(mob), Instant::now()));
    }
    let Some((mob, t)) = *last else { return false };
    if t.elapsed().as_secs_f32() >= SAFE_AFTER {
        return false;
    }
    let (mut mob_near, mut other_near) = (false, false);
    let Ok(wcm) = (unsafe { WorldChrMan::instance() }) else { return false };
    for set in wcm.chr_sets.iter().flatten() {
        for chr in set.characters() {
            let chr: &eldenring::cs::ChrIns = chr;
            if chr.modules.data.hp <= 0 || crate::combat::own_side(chr.team_type) {
                continue;
            }
            let q = chr.modules.physics.position;
            let d = Vec3::new(q.0 - mario.x, q.1 - mario.y, q.2 - mario.z).length();
            if d > HIT_RANGE {
                continue;
            }
            if key(&chr.field_ins_handle) == mob {
                mob_near = true;
            } else if crate::combat::hittable(chr.chr_type) {
                other_near = true;
            }
        }
    }
    mob_near && !other_near
}

/// Within this distance (m) an enemy could have just hit Mario.
const HIT_RANGE: f32 = 3.0;

/// Whether this enemy is in Mario's hands or flying (no normal hits on it).
pub fn is_carried(h: &FieldInsHandle) -> bool {
    match &*PHASE.lock().unwrap_or_else(|e| e.into_inner()) {
        Phase::Held { mob, .. } | Phase::Flying { mob, .. } | Phase::Limp { mob, .. } => key(mob) == key(h),
        Phase::Idle => false,
    }
}

pub fn reset() {
    *PHASE.lock().unwrap_or_else(|e| e.into_inner()) = Phase::Idle;
}

/// Another enemy the thrown one (at `p`) runs into: within a body's width, overlapping in height.
fn bumped(mob: &FieldInsHandle, p: Vec3) -> Option<FieldInsHandle> {
    use eldenring::cs::ChrIns;
    let wcm = unsafe { WorldChrMan::instance() }.ok()?;
    for set in wcm.chr_sets.iter().flatten() {
        for chr in set.characters() {
            let chr: &ChrIns = chr;
            if !crate::combat::hittable(chr.chr_type)
                || key(&chr.field_ins_handle) == key(mob)
                || chr.modules.data.hp <= 0
                || crate::combat::own_side(chr.team_type)
            {
                continue;
            }
            let q = chr.modules.physics.position;
            let r = chr.modules.physics.hit_radius.max(0.3) + 0.4;
            let (dx, dz) = (q.0 - p.x, q.2 - p.z);
            let h = chr.modules.physics.hit_height.max(1.0);
            if dx * dx + dz * dz < r * r && p.y > q.1 - 1.0 && p.y < q.1 + h {
                return Some(chr.field_ins_handle);
            }
        }
    }
    None
}

/// Every frame in Mario mode. `hands`: SM64's held-object point (game coordinates) and whether
/// Mario still holds it; `face` Mario's SM64 face angle; `action` his SM64 action; `hit` a map
/// ray. Returns an impact: (enemy, damage share of max HP in %).
pub fn update(
    dt: f32,
    hands: Option<Vec3>,
    face: f32,
    action: u32,
    hit: impl Fn(Vec3, Vec3) -> Option<Vec3>,
) -> Vec<(FieldInsHandle, f32)> {
    let Ok(wcm) = (unsafe { WorldChrMan::instance_mut() }) else { return Vec::new() };
    let mut phase = PHASE.lock().unwrap_or_else(|e| e.into_inner());
    // Mario's forward in game coordinates (SM64 is mirrored on X)
    let fwd = Vec3::new(-face.sin(), 0.0, face.cos());
    match *phase {
        Phase::Idle => Vec::new(),
        Phase::Held { mob, height, since } => {
            let Some(chr) = wcm.chr_ins_by_handle_mut(&mob) else {
                *phase = Phase::Idle;
                DROP.store(true, Ordering::Relaxed);
                return Vec::new();
            };
            if chr.modules.data.hp <= 0 {
                chr.modules.physics.gravity_disabled = false;
                *phase = Phase::Idle;
                DROP.store(true, Ordering::Relaxed);
                return Vec::new();
            }
            let ph = &mut chr.modules.physics;
            match hands {
                // (the pickup's first frames: SM64 hasn't placed the hands yet, keep it where it is)
                Some(p) if p != Vec3::ZERO => {
                    // held around its middle, facing away from Mario like a Bob-omb
                    let at = p - Vec3::Y * (height * 0.45) + fwd * 0.25;
                    ph.position = HavokPosition(at.x, at.y, at.z, 0.0);
                    ph.chr_proxy_pos_update_requested = true;
                    ph.gravity_disabled = true;
                    let q = glam::Quat::from_rotation_y(std::f32::consts::PI - face);
                    ph.orientation = eldenring::rotation::Quaternion(q.x, q.y, q.z, q.w);
                    chr.modules.fall.fall_timer = 0.0;
                    Vec::new()
                }
                Some(_) => Vec::new(),
                // (SM64 starts the pickup on its next tick: not holding yet isn't letting go)
                None if START.load(Ordering::Relaxed) || since.elapsed().as_secs_f32() < 0.25 => Vec::new(),
                None if action == ACT_THROWING || action == ACT_AIR_THROW => {
                    let pos = Vec3::new(ph.position.0, ph.position.1, ph.position.2);
                    let vel = fwd * THROW_SPEED + Vec3::Y * THROW_UP;
                    log("carry: thrown");
                    *phase = Phase::Flying { mob, pos, vel, since: Instant::now(), radius: 0.5 };
                    Vec::new()
                }
                None => {
                    // let go some other way (Mario got hurt, fell): just drop it
                    ph.gravity_disabled = false;
                    log(format!("carry: dropped without a throw (action {action:#x})"));
                    *phase = Phase::Idle;
                    Vec::new()
                }
            }
        }
        Phase::Flying { mob, pos, vel, since, radius } => {
            let Some(chr) = wcm.chr_ins_by_handle_mut(&mob) else {
                *phase = Phase::Idle;
                return Vec::new();
            };
            let ph = &mut chr.modules.physics;
            let mut vel = vel;
            vel.y -= GRAVITY * dt;
            let next = pos + vel * dt;
            let lead = (next - pos).normalize_or_zero() * radius;
            let impact = hit(pos + Vec3::Y * 0.5, next + lead + Vec3::Y * 0.5).or_else(|| hit(pos + Vec3::Y * 0.5, next - Vec3::Y * 0.1));
            if let Some(o) = bumped(&mob, next + Vec3::Y * 0.5).filter(|_| since.elapsed().as_secs_f32() > 0.05) {
                ph.gravity_disabled = false;
                log("carry: thrown enemy crashed into another one: dead");
                *phase = Phase::Idle;
                return vec![(mob, IMPACT_PCT), (o, BOWLED_PCT)];
            }
            match impact {
                Some(h) if since.elapsed().as_secs_f32() > 0.05 => {
                    let back = (h - pos).normalize_or_zero() * radius;
                    let rest = h - back;
                    ph.position = HavokPosition(rest.x, rest.y.max(h.y), rest.z, 0.0);
                    ph.chr_proxy_pos_update_requested = true;
                    ph.gravity_disabled = false;
                    log(format!("carry: thrown enemy hit something at {:.1} m/s: {IMPACT_PCT:.0}% of its HP", vel.length()));
                    *phase = Phase::Idle;
                    vec![(mob, IMPACT_PCT)]
                }
                // flew off into nothing: the game's own fall takes it from here
                _ if since.elapsed().as_secs_f32() > 3.0 => {
                    ph.gravity_disabled = false;
                    log("carry: thrown enemy flew off");
                    *phase = Phase::Idle;
                    Vec::new()
                }
                // a few frames of guided flight gave its ragdoll's bodies the throw's speed: limp
                // from here (the game's death ragdoll: it's done for anyway)
                _ if chr.chr_ctrl.ragdoll_ins != 0 && since.elapsed().as_secs_f32() > LIMP_AFTER => {
                    chr.chr_ctrl.chr_ragdoll_state = 2;
                    chr.modules.physics.gravity_disabled = false;
                    log(format!("carry: limp at {:.1} m/s", vel.length()));
                    *phase = Phase::Limp { mob, last: pos, speed: vel.length(), since: Instant::now(), frames: 0 };
                    Vec::new()
                }
                _ => {
                    ph.position = HavokPosition(next.x, next.y, next.z, 0.0);
                    ph.chr_proxy_pos_update_requested = true;
                    ph.gravity_disabled = true;
                    *phase = Phase::Flying { mob, pos: next, vel, since, radius };
                    Vec::new()
                }
            }
        }
        Phase::Limp { mob, last, speed, since, frames } => {
            let Some(chr) = wcm.chr_ins_by_handle_mut(&mob) else {
                *phase = Phase::Idle;
                return Vec::new();
            };
            let p = chr.modules.physics.position;
            let now = Vec3::new(p.0, p.1, p.2);
            let v = (now - last).length() / dt.max(1e-3);
            // (the first frames jump as it snaps to the ragdoll's hips: not speed)
            let settle = frames < 3;
            let smooth = if settle { speed } else { speed * 0.6 + v * 0.4 };
            let _ = smooth;
            // contact: the map right under it or ahead of it (its position lags the ragdoll a
            // little, so the rays reach past it), or another enemy
            let dir = (now - last).normalize_or_zero();
            // (the ground only on the way down: it's thrown from about hand height)
            let ground = !settle && now.y <= last.y && hit(now + Vec3::Y * 0.3, now - Vec3::Y * 0.35).is_some();
            let wall = !settle && hit(last + Vec3::Y * 0.5, now + dir * 0.6 + Vec3::Y * 0.5).is_some();
            let other = if settle { None } else { bumped(&mob, now + Vec3::Y * 0.5) };
            let age = since.elapsed().as_secs_f32();
            if ground || wall || other.is_some() || age > 3.0 {
                log(format!("carry: limp enemy hit something at {smooth:.1} m/s (ground {ground}, wall {wall}, enemy {}): dead", other.is_some()));
                *phase = Phase::Idle;
                let mut out = vec![(mob, IMPACT_PCT)];
                if let Some(o) = other {
                    out.push((o, BOWLED_PCT));
                }
                return out;
            }
            *phase = Phase::Limp { mob, last: now, speed: smooth, since, frames: frames + 1 };
            Vec::new()
        }
    }
}
