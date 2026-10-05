//! Mario's loadout: in Mario mode the player wears the Mario set (items 9990000-9990300, model 999,
//! added to regulation.bin) with bare fists, and nothing else can be equipped; switching Mario off
//! puts the previous loadout back.
//!
//! Uses the game's own equip and item-give functions, found by byte pattern (patterns and calling
//! conventions from The Grand Archives' Elden Ring cheat table).

use std::sync::OnceLock;

use eldenring::cs::{GameDataMan, MapItemMan, PlayerIns};
use fromsoftware_shared::FromStatic;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;

use crate::log;

const PROTECTOR: u32 = 0x1000_0000;
/// Mario set: (ChrAsm slot, item id)
pub const MARIO_SET: [(usize, u32); 4] =
    [(12, 660000 | PROTECTOR), (13, 660100 | PROTECTOR), (14, 660200 | PROTECTOR), (15, 660300 | PROTECTOR)];
const UNARMED: u32 = 110000;
const WEAPON_SLOTS: [usize; 6] = [0, 1, 2, 3, 4, 5];

type EquipFn = unsafe extern "C" fn(usize, u32, *const u32, u32, u64, u64, u64) -> u64;
type GiveFn = unsafe extern "C" fn(usize, *const u8, *mut u8, u64) -> u64;

struct Funcs {
    equip: EquipFn,
    give: GiveFn,
}

static FUNCS: OnceLock<Option<Funcs>> = OnceLock::new();

/// The game's image in memory.
fn image() -> &'static [u8] {
    let base = unsafe { GetModuleHandleW(None) }.map(|h| h.0 as usize).unwrap_or(0);
    let nt = base + unsafe { *((base + 0x3c) as *const u32) } as usize;
    let size = unsafe { *((nt + 0x50) as *const u32) } as usize;
    unsafe { std::slice::from_raw_parts(base as *const u8, size) }
}

/// All matches of a pattern like "?? 8b f1 ?? 8b d8" in the executable sections.
pub(crate) fn scan(pattern: &str) -> Vec<usize> {
    let pat: Vec<Option<u8>> = pattern.split_whitespace().map(|t| u8::from_str_radix(t, 16).ok()).collect();
    let img = image();
    let base = img.as_ptr() as usize;
    // only scan committed, readable memory: walk the section table
    let nt = base + unsafe { *((base + 0x3c) as *const u32) } as usize;
    let sections = unsafe { *((nt + 6) as *const u16) } as usize;
    let opt_size = unsafe { *((nt + 20) as *const u16) } as usize;
    let first = nt + 24 + opt_size;
    let mut out = Vec::new();
    for s in 0..sections {
        let h = first + s * 40;
        let characteristics = unsafe { *((h + 36) as *const u32) };
        if characteristics & 0x2000_0000 == 0 {
            continue; // not executable
        }
        let va = unsafe { *((h + 12) as *const u32) } as usize;
        let len = unsafe { *((h + 8) as *const u32) } as usize;
        let sec = &img[va..(va + len).min(img.len())];
        'outer: for i in 0..sec.len().saturating_sub(pat.len()) {
            for (k, p) in pat.iter().enumerate() {
                if let Some(b) = p {
                    if sec[i + k] != *b {
                        continue 'outer;
                    }
                }
            }
            out.push(base + va + i);
            if out.len() > 4 {
                return out;
            }
        }
    }
    out
}

/// Finds the game functions (once, a fraction of a second).
pub fn init() -> bool {
    FUNCS
        .get_or_init(|| {
            let t = std::time::Instant::now();
            let equip = scan("?? 8b f1 ?? 8b d8 ?? 63 ea ?? 8b f9");
            let give = scan("8b 02 83 f8 0a");
            log(format!("equip: pattern scan {:.0} ms, equip {} match(es), give {} match(es)", t.elapsed().as_secs_f32() * 1000.0, equip.len(), give.len()));
            if equip.len() != 1 || give.len() != 1 {
                return None;
            }
            Some(Funcs {
                equip: unsafe { std::mem::transmute::<usize, EquipFn>(equip[0] - 0x17) },
                give: unsafe { std::mem::transmute::<usize, GiveFn>(give[0] - 0x52) },
            })
        })
        .is_some()
}

