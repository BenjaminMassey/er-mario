//! The Mario set's menu icons, rendered from the SM64 model: cap (head), overalls (torso and
//! limbs), gloves (hands), shoes (feet). A small software rasteriser: z-buffer, 4x supersampling,
//! SM64 colours and textures, soft key/fill lighting, transparent background like the game's icons.

use glam::{DMat3, DVec3};

use super::flver::{CELLS, mean_u, textured};
use super::model::{MarioModel, Tri};
use super::tex::Image;

const SIZE: usize = 160;
const SS: usize = 4;

/// (SM64 parts, yaw and pitch in degrees); order: cap, overalls, gloves, shoes
const ICONS: [(&[i32], (f64, f64)); 4] = [
    (&[3], (150.0, 12.0)),
    (&[1, 2, 4, 5, 7, 8, 10, 11, 13, 14], (155.0, 8.0)),
    (&[6, 9], (160.0, 20.0)),
    (&[12, 15], (145.0, 25.0)),
];

struct RTri {
    part: i32,
    p: [DVec3; 3],
    n: [DVec3; 3],
    uv: [[f64; 2]; 3],
    col: [DVec3; 3],
    textured: bool,
    /// the atlas cell its texture is in
    cell: usize,
}

fn prepare(model: &MarioModel, t: &Tri) -> RTri {
    let m = &model.matrices[t.part as usize];
    // world normals: local normals through the part's rotation (row vectors)
    let rot = DMat3::from_cols_array(&[m[0], m[1], m[2], m[4], m[5], m[6], m[8], m[9], m[10]].map(|v| v as f64)).transpose();
    let tex = textured(t);
    let mut uv = t.uv.map(|v| v.map(|x| x as f64));
    // the export caught Mario mid-blink: closed-eyes texture cell (7) -> open eyes (5)
    let mu = mean_u(t) as f64;
    if tex && mu >= 7.0 / CELLS as f64 && mu <= 8.0 / CELLS as f64 {
        for v in &mut uv {
            v[0] -= 2.0 / CELLS as f64;
        }
    }
    let cell = ((uv.iter().map(|v| v[0]).sum::<f64>() / 3.0 * CELLS as f64).floor().max(0.0) as usize).min(CELLS - 1);
    RTri {
        cell,
        part: t.part,
        p: t.world.map(|v| DVec3::new(v[0] as f64, v[1] as f64, v[2] as f64)),
        n: t.normal.map(|v| {
            let n = DVec3::new(v[0] as f64, v[1] as f64, v[2] as f64);
            // row vector times matrix
            (rot.transpose() * n).normalize_or_zero()
        }),
        uv,
        col: t.color.map(|v| DVec3::new(v[0] as f64, v[1] as f64, v[2] as f64)),
        textured: tex,
    }
}

fn sample(atlas: &[u8], u: f64, v: f64) -> [f64; 4] {
    let (w, h) = (64 * CELLS, 64);
    let x = (u * w as f64 - 0.5).clamp(0.0, (w - 1) as f64);
    let y = (v * h as f64 - 0.5).clamp(0.0, (h - 1) as f64);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f64, y - y0 as f64);
    let px = |x: usize, y: usize| -> [f64; 4] { std::array::from_fn(|c| atlas[(y * w + x) * 4 + c] as f64 / 255.0) };
    let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
    std::array::from_fn(|k| a[k] * (1.0 - fx) * (1.0 - fy) + b[k] * fx * (1.0 - fy) + c[k] * (1.0 - fx) * fy + d[k] * fx * fy)
}

