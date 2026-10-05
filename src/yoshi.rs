//! Yoshi instead of Torrent. His model sits in Torrent's own (tools/build_yoshi.py for now), every
//! part bound to one horse bone; here his animations are read from the SM64 ROM and those bones
//! posed with them each frame, after the game's animation.

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use eldenring::cs::{ChrIns, WorldChrMan};
use fromsoftware_shared::FromStatic;
use glam::{Mat3, Quat, Vec3};

use crate::{combat, explore, log};

/// Horse bones of his mesh parts, in geo layout order (same list as the model builder).
const BONES: [&str; 17] = [
    "Pelvis", "Spine2", "Head", "Jaw", "L_UpperArm", "L_Forearm", "L_Hand", "R_UpperArm", "R_Forearm", "R_Hand", "L_Thigh", "L_Calf",
    "L_Foot", "Tail", "R_Thigh", "R_Calf", "R_Foot",
];
/// Horse bones between his torso's and those of his arms and head. They get the torso's pose:
/// left to the game, what hangs under them didn't stay where it was put.
const BETWEEN: [&str; 5] = ["L_Clavicle", "R_Clavicle", "Neck", "Neck1", "Neck2"];
const TORSO: usize = 1;
/// SM64 units -> metres, times the 0.25 scale node of his geo layout
const UNIT: f32 = 0.01 * 0.25;
/// On top of that: about his size next to Mario in SM64 (the bones carry it as their scale)
const SIZE: f32 = 1.1;
/// Torrent's saddle is where his back was at 1.6 times: Mario comes down by the difference.
pub const SEAT_DROP: f32 = 0.85 * (1.6 - SIZE);
/// How much faster than Torrent he goes
const RIDE_SPEED: f32 = 2.0;
/// SOUND_GENERAL_YOSHI_WALK
const SOUND_YOSHI_WALK: i32 = 0x306E_2081;
/// SOUND_GENERAL_YOSHI_TALK
const SOUND_YOSHI_TALK: i32 = 0x3070_3081;
/// Torrent's jump and his second one in the air
const JUMPS: [i32; 2] = [6130, 6131];
/// From this speed (m/s) he runs enemies over: his sprint is 20 to 23, a run around 13
const CHARGE_SPEED: f32 = 16.0;
const IDLE: usize = 0;
const WALK: usize = 1;
const JUMP: usize = 2;
/// The walk cycle's own pace (m/s at SIZE 1): faster rides play it faster.
const WALK_SPEED: f32 = 1.2;

struct Part {
    parent: Option<usize>,
    offset: Vec3,
    /// which of BONES carries its mesh
    slot: Option<usize>,
}

struct Anim {
    frames: i16,
    values: usize,
    index: usize,
}

struct Yoshi {
    seg: Vec<u8>,
    parts: Vec<Part>,
    anims: Vec<Anim>,
}

fn be16(d: &[u8], o: usize) -> Option<i16> {
    Some(i16::from_be_bytes(d.get(o..o + 2)?.try_into().ok()?))
}