/// Vagabond Knight rows: in Mario mode their model is switched to Mario's (999). New rows injected at
/// runtime aren't accepted by the game's equip code (it resolves armour through another table), and
/// a patched regulation.bin crashes me3, so Mario borrows the Vagabond set.
const VAGABOND: [u32; 4] = [660000, 660100, 660200, 660300];
const VAGABOND_MODEL: u16 = 1280;
const MARIO_MODEL: u16 = 999;
/// "Nothing" armour items per slot (equipping these empties the slot).
const BARE: [(usize, u32); 4] = [(12, 10000 | PROTECTOR), (13, 10100 | PROTECTOR), (14, 10200 | PROTECTOR), (15, 10300 | PROTECTOR)];

/// Menu icons: Vagabond's and Mario's (unused icon slots 13580-13583, packed by tools/build_icons.py).
const VAGABOND_ICONS: [u16; 4] = [14010, 14011, 14012, 14013];
const MARIO_ICONS: [u16; 4] = [13580, 13581, 13582, 13583];

/// The Vagabond rows' own hide flags (saved the first time Mario's model goes on).
static VAGABOND_HIDE: std::sync::Mutex<Option<[[u8; 96]; 4]>> = std::sync::Mutex::new(None);

/// Switches the Vagabond set's model and menu icons to Mario's (or back). With Mario's model every
/// part of the Tarnished is hidden too: face, beard and hair are skinned to bones Mario's parts
/// ride on (Neck...), so they would otherwise poke out of Mario.
fn set_vagabond_model(model: u16) -> bool {
    let Ok(repo) = (unsafe { eldenring::cs::SoloParamRepository::instance_mut() }) else { return false };
    let mario = model == MARIO_MODEL;
    let icons = if mario { MARIO_ICONS } else { VAGABOND_ICONS };
    let mut saved = VAGABOND_HIDE.lock().unwrap_or_else(|e| e.into_inner());
    if mario && saved.is_none() {
        let mut own = [[0; 96]; 4];
        for (k, id) in VAGABOND.into_iter().enumerate() {
            let Some(row) = repo.get::<eldenring::cs::EquipParamProtector>(id) else { return false };
            own[k] = get_hide(row);
        }
        *saved = Some(own);
    }
    for (k, id) in VAGABOND.into_iter().enumerate() {
        let Some(row) = repo.get_mut::<eldenring::cs::EquipParamProtector>(id) else { return false };
        row.set_equip_model_id(model);
        row.set_icon_id_m(icons[k]);
        row.set_icon_id_f(icons[k]);
        match (mario, *saved) {
            (true, _) => set_hide(row, [1; 96]),
            (false, Some(own)) => set_hide(row, own[k]),
            (false, None) => {}
        }
    }
    true
}

/// The Vagabond's starting template (CharaInitParam).
const VAGABOND_CLASS: u32 = 3000;

/// Mario starts without the Vagabond's sword, halberd and shield: he never uses them, and in
/// character creation they'd float next to him (weapons don't follow the bones Mario is posed on).
fn mario_class() {
    use eldenring::cs::{CharaInitParam, SoloParamRepository};
    let Ok(repo) = (unsafe { SoloParamRepository::instance_mut() }) else { return };
    let Some(row) = repo.get_mut::<CharaInitParam>(VAGABOND_CLASS) else { return };
    if row.equip_armer() != (VAGABOND[1] & !PROTECTOR) as i32 || (row.equip_wep_right() == -1 && row.equip_wep_left() == -1) {
        return;
    }
    log(format!(
        "equip: the Vagabond template loses its weapons ({} {} {} / {} {} {})",
        row.equip_wep_right(),
        row.equip_subwep_right(),
        row.equip_subwep_right3(),
        row.equip_wep_left(),
        row.equip_subwep_left(),
        row.equip_subwep_left3()
    ));
    row.set_equip_wep_right(-1);
    row.set_equip_subwep_right(-1);
    row.set_equip_subwep_right3(-1);
    row.set_equip_wep_left(-1);
    row.set_equip_subwep_left(-1);
    row.set_equip_subwep_left3(-1);
}

