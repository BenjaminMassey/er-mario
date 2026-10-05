//! Mario's attacks hurt Elden Ring characters, and their hits knock Mario back.
//!
//! Each SM64 tick, living characters near Mario are tested with libsm64's own interaction check
//! (`sm64_mario_attack`: punch/kick cones, sweep kick, dive, slide kick, ground pound, stomping from
//! above, with SM64's bounce and hit sound). A hit takes a fixed share of the target's max HP (much
//! less on bosses), the same at every level, so levelling doesn't make Mario stronger; then a small
//! game bullet on the target brings the stagger, blood, hit sounds and the final blow (so deaths,
//! rune drops and kill credit stay the game's own). Bullets 65-70 (unused dev bullets with attack
//! params 994-999) are repurposed at runtime as Mario's attacks.

use std::collections::HashMap;

use eldenring::cs::{
    AtkParam_Pc, Bullet, CSBulletManager, ChrIns, ChrType, FieldInsHandle, SoloParamRepository, WorldChrMan,
};
use eldenring::position::HavokPosition;
use fromsoftware_shared::{F32Vector4, FromStatic};

use crate::{log, sm64};

/// (bullet row, attack row, damage (flat, and % of the weapon's attack rating), poise damage,
/// hit reaction level, hit radius m)
#[derive(Clone, Copy)]
pub enum Attack {
    Punch,
    Kick,
    Sweep,
    GroundPound,
    Stomp,
    Dash,
}

impl Attack {
    fn spec(self) -> (i32, u32, u16, f32, u8, f32) {
        match self {
            // damage: % of the (fist) weapon's attack rating, flat value too
            // poise: a boss with up to 100 poise takes 5 ground pounds
            Attack::Punch => (65, 994, 150, 9.0, 1, 0.7),
            Attack::Kick => (66, 995, 200, 11.0, 1, 0.7),
            Attack::Sweep => (67, 996, 150, 14.0, 2, 0.9),
            Attack::GroundPound => (68, 997, 350, 20.0, 3, 1.5),
            Attack::Stomp => (69, 998, 250, 14.0, 2, 0.8),
            Attack::Dash => (70, 999, 175, 14.0, 2, 0.8),
        }
    }

    /// Share of a normal enemy's max HP per hit (%), and its er_mario.ini key.
    fn percent(self) -> (f32, &'static str) {
        match self {
            Attack::Punch => (25.0, "damage_punch"),
            Attack::Kick => (34.0, "damage_kick"),
            Attack::Sweep => (34.0, "damage_sweep"),
            Attack::Dash => (34.0, "damage_dive"),
            Attack::Stomp => (50.0, "damage_stomp"),
            Attack::GroundPound => (67.0, "damage_ground_pound"),
        }
    }

    /// On bosses the quick hits count for a bit more than their share: with a twentieth of it
    /// each, only ground pounds and stomps were worth doing.
    fn boss_bonus(self) -> f32 {
        match self {
            Attack::Punch | Attack::Kick | Attack::Sweep => 1.2,
            Attack::Dash => 1.1,
            Attack::Stomp | Attack::GroundPound => 1.0,
        }
    }

    const ALL: [Attack; 6] = [Attack::Punch, Attack::Kick, Attack::Sweep, Attack::GroundPound, Attack::Stomp, Attack::Dash];
}

// SM64 action and flag bits
const ACT_FLAG_AIR: u32 = 0x0000_0800;
const MARIO_PUNCHING: u32 = 0x0010_0000;
const MARIO_KICKING: u32 = 0x0020_0000;
const MARIO_TRIPPING: u32 = 0x0040_0000;
const ACT_GROUND_POUND: u32 = 0x008008A9;
const ACT_GROUND_POUND_LAND: u32 = 0x0080023C;

/// What kind of hit Mario's current state makes (after libsm64 said it is one).
fn classify(state: &sm64::SM64MarioState) -> Attack {
    let a = state.action;
    if state.flags & MARIO_PUNCHING != 0 {
        Attack::Punch
    } else if state.flags & MARIO_KICKING != 0 {
        Attack::Kick
    } else if state.flags & MARIO_TRIPPING != 0 {
        Attack::Sweep
    } else if a == ACT_GROUND_POUND || a == ACT_GROUND_POUND_LAND {
        Attack::GroundPound
    } else if a & ACT_FLAG_AIR != 0 && state.velocity[1] < 0.0 {
        Attack::Stomp
    } else {
        Attack::Dash
    }
}

