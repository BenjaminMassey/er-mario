//! Yoshi's model from the player's SM64 ROM (found there by yoshi.rs): his display lists turned
//! into vertices for Torrent's body mesh, his colours and eye textures into one albedo.

use std::collections::HashMap;

use super::flver::MountVertex;
use super::tex::Image;
use crate::yoshi::{self, be16, be32};

/// Albedo: 4 x 4 tiles, one per colour or texture, textures with a border of their edge pixels
const SIZE: usize = 1024;
const TILE: usize = SIZE / 4;
const PAD: usize = 24;
/// His textures are all 16 x 16 RGBA16
const TEXTURE: usize = 16;

pub struct Model {
    pub verts: Vec<MountVertex>,
    pub tris: Vec<[u16; 3]>,
    pub albedo: Image,
}

#[derive(Clone, Copy)]
struct Vtx {
    pos: [i16; 3],
    uv: [i16; 2],
    normal: [i8; 3],
    /// what it's drawn with: a light's colour, or a texture (offset in the segment) and its scale
    light: [u8; 3],
    texture: Option<(usize, [u32; 2])>,
}

struct State {
    loaded: [Option<Vtx>; 16],
    light: [u8; 3],
    texture: Option<usize>,
    scale: Option<[u32; 2]>,
}

/// F3D, as far as his model uses it: vertices, triangles, lights, texture on and off.
fn display_list(seg: &[u8], mut at: usize, st: &mut State, out: &mut Vec<[Vtx; 3]>, depth: usize) -> Option<()> {
    if depth > 8 {
        return None;
    }
    loop {
        let (w0, w1) = (be32(seg, at)?, be32(seg, at + 4)?);
        at += 8;
        let ptr = w1 as usize & 0xFF_FFFF;
        match w0 >> 24 {
            0xB8 => return Some(()),
            0x06 => {
                display_list(seg, ptr, st, out, depth + 1)?;
                if (w0 >> 16) & 0xFF == 1 {
                    return Some(());
                }
            }
            0x04 => {
                let (n, first) = (((w0 >> 20) & 0xF) as usize + 1, ((w0 >> 16) & 0xF) as usize);
                for k in 0..n {
                    let v = ptr + k * 16;
                    let s16 = |o: usize| be16(seg, v + o);
                    let b = |o: usize| seg.get(v + o).map(|&x| x as i8);
                    *st.loaded.get_mut(first + k)? = Some(Vtx {
                        pos: [s16(0)?, s16(2)?, s16(4)?],
                        uv: [s16(8)?, s16(10)?],
                        normal: [b(12)?, b(13)?, b(14)?],
                        light: st.light,
                        texture: st.texture.zip(st.scale),
                    });
                }
            }
            0xBF => {
                let v = |shift: u32| st.loaded.get(((w1 >> shift) & 0xFF) as usize / 10).copied().flatten();
                out.push([v(16)?, v(8)?, v(0)?]);
            }
            // (the diffuse light; the ambient one follows at 0x88)
            0x03 if (w0 >> 16) & 0xFF == 0x86 => st.light = [*seg.get(ptr)?, *seg.get(ptr + 1)?, *seg.get(ptr + 2)?],
            0xFD => st.texture = Some(ptr),
            0xBB => {
                if w0 & 0xFF != 0 {
                    st.scale = Some([w1 >> 16, w1 & 0xFFFF]);
                } else {
                    (st.scale, st.texture) = (None, None);
                }
            }
            _ => {}
        }
    }
}

fn rgba16(seg: &[u8], at: usize) -> Option<Vec<[f32; 4]>> {
    (0..TEXTURE * TEXTURE)
        .map(|i| {
            let p = be16(seg, at + i * 2)? as u16;
            let c = |shift: u16| ((p >> shift) & 31) as f32 / 31.0;
            Some([c(11), c(6), c(1), 1.0])
        })
        .collect()
}