/// Outside the world (character creation): Mario's model on the Vagabond set, so the preview
/// loads it. False until the params are there.
pub fn menu_mario() -> bool {
    use eldenring::cs::{EquipParamProtector, SoloParamRepository};
    if !crate::engine_mario::MENU_HOOKS.load(std::sync::atomic::Ordering::Relaxed) {
        return false;
    }
    let Ok(repo) = (unsafe { SoloParamRepository::instance() }) else { return false };
    // (asking for a row before the params are loaded panics)
    if repo.solo_param_holders[1].get_res_cap(0).is_none() {
        return false;
    }
    if repo.get::<EquipParamProtector>(VAGABOND[1]).is_none_or(|row| row.equip_model_id() != MARIO_MODEL) {
        if !set_vagabond_model(MARIO_MODEL) {
            return false;
        }
        log("equip: Mario's model on the Vagabond set (menus)");
    }
    mario_class();
    true
}

/// The Mario set's chest piece, as a ChrAsm has it.
pub const MARIO_CHEST: i32 = VAGABOND[1] as i32;

/// Set while the armour slots are being emptied (so that equipping the set afterwards loads the
/// model that was just switched); the game applies equips a little later, so this is a phase the
/// lock works through, not a one-off call.
static BARING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn armour_is_bare(player: &PlayerIns) -> bool {
    BARE.iter().all(|&(slot, item)| equipped(player, slot) == item)
}

/// PlayerGameData of the local player.
fn player_game_data() -> Option<usize> {
    let gdm = unsafe { GameDataMan::instance() }.ok()?;
    Some(gdm.main_player_game_data.as_ptr() as usize)
}

/// Inventory index (as the equip function wants it) of an item, if the player has it.
fn inventory_index(item: u32) -> Option<u32> {
    let pgd = player_game_data()?;
    let gdm = unsafe { GameDataMan::instance() }.ok()?;
    let items = &gdm.main_player_game_data.equipment.equip_inventory_data.items_data;
    let head = items.normal_items_head.as_ptr() as usize;
    let len = items.normal_items_len as usize;
    let tail = unsafe { *((pgd + 0x408 + 0x1c) as *const u32) };
    (0..len).find(|i| unsafe { *((head + i * 0x18 + 4) as *const u32) } == item).map(|i| i as u32 + tail)
}

/// Puts `item` (from the inventory) into ChrAsm `slot`. The "nothing" armour items are given first
/// if the inventory lacks them.
fn equip(slot: usize, item: u32) -> bool {
    if inventory_index(item).is_none() && BARE.iter().any(|b| b.1 == item) {
        give(&[item]);
    }
    let (Some(Some(f)), Some(pgd), Some(idx)) = (FUNCS.get(), player_game_data(), inventory_index(item)) else {
        return false;
    };
    let data = [item, 0, 0, 0];
    unsafe { (f.equip)(pgd + 0x2b0, slot as u32, data.as_ptr(), idx, 1, 1, 0) };
    true
}

/// Adds items (quantity 1) to the inventory.
/// The Spectral Steed Whistle (goods 130).
const WHISTLE: u32 = 0x4000_0000 | 130;

/// Mario gets Torrent's whistle if he doesn't have it yet (the game hands it out a few graces in).
pub fn give_whistle() {
    if init() && inventory_index(WHISTLE).is_none() {
        log("equip: giving the Spectral Steed Whistle");
        give(&[WHISTLE]);
    }
}

