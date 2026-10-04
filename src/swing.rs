//! Bowser's tail swing on bosses: break a boss's stance, and Mario's next hit grabs him like
//! Bowser (SM64's own grab, swing and throw: spin the stick to wind up, B to let go). The boss
//! is carried around Mario at arm's length while he swings, then flies off where Mario faces,
//! and whatever he hits first (a wall, a rock, the ground) hurts him badly.

use std::sync::Mutex;
use std::time::Instant;

use eldenring::cs::{FieldInsHandle, WorldChrMan};
use eldenring::position::HavokPosition;
use fromsoftware_shared::FromStatic;
use glam::Vec3;

use crate::log;

pub const ACT_PICKING_UP_BOWSER: u32 = 0x390;
pub const ACT_HOLDING_BOWSER: u32 = 0x391;
pub const ACT_RELEASING_BOWSER: u32 = 0x392;
/// SOUND_OBJ_BOWSER_TAIL_PICKUP, SOUND_GENERAL_BOWSER_BOMB_EXPLOSION
pub const SOUND_GRAB: i32 = 0x5005_0081;
pub const SOUND_IMPACT: i32 = 0x312F_0081;

/// How long a broken stance leaves a boss open to the grab (s)
const OPEN_FOR: f32 = 3.0;
/// The game's ragdoll blend (ChrCtrl +0x128 state, +0x12C amount): 0 animated, 4 blended, 2 full
/// (its death ragdoll, never used here). Amount 1 = all ragdoll.
const RAGDOLL_FULL: f32 = 0.99;
const DOWN_FOR: f32 = 2.0;
/// A ragdoll going faster than this by itself is running away (on a mounted boss it gained speed
/// on its own, 41 -> 58 m/s, until the physics hung the game): that throw's ragdoll ends at once.
/// It only happens on some throws, so the boss keeps his ragdoll for the next ones.
const RUNAWAY_MIN: f32 = 35.0;
const RUNAWAY_DOWN: f32 = 30.0;

/// Character ids (cXXXX) that never go ragdoll: `no_ragdoll` in er_mario.ini (e.g. "4750, 3251").
fn no_ragdoll_ids() -> Vec<u32> {
    crate::paths::config("no_ragdoll")
        .map(|v| v.split(|c: char| c == ',' || c.is_whitespace()).filter_map(|t| t.trim().trim_start_matches(['c', 'C']).parse().ok()).collect())
        .unwrap_or_default()
}

fn ragdoll_allowed(chr: &eldenring::cs::ChrIns) -> bool {
    ragdoll_on() && chr.chr_ctrl.ragdoll_ins != 0 && !no_ragdoll_ids().contains(&chr.character_id)
}

/// A runaway ragdoll: ended now, the boss protected while he settles.
fn runaway(chr: &mut eldenring::cs::ChrIns, v: f32) {
    set_ragdoll(chr, 0.0);
    clear_fall(chr);
    chr.modules.physics.gravity_disabled = false;
    log(format!("swing: runaway ragdoll on c{:04} ({v:.1} m/s): ended for this throw", chr.character_id));
}
const GET_UP: f32 = 1.0;

/// Thrown bosses go limp: on unless `boss_ragdoll = off` in er_mario.ini.
fn ragdoll_on() -> bool {
    !crate::paths::config("boss_ragdoll").is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "off" | "0" | "false" | "no"))
}

/// Ragdoll amount 0..1 (0 = back to normal animation). The game's ragdoll states (ChrCtrl +0x128,
/// synced into the ragdoll each frame by 0x1403cca60): 0 off, 2 full (death), 3 full, 4 blend.
/// Which one we use: boss_ragdoll_state in er_mario.ini (default 2: the full ragdoll with map
/// collision; the boss must not be hit while in it, it counts as dead then, and it is switched
/// back to 0 after DOWN_FOR, which gets him back up).
fn set_ragdoll(chr: &mut eldenring::cs::ChrIns, amount: f32) {
    let state = crate::paths::config("boss_ragdoll_state").and_then(|v| v.parse().ok()).unwrap_or(2u8);
    let c = &mut chr.chr_ctrl;
    if amount <= 0.0 {
        c.chr_ragdoll_state = 0;
        c.ragdoll_revive_time = 1.0;
    } else if state == 4 {
        c.chr_ragdoll_state = 4;
        c.ragdoll_revive_time = amount.min(RAGDOLL_FULL);
    } else {
        c.chr_ragdoll_state = state;
    }
}