fn be32(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

fn mio0(d: &[u8], o: usize) -> Option<Vec<u8>> {
    let (size, mut c, mut r) = (be32(d, o + 4)? as usize, o + be32(d, o + 8)? as usize, o + be32(d, o + 12)? as usize);
    let mut out = Vec::with_capacity(size);
    let mut bit = 0;
    while out.len() < size {
        if d.get(o + 16 + bit / 8)? & (0x80 >> (bit % 8)) != 0 {
            out.push(*d.get(r)?);
            r += 1;
        } else {
            let v = be16(d, c)? as u16 as usize;
            c += 2;
            let (n, back) = ((v >> 12) + 3, (v & 0xFFF) + 1);
            for _ in 0..n {
                out.push(*out.get(out.len().checked_sub(back)?)?);
            }
        }
        bit += 1;
    }
    Some(out)
}

/// A geo layout from its first command on: animated parts with their parent, offset and display
/// lists; None if it isn't Yoshi's (24 parts, an eye switch with two lists).
fn walk_layout(rom: &[u8], mut at: usize) -> Option<Vec<(Part, Vec<usize>)>> {
    let mut parts: Vec<(Part, Vec<usize>)> = Vec::new();
    let (mut stack, mut last, mut eyes) = (Vec::new(), None, 0);
    loop {
        match *rom.get(at)? {
            0x01 => return (eyes == 2 && parts.len() == 24).then_some(parts),
            0x04 => {
                stack.push(last);
                at += 4;
            }
            0x05 => {
                last = stack.pop()?;
                at += 4;
            }
            0x13 => {
                let t = [2, 4, 6].map(|k| be16(rom, at + k).unwrap_or(0) as f32);
                let dl = be32(rom, at + 8)? as usize & 0xFF_FFFF;
                parts.push((Part { parent: *stack.last()?, offset: Vec3::from(t), slot: None }, if dl != 0 { vec![dl] } else { Vec::new() }));
                last = Some(parts.len() - 1);
                at += 12;
            }
            0x15 => {
                // the eye switch's lists hang off the head; only the first (eyes open) counts
                if eyes == 0 {
                    let owner = (*stack.last()?)?;
                    parts.get_mut(owner)?.1.push(be32(rom, at + 4)? as usize & 0xFF_FFFF);
                }
                eyes += 1;
                at += 8;
            }
            0x0E | 0x16 | 0x1D => at += 8,
            _ => return None,
        }
        if parts.len() > 64 {
            return None;
        }
    }
}

fn load(rom: &[u8]) -> Option<Yoshi> {
    // his layout starts like a few others: a round shadow, then the quarter scale
    const START: [u8; 20] = [0x16, 0, 0, 1, 0, 0xC8, 0, 0x64, 4, 0, 0, 0, 0x1D, 0, 0, 0, 0, 0, 0x40, 0];
    let mut at = find(rom, &START, 0)?;
    let layout = loop {
        if let Some(l) = walk_layout(rom, at) {
            break l;
        }
        at = find(rom, &START, at + 4)?;
    };
    let lists: Vec<usize> = layout.iter().flat_map(|p| p.1.iter().copied()).collect();
    let top = *lists.iter().max()?;
    // the compressed block his display lists point into has his animations too
    let mut o = find(rom, b"MIO0", 0)?;
    let (seg, anims) = loop {
        let size = be32(rom, o + 4)? as usize;
        if o % 4 == 0 && top < size && size < 0x10_0000 {
            if let Some(seg) = mio0(rom, o) {
                let anims: Vec<Anim> = (0..size.saturating_sub(0x18))
                    .step_by(4)
                    .filter_map(|a| {
                        let (bones, vals, idx) = (be16(&seg, a + 10)?, be32(&seg, a + 12)? as usize, be32(&seg, a + 16)? as usize);
                        (bones == 24 && vals >> 24 == 5 && idx >> 24 == 5 && vals & 0xFF_FFFF < a && idx & 0xFF_FFFF < a).then(|| Anim {
                            frames: be16(&seg, a + 8).unwrap_or(1).max(1),
                            values: vals & 0xFF_FFFF,
                            index: idx & 0xFF_FFFF,
                        })
                    })
                    .collect();
                if anims.len() >= 2 && lists.iter().all(|&dl| matches!(seg.get(dl), Some(0x03 | 0x04 | 0x06 | 0xB6 | 0xB7 | 0xBB | 0xE7 | 0xFC | 0xFD))) {
                    break (seg, anims);
                }
            }
        }
        o = find(rom, b"MIO0", o + 4)?;
    };
    let mut slot = 0;
    let parts = layout
        .into_iter()
        .map(|(mut p, lists)| {
            if !lists.is_empty() {
                p.slot = Some(slot);
                slot += 1;
            }
            p
        })
        .collect();
    if slot != BONES.len() {
        return None;
    }
    Some(Yoshi { seg, parts, anims })
}

static YOSHI: OnceLock<Option<Yoshi>> = OnceLock::new();

/// Reads him from the ROM (on a thread of its own: a second or so of unpacking).
pub fn init() {
    std::thread::spawn(|| {
        let t = Instant::now();
        let y = crate::paths::read_rom().ok().and_then(|rom| load(&rom));
        match &y {
            Some(y) => log(format!("yoshi: {} parts, {} animations from the ROM ({:.1} s)", y.parts.len(), y.anims.len(), t.elapsed().as_secs_f32())),
            None => log("yoshi: not found in the ROM, Torrent's bones are left alone"),
        }
        let _ = YOSHI.set(y);
    });
}

impl Yoshi {
    fn value(&self, a: &Anim, slot: usize, frame: usize) -> f32 {
        let n = be16(&self.seg, a.index + slot * 4).unwrap_or(1) as u16 as usize;
        let first = be16(&self.seg, a.index + slot * 4 + 2).unwrap_or(0) as u16 as usize;
        be16(&self.seg, a.values + (first + frame.min(n.saturating_sub(1))) * 2).unwrap_or(0) as f32
    }

    /// One frame's part transforms in SM64 model space (his units).
    fn frame(&self, a: &Anim, frame: usize) -> Vec<(Vec3, Quat)> {
        let mut out: Vec<(Vec3, Quat)> = Vec::with_capacity(self.parts.len());
        for (k, p) in self.parts.iter().enumerate() {
            let slot = 3 + k * 3;
            let ang = [0, 1, 2].map(|i| self.value(a, slot + i, frame) / 32768.0 * std::f32::consts::PI);
            // SM64's mtxf_rotate_xyz_and_translate: Z * Y * X
            let rot = Quat::from_rotation_z(ang[2]) * Quat::from_rotation_y(ang[1]) * Quat::from_rotation_x(ang[0]);
            let mut t = p.offset;
            if k == 0 {
                t += Vec3::new(self.value(a, 0, frame), self.value(a, 1, frame), self.value(a, 2, frame));
            }
            out.push(match p.parent {
                Some(parent) => (out[parent].0 + out[parent].1 * t, (out[parent].1 * rot).normalize()),
                None => (t, rot),
            });
        }
        out
    }

    /// The mesh parts at time `at` (frames, blended between two), in the game's model space.
    fn pose(&self, anim: usize, at: f32) -> [(Vec3, Quat); 17] {
        let a = &self.anims[anim.min(self.anims.len() - 1)];
        let n = a.frames as usize;
        // (the jump is played once and held)
        let (f0, f1, t) = if anim == JUMP {
            (at.floor() as usize, at.floor() as usize + 1, at.fract())
        } else {
            (at.floor() as usize % n, (at.floor() as usize + 1) % n, at.fract())
        };
        let (p0, p1) = (self.frame(a, f0.min(n - 1)), self.frame(a, f1.min(n - 1)));
        let mut out = [(Vec3::ZERO, Quat::IDENTITY); 17];
        for (k, p) in self.parts.iter().enumerate() {
            let Some(slot) = p.slot else { continue };
            let pos = p0[k].0.lerp(p1[k].0, t) * UNIT * SIZE;
            let rot = p0[k].1.slerp(p1[k].1, t);
            // SM64 is mirrored on X
            let m = Mat3::from_quat(rot);
            let mirrored = Mat3::from_cols(
                Vec3::new(m.x_axis.x, -m.x_axis.y, -m.x_axis.z),
                Vec3::new(-m.y_axis.x, m.y_axis.y, m.y_axis.z),
                Vec3::new(-m.z_axis.x, m.z_axis.y, m.z_axis.z),
            );
            // ...and looks down +Z where the game's characters look down -Z: half a turn
            let turn = Quat::from_rotation_y(std::f32::consts::PI);
            out[slot] = (Vec3::new(pos.x, pos.y, -pos.z), (turn * Quat::from_mat3(&mirrored)).normalize());
        }
        out
    }
}

struct State {
    chr: usize,
    handle: eldenring::cs::FieldInsHandle,
    pose: [(Vec3, Quat); 17],
    anim: usize,
    at: f32,
    last: Instant,
    pos: Vec3,
    /// skeleton and BONES' indices in it
    bones: Option<(usize, [usize; 22])>,
    /// Torrent's own animation
    theirs: i32,
    /// his speed over the ground (m/s), a little smoothed, and whether Mario's on him
    vel: Vec3,
    ridden: bool,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// What he costs (debug): runs, time in all and the slowest run (ns) of tick [0] and apply [1].
static COST: [[std::sync::atomic::AtomicU64; 3]; 2] = [const { [const { std::sync::atomic::AtomicU64::new(0) }; 3] }; 2];

struct Timed(usize, Instant);

impl Drop for Timed {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering::Relaxed;
        let ns = self.1.elapsed().as_nanos() as u64;
        COST[self.0][0].fetch_add(1, Relaxed);
        COST[self.0][1].fetch_add(ns, Relaxed);
        COST[self.0][2].fetch_max(ns, Relaxed);
    }
}

fn report_cost() {
    use std::sync::atomic::Ordering::Relaxed;
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let since = last.map_or(0.0, |t| t.elapsed().as_secs_f32());
    if last.is_some() && since < 2.0 {
        return;
    }
    *last = Some(Instant::now());
    let read = |i: usize| [0, 1, 2].map(|k| COST[i][k].swap(0, Relaxed) as f32);
    let (tick, pose) = (read(0), read(1));
    if tick[0] > 0.0 && since > 0.0 {
        log(format!(
            "yoshi: cost per frame {:.3} ms (finding him {:.3} ms, slowest {:.2}; posing {:.3} ms in {:.1} writes, slowest {:.2})",
            (tick[1] + pose[1]) / tick[0] / 1e6,
            tick[1] / tick[0] / 1e6,
            tick[2] / 1e6,
            pose[1] / tick[0] / 1e6,
            pose[0] / tick[0],
            pose[2] / 1e6
        ));
    }
}

fn c_str(p: usize) -> String {
    let mut s = Vec::new();
    if p != 0 && explore::readable(p & !7, 72) {
        for k in 0..64 {
            match unsafe { *((p + k) as *const u8) } {
                0 => break,
                b => s.push(b),
            }
        }
    }
    String::from_utf8(s).unwrap_or_default()
}

fn map_bones(skeleton: usize) -> Option<[usize; 22]> {
    let names = explore::read_u64(skeleton + 0x30)? as usize;
    let count = (explore::read_u64(skeleton + 0x38)? as u32 as usize).min(1024);
    if names == 0 || !explore::readable(names, count * 16) {
        return None;
    }
    let all: Vec<String> = (0..count).map(|i| c_str(unsafe { *((names + i * 16) as *const usize) } & !1)).collect();
    let mut out = [0; 22];
    for (slot, name) in BONES.iter().chain(BETWEEN.iter()).enumerate() {
        out[slot] = all.iter().position(|n| n == name)?;
    }
    log(format!("yoshi: Torrent's skeleton has {count} bones, his go on {out:?}"));
    Some(out)
}

static CALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Mario whistles for him: his voice is what's heard (played from the frame task, the input
/// task shouldn't wait for the SM64 thread).
pub fn call() {
    CALLED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Once a frame: finds Torrent and moves the animation on (idle standing, the walk cycle at the
/// pace he's going).
pub fn tick() {
    let Some(Some(yoshi)) = YOSHI.get() else { return };
    if crate::debug() {
        report_cost();
    }
    let _timed = Timed(0, Instant::now());
    if CALLED.swap(false, std::sync::atomic::Ordering::Relaxed) {
        crate::worker::call("yoshi voice", |_| unsafe { crate::sm64::sm64_play_sound_global(SOUND_YOSHI_TALK) });
    }
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let torrent = (unsafe { WorldChrMan::instance_mut() }).ok().and_then(|wcm| {
        let found: Option<&mut ChrIns> =
            wcm.summon_buddy_chr_set.characters().chain(wcm.chr_sets.iter().flatten().flat_map(|set| set.characters())).find(|c| combat::is_torrent(c));
        // ridden he's quicker than Torrent: his animations play faster, and cover more ground
        // with it (not while Mario gets on or off, those are played in step with the rider's)
        let mounted = wcm.main_player.as_ref().is_some_and(|p| p.chr_ins.modules.ride.is_mounted);
        found.map(|c| {
            // (and not in the air: a jump at that speed is over before it has begun)
            let t = &c.modules.time_act;
            let theirs = t.anim_queue[(t.read_idx % 10) as usize].anim_id;
            let airborne = !c.modules.physics.touching_solid_ground || JUMPS.contains(&theirs);
            c.modules.behavior.animation_speed = if mounted && !airborne { RIDE_SPEED } else { 1.0 };
            // no hooves or rattling tack on Yoshi: the game's switch for a character's sounds
            // (it turns them on by distance) and the mimic veil's, which mutes steps
            // nothing lands on him: no damage, and no hit to break his stride (twice a second
            // wasn't enough for the health, torrent_cant_die: a hard hit still killed him)
            c.debug_flags.set_disabled_hit(true);
            c.chr_flags1c5.set_is_invincible(true);
            if c.modules.data.hp > 0 {
                c.modules.data.hp = c.modules.data.max_hp;
            }
            // (hits on him landed all the same, with a flinch each and his end after a few:
            // the dodge frames' switch, poise no hit gets through, and the health the game keeps
            // for the mount on the player's side, which is the one it goes by)
            c.modules.action_flag.action_modifiers_flags.set_perfect_invincibility(true);
            c.modules.super_armor.sa_durability = 1.0e6;
            c.modules.toughness.toughness = 1.0e6;
            if let Ok(gdm) = unsafe { eldenring::cs::GameDataMan::instance() } {
                let ride = unsafe { *((gdm.main_player_game_data.as_ptr() as usize + 0x8e0) as *const usize) };
                if ride != 0 && explore::readable(ride, 0x40) {
                    let hp = (ride + 0x30) as *mut u32;
                    let (now, full) = (unsafe { *hp }, c.modules.data.max_hp.max(1) as u32);
                    if now != 0 && now < full {
                        if crate::debug() && full - now > 20 {
                            log(format!("yoshi: the mount's health was at {now} of {full}"));
                        }
                        unsafe { *hp = full };
                    }
                }
            }
            c.chr_flags1ca.set_sounds_active(false);
            c.chr_flags1c7.set_mimicry_enabled(true);
            if crate::debug() {
                static LAST: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);
                if LAST.swap(theirs, std::sync::atomic::Ordering::Relaxed) != theirs {
                    log(format!("yoshi: Torrent anim -> {theirs} (airborne {airborne})"));
                }
            }
            (c as *mut ChrIns as usize, c.field_ins_handle.clone(), c.modules.physics.position, airborne, theirs, mounted)
        })
    });
    let Some((chr, handle, p, airborne, theirs, ridden)) = torrent else {
        *state = None;
        return;
    };
    let pos = Vec3::new(p.0, p.1, p.2);
    let now = Instant::now();
    let s = state.get_or_insert_with(|| State { chr, handle: handle.clone(), pose: yoshi.pose(IDLE, 0.0), anim: IDLE, at: 0.0, last: now, pos, bones: None, theirs, vel: Vec3::ZERO, ridden: false });
    if s.chr != chr {
        (s.chr, s.handle, s.bones, s.pos) = (chr, handle, None, pos);
    }
    let dt = now.duration_since(s.last).as_secs_f32().min(0.1);
    s.last = now;
    let step = Vec3::new(pos.x - s.pos.x, 0.0, pos.z - s.pos.z);
    let moved = step.length();
    s.pos = pos;
    // (a jump of the world's origin isn't a gallop)
    let speed = if dt > 0.0 && moved < 5.0 { moved / dt } else { 0.0 };
    if dt > 0.0 && moved < 5.0 {
        s.vel = s.vel.lerp(step / dt, 0.3);
    }
    s.ridden = ridden;
    if crate::debug() && ridden {
        static LAST: Mutex<Option<Instant>> = Mutex::new(None);
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_none_or(|t| t.elapsed().as_secs_f32() > 1.0) && s.vel.length() > 1.0 {
            *last = Some(now);
            log(format!("yoshi: going {:.1} m/s", s.vel.length()));
        }
    }
    let anim = if airborne && yoshi.anims.len() > JUMP {
        JUMP
    } else if speed > 0.5 {
        WALK
    } else {
        IDLE
    };
    // (from the start again for the second jump in the air)
    let jumped = theirs != s.theirs && JUMPS.contains(&theirs);
    s.theirs = theirs;
    if anim != s.anim || jumped {
        (s.anim, s.at) = (anim, 0.0);
    }
    let rate = if anim == WALK { (speed / (WALK_SPEED * SIZE)).clamp(0.5, 4.0) } else { 1.0 };
    let before = s.at;
    s.at += dt * 30.0 * rate;
    // his steps, on the two frames of the walk SM64 plays them on
    if anim == WALK && [0.0, 15.0].iter().any(|f| ((before - f) / 30.0).floor() != ((s.at - f) / 30.0).floor()) {
        crate::worker::call("yoshi step", |_| unsafe { crate::sm64::sm64_play_sound_global(SOUND_YOSHI_WALK) });
    }
    s.pose = yoshi.pose(anim, s.at);
}

/// At full speed with Mario on him: where he is and his velocity (trample.rs).
pub fn charge() -> Option<(Vec3, Vec3)> {
    let state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let s = state.as_ref()?;
    (s.ridden && s.last.elapsed().as_secs_f32() < 0.1 && s.vel.length() > CHARGE_SPEED).then_some((s.pos, s.vel))
}

/// Puts his pose on Torrent's bones (after the game's animation, in every pose task).
pub fn apply() {
    let _timed = Timed(1, Instant::now());
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(s) = state.as_mut() else { return };
    if s.last.elapsed().as_secs_f32() > 0.1 {
        return;
    }
    // Asked from the game again every time: he can be taken out of the world between the frame
    // task that found him and this one (riding fast through loading areas), and a write to his
    // old pose buffers then lands in freed memory.
    let Ok(wcm) = (unsafe { WorldChrMan::instance_mut() }) else { return };
    let live = match wcm.summon_buddy_chr_set.chr_ins_by_handle_mut(&s.handle) {
        Some(c) => Some(c as *mut ChrIns as usize),
        None => wcm.chr_ins_by_handle_mut(&s.handle).map(|c| c as *mut ChrIns as usize),
    };
    if live != Some(s.chr) {
        return;
    }
    let raw = |a: usize| unsafe { *(a as *const usize) };
    // ChrIns +0x398 pose importer: +0x48 skeleton, +0x50 local / +0x60 model pose, 0x30 a bone
    let imp = raw(s.chr + 0x398);
    if imp == 0 {
        return;
    }
    let (skeleton, local, model, count) = (raw(imp + 0x48), raw(imp + 0x50), raw(imp + 0x60), raw(imp + 0x68) as u32 as usize);
    if s.bones.is_none_or(|b| b.0 != skeleton) {
        let ok = explore::readable(imp, 0x70) && count <= 1024 && explore::readable(model, count * 0x30) && explore::readable(local, count * 0x30);
        s.bones = ok.then(|| map_bones(skeleton)).flatten().map(|b| (skeleton, b));
    }
    let Some((_, bones)) = s.bones else { return };
    let parents = raw(skeleton + 0x20);
    type Qs = (Vec3, Quat);
    let read = |b: usize| -> Qs {
        let v = unsafe { *((local + b * 0x30) as *const [f32; 12]) };
        (Vec3::new(v[0], v[1], v[2]), Quat::from_xyzw(v[4], v[5], v[6], v[7]).normalize())
    };
    let write = |base: usize, b: usize, t: Qs, s: f32| unsafe {
        *((base + b * 0x30) as *mut [f32; 12]) = [t.0.x, t.0.y, t.0.z, 0.0, t.1.x, t.1.y, t.1.z, t.1.w, s, s, s, 0.0];
    };
    // Every bone's place worked out from the local poses, top down (the model pose buffer isn't
    // up to date for all of them at this point: a local pose set against a stale parent sent
    // his arms and head off when the game brought it up to date).
    let n = count.min(512);
    let mut slot_of = [usize::MAX; 512];
    for (i, &b) in bones.iter().enumerate() {
        if b < n {
            slot_of[b] = i;
        }
    }
    let mut world: Vec<Qs> = Vec::with_capacity(n);
    for b in 0..n {
        let parent = unsafe { *((parents + b * 2) as *const i16) };
        let above: Qs = if parent >= 0 && (parent as usize) < b { world[parent as usize] } else { (Vec3::ZERO, Quat::IDENTITY) };
        let i = slot_of[b];
        if i == usize::MAX {
            let l = read(b);
            world.push((above.0 + above.1 * l.0, (above.1 * l.1).normalize()));
            continue;
        }
        let want = s.pose[if i < BONES.len() { i } else { TORSO }];
        // (the game multiplies the local scales down the chain: under one of his own bones the
        // size is already there, and the offset is in that bone's bigger units)
        let grown = parent >= 0 && slot_of[parent as usize] != usize::MAX;
        let inv = above.1.inverse();
        let offset = inv * (want.0 - above.0) / if grown { SIZE } else { 1.0 };
        write(local, b, (offset, (inv * want.1).normalize()), if grown { 1.0 } else { SIZE });
        write(model, b, want, SIZE);
        world.push(want);
    }
}