/// Mirror of the game's bullet spawn request (fields are private in the bindings).
#[repr(C)]
struct SpawnRequest {
    owner: FieldInsHandle,
    behavior_id: i32,
    magic_id: i32,
    unk10: u32,
    bullet_id: i32,
    goods_id: i32,
    dummy_poly_id: i32,
    target: [u8; 8],
    unk28: u32,
    unk2c: u32,
    unk30: F32Vector4,
    unk40: u32,
    unk44: u32,
    pad48: [u8; 8],
    acceleration_angle: F32Vector4,
    unk60: F32Vector4,
    angle: F32Vector4,
    position: F32Vector4,
    rest: [u8; 0x80],
}
const _: () = assert!(size_of::<SpawnRequest>() == 0x110);

/// The bullets' own (flat) damage: just enough to always land the final blow.
const BULLET_DAMAGE: u16 = 10;
/// Share of the normal damage bosses (anything with a boss health bar) take.
const BOSS_FACTOR: f32 = 0.05;
const INVADER_FACTOR: f32 = 0.25;

/// Team types on the player's side (the player, co-op phantoms, summons and spirit ashes): Mario
/// doesn't hurt those. Everyone else can be hit, friendly NPCs included, like with a weapon.
pub fn own_side(team: u8) -> bool {
    matches!(team, 1 | 2 | 5 | 12)
}

/// Bosses and other strong enemies (team 7), also before their health bar shows.
const TEAM_STRONG_ENEMY: u8 = 7;