/// Makes the whistle the selected quick item, putting it into a free quick slot first if it
/// isn't in one. False if he has no whistle or all ten slots are taken by other things.
pub fn select_whistle() -> bool {
    let Ok(gdm) = (unsafe { GameDataMan::instance_mut() }) else { return false };
    let Some(idx) = inventory_index(WHISTLE) else { return false };
    let equipment = &mut gdm.main_player_game_data.equipment;
    let ids = unsafe { &mut *(&mut equipment.equipment_entries.quick_tems as *mut _ as *mut [u32; 10]) };
    let slot = match ids.iter().position(|&id| id == WHISTLE) {
        Some(slot) => slot,
        None => {
            let Some(free) = ids.iter().position(|&id| id == u32::MAX) else {
                log("equip: no free quick slot for the whistle");
                return false;
            };
            let items = &equipment.equip_inventory_data.items_data;
            let entries = items.normal_items_head.as_ptr() as usize;
            let Some(entry) = (0..items.normal_items_len as usize).map(|i| entries + i * 0x18).find(|&e| unsafe { *((e + 4) as *const u32) } == WHISTLE) else {
                return false;
            };
            // a quick slot is the item's gaitem handle and inventory index, like the armour slots
            let slots = unsafe { &mut *(&mut equipment.equip_item_data.quick_slots as *mut _ as *mut [[u32; 2]; 10]) };
            log(format!("equip: whistle into quick slot {free} (was {:#x} {})", slots[free][0], slots[free][1] as i32));
            slots[free] = [unsafe { *(entry as *const u32) }, idx];
            ids[free] = WHISTLE;
            free
        }
    };
    equipment.equip_item_data.selected_quick_slot = slot as i32;
    true
}

fn give(items: &[u32]) {
    let Some(Some(f)) = FUNCS.get() else { return };
    let Ok(man) = (unsafe { MapItemMan::instance_mut() }) else { return };
    let mut buf = [0u8; 4 + 16 * 10];
    buf[..4].copy_from_slice(&(items.len().min(10) as u32).to_le_bytes());
    for (k, item) in items.iter().take(10).enumerate() {
        let e = 4 + k * 16;
        buf[e..e + 4].copy_from_slice(&item.to_le_bytes());
        buf[e + 4..e + 8].copy_from_slice(&1u32.to_le_bytes());
        buf[e + 8..e + 12].copy_from_slice(&u32::MAX.to_le_bytes());
        buf[e + 12..e + 16].copy_from_slice(&u32::MAX.to_le_bytes());
    }
    let mut scratch = [0u8; 128];
    unsafe { (f.give)(man as *mut MapItemMan as usize, buf.as_ptr(), scratch.as_mut_ptr(), 0) };
}

/// Item id currently in a ChrAsm slot.
fn equipped(player: &PlayerIns, slot: usize) -> u32 {
    let id = player.chr_asm.equipment_param_ids[slot] as u32;
    if (12..=15).contains(&slot) { id | PROTECTOR } else { id }
}

/// What the player wore before Mario mode.
pub struct Loadout(Vec<(usize, u32)>);

/// Mario mode on: remember the loadout, make sure the Mario set is in the inventory, wear it.
pub fn enter(player: &PlayerIns) -> Option<Loadout> {
    if !init() {
        return None;
    }
    crate::names::apply(true);
    crate::voice::mute(true);
    if !set_vagabond_model(MARIO_MODEL) {
        log("equip: Vagabond rows not found, leaving the equipment alone");
        return None;
    }
    let saved = Loadout(WEAPON_SLOTS.iter().chain([12, 13, 14, 15].iter()).map(|&s| (s, equipped(player, s))).collect());
    // calibration log: our index for the equipped chest vs the game's own record
    if let Some(gdm) = unsafe { GameDataMan::instance() }.ok() {
        let game_idx = gdm.main_player_game_data.equipment.equipment_item_idx_list[13];
        log(format!("equip: chest {:#x}: our index {:?}, game's {game_idx}", equipped(player, 13), inventory_index(equipped(player, 13))));
    }
    let missing: Vec<u32> = MARIO_SET.iter().map(|&(_, i)| i).filter(|&i| inventory_index(i).is_none()).collect();
    if !missing.is_empty() {
        log(format!("equip: giving the Mario set {missing:x?}"));
        give(&missing);
    }
    // empty the armour slots first; the lock then equips the set, which loads the Mario model
    BARING.store(true, std::sync::atomic::Ordering::Relaxed);
    enforce(player);
    Some(saved)
}