pub fn model(rom: &[u8]) -> Option<Model> {
    let y = yoshi::load(rom)?;
    let mut parts: Vec<(usize, Vec<[Vtx; 3]>)> = Vec::new();
    for p in &y.parts {
        let Some(slot) = p.slot else { continue };
        let mut tris = Vec::new();
        for &dl in &p.lists {
            let mut st = State { loaded: [None; 16], light: [255; 3], texture: None, scale: None };
            display_list(&y.seg, dl, &mut st, &mut tris, 0)?;
        }
        parts.push((slot, tris));
    }
    // one tile per colour and per texture
    #[derive(PartialEq, Eq, Hash, Clone, Copy, PartialOrd, Ord)]
    enum Tile {
        Colour([u8; 3]),
        /// a texture, and the light's colour it's multiplied with (the nostrils are a dark blot
        /// on white, lit green; the eyes are lit white)
        Texture(usize, [u8; 3]),
    }
    let tile_of = |v: &Vtx| v.texture.map_or(Tile::Colour(v.light), |(t, _)| Tile::Texture(t, v.light));
    let mut tiles: Vec<Tile> = parts.iter().flat_map(|(_, tris)| tris.iter().map(|t| tile_of(&t[0]))).collect();
    tiles.sort();
    tiles.dedup();
    if tiles.len() > 16 {
        return None;
    }
    let mut albedo = Image::new(SIZE, SIZE, [0.0, 0.0, 0.0, 1.0]);
    for (i, tile) in tiles.iter().enumerate() {
        let (x0, y0) = (i % 4 * TILE, i / 4 * TILE);
        let texture = match tile {
            Tile::Texture(at, _) => Some(rgba16(&y.seg, *at)?),
            Tile::Colour(_) => None,
        };
        for py in 0..TILE {
            for px in 0..TILE {
                albedo.px[(y0 + py) * SIZE + x0 + px] = match (tile, &texture) {
                    (Tile::Colour(c), _) => [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, 1.0],
                    (Tile::Texture(_, c), Some(t)) => {
                        let inner = TILE - 2 * PAD;
                        let cell = |p: usize| (p.saturating_sub(PAD).min(inner - 1) * TEXTURE / inner).min(TEXTURE - 1);
                        let p = t[cell(py) * TEXTURE + cell(px)];
                        let mix = |k: usize| p[k] * c[k] as f32 / 255.0;
                        [mix(0), mix(1), mix(2), 1.0]
                    }
                    _ => [0.0, 0.0, 0.0, 1.0],
                };
            }
        }
    }
    let mut verts: Vec<MountVertex> = Vec::new();
    let mut index: HashMap<(usize, [i16; 3], [i8; 3], [i64; 2]), u16> = HashMap::new();
    let mut tris = Vec::new();
    let unit = yoshi::UNIT as f64;
    for (slot, part) in &parts {
        for tri in part {
            // (a triangle's tile is its first vertex's, like its colour in SM64's lighting)
            let i = tiles.iter().position(|t| *t == tile_of(&tri[0]))?;
            let (x0, y0) = ((i % 4 * TILE) as f64, (i / 4 * TILE) as f64);
            let mut ids = [0u16; 3];
            for (k, v) in tri.iter().enumerate() {
                let uv = match v.texture {
                    Some((_, scale)) => {
                        let inner = (TILE - 2 * PAD) as f64;
                        let s = [0, 1].map(|a| (v.uv[a] as f64 / 32.0 * scale[a] as f64 / 65536.0 / TEXTURE as f64).clamp(0.0, 1.0));
                        [x0 + PAD as f64 + s[0] * inner, y0 + PAD as f64 + s[1] * inner]
                    }
                    None => [x0 + TILE as f64 / 2.0, y0 + TILE as f64 / 2.0],
                };
                let key = (*slot, v.pos, v.normal, [uv[0].round() as i64, uv[1].round() as i64]);
                ids[k] = match index.get(&key) {
                    Some(&id) => id,
                    None => {
                        let id = u16::try_from(verts.len()).ok()?;
                        // SM64 is mirrored on X
                        verts.push(MountVertex {
                            pos: [-(v.pos[0] as f64) * unit, v.pos[1] as f64 * unit, v.pos[2] as f64 * unit],
                            normal: [-(v.normal[0] as f64), v.normal[1] as f64, v.normal[2] as f64],
                            uv: [uv[0] / SIZE as f64, uv[1] / SIZE as f64],
                            bone: *slot,
                        });
                        index.insert(key, id);
                        id
                    }
                };
            }
            tris.push(ids);
        }
    }
    Some(Model { verts, tris, albedo })
}