fn config_f32(key: &str, default: f32) -> f32 {
    crate::paths::config(key).and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// Patches the six bullets/attacks into Mario's moves (once the params are loaded).
fn patch_params() -> bool {
    let Ok(repo) = (unsafe { SoloParamRepository::instance_mut() }) else { return false };
    for attack in Attack::ALL {
        let (bullet, atk, damage, poise, level, radius) = attack.spec();
        let Some(b) = repo.get_mut::<Bullet>(bullet as u32) else { return false };
        b.set_atk_id_bullet(atk as i32);
        b.set_life(0.1);
        b.set_dist(0.0);
        b.set_init_vellocity(0.0);
        b.set_max_vellocity(0.0);
        b.set_min_vellocity(0.0);
        b.set_accel_in_range(0.0);
        b.set_accel_out_range(0.0);
        b.set_gravity_in_range(0.0);
        b.set_gravity_out_range(0.0);
        b.set_hit_radius(radius);
        b.set_hit_radius_max(radius);
        b.set_spread_time(0.0);
        b.set_num_shoot(1);
        b.set_homing_angle(0);
        b.set_is_penetrate_chr(true);
        b.set_is_penetrate_obj(true);
        b.set_is_penetrate_map(true);
        b.set_is_hit_both_team(false);
        let Some(a) = repo.get_mut::<AtkParam_Pc>(atk) else { return false };
        // the real damage is a share of the target's max HP (Combat::deal); the bullet itself only
        // chips (and lands the final blow), whatever the weapon and stats
        let _ = damage;
        a.set_atk_phys_correction(0);
        a.set_atk_mag_correction(0);
        a.set_atk_fire_correction(0);
        a.set_atk_thun_correction(0);
        a.set_atk_stam_correction(100);
        a.set_guard_atk_rate_correction(100);
        a.set_guard_break_correction(100);
        a.set_atk_phys(BULLET_DAMAGE);
        a.set_atk_obj(300); // breaks barrels, crates and other destructible objects
        a.set_atk_mag(0);
        a.set_atk_fire(0);
        a.set_atk_thun(0);
        a.set_atk_stam(30);
        // a bit under the meter's share, so an 80 poise boss doesn't break a hit early
        a.set_atk_super_armor(poise * 0.8);
        a.set_dmg_level(level);
        a.set_atk_attribute(1); // strike
        a.set_is_add_base_atk(false);
        a.set_oppose_target(true);
        a.set_friendly_target(false);
        a.set_self_target(false);
    }
    log("combat: Mario's attacks patched into bullets 65-70");
    true
}

pub struct Combat {
    patched: bool,
    /// per target: SM64 tick of the last hit (one hit per swing)
    cooldown: HashMap<u64, u32>,
    last_hp: Option<i32>,
    /// characters Mario brought to 1 HP: SM64 tick, for the fallback kill if the bullet misses
    finishing: HashMap<u64, (FieldInsHandle, u32)>,
    /// characters Mario hit recently (their deaths heal him)
    victims: HashMap<u64, (FieldInsHandle, u32)>,
    /// small HP losses (poison and co.) add up here, a wedge per 10% of max HP
    drain: f32,
    /// stomps per enemy since Mario last stood on the ground
    stomps: HashMap<u64, u32>,
}

/// What an Elden Ring hit does to Mario.
pub enum Hurt {
    /// wedges, with SM64's knockback
    Hit(u32),
    /// one wedge, no knockback
    Drain,
}

impl Combat {
    pub fn new() -> Self {
        Combat { patched: false, cooldown: HashMap::new(), last_hp: None, finishing: HashMap::new(), victims: HashMap::new(), drain: 0.0, stomps: HashMap::new() }
    }
}

/// A living character near Mario, in SM64 coordinates.
pub struct Target {
    key: u64,
    /// characters: their handle (props have none)
    handle: Option<FieldInsHandle>,
    /// body size in SM64 units (horizontal radius, height)
    pub radius: f32,
    pub height: f32,
    er: HavokPosition,
    pub sm: [f32; 3],
}

impl Target {
    /// A breakable prop (crate, barrel, clutter), not a character.
    pub fn is_prop(&self) -> bool {
        self.handle.is_none()
    }
}

fn handle_key(h: &FieldInsHandle) -> u64 {
    unsafe { std::mem::transmute_copy::<FieldInsHandle, u64>(h) }
}

/// Enemies and NPCs, the red NPC invaders included; some bosses (Margit) are type 7. Never the
/// player, phantoms or the ghost kinds (bloodstains, messages, graces).
pub fn hittable(t: ChrType) -> bool {
    matches!(t, ChrType::Npc | ChrType::Unk6 | ChrType::Unk7 | ChrType::Unk9 | ChrType::Unk12 | ChrType::BloodyFingerNpc | ChrType::RecusantNpc)
}

/// Torrent: the mount the game has down for the player (his model id is no help, c8002 here
/// where c8000 was expected).
pub fn is_torrent(chr: &ChrIns) -> bool {
    let Ok(gdm) = (unsafe { eldenring::cs::GameDataMan::instance() }) else { return false };
    handle_key(&gdm.main_player_game_data.mount_handle) == handle_key(&chr.field_ins_handle)
}

/// A body wider or taller than this (m) is a big enemy: a man is about 0.4 by 1.8
const BIG_WIDTH: f32 = 0.9;
const BIG_HEIGHT: f32 = 2.8;

/// Enemies within `reach` of a point, their own width added: handle, key, and whether it's a
/// boss or one of the strong ones.
pub fn in_the_way(at: glam::Vec3, reach: f32) -> Vec<(FieldInsHandle, u64, bool)> {
    let Ok(wcm) = (unsafe { WorldChrMan::instance() }) else { return Vec::new() };
    let mut out = Vec::new();
    for set in wcm.chr_sets.iter().flatten() {
        for chr in set.characters() {
            let chr: &ChrIns = chr;
            if !hittable(chr.chr_type) || chr.modules.data.hp <= 0 || chr.team_type == 0 || own_side(chr.team_type) {
                continue;
            }
            let (p, ph) = (chr.modules.physics.position, &chr.modules.physics);
            let width = ph.hit_radius.max(ph.chr_hit_radius).clamp(0.3, 4.0);
            let (dx, dy, dz) = (p.0 - at.x, p.1 - at.y, p.2 - at.z);
            if dx * dx + dz * dz > (reach + width).powi(2) || !(-3.0..2.0).contains(&dy) || is_torrent(chr) {
                continue;
            }
            // (trolls, dragons and the like count with the bosses: by the size of their body)
            let height = ph.hit_height.max(ph.chr_hit_height);
            let big = width > BIG_WIDTH || height > BIG_HEIGHT;
            let handle = chr.field_ins_handle;
            out.push((handle, handle_key(&handle), big || is_boss(&handle) || chr.team_type == TEAM_STRONG_ENEMY));
        }
    }
    out
}

/// Enemies whose hits count for less against Mario: character id (cXXXX) and how much of the
/// hit is left. The Godskin Noble (c3550, the fat one with the rapier) took three wedges a hit.
const SOFTER: [(u32, f32); 1] = [(3550, 0.6)];

/// Who hit the player last (character id), and what's left of his hits (1 = all of it).
pub fn last_attacker(player: &ChrIns) -> (Option<u32>, f32) {
    let id = unsafe { WorldChrMan::instance() }.ok().and_then(|wcm| wcm.chr_ins_by_handle(&player.last_hit_by)).map(|c| c.character_id);
    (id, id.and_then(|id| SOFTER.iter().find(|s| s.0 == id)).map_or(1.0, |s| s.1))
}

/// Characters within `range` metres of `center` (not the player, alive).
pub fn nearby(center: &HavokPosition, range: f32, origin: [f32; 3]) -> Vec<Target> {
    let Ok(wcm) = (unsafe { WorldChrMan::instance() }) else { return Vec::new() };
    let mut out = Vec::new();
    for set in wcm.chr_sets.iter().flatten() {
        for chr in set.characters() {
            let chr: &ChrIns = chr;
            if !hittable(chr.chr_type) {
                // diagnostics: what else is near Mario (once each)
                let p = chr.modules.physics.position;
                let (dx, dz) = (p.0 - center.0, p.2 - center.2);
                if dx * dx + dz * dz < range * range && !matches!(chr.chr_type, ChrType::Local) {
                    static SKIPPED: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());
                    let key = handle_key(&chr.field_ins_handle);
                    let mut skipped = SKIPPED.lock().unwrap_or_else(|e| e.into_inner());
                    if !skipped.contains(&key) {
                        skipped.push(key);
                        log(format!("combat: skipped character of type {:?}, team {}, hp {}", chr.chr_type, chr.team_type, chr.modules.data.hp));
                    }
                }
                continue;
            }
            // (team 0 belongs to no side: the game's invisible helpers, e.g. one at every grace with
            // ~1900 HP and a 0.1 m body, which Mario could punch to death)
            if chr.modules.data.hp <= 0 || chr.team_type == 0 || is_torrent(chr) {
                continue;
            }
            let p = chr.modules.physics.position;
            let (dx, dy, dz) = (p.0 - center.0, p.1 - center.1, p.2 - center.2);
            if dx * dx + dz * dz > range * range || dy.abs() > range {
                continue;
            }
            // the character's own body size (bosses like Margit are far bigger than a human)
            let ph = &chr.modules.physics;
            let r = ph.hit_radius.max(ph.chr_hit_radius);
            let h = ph.hit_height.max(ph.chr_hit_height);
            // (up to giant size: capped at 3 m the Fire Giant's ankles were outside his own body)
            let radius = if r.is_finite() && r > 0.1 { (r * 100.0).clamp(40.0, 1500.0) } else { TARGET_RADIUS };
            let height = if h.is_finite() && h > 0.3 { (h * 100.0).clamp(80.0, 4000.0) } else { TARGET_HEIGHT };
            {
                static SEEN: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());
                let key = handle_key(&chr.field_ins_handle);
                let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
                if !seen.contains(&key) {
                    seen.push(key);
                    if seen.len() > 512 {
                        seen.remove(0);
                    }
                    log(format!("combat: target size {radius:.0} x {height:.0} units (hit {r:.2}/{h:.2} m)"));
                }
            }
            out.push(Target {
                key: handle_key(&chr.field_ins_handle),
                handle: Some(chr.field_ins_handle.clone()),
                radius,
                height,
                // the bullet spawns 1 m above this: 0.4 m above the feet reaches sheep and dogs too
                er: HavokPosition(p.0, p.1 - 0.6, p.2, 0.0),
                sm: crate::collision::er_to_sm(origin, &p),
            });
        }
    }
    // breakable props (barrels, crates...)
    for (index, p, layer) in crate::havok_col::props_near(glam::Vec3::new(center.0, center.1, center.2), range) {
        let lift = if layer == 0x1e { 0.4 } else { 0.0 };
        let base = HavokPosition(p.x, p.y - lift, p.z, 0.0);
        out.push(Target {
            key: (1 << 63) | index as u64,
            handle: None,
            // props are solid to Mario (his wall radius keeps him ~90 units from a barrel's
            // centre), so punches and dives need the extra reach
            radius: 80.0,
            height: 90.0,
            sm: crate::collision::er_to_sm(origin, &base),
            er: HavokPosition(p.x, p.y - lift - 0.6, p.z, 0.0),
        });
    }
    out
}

