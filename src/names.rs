//! Mario names and descriptions for the Vagabond Knight set while Mario mode is on (the set is
//! Mario's body then, see equip.rs). The loaded FMG text is patched in memory: the string offset
//! entries of those items point at our strings, and are put back when Mario mode ends.
//!
//! Layout (from The Grand Archives' cheat table): [[MsgRepository]+8] = FMG table by binder id;
//! FMG +0x0C group count, +0x18 pointer to the string offset table, +0x28 groups of 0x10 bytes
//! (offset index, first id, last id, pad); a string lives at fmg + offset.

use std::sync::Mutex;

use eldenring::cs::MsgRepositoryImp;
use fromsoftware_shared::FromStatic;

use crate::{explore, log};

/// FMG binder ids: names (patch/base/DLC), short descriptions, long descriptions.
const NAME_FMGS: [usize; 4] = [117, 12, 313, 413];
const INFO_FMGS: [usize; 3] = [22, 314, 414];
const CAPTION_FMGS: [usize; 3] = [26, 315, 415];

/// (item id, name, short description, long description)
const TEXT: [(u32, &str, &str, &str); 4] = [
    (
        660000,
        "Mario's Cap",
        "Red cap marked with an M.",
        "Red cap marked with an M, worn by a plumber from a distant kingdom.\n\n\
         Said to have been knocked from its owner's head countless times, yet it always finds its way back. \
         Without it, even a hero takes far more damage.",
    ),
    (
        660100,
        "Mario's Overalls",
        "Blue overalls with golden buttons.",
        "Blue overalls with two golden buttons, over a red shirt.\n\n\
         Worn by the hero who leapt into paintings to rescue a princess. \
         Made for jumping, not for blocking blades, yet the one who wears them seldom stands still long enough to be struck.",
    ),
    (
        660200,
        "Mario's Gloves",
        "Plain white gloves.",
        "Plain white gloves.\n\n\
         The fists that wear them have broken bricks, bullies and the shell of a tyrant king. \
         They know no incantation but a punch, a kick and a ground pound.",
    ),
    (
        660300,
        "Mario's Shoes",
        "Sturdy brown work shoes.",
        "Sturdy brown work shoes.\n\n\
         They have carried their wearer through a triple jump, a wall kick and a long fall onto a foe's head. \
         Wahoo.",
    ),
];

struct Patch {
    slot: usize,
    original: u64,
    replacement: u64,
}

static PATCHES: Mutex<Vec<Patch>> = Mutex::new(Vec::new());

/// Address of the string-offset slot for `id` in FMG `fmg`, if that FMG has the id.
fn slot_for(fmg: usize, id: u32) -> Option<usize> {
    let ranges = unsafe { *((fmg + 0xC) as *const u32) } as usize;
    let offsets = unsafe { *((fmg + 0x18) as *const usize) };
    if ranges > 100_000 || offsets == 0 {
        return None;
    }
    for r in 0..ranges {
        // groups at +0x28: offset index, first id, last id, pad
        let e = fmg + 0x28 + r * 0x10;
        let (index, first, last) = unsafe { (*(e as *const i32), *((e + 4) as *const i32), *((e + 8) as *const i32)) };
        if (first..=last).contains(&(id as i32)) {
            let i = (index + (id as i32 - first)) as usize;
            let slot = offsets + i * 8;
            // an empty entry means "no text here", leave it
            return (unsafe { *(slot as *const u64) } != 0).then_some(slot);
        }
    }
    None
}

/// Finds every text slot to patch and builds the replacement strings (once).
fn prepare() {
    let mut patches = PATCHES.lock().unwrap_or_else(|e| e.into_inner());
    if !patches.is_empty() {
        return;
    }
    let Ok(repo) = (unsafe { MsgRepositoryImp::instance() }) else { return };
    // [[repo+8]] is the FMG table (the cheat table reads one level deeper than it looks)
    let Some(level1) = explore::read_u64(repo as *const _ as usize + 8) else { return };
    let Some(table) = explore::read_u64(level1 as usize) else { return };
    let table = table as usize;
    let valid = |f: usize| {
        explore::read_u64(table + f * 8)
            .filter(|&p| p != 0 && explore::readable(p as usize, 0x40))
            .is_some_and(|p| explore::read_u64(p as usize + 0x18).is_some_and(|o| explore::readable(o as usize, 8)))
    };
    log(format!("names: repo {:#x}, table {table:#x}, FMG 12 valid {}", repo as *const _ as usize, valid(12)));
    for (id, name, info, caption) in TEXT {
        for (fmgs, text) in [(&NAME_FMGS[..], name), (&INFO_FMGS[..], info), (&CAPTION_FMGS[..], caption)] {
            let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
            let string = Box::leak(wide.into_boxed_slice()).as_ptr() as usize;
            for &f in fmgs {
                let Some(fmg) = explore::read_u64(table + f * 8).filter(|&p| p != 0) else { continue };
                let fmg = fmg as usize;
                if !explore::readable(fmg, 0x40) {
                    continue;
                }
                if let Some(slot) = slot_for(fmg, id) {
                    let original = unsafe { *(slot as *const u64) };
                    patches.push(Patch { slot, original, replacement: string.wrapping_sub(fmg) as u64 });
                }
            }
        }
    }
    log(format!("names: {} text entries found for the Mario set", patches.len()));
}

/// Mario names on (true) or the original Vagabond texts back (false).
pub fn apply(mario: bool) {
    if mario {
        prepare();
    }
    for p in PATCHES.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        unsafe { *(p.slot as *mut u64) = if mario { p.replacement } else { p.original } };
    }
}