/// While thrown and down: no falling as far as the game is concerned (its fall damage kills), and
/// his HP no lower than `guard` (0 = not set yet).
fn protect(chr: &mut eldenring::cs::ChrIns, guard: &mut i32) {
    chr.modules.fall.fall_timer = 0.0;
    let hp = chr.modules.data.hp;
    if *guard <= 0 {
        *guard = hp;
    } else if hp < *guard {
        chr.modules.data.hp = *guard;
    }
}

/// Thrown off the map (far below where he was thrown from, or falling for long): back where
/// Mario threw him from, standing, like Bowser jumping back onto his platform.
/// Thrown off the map: far below where he was thrown from, or falling nonstop for a long time.
fn off_the_map(chr: &eldenring::cs::ChrIns, home: Vec3, falling_for: f32) -> bool {
    let y = chr.modules.physics.position.1;
    let gone = y < home.y - 25.0 || falling_for > 5.0;
    if gone {
        log(format!("swing: off the map at {:.1} m below the throw, falling for {falling_for:.1} s", home.y - y));
    }
    gone
}

fn bring_back(chr: &mut eldenring::cs::ChrIns, home: Vec3) {
    set_ragdoll(chr, 0.0);
    hold_at(chr, home);
    log("swing: thrown off the map: he's back where he was thrown from");
}

/// At `home`, not falling, and the game's fell-out-of-the-world flag cleared (CSChrFallModule
/// +0x1C, next to the fall timer).
fn hold_at(chr: &mut eldenring::cs::ChrIns, home: Vec3) {
    let ph = &mut chr.modules.physics;
    ph.position = HavokPosition(home.x, home.y + 0.5, home.z, 0.0);
    ph.chr_proxy_pos_update_requested = true;
    ph.gravity_disabled = false;
    let fall = &mut chr.modules.fall;
    fall.fall_timer = 0.0;
    unsafe { *((&mut **fall as *mut eldenring::cs::CSChrFallModule as *mut u8).add(0x1C)) = 0 };
}

/// No falling as far as the game is concerned: fall timer and fell-out-of-the-world flag
/// (CSChrFallModule +0x1C) cleared.
fn clear_fall(chr: &mut eldenring::cs::ChrIns) {
    let fall = &mut chr.modules.fall;
    fall.fall_timer = 0.0;
    unsafe { *((&mut **fall as *mut eldenring::cs::CSChrFallModule as *mut u8).add(0x1C)) = 0 };
}

/// Whether Mario has a boss by the tail (the boss can't hurt him then).
pub fn holding() -> bool {
    matches!(STATE.lock().unwrap_or_else(|e| e.into_inner()).phase, Phase::Held { .. })
}

/// Whether Mario is doing something with this boss (holding, throwing, or he's down or settling
/// from a throw): his final blow waits for the throw's impact.
/// Whether a hit Mario (at `mario`) just took came from the boss he holds or just threw: Mario has
/// him by the tail or he's flying, and no other enemy is within reach (the game doesn't say who
/// hit the player). Everyone else still hurts him.
pub fn harmless(mario: Vec3) -> bool {
    let boss = {
        let st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        match st.phase {
            Phase::Held { boss, .. } | Phase::Flying { boss, .. } | Phase::Limp { boss, .. } => boss,
            _ => return false,
        }
    };
    let Ok(wcm) = (unsafe { WorldChrMan::instance() }) else { return false };
    for set in wcm.chr_sets.iter().flatten() {
        for chr in set.characters() {
            let chr: &eldenring::cs::ChrIns = chr;
            if key(&chr.field_ins_handle) == key(&boss) || chr.modules.data.hp <= 0 || crate::combat::own_side(chr.team_type) {
                continue;
            }
            if !crate::combat::hittable(chr.chr_type) {
                continue;
            }
            let q = chr.modules.physics.position;
            if Vec3::new(q.0 - mario.x, q.1 - mario.y, q.2 - mario.z).length() <= 3.0 {
                return false;
            }
        }
    }
    true
}