/// Whether Mario can pick this character up (a regular enemy: not a boss, not on his side).
fn liftable(handle: &FieldInsHandle) -> bool {
    if is_boss(handle) {
        return false;
    }
    let Ok(wcm) = (unsafe { WorldChrMan::instance() }) else { return false };
    wcm.chr_ins_by_handle(handle).is_some_and(|c| !own_side(c.team_type) && !boss_class(handle, c))
}

/// Characters that have shown a boss bar at some point (the bar only appears once a fight starts).
static SEEN_BOSSES: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());
/// Big or tough characters count as bosses for damage even before (or without) a boss bar.
const BOSS_RADIUS: f32 = 1.5;
const BOSS_MAX_HP: i32 = 2500;

/// Whether a character takes boss damage: a boss bar now or before, the strong-enemy team, or
/// big / tough enough (a field boss hit before his bar showed took a regular enemy's 50%).
fn boss_class(handle: &FieldInsHandle, chr: &ChrIns) -> bool {
    is_boss(handle)
        || chr.team_type == TEAM_STRONG_ENEMY
        || SEEN_BOSSES.lock().unwrap_or_else(|e| e.into_inner()).contains(&handle_key(handle))
        || chr.modules.physics.hit_radius >= BOSS_RADIUS
        || chr.modules.data.max_hp >= BOSS_MAX_HP
}

