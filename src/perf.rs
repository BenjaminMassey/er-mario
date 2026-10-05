//! Worst cases of the mod's own parts. The averages in the perf log hide a hitch: one slow frame
//! in a few hundred. Each part times itself with `span`, and every 2 s the log names the parts
//! whose slowest run was long enough to feel.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

pub const FRAME: usize = 0;
pub const HUD: usize = 1;
pub const INPUT: usize = 2;
pub const POSE: usize = 3;
pub const POSE_LATE: usize = 4;
pub const MENU_MODEL: usize = 5;
pub const OVERLAY: usize = 6;
pub const LAKITU: usize = 7;
// inside the frame task
pub const MOVING: usize = 8;
pub const COLLISION: usize = 9;
pub const SURFACES: usize = 10;
pub const TARGETS: usize = 11;
pub const TICK: usize = 12;
// inside the HUD task
pub const MENU_WALK: usize = 13;
pub const CLASS_NAME: usize = 14;
// inside the collision query
pub const HAVOK_QUERY: usize = 15;
pub const TRIANGLES: usize = 16;
pub const FLOOR_PATCHES: usize = 17;
const NAMES: [&str; 18] = [
    "frame task",
    "HUD task",
    "input task",
    "pose task",
    "late pose tasks",
    "menu model hook",
    "overlay",
    "camera",
    "moving objects",
    "collision query",
    "loading surfaces",
    "finding targets",
    "SM64 tick",
    "menu model walk",
    "class name search",
    "reading Havok",
    "turning triangles",
    "floor patches",
];

/// per part: slowest run and all runs together since the last report (ns)
static WORST: [AtomicU64; 18] = [const { AtomicU64::new(0) }; 18];
static TOTAL: [AtomicU64; 18] = [const { AtomicU64::new(0) }; 18];
/// A run this long is worth a line (ms): a quarter of a frame at 60 fps.
const SLOW_MS: f32 = 4.0;
/// So are many short runs that add up to this over the 2 s (ms): 5% of the time.
const BUSY_MS: f32 = 100.0;

pub struct Span(usize, Instant);

pub fn span(part: usize) -> Span {
    Span(part, Instant::now())
}

impl Drop for Span {
    fn drop(&mut self) {
        let ns = self.1.elapsed().as_nanos() as u64;
        WORST[self.0].fetch_max(ns, Ordering::Relaxed);
        TOTAL[self.0].fetch_add(ns, Ordering::Relaxed);
    }
}

/// The slow parts since the last call, if any: "HUD task 12.3 ms (41 ms in all), ...".
pub fn report() -> Option<String> {
    let mut slow: Vec<(f32, f32, &str)> = (0..NAMES.len())
        .map(|i| (WORST[i].swap(0, Ordering::Relaxed) as f32 / 1e6, TOTAL[i].swap(0, Ordering::Relaxed) as f32 / 1e6, NAMES[i]))
        .filter(|p| p.0 >= SLOW_MS || p.1 >= BUSY_MS)
        .collect();
    slow.sort_by(|a, b| b.0.total_cmp(&a.0));
    (!slow.is_empty()).then(|| slow.iter().map(|(worst, all, name)| format!("{name} {worst:.1} ms ({all:.0} ms in all)")).collect::<Vec<_>>().join(", "))
}