/// Every loaded text slot whose string is exactly `text` (all FMGs, all ids).
fn find_text(text: &str) -> Vec<(usize, usize)> {
    let Ok(repo) = (unsafe { MsgRepositoryImp::instance() }) else { return Vec::new() };
    let Some(table) = explore::read_u64(repo as *const _ as usize + 8).and_then(|l| explore::read_u64(l as usize)) else {
        return Vec::new();
    };
    let wanted: Vec<u16> = text.encode_utf16().collect();
    let mut out = Vec::new();
    for f in 0..1024usize {
        let Some(fmg) = explore::read_u64(table as usize + f * 8).filter(|&p| p != 0).map(|p| p as usize) else { continue };
        if !explore::readable(fmg, 0x40) {
            continue;
        }
        let ranges = unsafe { *((fmg + 0xC) as *const u32) } as usize;
        let offsets = unsafe { *((fmg + 0x18) as *const usize) };
        if ranges > 100_000 || !explore::readable(offsets, 8) {
            continue;
        }
        for r in 0..ranges {
            let e = fmg + 0x28 + r * 0x10;
            let (index, first, last) = unsafe { (*(e as *const i32), *((e + 4) as *const i32), *((e + 8) as *const i32)) };
            if last < first || last - first > 1_000_000 {
                continue;
            }
            for k in 0..=(last - first) as usize {
                let slot = offsets + (index as usize + k) * 8;
                let off = unsafe { *(slot as *const u64) } as usize;
                if off == 0 {
                    continue;
                }
                let s = fmg + off;
                let matches = (0..=wanted.len()).all(|i| {
                    let c = unsafe { *((s + i * 2) as *const u16) };
                    if i == wanted.len() { c == 0 } else { c == wanted[i] }
                });
                if matches {
                    out.push((slot, fmg));
                }
            }
        }
    }
    out
}

/// Temporarily shows `replacement` for message `id` of FMG `binder`. Returns the patch to undo
/// with `restore`, or None while that text isn't loaded.
pub fn override_id(binder: usize, id: i32, replacement: &str) -> Option<(usize, u64)> {
    let repo = unsafe { MsgRepositoryImp::instance() }.ok()?;
    let table = explore::read_u64(repo as *const _ as usize + 8).and_then(|l| explore::read_u64(l as usize))?;
    let fmg = explore::read_u64(table as usize + binder * 8).filter(|&p| p != 0)? as usize;
    if !explore::readable(fmg, 0x40) {
        return None;
    }
    let ranges = unsafe { *((fmg + 0xC) as *const u32) } as usize;
    let offsets = unsafe { *((fmg + 0x18) as *const usize) };
    if ranges > 100_000 || !explore::readable(offsets, 8) {
        return None;
    }
    for r in 0..ranges {
        let e = fmg + 0x28 + r * 0x10;
        let (index, first, last) = unsafe { (*(e as *const i32), *((e + 4) as *const i32), *((e + 8) as *const i32)) };
        if id < first || id > last {
            continue;
        }
        let slot = offsets + (index + (id - first)) as usize * 8;
        if !explore::readable(slot, 8) {
            return None;
        }
        let wide: Vec<u16> = replacement.encode_utf16().chain([0]).collect();
        let string = Box::leak(wide.into_boxed_slice()).as_ptr() as usize;
        let original = unsafe { *(slot as *const u64) };
        unsafe { *(slot as *mut u64) = string.wrapping_sub(fmg) as u64 };
        return Some((slot, original));
    }
    None
}

/// Temporarily shows `replacement` wherever the game would show `text`. Returns the patches to
/// undo with `restore`.
pub fn override_text(text: &str, replacement: &str) -> Vec<(usize, u64)> {
    let wide: Vec<u16> = replacement.encode_utf16().chain([0]).collect();
    let string = Box::leak(wide.into_boxed_slice()).as_ptr() as usize;
    let slots = find_text(text);
    log(format!("names: {} slot(s) with {text:?}", slots.len()));
    slots
        .into_iter()
        .map(|(slot, fmg)| {
            let original = unsafe { *(slot as *const u64) };
            unsafe { *(slot as *mut u64) = string.wrapping_sub(fmg) as u64 };
            (slot, original)
        })
        .collect()
}

/// The Vagabond is Mario in character creation too (the card's picture is his, assets). Tried
/// until the text is loaded; only finds the English name.
pub fn class_name() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    if DONE.load(Ordering::Relaxed) {
        return;
    }
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if last.is_some_and(|t| t.elapsed().as_secs_f32() < 2.0) {
        return;
    }
    *last = Some(std::time::Instant::now());
    if !override_text("Vagabond", "Mario").is_empty() {
        DONE.store(true, Ordering::Relaxed);
    }
}

pub fn restore(patches: &[(usize, u64)]) {
    for &(slot, original) in patches {
        unsafe { *(slot as *mut u64) = original };
    }
}

/// The game's text with this id, from whichever loaded FMG has it (boss names: NpcName).
pub fn text(id: i32) -> Option<String> {
    let repo = unsafe { MsgRepositoryImp::instance() }.ok()?;
    let table = explore::read_u64(explore::read_u64(repo as *const _ as usize + 8)? as usize)? as usize;
    for f in 0..1024usize {
        let Some(fmg) = explore::read_u64(table + f * 8).filter(|&p| p != 0).map(|p| p as usize) else { continue };
        if !explore::readable(fmg, 0x40) {
            continue;
        }
        let Some(slot) = slot_for(fmg, id as u32) else { continue };
        let s = fmg + unsafe { *(slot as *const u64) } as usize;
        let mut units = Vec::new();
        while units.len() < 200 {
            let c = unsafe { *((s + units.len() * 2) as *const u16) };
            if c == 0 {
                break;
            }
            units.push(c);
        }
        if !units.is_empty() {
            return Some(String::from_utf16_lossy(&units));
        }
    }
    None
}