/// Mario mode: re-equip the Mario set / fists wherever something else is worn. Returns true if it had to.
pub fn enforce(player: &PlayerIns) -> bool {
    if !matches!(FUNCS.get(), Some(Some(_))) {
        return false;
    }
    let mut changed = false;
    static LOGGED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if BARING.load(std::sync::atomic::Ordering::Relaxed) && armour_is_bare(player) {
        BARING.store(false, std::sync::atomic::Ordering::Relaxed);
        log("equip: armour emptied, putting the Mario set on");
    }
    let armour = if BARING.load(std::sync::atomic::Ordering::Relaxed) { BARE } else { MARIO_SET };
    let wanted = armour.iter().copied().chain(WEAPON_SLOTS.iter().map(|&s| (s, UNARMED)));
    for (slot, item) in wanted {
        let now = equipped(player, slot);
        if now != item {
            let ok = equip(slot, item);
            if LOGGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 20 {
                log(format!("equip: slot {slot} has {now:#x}, want {item:#x}: {}", if ok { "equipping" } else { "not in inventory" }));
            }
            changed |= ok;
        }
    }
    changed
}

/// Mario mode off, step 1: Vagabond model back, armour slots emptied (so the model reloads).
pub fn leave(_saved: &Loadout) {
    set_vagabond_model(VAGABOND_MODEL);
    crate::names::apply(false);
    crate::voice::mute(false);
}

/// Mario mode off, step 2 (called every frame until it returns true): empty the armour slots, then
/// put the previous loadout back.
pub fn restore(player: &PlayerIns, saved: &Loadout) -> bool {
    if !armour_is_bare(player) {
        for (slot, item) in BARE {
            if equipped(player, slot) != item {
                equip(slot, item);
            }
        }
        return false;
    }
    for &(slot, item) in &saved.0 {
        equip(slot, item);
    }
    true
}



/// Every "hide this body part" flag of an armour row (face, beard, hair, body...).
fn get_hide(row: &eldenring::param::EQUIP_PARAM_PROTECTOR_ST) -> [u8; 96] {
    [
        row.invisible_flag_sex_ver00(),
        row.invisible_flag_sex_ver01(),
        row.invisible_flag_sex_ver02(),
        row.invisible_flag_sex_ver03(),
        row.invisible_flag_sex_ver04(),
        row.invisible_flag_sex_ver05(),
        row.invisible_flag_sex_ver06(),
        row.invisible_flag_sex_ver07(),
        row.invisible_flag_sex_ver08(),
        row.invisible_flag_sex_ver09(),
        row.invisible_flag_sex_ver10(),
        row.invisible_flag_sex_ver11(),
        row.invisible_flag_sex_ver12(),
        row.invisible_flag_sex_ver13(),
        row.invisible_flag_sex_ver14(),
        row.invisible_flag_sex_ver15(),
        row.invisible_flag_sex_ver16(),
        row.invisible_flag_sex_ver17(),
        row.invisible_flag_sex_ver18(),
        row.invisible_flag_sex_ver19(),
        row.invisible_flag_sex_ver20(),
        row.invisible_flag_sex_ver21(),
        row.invisible_flag_sex_ver22(),
        row.invisible_flag_sex_ver23(),
        row.invisible_flag_sex_ver24(),
        row.invisible_flag_sex_ver25(),
        row.invisible_flag_sex_ver26(),
        row.invisible_flag_sex_ver27(),
        row.invisible_flag_sex_ver28(),
        row.invisible_flag_sex_ver29(),
        row.invisible_flag_sex_ver30(),
        row.invisible_flag_sex_ver31(),
        row.invisible_flag_sex_ver32(),
        row.invisible_flag_sex_ver33(),
        row.invisible_flag_sex_ver34(),
        row.invisible_flag_sex_ver35(),
        row.invisible_flag_sex_ver36(),
        row.invisible_flag_sex_ver37(),
        row.invisible_flag_sex_ver38(),
        row.invisible_flag_sex_ver39(),
        row.invisible_flag_sex_ver40(),
        row.invisible_flag_sex_ver41(),
        row.invisible_flag_sex_ver42(),
        row.invisible_flag_sex_ver43(),
        row.invisible_flag_sex_ver44(),
        row.invisible_flag_sex_ver45(),
        row.invisible_flag_sex_ver46(),
        row.invisible_flag_sex_ver47(),
        row.invisible_flag_sex_ver48(),
        row.invisible_flag_sex_ver49(),
        row.invisible_flag_sex_ver50(),
        row.invisible_flag_sex_ver51(),
        row.invisible_flag_sex_ver52(),
        row.invisible_flag_sex_ver53(),
        row.invisible_flag_sex_ver54(),
        row.invisible_flag_sex_ver55(),
        row.invisible_flag_sex_ver56(),
        row.invisible_flag_sex_ver57(),
        row.invisible_flag_sex_ver58(),
        row.invisible_flag_sex_ver59(),
        row.invisible_flag_sex_ver60(),
        row.invisible_flag_sex_ver61(),
        row.invisible_flag_sex_ver62(),
        row.invisible_flag_sex_ver63(),
        row.invisible_flag_sex_ver64(),
        row.invisible_flag_sex_ver65(),
        row.invisible_flag_sex_ver66(),
        row.invisible_flag_sex_ver67(),
        row.invisible_flag_sex_ver68(),
        row.invisible_flag_sex_ver69(),
        row.invisible_flag_sex_ver70(),
        row.invisible_flag_sex_ver71(),
        row.invisible_flag_sex_ver72(),
        row.invisible_flag_sex_ver73(),
        row.invisible_flag_sex_ver74(),
        row.invisible_flag_sex_ver75(),
        row.invisible_flag_sex_ver76(),
        row.invisible_flag_sex_ver77(),
        row.invisible_flag_sex_ver78(),
        row.invisible_flag_sex_ver79(),
        row.invisible_flag_sex_ver80(),
        row.invisible_flag_sex_ver81(),
        row.invisible_flag_sex_ver82(),
        row.invisible_flag_sex_ver83(),
        row.invisible_flag_sex_ver84(),
        row.invisible_flag_sex_ver85(),
        row.invisible_flag_sex_ver86(),
        row.invisible_flag_sex_ver87(),
        row.invisible_flag_sex_ver88(),
        row.invisible_flag_sex_ver89(),
        row.invisible_flag_sex_ver90(),
        row.invisible_flag_sex_ver91(),
        row.invisible_flag_sex_ver92(),
        row.invisible_flag_sex_ver93(),
        row.invisible_flag_sex_ver94(),
        row.invisible_flag_sex_ver95(),
    ]
}