/// Whether a character is a boss right now (its health bar is on screen).
fn is_boss(handle: &FieldInsHandle) -> bool {
    let key = handle_key(handle);
    unsafe { eldenring::cs::CSFeManImp::instance() }
        .is_ok_and(|fe| fe.boss_health_displays.iter().any(|e| !e.field_ins_handle.is_empty() && handle_key(&e.field_ins_handle) == key))
}

/// Characters Mario hit recently: their health bar and damage number (the overlay draws them;
/// hud.rs). Boss damage goes on the game's boss bar.
struct Tag {
    handle: FieldInsHandle,
    hit: std::time::Instant,
    /// damage in the current combo, and the HP before it
    dmg: i32,
    before: i32,
}

static TAGS: std::sync::Mutex<Vec<Tag>> = std::sync::Mutex::new(Vec::new());
/// a combo ends (the damage number resets) this long after the last hit; the bar goes 1.5 s later
const TAG_COMBO: f32 = 1.5;

fn show_damage(handle: &FieldInsHandle, before: i32, dmg: i32, _boss_bar: bool) {
    let key = handle_key(handle);
    let mut tags = TAGS.lock().unwrap_or_else(|e| e.into_inner());
    match tags.iter_mut().find(|t| handle_key(&t.handle) == key) {
        Some(t) if t.hit.elapsed().as_secs_f32() < TAG_COMBO => {
            t.dmg += dmg;
            t.hit = std::time::Instant::now();
        }
        Some(t) => *t = Tag { handle: *handle, hit: std::time::Instant::now(), dmg, before },
        None => tags.push(Tag { handle: *handle, hit: std::time::Instant::now(), dmg, before }),
    }
}

/// Game thread, every frame: the bars to draw (head position, HP share now, HP share before the
/// combo, combo damage while it lasts).
pub fn tags() -> Vec<crate::hud::Tag> {
    let Ok(wcm) = (unsafe { WorldChrMan::instance() }) else { return Vec::new() };
    let mut tags = TAGS.lock().unwrap_or_else(|e| e.into_inner());
    tags.retain(|t| t.hit.elapsed().as_secs_f32() < TAG_COMBO * 2.0 && wcm.chr_ins_by_handle(&t.handle).is_some());
    let bosses = boss_keys();
    tags.iter()
        .filter(|t| !bosses.contains(&handle_key(&t.handle)))
        .filter_map(|t| {
            let chr = wcm.chr_ins_by_handle(&t.handle)?;
            let (hp, max) = (chr.modules.data.hp.max(0) as f32, chr.modules.data.max_hp.max(1) as f32);
            let p = chr.modules.physics.position;
            let combo = t.hit.elapsed().as_secs_f32() < TAG_COMBO;
            Some(crate::hud::Tag {
                pos: glam::Vec3::new(p.0, p.1, p.2),
                hp: hp / max,
                before: if combo { t.before as f32 / max } else { hp / max },
                dmg: if combo { t.dmg } else { 0 },
            })
        })
        .collect()
}

/// A thrown character hit something: `pct` of its max HP, lethal if that's all it had left.
pub fn impact(combat: &mut Combat, handle: &FieldInsHandle, pct: f32, tick: u32) {
    let Ok(wcm) = (unsafe { WorldChrMan::instance_mut() }) else { return };
    let Some(chr) = wcm.chr_ins_by_handle_mut(handle) else { return };
    let data = &mut chr.modules.data;
    let (hp, max) = (data.hp, data.max_hp.max(1));
    let dmg = ((max as f32 * pct / 100.0).ceil() as i32).max(1);
    // (throws kill right there when the impact takes the rest of his HP)
    data.hp = (hp - dmg).max(0);
    let dealt = hp - data.hp;
    show_damage(handle, hp, dealt, true);
    if data.hp == 1 {
        combat.finishing.entry(handle_key(handle)).or_insert((*handle, tick));
    }
    combat.victims.insert(handle_key(handle), (*handle, tick));
    if !is_boss(handle) {
        pass_to_bar(wcm, dealt);
    }
}

/// The bosses on screen (their boss bars).
pub fn boss_handles() -> Vec<FieldInsHandle> {
    let handles: Vec<FieldInsHandle> = unsafe { eldenring::cs::CSFeManImp::instance() }
        .map(|fe| fe.boss_health_displays.iter().filter(|e| !e.field_ins_handle.is_empty()).map(|e| e.field_ins_handle).collect())
        .unwrap_or_default();
    let mut seen = SEEN_BOSSES.lock().unwrap_or_else(|e| e.into_inner());
    for h in &handles {
        let k = handle_key(h);
        if !seen.contains(&k) {
            seen.push(k);
        }
    }
    handles
}

fn boss_keys() -> Vec<u64> {
    unsafe { eldenring::cs::CSFeManImp::instance() }
        .map(|fe| fe.boss_health_displays.iter().filter(|e| !e.field_ins_handle.is_empty()).map(|e| handle_key(&e.field_ins_handle)).collect())
        .unwrap_or_default()
}