/// Supersampled, premultiplied by coverage: `finish` makes the final image.
fn raster(model: &MarioModel, parts: &[i32], (yaw, pitch): (f64, f64), (w, h): (usize, usize)) -> Image {
    let mut tris: Vec<RTri> = model.tris.iter().filter(|t| parts.contains(&t.part)).map(|t| prepare(model, t)).collect();
    if parts.len() == 2 {
        // a pair (hands, feet): pull the two pieces together so they fill the icon
        let center = |q: i32| {
            let pts: Vec<DVec3> = tris.iter().filter(|t| t.part == q).flat_map(|t| t.p).collect();
            pts.iter().copied().sum::<DVec3>() / pts.len().max(1) as f64
        };
        let (c0, c1) = (center(parts[0]), center(parts[1]));
        let mid = (c0 + c1) / 2.0;
        for (q, c) in [(parts[0], c0), (parts[1], c1)] {
            let off = c - mid;
            let pts: Vec<DVec3> = tris.iter().filter(|t| t.part == q).flat_map(|t| t.p).collect();
            let (lo, hi) = pts.iter().fold((DVec3::MAX, DVec3::MIN), |(a, b), &p| (a.min(p), b.max(p)));
            let extent = (hi - lo).max_element();
            let shift = off - off.normalize_or_zero() * extent * 0.55;
            for t in tris.iter_mut().filter(|t| t.part == q) {
                t.p = t.p.map(|p| p - shift);
            }
        }
    }
    let (yaw, pitch) = (yaw.to_radians(), pitch.to_radians());
    let ry = DMat3::from_cols(DVec3::new(yaw.cos(), 0.0, -yaw.sin()), DVec3::Y, DVec3::new(yaw.sin(), 0.0, yaw.cos()));
    let rx = DMat3::from_cols(DVec3::X, DVec3::new(0.0, pitch.cos(), pitch.sin()), DVec3::new(0.0, -pitch.sin(), pitch.cos()));
    let rot = rx * ry;
    let (lo, hi) = tris.iter().flat_map(|t| t.p).fold((DVec3::MAX, DVec3::MIN), |(a, b), p| (a.min(p), b.max(p)));
    let center = (lo + hi) / 2.0;
    for t in &mut tris {
        t.p = t.p.map(|p| rot * (p - center));
        t.n = t.n.map(|n| rot * n);
    }
    let (ex, ey) = tris.iter().flat_map(|t| t.p).fold((1e-9, 1e-9), |(x, y): (f64, f64), p| (x.max(p.x.abs()), y.max(p.y.abs())));
    let scale = (w as f64 * 0.44 / ex).min(h as f64 * 0.44 / ey);
    let mut zbuf = vec![f64::INFINITY; w * h];
    let mut img = Image::new(w, h, [0.0; 4]);
    let light = DVec3::new(-0.5, 0.7, -0.6).normalize();
    for t in &tris {
        let sx = t.p.map(|p| w as f64 / 2.0 + p.x * scale);
        let sy = t.p.map(|p| h as f64 / 2.0 - p.y * scale);
        let x0 = sx.iter().copied().fold(f64::MAX, f64::min).floor().max(0.0) as i64;
        let x1 = (sx.iter().copied().fold(f64::MIN, f64::max).ceil() as i64).min(w as i64 - 1);
        let y0 = sy.iter().copied().fold(f64::MAX, f64::min).floor().max(0.0) as i64;
        let y1 = (sy.iter().copied().fold(f64::MIN, f64::max).ceil() as i64).min(h as i64 - 1);
        let d = (sy[1] - sy[2]) * (sx[0] - sx[2]) + (sx[2] - sx[1]) * (sy[0] - sy[2]);
        if x1 < x0 || y1 < y0 || d.abs() < 1e-9 {
            continue;
        }
        for py in y0..=y1 {
            for px in x0..=x1 {
                let (gx, gy) = (px as f64 + 0.5, py as f64 + 0.5);
                let w0 = ((sy[1] - sy[2]) * (gx - sx[2]) + (sx[2] - sx[1]) * (gy - sy[2])) / d;
                let w1 = ((sy[2] - sy[0]) * (gx - sx[2]) + (sx[0] - sx[2]) * (gy - sy[2])) / d;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let ws = [w0, w1, w2];
                let bary = |v: [DVec3; 3]| v[0] * w0 + v[1] * w1 + v[2] * w2;
                let mut z = w0 * t.p[0].z + w1 * t.p[1].z + w2 * t.p[2].z;
                if t.textured {
                    z -= 0.5; // SM64 decals (eyes, logo, buttons) sit on the surface
                }
                let i = py as usize * w + px as usize;
                if z >= zbuf[i] {
                    continue;
                }
                zbuf[i] = z;
                let mut c = bary(t.col);
                if t.textured {
                    // SM64 clamps its textures: UVs past the edge (the eyes' corners) must not
                    // run into the next cell of the atlas (the half-closed eye's lash)
                    let half = 0.5 / (64 * CELLS) as f64;
                    let u: f64 = (0..3).map(|k| ws[k] * t.uv[k][0]).sum();
                    let u = u.clamp(t.cell as f64 / CELLS as f64 + half, (t.cell + 1) as f64 / CELLS as f64 - half);
                    let v = (0..3).map(|k| ws[k] * t.uv[k][1]).sum();
                    let s = sample(&model.atlas, u, v);
                    c = c * (1.0 - s[3]) + DVec3::new(s[0], s[1], s[2]) * s[3];
                }
                let mut n = bary(t.n).normalize_or_zero();
                if n.dot(DVec3::new(0.0, 0.0, -1.0)) < 0.0 {
                    n = -n;
                }
                let diffuse = n.dot(light).clamp(0.0, 1.0);
                let rim = (1.0 - n.z.abs()).clamp(0.0, 1.0).powi(3);
                let shade = 0.5 + 0.6 * diffuse + 0.15 * rim;
                let c = (c * shade).clamp(DVec3::ZERO, DVec3::ONE);
                img.px[i] = [c.x as f32, c.y as f32, c.z as f32, 1.0];
            }
        }
    }
    img
}

/// Shrinks a raster by `f` to straight-alpha RGBA.
fn finish(img: &Image, f: usize) -> Vec<u8> {
    // straight alpha after averaging: un-premultiply the edge pixels
    let mut small = img.shrink(f);
    for p in &mut small.px {
        if p[3] > 0.0 {
            for c in 0..3 {
                p[c] /= p[3];
            }
        }
    }
    small.rgba8()
}

/// cap, overalls, gloves, shoes (160x160 RGBA each)
pub fn render_all(model: &MarioModel) -> [Vec<u8>; 4] {
    ICONS.map(|(parts, angles)| finish(&raster(model, parts, angles, (SIZE * SS, SIZE * SS)), SS))
}

/// The Vagabond's card in character creation (SB_Preset MENU_Ch_01), hi and low: 1064x1368 and
/// half that, the sprite's size down to whole BC7 blocks.
pub const PORTRAIT: (usize, usize) = (1064, 1368);

/// All of Mario for that card, as (hi, low) RGBA.
pub fn portrait(model: &MarioModel) -> (Vec<u8>, Vec<u8>) {
    let parts: Vec<i32> = (1..16).collect();
    let img = raster(model, &parts, (160.0, 6.0), (PORTRAIT.0 * 2, PORTRAIT.1 * 2));
    (finish(&img, 2), finish(&img, 4))
}