pub fn busy_with(h: &FieldInsHandle) -> bool {
    let st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    matches!(st.phase, Phase::Held { boss, .. } | Phase::Flying { boss, .. } | Phase::Limp { boss, .. } | Phase::Down { boss, .. } | Phase::Settling { boss, .. } if key(&boss) == key(h))
}

/// Whether this boss is lying there after a throw (no damage then).
pub fn is_down(h: &FieldInsHandle) -> bool {
    let st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    matches!(st.phase, Phase::Down { boss, .. } | Phase::Flying { boss, .. } | Phase::Limp { boss, .. } | Phase::Returning { boss, .. } if key(&boss) == key(h))
}

/// Mario's stagger meter (% of full) fills by the hit's poise damage against the boss's own
/// poise, kept in this range so no boss breaks in one hit or never. It drains after a few
/// seconds without hits.
const STANCE_POISE: (f32, f32) = (100.0, 200.0);
const STANCE_HOLD: f32 = 4.0;
const STANCE_DRAIN: f32 = 15.0;
/// SOUND_OBJ_BOWSER_DEFEATED: the cue that the boss is open for the grab
pub const SOUND_STAGGER: i32 = 0x5006_0081;
/// Impact damage: share of the boss's max HP, from a slow throw to a full-speed one
const IMPACT_MIN: f32 = 8.0;
const IMPACT_MAX: f32 = 25.0;
const GRAVITY: f32 = 16.0;

enum Phase {
    Idle,
    /// Mario has him: (boss, arm's length in m)
    Held { boss: FieldInsHandle, reach: f32 },
    Flying { boss: FieldInsHandle, pos: Vec3, vel: Vec3, since: Instant, radius: f32 },
    /// knocked flat after the impact (his ragdoll), until he gets back up
    Down { boss: FieldInsHandle, until: Instant, safe: Option<Vec3> },
    /// landed without a ragdoll (big bosses): a moment of protection while the game settles him
    /// (he can stay in his falling animation, and the game's fall death would take him)
    Settling { boss: FieldInsHandle, until: Instant },
    /// brought back after a throw off the map: held there a moment (the ragdoll lets go, the
    /// game's out-of-the-world check is kept off)
    Returning { boss: FieldInsHandle, until: Instant },
    /// flying limp: his ragdoll carries the throw's speed (the physics moves him now); `peak` the
    /// fastest he went, `still` how long he's barely moved
    Limp { boss: FieldInsHandle, last: Vec3, peak: f32, since: Instant, still: f32, frames: u32, radius: f32 },
}

struct State {
    phase: Phase,
    /// how long the thrown boss has been falling without a break (s)
    falling: f32,
    /// bosses whose stance just broke: (handle, until)
    open: Vec<(FieldInsHandle, Instant)>,
    /// per boss: poise last frame (a break shows as a drop to zero or a reset to full)
    toughness: Vec<(u64, f32)>,
    /// per boss: Mario's stagger meter (%) and his last hit
    meter: Vec<(FieldInsHandle, f32, Instant)>,
    /// Mario's face angle last tick (the swing's speed)
    last_face: Option<f32>,
    spin: f32,
    /// the downed boss's last position and how long he's lain still
    rest: (Vec3, f32),
    /// the speed the boss went limp at (a ragdoll much faster than that is running away)
    throw_speed: f32,
    /// the thrown boss's HP floor while flying / lying (nothing but the throw's own impact may
    /// take more: the game's fall damage would kill him)
    guard_hp: i32,
    /// where he was thrown from (a throw off the map brings him back here, like SM64's Bowser)
    home: Vec3,
}