/// Game thread, every frame: the boss bars to draw (the game's boss list: name, HP now and
/// before Mario's combo, combo damage).
pub fn bosses() -> Vec<crate::hud::BossBar> {
    static NAMES: std::sync::Mutex<Vec<(i32, String)>> = std::sync::Mutex::new(Vec::new());
    let (Ok(fe), Ok(wcm)) = (unsafe { eldenring::cs::CSFeManImp::instance() }, unsafe { WorldChrMan::instance() }) else {
        return Vec::new();
    };
    let tags = TAGS.lock().unwrap_or_else(|e| e.into_inner());
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    fe.boss_health_displays
        .iter()
        .filter(|e| !e.field_ins_handle.is_empty())
        .filter_map(|e| {
            let chr = wcm.chr_ins_by_handle(&e.field_ins_handle)?;
            let (hp, max) = (chr.modules.data.hp.max(0) as f32, chr.modules.data.max_hp.max(1) as f32);
            let name = match names.iter().find(|(id, _)| *id == e.fmg_id) {
                Some((_, n)) => n.clone(),
                None => {
                    let n = crate::names::text(e.fmg_id).unwrap_or_default();
                    names.push((e.fmg_id, n.clone()));
                    n
                }
            };
            let key = handle_key(&e.field_ins_handle);
            let combo = tags.iter().find(|t| handle_key(&t.handle) == key && t.hit.elapsed().as_secs_f32() < TAG_COMBO);
            Some(crate::hud::BossBar {
                name,
                hp: hp / max,
                before: combo.map(|t| t.before as f32 / max).unwrap_or(hp / max),
                dmg: combo.map(|t| t.dmg).unwrap_or(0),
            })
        })
        .collect()
}

/// Damage to a boss body that isn't the one on the boss bar (the Fire Giant's first phase: the
/// bar is his second-phase character, with about twice the HP, and the game passes each hit on
/// to it): the bar's character loses the same HP, or the bar stood still for the whole first phase.
fn pass_to_bar(wcm: &mut WorldChrMan, dealt: i32) {
    let on_bar: Vec<FieldInsHandle> = unsafe { eldenring::cs::CSFeManImp::instance() }
        .map(|fe| fe.boss_health_displays.iter().filter(|e| !e.field_ins_handle.is_empty()).map(|e| e.field_ins_handle.clone()).collect())
        .unwrap_or_default();
    let [only] = on_bar.as_slice() else { return };
    let Some(main) = wcm.chr_ins_by_handle_mut(only).filter(|c| !own_side(c.team_type) && c.modules.data.hp > 1) else { return };
    let d = &mut main.modules.data;
    let hp = d.hp;
    d.hp = (hp - dealt).max(1);
    show_damage(only, hp, hp - d.hp, true);
    log(format!("combat: passed on to the boss bar's character: {dealt} of {} HP, {} left", d.max_hp, d.hp));
}

/// Takes the attack's share of the character's max HP (never the last point: the bullet deals the
/// final blow, so the kill is the game's own). Returns true if it's down to that last point.
fn take_share(handle: &FieldInsHandle, attack: Attack) -> bool {
    let Ok(wcm) = (unsafe { WorldChrMan::instance_mut() }) else { return false };
    let bar = is_boss(handle);
    // a thrown boss in the air or lying limp takes no hits (the throw's impact is the damage)
    if crate::swing::is_down(handle) {
        return false;
    }
    let Some(chr) = wcm.chr_ins_by_handle_mut(handle) else { return false };
    let team = chr.team_type;
    if own_side(team) {
        log(format!("combat: team {team} is on the player's side, no damage"));
        return false;
    }
    let boss = bar || boss_class(handle, chr);
    let (default, key) = attack.percent();
    let mut pct = config_f32(key, default);
    if boss {
        pct *= config_f32("boss_damage_factor", BOSS_FACTOR) * attack.boss_bonus();
        if let Some((t, tmax)) = crate::swing::toughness_of(handle) {
            log(format!("combat: boss poise {t:.0}/{tmax:.0}"));
        }
        crate::swing::add_stance(handle, attack.spec().3);
    }
    // the red NPC invaders are fights of their own, not two-hit mobs
    if !boss && matches!(chr.chr_type, ChrType::BloodyFingerNpc | ChrType::RecusantNpc) {
        pct *= config_f32("invader_damage_factor", INVADER_FACTOR);
    }
    let data = &mut chr.modules.data;
    let (hp, max) = (data.hp, data.max_hp.max(1));
    let dmg = ((max as f32 * pct / 100.0).ceil() as i32).max(1);
    data.hp = (hp - dmg).max(1);
    show_damage(handle, hp, hp - data.hp, bar);
    log(format!("combat: {:?} on team {team}{}: {dmg} of {max} HP ({pct:.1}%), {} left", attack as u8, if boss { " boss" } else { "" }, data.hp));
    let (last, dealt) = (data.hp == 1, hp - data.hp);
    if boss && !bar {
        pass_to_bar(wcm, dealt);
    }
    last
}