fn set_hide(row: &mut eldenring::param::EQUIP_PARAM_PROTECTOR_ST, v: [u8; 96]) {
    row.set_invisible_flag_sex_ver00(v[0]);
    row.set_invisible_flag_sex_ver01(v[1]);
    row.set_invisible_flag_sex_ver02(v[2]);
    row.set_invisible_flag_sex_ver03(v[3]);
    row.set_invisible_flag_sex_ver04(v[4]);
    row.set_invisible_flag_sex_ver05(v[5]);
    row.set_invisible_flag_sex_ver06(v[6]);
    row.set_invisible_flag_sex_ver07(v[7]);
    row.set_invisible_flag_sex_ver08(v[8]);
    row.set_invisible_flag_sex_ver09(v[9]);
    row.set_invisible_flag_sex_ver10(v[10]);
    row.set_invisible_flag_sex_ver11(v[11]);
    row.set_invisible_flag_sex_ver12(v[12]);
    row.set_invisible_flag_sex_ver13(v[13]);
    row.set_invisible_flag_sex_ver14(v[14]);
    row.set_invisible_flag_sex_ver15(v[15]);
    row.set_invisible_flag_sex_ver16(v[16]);
    row.set_invisible_flag_sex_ver17(v[17]);
    row.set_invisible_flag_sex_ver18(v[18]);
    row.set_invisible_flag_sex_ver19(v[19]);
    row.set_invisible_flag_sex_ver20(v[20]);
    row.set_invisible_flag_sex_ver21(v[21]);
    row.set_invisible_flag_sex_ver22(v[22]);
    row.set_invisible_flag_sex_ver23(v[23]);
    row.set_invisible_flag_sex_ver24(v[24]);
    row.set_invisible_flag_sex_ver25(v[25]);
    row.set_invisible_flag_sex_ver26(v[26]);
    row.set_invisible_flag_sex_ver27(v[27]);
    row.set_invisible_flag_sex_ver28(v[28]);
    row.set_invisible_flag_sex_ver29(v[29]);
    row.set_invisible_flag_sex_ver30(v[30]);
    row.set_invisible_flag_sex_ver31(v[31]);
    row.set_invisible_flag_sex_ver32(v[32]);
    row.set_invisible_flag_sex_ver33(v[33]);
    row.set_invisible_flag_sex_ver34(v[34]);
    row.set_invisible_flag_sex_ver35(v[35]);
    row.set_invisible_flag_sex_ver36(v[36]);
    row.set_invisible_flag_sex_ver37(v[37]);
    row.set_invisible_flag_sex_ver38(v[38]);
    row.set_invisible_flag_sex_ver39(v[39]);
    row.set_invisible_flag_sex_ver40(v[40]);
    row.set_invisible_flag_sex_ver41(v[41]);
    row.set_invisible_flag_sex_ver42(v[42]);
    row.set_invisible_flag_sex_ver43(v[43]);
    row.set_invisible_flag_sex_ver44(v[44]);
    row.set_invisible_flag_sex_ver45(v[45]);
    row.set_invisible_flag_sex_ver46(v[46]);
    row.set_invisible_flag_sex_ver47(v[47]);
    row.set_invisible_flag_sex_ver48(v[48]);
    row.set_invisible_flag_sex_ver49(v[49]);
    row.set_invisible_flag_sex_ver50(v[50]);
    row.set_invisible_flag_sex_ver51(v[51]);
    row.set_invisible_flag_sex_ver52(v[52]);
    row.set_invisible_flag_sex_ver53(v[53]);
    row.set_invisible_flag_sex_ver54(v[54]);
    row.set_invisible_flag_sex_ver55(v[55]);
    row.set_invisible_flag_sex_ver56(v[56]);
    row.set_invisible_flag_sex_ver57(v[57]);
    row.set_invisible_flag_sex_ver58(v[58]);
    row.set_invisible_flag_sex_ver59(v[59]);
    row.set_invisible_flag_sex_ver60(v[60]);
    row.set_invisible_flag_sex_ver61(v[61]);
    row.set_invisible_flag_sex_ver62(v[62]);
    row.set_invisible_flag_sex_ver63(v[63]);
    row.set_invisible_flag_sex_ver64(v[64]);
    row.set_invisible_flag_sex_ver65(v[65]);
    row.set_invisible_flag_sex_ver66(v[66]);
    row.set_invisible_flag_sex_ver67(v[67]);
    row.set_invisible_flag_sex_ver68(v[68]);
    row.set_invisible_flag_sex_ver69(v[69]);
    row.set_invisible_flag_sex_ver70(v[70]);
    row.set_invisible_flag_sex_ver71(v[71]);
    row.set_invisible_flag_sex_ver72(v[72]);
    row.set_invisible_flag_sex_ver73(v[73]);
    row.set_invisible_flag_sex_ver74(v[74]);
    row.set_invisible_flag_sex_ver75(v[75]);
    row.set_invisible_flag_sex_ver76(v[76]);
    row.set_invisible_flag_sex_ver77(v[77]);
    row.set_invisible_flag_sex_ver78(v[78]);
    row.set_invisible_flag_sex_ver79(v[79]);
    row.set_invisible_flag_sex_ver80(v[80]);
    row.set_invisible_flag_sex_ver81(v[81]);
    row.set_invisible_flag_sex_ver82(v[82]);
    row.set_invisible_flag_sex_ver83(v[83]);
    row.set_invisible_flag_sex_ver84(v[84]);
    row.set_invisible_flag_sex_ver85(v[85]);
    row.set_invisible_flag_sex_ver86(v[86]);
    row.set_invisible_flag_sex_ver87(v[87]);
    row.set_invisible_flag_sex_ver88(v[88]);
    row.set_invisible_flag_sex_ver89(v[89]);
    row.set_invisible_flag_sex_ver90(v[90]);
    row.set_invisible_flag_sex_ver91(v[91]);
    row.set_invisible_flag_sex_ver92(v[92]);
    row.set_invisible_flag_sex_ver93(v[93]);
    row.set_invisible_flag_sex_ver94(v[94]);
    row.set_invisible_flag_sex_ver95(v[95]);
}