static STATE: Mutex<State> =
    Mutex::new(State { phase: Phase::Idle, falling: 0.0, open: Vec::new(), toughness: Vec::new(), meter: Vec::new(), last_face: None, spin: 0.0, rest: (Vec3::ZERO, 0.0), throw_speed: 0.0, guard_hp: 0, home: Vec3::ZERO });
/// the stagger cue to play (SM64 thread)
static CUE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn take_cue() -> bool {
    CUE.swap(false, std::sync::atomic::Ordering::Relaxed)
}

/// Mario hit a boss: his stagger meter fills; full, the boss is open for the grab (and his poise
/// is broken, so the game staggers him too).
pub fn add_stance(h: &FieldInsHandle, poise: f32) {
    // bosses without a poise value count as 100
    let max = toughness_of(h).map(|(_, max)| max).filter(|m| *m > 1.0).unwrap_or(100.0);
    let amount = poise / max.clamp(STANCE_POISE.0, STANCE_POISE.1) * 100.0;
    let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let k = key(h);
    if st.open.iter().any(|(o, _)| key(o) == k) {
        return;
    }
    let now = Instant::now();
    let i = match st.meter.iter().position(|(m, _, _)| key(m) == k) {
        Some(i) => i,
        None => {
            st.meter.push((*h, 0.0, now));
            st.meter.len() - 1
        }
    };
    let e = &mut st.meter[i];
    e.1 += amount;
    e.2 = now;
    log(format!("swing: stagger meter {:.0}%", e.1.min(100.0)));
    if e.1 >= 100.0 {
        st.meter.remove(i);
        st.open.push((*h, now + std::time::Duration::from_secs_f32(OPEN_FOR)));
        CUE.store(true, std::sync::atomic::Ordering::Relaxed);
        log("swing: boss staggered: grab him!");
        if let Some(chr) = unsafe { WorldChrMan::instance_mut() }.ok().and_then(|w| w.chr_ins_by_handle_mut(h)) {
            chr.modules.super_armor.sa_durability = 0.0;
        }
    }
}
/// the grab to start on the next SM64 tick
static START: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn key(h: &FieldInsHandle) -> u64 {
    unsafe { std::mem::transmute_copy::<FieldInsHandle, u64>(h) }
}

/// Every frame: watch the bosses' stance (those with a boss bar).
pub fn watch_stances(bosses: &[FieldInsHandle]) {
    let Ok(wcm) = (unsafe { WorldChrMan::instance() }) else { return };
    let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    st.open.retain(|(_, until)| Instant::now() < *until);
    // the stagger meters drain when Mario stops hitting
    for e in st.meter.iter_mut() {
        if e.2.elapsed().as_secs_f32() > STANCE_HOLD {
            e.1 = (e.1 - STANCE_DRAIN / 60.0).max(0.0);
        }
    }
    st.meter.retain(|e| e.1 > 0.0);
    // research: every boss animation change (to see whether knockdown / stagger IDs are shared)
    {
        static LAST: Mutex<Vec<(u64, i32)>> = Mutex::new(Vec::new());
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        for h in bosses {
            let Some(chr) = wcm.chr_ins_by_handle(h) else { continue };
            let t = &chr.modules.time_act;
            let anim = t.anim_queue[(t.read_idx % 10) as usize].anim_id;
            let k = key(h);
            match last.iter_mut().find(|(kk, _)| *kk == k) {
                Some(e) if e.1 == anim => {}
                Some(e) => {
                    crate::dlog(format!("boss anim: c{:04} {} -> {anim}", chr.character_id, e.1));
                    e.1 = anim;
                }
                None => last.push((k, anim)),
            }
        }
    }
    for h in bosses {
        let Some(chr) = wcm.chr_ins_by_handle(h) else { continue };
        // (enemies' stance is their poise: the toughness module is the player's)
        let t = &chr.modules.super_armor;
        let (now, max) = (t.sa_durability, t.sa_durability_max.max(1.0));
        let broken_flag = chr.modules.super_armor.poise_broken_state;
        let k = key(h);
        let last = st.toughness.iter().find(|(kk, _)| *kk == k).map(|(_, v)| *v);
        let broke = broken_flag || (now <= 0.0 && last.is_some_and(|l| l > 0.0)) || last.is_some_and(|l| l < max * 0.3 && now >= max * 0.95);
        match st.toughness.iter_mut().find(|(kk, _)| *kk == k) {
            Some(e) => e.1 = now,
            None => st.toughness.push((k, now)),
        }
        if broke && !st.open.iter().any(|(o, _)| key(o) == k) {
            log(format!("swing: boss stance broken (toughness {now:.0}/{max:.0}): grab him!"));
            st.open.push((*h, Instant::now() + std::time::Duration::from_secs_f32(OPEN_FOR)));
        }
    }
}