/// Target body in SM64 units: horizontal radius and height (a human-sized capsule).
const TARGET_RADIUS: f32 = 55.0;
const TARGET_HEIGHT: f32 = 180.0;
/// Mario's own hitbox radius in SM64 (units) plus a little reach.
const MARIO_RADIUS: f32 = 45.0;
/// A punch's reach beyond the target's body (SM64 units), within this cone (cos 50°) in front.
const PUNCH_REACH: f32 = 80.0;
const PUNCH_CONE_COS: f32 = 0.64;

/// Called on the libsm64 thread right after a tick: which targets Mario hits now.
/// Stomps in a row on one enemy before Mario has to land (SM64's head bounce would juggle forever).
const MAX_STOMPS: u32 = 1;

pub fn hits(id: i32, state: &sm64::SM64MarioState, targets: &[([f32; 3], f32, f32, usize)], no_stomp: &[usize]) -> Vec<(usize, Attack, bool)> {
    // diving (SM64's dive or its belly slide): kept with each hit, a dive into an enemy's back
    // picks it up like diving into a Bob-omb
    const ACT_FLAG_DIVING: u32 = 0x0008_0000;
    let diving = state.action & ACT_FLAG_DIVING != 0;
    let mut out = Vec::new();
    let m = state.position;
    for &(t, radius, height, index) in targets {
        // stomped out: no hit and no bounce, Mario drops past
        if no_stomp.contains(&index) && matches!(classify(state), Attack::Stomp) {
            continue;
        }
        let (dx, dz) = (t[0] - m[0], t[2] - m[2]);
        let horizontal = (dx * dx + dz * dz).sqrt();
        let attack = classify(state);
        // punches reach a bit further than SM64's (its hitbox is made for small objects): up to
        // PUNCH_REACH, in front of Mario (picking enemies up from behind was hard to land)
        let in_front = horizontal > 1.0 && {
            let (fx, fz) = (state.face_angle.sin(), state.face_angle.cos());
            (dx * fx + dz * fz) / horizontal >= PUNCH_CONE_COS
        };
        let punch_reach = matches!(attack, Attack::Punch) && in_front && horizontal <= PUNCH_REACH + radius;
        if (horizontal > MARIO_RADIUS + radius && !punch_reach) || m[1] > t[1] + height || m[1] + 100.0 < t[1] {
            continue;
        }
        // aim at mid-body: stomps need Mario above that
        if unsafe { sm64::sm64_mario_attack(id, t[0], t[1] + height * 0.5, t[2], height * 0.5) } || punch_reach {
            out.push((index, attack, diving));
        }
    }
    out
}