/// Poise of a character (for the combat log).
pub fn toughness_of(h: &FieldInsHandle) -> Option<(f32, f32)> {
    let wcm = unsafe { WorldChrMan::instance() }.ok()?;
    let chr = wcm.chr_ins_by_handle(h)?;
    Some((chr.modules.super_armor.sa_durability, chr.modules.super_armor.sa_durability_max))
}

/// A hit landed on `h`: if its stance is broken, Mario grabs it (returns true: no normal damage).
pub fn try_grab(h: &FieldInsHandle, radius_m: f32) -> bool {
    let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if !matches!(st.phase, Phase::Idle) {
        return false;
    }
    let k = key(h);
    let Some(i) = st.open.iter().position(|(o, _)| key(o) == k) else { return false };
    st.open.remove(i);
    st.phase = Phase::Held { boss: *h, reach: radius_m + 0.9 };
    st.last_face = None;
    st.spin = 0.0;
    START.store(true, std::sync::atomic::Ordering::Relaxed);
    log("swing: grabbed the boss by the tail");
    true
}

/// SM64 tick: whether to start the grab now (Mario goes into SM64's Bowser pickup).
pub fn take_start() -> bool {
    START.swap(false, std::sync::atomic::Ordering::Relaxed)
}

/// Every frame in Mario mode: carry, throw and fly the boss. `mario` feet (game coordinates),
/// `face` SM64 face angle, `action` Mario's SM64 action. Returns an impact: (boss, damage share
/// of max HP in %).
pub fn update(dt: f32, mario: Vec3, face: f32, action: u32, hit: impl Fn(Vec3, Vec3) -> Option<Vec3>) -> Option<(FieldInsHandle, f32)> {
    let Ok(wcm) = (unsafe { WorldChrMan::instance_mut() }) else { return None };
    let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    // Mario's forward in game coordinates (SM64 is mirrored on X)
    let fwd = Vec3::new(-face.sin(), 0.0, face.cos());
    // the swing's angular speed (rad/s) from Mario's turning
    if let Some(l) = st.last_face {
        let mut d = face - l;
        while d > std::f32::consts::PI {
            d -= std::f32::consts::TAU;
        }
        while d < -std::f32::consts::PI {
            d += std::f32::consts::TAU;
        }
        st.spin = st.spin * 0.7 + (d / dt.max(1e-3)) * 0.3;
    }
    st.last_face = Some(face);
    match st.phase {
        Phase::Idle => None,
        Phase::Down { boss, until, safe } => {
            let Some(chr) = wcm.chr_ins_by_handle_mut(&boss) else {
                st.phase = Phase::Idle;
                return None;
            };
            protect(chr, &mut st.guard_hp);
            // he gets back up only once he's lying still (switching a ragdoll off while it still
            // tumbles crashed the game): barely moving for half a second, at least DOWN_FOR after
            // landing, and after 8 s once he's merely slow
            let p = chr.modules.physics.position;
            let now = Vec3::new(p.0, p.1, p.2);
            let v = (now - st.rest.0).length() / dt.max(1e-3);
            st.rest = (now, if v < 0.5 { st.rest.1 + dt } else { 0.0 });
            if off_the_map(chr, st.home, 0.0) {
                bring_back(chr, st.home);
                st.phase = Phase::Returning { boss, until: Instant::now() + std::time::Duration::from_secs(2) };
                return None;
            }
            let left = until.saturating_duration_since(Instant::now()).as_secs_f32();
            let waited = DOWN_FOR + GET_UP - left;
            let at_rest = st.rest.1 > 0.5 || (waited > 8.0 && v < 3.0);
            if waited > 0.3 && v > RUNAWAY_DOWN {
                runaway(chr, v);
                st.phase = Phase::Settling { boss, until: Instant::now() + std::time::Duration::from_secs(2) };
            } else if left > 0.0 || !at_rest {
                set_ragdoll(chr, RAGDOLL_FULL);
            } else {
                set_ragdoll(chr, 0.0);
                if let Some(p) = safe {
                    // Only relocate after the ragdoll has settled: switching it
                    // off while tumbling is unsafe. Restore the near-side contact
                    // before giving control back to the boss AI.
                    chr.modules.physics.position = HavokPosition(p.x, p.y, p.z, 0.0);
                    chr.modules.physics.chr_proxy_pos_update_requested = true;
                    chr.modules.physics.gravity_disabled = false;
                    clear_fall(chr);
                }
                log("swing: boss back on his feet");
                st.phase = Phase::Idle;
            }
            None
        }
        Phase::Settling { boss, until } => {
            let Some(chr) = wcm.chr_ins_by_handle_mut(&boss) else {
                st.phase = Phase::Idle;
                return None;
            };
            protect(chr, &mut st.guard_hp);
            clear_fall(chr);
            // until he's back on the ground (he can hang in his falling animation for a while)
            let grounded = chr.modules.physics.is_touching_ground;
            let over = Instant::now() >= until;
            if (over && grounded) || over && Instant::now() >= until + std::time::Duration::from_secs(8) {
                if !grounded {
                    log("swing: boss still not on the ground 10 s after landing, protection ends");
                }
                st.phase = Phase::Idle;
                st.guard_hp = 0;
            }
            None
        }
        Phase::Returning { boss, until } => {
            let Some(chr) = wcm.chr_ins_by_handle_mut(&boss) else {
                st.phase = Phase::Idle;
                return None;
            };
            protect(chr, &mut st.guard_hp);
            set_ragdoll(chr, 0.0);
            hold_at(chr, st.home);
            if Instant::now() >= until {
                log("swing: boss back in the fight");
                st.phase = Phase::Idle;
            }
            None
        }
        Phase::Limp { boss, last, peak, since, still, frames, radius } => {
            let Some(chr) = wcm.chr_ins_by_handle_mut(&boss) else {
                st.phase = Phase::Idle;
                return None;
            };
            protect(chr, &mut st.guard_hp);
            let p = chr.modules.physics.position;
            let now = Vec3::new(p.0, p.1, p.2);
            let v = (now - last).length() / dt.max(1e-3);
            if frames < 6 {
                crate::dlog(format!("swing: limp frame {frames}: speed {v:.1} m/s"));
            }
            // (the first frames jump as the character snaps to the ragdoll's hips: not speed)
            let settle = frames < 3;
            // `peak` is his smoothed speed here
            let speed = if settle { peak } else { peak * 0.6 + v * 0.4 };
            let still = if v < 1.0 { still + dt } else { 0.0 };
            let age = since.elapsed().as_secs_f32();
            if !settle && v > (st.throw_speed * 1.4).max(RUNAWAY_MIN) {
                runaway(chr, v);
                st.phase = Phase::Settling { boss, until: Instant::now() + std::time::Duration::from_secs(2) };
                st.guard_hp = 0;
                return None;
            }
            // falling nonstop (a frame of real drop, not the ragdoll's jitter)
            st.falling = if now.y < last.y - 0.01 { st.falling + dt } else { 0.0 };
            if off_the_map(chr, st.home, st.falling) {
                bring_back(chr, st.home);
                st.phase = Phase::Returning { boss, until: Instant::now() + std::time::Duration::from_secs(2) };
                st.guard_hp = 0;
                return Some((boss, IMPACT_MAX));
            }
            // the impact: his speed collapses, he runs into the map, or he's come to rest
            let stopped = !settle && v < speed * 0.35;
            let contact = hit(last + Vec3::Y * 0.5, now + Vec3::Y * 0.5);
            let into_map = contact.is_some();
            if stopped || into_map || still > 0.3 || age > 4.0 {
                let pct = IMPACT_MIN + (IMPACT_MAX - IMPACT_MIN) * ((speed - 10.0) / 30.0).clamp(0.0, 1.0);
                log(format!("swing: limp boss landed at {speed:.1} m/s (stopped {stopped}, map {into_map}): {pct:.0}% of his HP"));
                let safe = contact.map(|h| {
                    let rest = Vec3::from_array(crate::throw_collision::stop_before_hit((last + Vec3::Y * 0.5).to_array(), h.to_array(), radius)) - Vec3::Y * 0.5;
                    Vec3::new(rest.x, rest.y.max(h.y), rest.z)
                });
                st.phase = Phase::Down { boss, until: Instant::now() + std::time::Duration::from_secs_f32(DOWN_FOR + GET_UP), safe };
                st.rest = (now, 0.0);
                // the guard takes his HP after this impact's damage as the new floor
                st.guard_hp = 0;
                return Some((boss, pct));
            }
            st.phase = Phase::Limp { boss, last: now, peak: speed, since, still, frames: frames + 1, radius };
            None
        }
        Phase::Held { boss, reach } => {
            let Some(chr) = wcm.chr_ins_by_handle_mut(&boss) else {
                st.phase = Phase::Idle;
                return None;
            };
            // (held off the ground: the game would count it as falling)
            clear_fall(chr);
            let holding = action == ACT_PICKING_UP_BOWSER || action == ACT_HOLDING_BOWSER;
            let released = action == ACT_RELEASING_BOWSER;
            let ph = &mut chr.modules.physics;
            if holding || START.load(std::sync::atomic::Ordering::Relaxed) {
                // at arm's length where Mario faces, a little off the ground, facing away (Bowser's
                // tail is in Mario's hands)
                let start = mario + Vec3::Y * 0.4;
                let radius = (reach - 0.9).max(0.4);
                let desired = start + fwd * reach;
                let p = hit(start, desired + fwd * radius)
                    .map(|h| Vec3::from_array(crate::throw_collision::stop_before_hit(start.to_array(), h.to_array(), radius)))
                    .unwrap_or(desired);
                ph.position = HavokPosition(p.x, p.y, p.z, 0.0);
                ph.chr_proxy_pos_update_requested = true;
                ph.gravity_disabled = true;
                let q = glam::Quat::from_rotation_y(std::f32::consts::PI - face);
                ph.orientation = eldenring::rotation::Quaternion(q.x, q.y, q.z, q.w);
                None
            } else if released {
                // thrown where Mario faces, as fast as he was swinging
                let speed = (st.spin.abs() * reach * 1.5).clamp(10.0, 40.0);
                let vel = fwd * speed + Vec3::Y * 9.0;
                let pos = Vec3::new(ph.position.0, ph.position.1, ph.position.2);
                log(format!("swing: thrown at {speed:.1} m/s"));
                st.guard_hp = 0;
                st.home = mario;
                let radius = (reach - 0.9).max(0.4);
                st.phase = Phase::Flying { boss, pos, vel, since: Instant::now(), radius };
                None
            } else {
                // let go some other way (hurt, fell): just drop him
                ph.gravity_disabled = false;
                log(format!("swing: let go without a throw (action {action:#x})"));
                st.phase = Phase::Idle;
                None
            }
        }
        Phase::Flying { boss, pos, vel, since, radius } => {
            let Some(chr) = wcm.chr_ins_by_handle_mut(&boss) else {
                st.phase = Phase::Idle;
                return None;
            };
            protect(chr, &mut st.guard_hp);
            let ph = &mut chr.modules.physics;
            let mut vel = vel;
            vel.y -= GRAVITY * dt;
            let next = pos + vel * dt;
            // his body's leading edge against the map: walls, rocks, the ground
            let lead = (next - pos).normalize_or_zero() * radius;
            let impact = hit(pos + Vec3::Y * 0.5, next + lead + Vec3::Y * 0.5).or_else(|| hit(pos + Vec3::Y * 0.5, next - Vec3::Y * 0.1));
            let speed = vel.length();
            match impact {
                Some(h) => {
                    let rest = Vec3::from_array(crate::throw_collision::stop_before_hit((pos + Vec3::Y * 0.5).to_array(), h.to_array(), radius)) - Vec3::Y * 0.5;
                    ph.position = HavokPosition(rest.x, rest.y.max(h.y), rest.z, 0.0);
                    ph.chr_proxy_pos_update_requested = true;
                    ph.gravity_disabled = false;
                    // lying there for a moment if he went limp, then back up (the game doesn't
                    // bring a ragdoll back by itself)
                    // (experiment, boss_ragdoll = on) he collapses where he hit: the game's
                    // blendable ragdoll (state 4), not its death ragdoll (state 2). Not in the air:
                    // the ragdoll's bodies don't get his flight's speed and would stretch him
                    if ragdoll_allowed(chr) {
                        set_ragdoll(chr, RAGDOLL_FULL);
                        log("swing: ragdoll on impact");
                    }
                    st.rest = (rest, 0.0);
                    st.phase = if chr.chr_ctrl.chr_ragdoll_state != 0 {
                        Phase::Down { boss, until: Instant::now() + std::time::Duration::from_secs_f32(DOWN_FOR + GET_UP), safe: Some(Vec3::new(rest.x, rest.y.max(h.y), rest.z)) }
                    } else {
                        Phase::Settling { boss, until: Instant::now() + std::time::Duration::from_secs(2) }
                    };
                    let pct = IMPACT_MIN + (IMPACT_MAX - IMPACT_MIN) * ((speed - 10.0) / 30.0).clamp(0.0, 1.0);
                    log(format!("swing: boss hit something at {speed:.1} m/s: {pct:.0}% of his HP"));
                    st.guard_hp = 0;
                    Some((boss, pct))
                }
                _ if since.elapsed().as_secs_f32() > 4.0 || next.y < st.home.y - 15.0 => {
                    let home = st.home;
                    bring_back(chr, home);
                    st.phase = Phase::Returning { boss, until: Instant::now() + std::time::Duration::from_secs(2) };
                    st.guard_hp = 0;
                    Some((boss, IMPACT_MAX))
                }
                _ if ragdoll_allowed(chr) && since.elapsed().as_secs_f32() > 0.1 => {
                    // a few frames of guided flight gave his ragdoll's bodies the throw's speed:
                    // limp from here, the physics flies him (gravity, collision)
                    set_ragdoll(chr, RAGDOLL_FULL);
                    chr.modules.physics.gravity_disabled = false;
                    log(format!("swing: limp flight at {speed:.1} m/s"));
                    st.throw_speed = speed;
                    st.falling = 0.0;
                    st.phase = Phase::Limp { boss, last: pos, peak: speed, since: Instant::now(), still: 0.0, frames: 0, radius };
                    None
                }
                _ => {
                    let ph = &mut chr.modules.physics;
                    ph.position = HavokPosition(next.x, next.y, next.z, 0.0);
                    ph.chr_proxy_pos_update_requested = true;
                    ph.gravity_disabled = true;
                    st.phase = Phase::Flying { boss, pos: next, vel, since, radius };
                    None
                }
            }
        }
    }
}

/// Mario died or left Mario mode: let go of the boss (his gravity back on).
pub fn reset() {
    let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let boss = match st.phase {
        Phase::Held { boss, .. } | Phase::Flying { boss, .. } | Phase::Down { boss, .. } | Phase::Limp { boss, .. } | Phase::Returning { boss, .. } | Phase::Settling { boss, .. } => Some(boss),
        Phase::Idle => None,
    };
    if let (Some(boss), Ok(wcm)) = (boss, unsafe { WorldChrMan::instance_mut() }) {
        if let Some(chr) = wcm.chr_ins_by_handle_mut(&boss) {
            chr.modules.physics.gravity_disabled = false;
            if chr.modules.data.hp > 0 {
                set_ragdoll(chr, 0.0);
            }
        }
    }
    st.phase = Phase::Idle;
    st.open.clear();
}