impl Combat {
    /// Game thread: deals the damage for libsm64's hits. `tick` is the SM64 tick counter.
    pub fn deal(&mut self, player: &ChrIns, targets: &[Target], hits: &[(usize, Attack, bool)], tick: u32) {
        if !self.patched {
            self.patched = patch_params();
            if !self.patched {
                return;
            }
        }
        self.cooldown.retain(|_, t| tick.wrapping_sub(*t) < 12);
        // the final blow is the bullet's; if it can't land (some small or odd characters), finish
        // them after 0.5 s
        self.finishing.retain(|_, (handle, t)| {
            // (a boss Mario has grabbed or thrown: the throw's impact deals the final blow)
            if tick.wrapping_sub(*t) < 15 || crate::swing::busy_with(handle) {
                return true;
            }
            if let Some(chr) = unsafe { WorldChrMan::instance_mut() }.ok().and_then(|w| w.chr_ins_by_handle_mut(handle)) {
                if chr.modules.data.hp == 1 {
                    chr.modules.data.hp = 0;
                    log("combat: final blow missed, finished directly");
                }
            }
            false
        });
        for &(index, attack, diving) in hits {
            let Some(target) = targets.get(index) else { continue };
            if self.cooldown.contains_key(&target.key) {
                continue;
            }
            self.cooldown.insert(target.key, tick);
            if matches!(attack, Attack::Stomp) {
                *self.stomps.entry(target.key).or_insert(0) += 1;
            }
            if let Some(handle) = &target.handle {
                // a thrown boss in the air or lying limp: no hit at all (not even the game's own,
                // which could finish a ragdolled character)
                if crate::swing::is_down(handle) || crate::carry::is_carried(handle) {
                    continue;
                }
                // a regular enemy punched or dived into from behind: Mario picks it up like a
                // Bob-omb (carry.rs)
                if (matches!(attack, Attack::Punch) || diving) && liftable(handle) {
                    let me = player.modules.physics.position;
                    if crate::carry::try_pick_up(handle, glam::Vec3::new(me.0, me.1, me.2), target.radius / 100.0, target.height / 100.0) {
                        self.victims.insert(target.key, (*handle, tick));
                        continue;
                    }
                }
                // a boss with a broken stance: this hit grabs him by the tail (swing.rs)
                if crate::swing::try_grab(handle, target.radius / 100.0) {
                    self.victims.insert(target.key, (*handle, tick));
                    continue;
                }
                self.victims.insert(target.key, (*handle, tick));
                match attack {
                    Attack::Stomp => crate::squish::start(handle, 0.35, 0.0),
                    Attack::GroundPound => crate::squish::start(handle, 0.8, 1.0),
                    _ => {}
                }
                if take_share(handle, attack) {
                    self.finishing.entry(target.key).or_insert((*handle, tick));
                }
            }
            let (bullet, ..) = attack.spec();
            let p = &target.er;
            let from = player.modules.physics.position;
            let dir = glam::Vec3::new(p.0 - from.0, 0.0, p.2 - from.2).normalize_or(glam::Vec3::Z);
            let request = SpawnRequest {
                owner: player.field_ins_handle.clone(),
                behavior_id: -1,
                magic_id: -1,
                unk10: 0,
                bullet_id: bullet,
                goods_id: -1,
                dummy_poly_id: -1,
                target: [0xFF; 8],
                unk28: 0,
                unk2c: 0,
                unk30: F32Vector4(p.0, p.1 + 1.0, p.2, 0.0),
                unk40: 0,
                unk44: 0,
                pad48: [0; 8],
                acceleration_angle: F32Vector4(dir.x, dir.y, dir.z, 0.0),
                unk60: F32Vector4(0.0, 0.0, 0.0, 0.0),
                angle: F32Vector4(dir.x, dir.y, dir.z, 0.0),
                position: F32Vector4(p.0, p.1 + 1.0, p.2, 0.0),
                rest: [0; 0x80],
            };
            let Ok(manager) = (unsafe { CSBulletManager::instance_mut() }) else { return };
            let result = manager.spawn_bullet(unsafe { &*(&request as *const SpawnRequest as *const _) });
            log(format!("combat: {:?} hit -> bullet {bullet}: {result:?}", attack as u8));
        }
    }

    /// Game thread, every tick: the targets Mario can't stomp again before landing (indexes into
    /// `targets`); landing resets the count.
    pub fn stomp_limits(&mut self, targets: &[Target], grounded: bool) -> Vec<usize> {
        if grounded {
            self.stomps.clear();
        }
        targets
            .iter()
            .enumerate()
            .filter(|(_, t)| self.stomps.get(&t.key).is_some_and(|&n| n >= MAX_STOMPS))
            .map(|(i, _)| i)
            .collect()
    }

    /// Game thread: how the Tarnished's HP loss since the last check turns into Mario's health
    /// (his HP is refilled right after, so SM64's power meter is what counts): a real hit costs
    /// wedges by its size with SM64's knockback, poison / bleed / rot ticks drain a wedge per 10%.
    pub fn took_damage(&mut self, hp: i32, max: i32, scale: f32) -> Option<Hurt> {
        let lost = self.last_hp.map(|old| old - hp).unwrap_or(0);
        self.last_hp = Some(max);
        if lost <= 0 {
            return None;
        }
        let frac = lost as f32 / max.max(1) as f32 * scale;
        if frac >= 0.04 {
            return Some(Hurt::Hit(if frac < 0.25 { 1 } else if frac < 0.5 { 2 } else { 3 }));
        }
        self.drain += frac;
        if self.drain >= 0.1 {
            self.drain -= 0.1;
            return Some(Hurt::Drain);
        }
        None
    }

    /// Game thread, every SM64 tick: where the characters Mario hit in the last 10 s died, for
    /// those that died since (each drops a coin).
    pub fn kills(&mut self, tick: u32) -> Vec<glam::Vec3> {
        let Ok(wcm) = (unsafe { WorldChrMan::instance() }) else { return Vec::new() };
        let mut out = Vec::new();
        self.victims.retain(|_, (handle, t)| match wcm.chr_ins_by_handle(handle) {
            Some(chr) if chr.modules.data.hp <= 0 => {
                let p = chr.modules.physics.position;
                out.push(glam::Vec3::new(p.0, p.1, p.2));
                false
            }
            Some(_) => tick.wrapping_sub(*t) < 300,
            None => false,
        });
        out
    }
}
