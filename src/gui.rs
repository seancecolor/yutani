use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::driver::spyder::{CmdSet, SpyderWorker, XyzReading};
use crate::icc::{
    bt1886_params, ciede2000, eotf_target_luminance, evaluate_grayscale_de00,
    evaluate_verification_patch_de00, generate_profile_from_measurements, xyz_to_lab,
    CalibrationMeasurements, ColorGamut, DisplayTarget, EotfConfig, VcgtSummary,
    VerificationPatchReading,
};

/// Duration of the Yutani CRT terminal startup sequence.
const BOOT_SEQUENCE_DURATION: Duration = Duration::from_millis(3000);

/// Time to let the display panel settle on a new color patch before triggering the sensor read.
const PATCH_SETTLE_DURATION: Duration = Duration::from_millis(450);

pub fn run_gui() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_app_id("yutani.calibration")
            .with_title("YUTANI // COLORIMETRY SUBSYSTEM")
            .with_icon(std::sync::Arc::new(build_yutani_window_icon()))
            .with_inner_size([960.0, 680.0])
            .with_fullscreen(false),
        follow_system_theme: false,
        default_theme: eframe::Theme::Dark,
        ..Default::default()
    };

    eframe::run_native(
        "YUTANI // COLORIMETRY SUBSYSTEM",
        options,
        Box::new(|cc| {
            configure_yutani_theme(&cc.egui_ctx);
            Box::new(YutaniApp::new())
        }),
    )
}

/// Generate a crisp 64x64 RGBA window icon matching `assets/yutani.svg` (CRT amber chassis,
/// CIE gamut triangle, and Spyder optical aperture crosshair).
fn build_yutani_window_icon() -> egui::IconData {
    let size = 64usize;
    let rgba = render_yutani_icon_rgba(size);
    egui::IconData {
        rgba,
        width: size as u32,
        height: size as u32,
    }
}

/// Point-to-segment distance helper in normalized 256x256 icon space.
fn dist_to_segment(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let vx = bx - ax;
    let vy = by - ay;
    let wx = px - ax;
    let wy = py - ay;
    let c1 = wx * vx + wy * vy;
    if c1 <= 0.0 {
        return ((px - ax).powi(2) + (py - ay).powi(2)).sqrt();
    }
    let c2 = vx * vx + vy * vy;
    if c2 <= c1 {
        return ((px - bx).powi(2) + (py - by).powi(2)).sqrt();
    }
    let b = c1 / c2;
    let pbx = ax + b * vx;
    let pby = ay + b * vy;
    ((px - pbx).powi(2) + (py - pby).powi(2)).sqrt()
}

/// Render the Yutani application icon at any square resolution (`size x size`) as RGBA8888 bytes.
/// Includes rounded CRT chassis bezel, corner telemetry brackets, CIE 1931 gamut triangle with
/// R/G/B primary vertex nodes, Spyder optical aperture ring, and D65 whitepoint crosshair.
pub fn render_yutani_icon_rgba(size: usize) -> Vec<u8> {
    let s = size.max(16);
    let mut rgba = vec![0u8; s * s * 4];
    let scale = 256.0_f32 / (s as f32);

    // Triangle vertices in 256x256 space: Green (top), Red (right), Blue (bottom-left)
    let gx = 96.0_f32;
    let gy = 48.0_f32;
    let rx = 204.0_f32;
    let ry = 166.0_f32;
    let bx = 58.0_f32;
    let by = 196.0_f32;

    for y in 0..s {
        for x in 0..s {
            let idx = (y * s + x) * 4;
            let ux = (x as f32 + 0.5) * scale;
            let uy = (y as f32 + 0.5) * scale;

            // Signed distance to rounded box centered at (128, 128), half-size 116, corner radius 26
            let qx = (ux - 128.0).abs() - (120.0 - 26.0);
            let qy = (uy - 128.0).abs() - (120.0 - 26.0);
            let sdf_box =
                (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - 26.0;

            if sdf_box > 1.0 {
                // Transparent outside the rounded CRT frame
                continue;
            }

            // Base dark CRT phosphor radial gradient
            let cdx = ux - 128.0;
            let cdy = uy - 132.0;
            let cdist = (cdx * cdx + cdy * cdy).sqrt();
            let vignette = (1.0 - (cdist / 160.0)).clamp(0.0, 1.0);
            let mut r_col = 8.0 + 18.0 * vignette;
            let mut g_col = 8.0 + 12.0 * vignette;
            let mut b_col = 6.0 + 4.0 * vignette;

            // Subtle CRT scanline texture
            if ((uy as i32) / 4) % 2 == 0 {
                r_col *= 0.90;
                g_col *= 0.90;
                b_col *= 0.90;
            }

            // Barycentric coordinates for CIE gamut triangle fill
            let denom = (ry - by) * (gx - bx) + (bx - rx) * (gy - by);
            let w1 = ((ry - by) * (ux - bx) + (bx - rx) * (uy - by)) / denom;
            let w2 = ((by - gy) * (ux - bx) + (gx - bx) * (uy - by)) / denom;
            let w3 = 1.0 - w1 - w2;
            if w1 >= 0.0 && w2 >= 0.0 && w3 >= 0.0 {
                // Blend subtle RGB gamut glow inside the triangle
                let tri_r = w1 * 65.0 + w2 * 245.0 + w3 * 70.0;
                let tri_g = w1 * 230.0 + w2 * 75.0 + w3 * 145.0;
                let tri_b = w1 * 95.0 + w2 * 64.0 + w3 * 255.0;
                r_col = r_col * 0.72 + tri_r * 0.28;
                g_col = g_col * 0.72 + tri_g * 0.28;
                b_col = b_col * 0.72 + tri_b * 0.28;
            }

            // Inner dark bezel ring
            if (sdf_box + 14.0).abs() <= 1.6 {
                r_col = 90.0;
                g_col = 60.0;
                b_col = 14.0;
            }

            // Outer amber CRT bezel border
            if sdf_box >= -5.5 {
                r_col = 255.0;
                g_col = 180.0;
                b_col = 40.0;
            }

            // Corner telemetry brackets
            let in_corner_x = (ux >= 30.0 && ux <= 58.0 && (uy - 32.0).abs() <= 2.2)
                || (ux >= 198.0 && ux <= 226.0 && (uy - 32.0).abs() <= 2.2)
                || (ux >= 30.0 && ux <= 58.0 && (uy - 224.0).abs() <= 2.2)
                || (ux >= 198.0 && ux <= 226.0 && (uy - 224.0).abs() <= 2.2);
            let in_corner_y = (uy >= 30.0 && uy <= 58.0 && (ux - 32.0).abs() <= 2.2)
                || (uy >= 30.0 && uy <= 58.0 && (ux - 224.0).abs() <= 2.2)
                || (uy >= 198.0 && uy <= 226.0 && (ux - 32.0).abs() <= 2.2)
                || (uy >= 198.0 && uy <= 226.0 && (ux - 224.0).abs() <= 2.2);
            if in_corner_x || in_corner_y {
                r_col = 255.0;
                g_col = 185.0;
                b_col = 50.0;
            }

            // CIE Gamut triangle edges
            let d_gr = dist_to_segment(ux, uy, gx, gy, rx, ry);
            let d_rb = dist_to_segment(ux, uy, rx, ry, bx, by);
            let d_bg = dist_to_segment(ux, uy, bx, by, gx, gy);
            let d_tri = d_gr.min(d_rb).min(d_bg);
            if d_tri <= 2.2 {
                r_col = 255.0;
                g_col = 180.0;
                b_col = 40.0;
            }

            // Spyder Optical Aperture Reticle Ring (center 128, 134)
            let rdx = ux - 128.0;
            let rdy = uy - 134.0;
            let rdist = (rdx * rdx + rdy * rdy).sqrt();

            // Darken interior of inner optical lens
            if rdist < 26.0 {
                r_col *= 0.45;
                g_col *= 0.45;
                b_col *= 0.45;
            }

            // Outer dashed amber reticle ring (r = 46)
            if (rdist - 46.0).abs() <= 2.4 {
                let angle = rdy.atan2(rdx);
                let sector = ((angle + std::f32::consts::PI) / (std::f32::consts::PI / 6.0)) as i32;
                if sector % 2 == 0 {
                    r_col = 255.0;
                    g_col = 190.0;
                    b_col = 55.0;
                }
            }

            // Inner cyan optical aperture ring (r = 25)
            if (rdist - 25.0).abs() <= 2.2 {
                r_col = 90.0;
                g_col = 235.0;
                b_col = 184.0;
            }

            // D65 Whitepoint Crosshair
            if (rdx.abs() <= 1.6 && rdy.abs() <= 36.0) || (rdy.abs() <= 1.6 && rdx.abs() <= 36.0) {
                r_col = 255.0;
                g_col = 185.0;
                b_col = 45.0;
            }

            // D65 Whitepoint center target dot
            if rdist <= 6.0 {
                r_col = 255.0;
                g_col = 244.0;
                b_col = 208.0;
            }

            // Primary vertex nodes (Green, Red, Blue)
            let dg = ((ux - gx).powi(2) + (uy - gy).powi(2)).sqrt();
            let dr = ((ux - rx).powi(2) + (uy - ry).powi(2)).sqrt();
            let db = ((ux - bx).powi(2) + (uy - by).powi(2)).sqrt();
            if dg <= 8.5 {
                if dg >= 6.5 {
                    r_col = 10.0;
                    g_col = 10.0;
                    b_col = 8.0;
                } else {
                    r_col = 75.0;
                    g_col = 230.0;
                    b_col = 96.0;
                }
            } else if dr <= 8.5 {
                if dr >= 6.5 {
                    r_col = 10.0;
                    g_col = 10.0;
                    b_col = 8.0;
                } else {
                    r_col = 245.0;
                    g_col = 75.0;
                    b_col = 64.0;
                }
            } else if db <= 8.5 {
                if db >= 6.5 {
                    r_col = 10.0;
                    g_col = 10.0;
                    b_col = 8.0;
                } else {
                    r_col = 85.0;
                    g_col = 155.0;
                    b_col = 255.0;
                }
            }

            let alpha = if sdf_box > 0.0 {
                ((1.0 - sdf_box) * 255.0).clamp(0.0, 255.0) as u8
            } else {
                255u8
            };

            rgba[idx] = r_col.clamp(0.0, 255.0) as u8;
            rgba[idx + 1] = g_col.clamp(0.0, 255.0) as u8;
            rgba[idx + 2] = b_col.clamp(0.0, 255.0) as u8;
            rgba[idx + 3] = alpha;
        }
    }

    rgba
}

fn png_crc32(chunk_type: &[u8; 4], data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in chunk_type.iter().chain(data.iter()) {
        crc ^= b as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

fn write_png_chunk(out: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(chunk_type);
    out.extend_from_slice(data);
    let crc = png_crc32(chunk_type, data);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// Export the Yutani RGBA icon directly to a standalone valid PNG file on disk without requiring
/// external tools (`rsvg-convert` or `ImageMagick`).
pub fn export_yutani_png_icon(path: &std::path::Path, size: u32) -> anyhow::Result<()> {
    let s = size.clamp(16, 512) as usize;
    let rgba = render_yutani_icon_rgba(s);

    // Build raw scanlines (each row prefixed with filter byte 0 = None)
    let row_bytes = s * 4;
    let mut raw_scanlines = Vec::with_capacity((row_bytes + 1) * s);
    for y in 0..s {
        raw_scanlines.push(0u8);
        let start = y * row_bytes;
        raw_scanlines.extend_from_slice(&rgba[start..start + row_bytes]);
    }

    // Wrap raw scanlines in valid zlib stream using uncompressed DEFLATE blocks (RFC 1950 / 1951)
    let mut zlib_data = Vec::with_capacity(raw_scanlines.len() + 64);
    zlib_data.push(0x78); // CMF: deflate, 32K window
    zlib_data.push(0x01); // FLG: no preset dict, FCHECK valid (0x7801 % 31 == 0)

    let chunks: Vec<&[u8]> = raw_scanlines.chunks(65535).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        let is_last = i + 1 == chunks.len();
        zlib_data.push(if is_last { 0x01 } else { 0x00 });
        let len = chunk.len() as u16;
        let nlen = !len;
        zlib_data.extend_from_slice(&len.to_le_bytes());
        zlib_data.extend_from_slice(&nlen.to_le_bytes());
        zlib_data.extend_from_slice(chunk);
    }

    // Compute Adler-32 checksum over uncompressed scanlines
    let mut s1 = 1u32;
    let mut s2 = 0u32;
    for &b in &raw_scanlines {
        s1 = (s1 + (b as u32)) % 65521;
        s2 = (s2 + s1) % 65521;
    }
    let adler = (s2 << 16) | s1;
    zlib_data.extend_from_slice(&adler.to_be_bytes());

    let mut png = Vec::with_capacity(zlib_data.len() + 128);
    // PNG magic signature
    png.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);

    // IHDR chunk (width, height, bit_depth=8, color_type=6 RGBA, compression=0, filter=0, interlace=0)
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(s as u32).to_be_bytes());
    ihdr.extend_from_slice(&(s as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    write_png_chunk(&mut png, b"IHDR", &ihdr);

    // IDAT chunk
    write_png_chunk(&mut png, b"IDAT", &zlib_data);

    // IEND chunk
    write_png_chunk(&mut png, b"IEND", &[]);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, png)?;
    Ok(())
}

fn configure_yutani_theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    let bg_black = egui::Color32::from_rgb(8, 8, 8);
    let panel_dark = egui::Color32::from_rgb(14, 12, 8);
    let amber = egui::Color32::from_rgb(255, 180, 40);
    let dim_amber = egui::Color32::from_rgb(140, 95, 20);
    let dark_amber = egui::Color32::from_rgb(45, 28, 8);
    let hover_amber = egui::Color32::from_rgb(85, 52, 12);

    style.override_font_id = Some(egui::FontId::monospace(14.0));
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 8.0);

    let mut visuals = egui::Visuals::dark();
    visuals.window_fill = bg_black;
    visuals.panel_fill = panel_dark;
    visuals.faint_bg_color = bg_black;
    visuals.extreme_bg_color = bg_black;
    visuals.override_text_color = Some(amber);
    visuals.window_rounding = egui::Rounding::ZERO;
    visuals.menu_rounding = egui::Rounding::ZERO;

    visuals.widgets.noninteractive.bg_fill = bg_black;
    visuals.widgets.noninteractive.weak_bg_fill = bg_black;
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, dim_amber);
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, amber);
    visuals.widgets.noninteractive.rounding = egui::Rounding::ZERO;

    visuals.widgets.inactive.bg_fill = dark_amber;
    visuals.widgets.inactive.weak_bg_fill = dark_amber;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, amber);
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, amber);
    visuals.widgets.inactive.rounding = egui::Rounding::ZERO;

    visuals.widgets.hovered.bg_fill = hover_amber;
    visuals.widgets.hovered.weak_bg_fill = hover_amber;
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.5_f32, amber);
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.5_f32, amber);
    visuals.widgets.hovered.rounding = egui::Rounding::ZERO;

    visuals.widgets.active.bg_fill = amber;
    visuals.widgets.active.weak_bg_fill = amber;
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.5_f32, amber);
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.5_f32, bg_black);
    visuals.widgets.active.rounding = egui::Rounding::ZERO;

    style.visuals = visuals;
    ctx.set_style(style);
}

/// Selectable patch sequence density preset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PatchPreset {
    /// 14 patches: Black, White, R/G/B primaries, and 9 linear grayscale steps (10%..90%).
    Standard14,
    /// 33 patches: Dense shadow toe (2%, 4%, 6%, 8%), highlight shoulder (95%, 98%),
    /// and 4-step separate R, G, B primary sub-ramps (20%, 40%, 60%, 80%).
    #[default]
    Extended33,
    /// 54 patches: Full 18-step grayscale + 5-step R/G/B primary sub-ramps +
    /// R/G/B/C/M/Y saturation sweeps (25%, 50%, 75%) + ColorChecker verification patches.
    Studio54,
}

impl PatchPreset {
    pub const ALL: [PatchPreset; 3] = [
        PatchPreset::Standard14,
        PatchPreset::Extended33,
        PatchPreset::Studio54,
    ];

    pub fn button_label(self) -> &'static str {
        match self {
            Self::Standard14 => " [ 14-PATCH FAST ] ",
            Self::Extended33 => " [ 33-PATCH PRECISION (TOE/SHOULDER + RGB RAMPS) ] ",
            Self::Studio54 => " [ 54-PATCH STUDIO (+ SATURATION & COLORCHECKER) ] ",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::Standard14 => "14-PATCH FAST",
            Self::Extended33 => "33-PATCH PRECISION",
            Self::Studio54 => "54-PATCH STUDIO",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct PatchSpec {
    label: &'static str,
    color: egui::Color32,
    /// Exact normalized `[R, G, B]` stimulus in `0.0..=1.0` for 10-bit/12-bit spatio-temporal FRC dithering.
    exact_rgb: [f64; 3],
    kind: PatchKind,
}

#[derive(Clone, Copy, Debug)]
enum PatchKind {
    Black,
    White,
    Red,
    Green,
    Blue,
    Gray(f64),
    /// Separate per-channel sub-ramp patch (`channel`: 0=R, 1=G, 2=B; `level` in `0.0..=1.0`).
    PrimaryRamp { channel: usize, level: f64 },
    /// Saturation sweep or ColorChecker verification patch with normalized `[R, G, B]` stimulus.
    Verification {
        label: &'static str,
        stimulus_rgb: [f64; 3],
    },
}

fn build_calibration_patches(preset: PatchPreset) -> Vec<PatchSpec> {
    let mut patches = vec![
        PatchSpec {
            label: "BLACK LEVEL (0%)",
            color: egui::Color32::from_rgb(0, 0, 0),
            exact_rgb: [0.0, 0.0, 0.0],
            kind: PatchKind::Black,
        },
        PatchSpec {
            label: "PEAK WHITE (100%)",
            color: egui::Color32::from_rgb(255, 255, 255),
            exact_rgb: [1.0, 1.0, 1.0],
            kind: PatchKind::White,
        },
        PatchSpec {
            label: "RED PRIMARY (100%)",
            color: egui::Color32::from_rgb(255, 0, 0),
            exact_rgb: [1.0, 0.0, 0.0],
            kind: PatchKind::Red,
        },
        PatchSpec {
            label: "GREEN PRIMARY (100%)",
            color: egui::Color32::from_rgb(0, 255, 0),
            exact_rgb: [0.0, 1.0, 0.0],
            kind: PatchKind::Green,
        },
        PatchSpec {
            label: "BLUE PRIMARY (100%)",
            color: egui::Color32::from_rgb(0, 0, 255),
            exact_rgb: [0.0, 0.0, 1.0],
            kind: PatchKind::Blue,
        },
    ];

    // 1. Grayscale steps: Standard 9-step vs Dense 16-step (with 2%, 4%, 6%, 8% shadow toe & 95%, 98% shoulder)
    let gray_steps: &[(&'static str, f64)] = match preset {
        PatchPreset::Standard14 => &[
            ("GRAY 10%", 0.10),
            ("GRAY 20%", 0.20),
            ("GRAY 30%", 0.30),
            ("GRAY 40%", 0.40),
            ("GRAY 50%", 0.50),
            ("GRAY 60%", 0.60),
            ("GRAY 70%", 0.70),
            ("GRAY 80%", 0.80),
            ("GRAY 90%", 0.90),
        ],
        PatchPreset::Extended33 | PatchPreset::Studio54 => &[
            ("GRAY 02% (SHADOW TOE)", 0.02),
            ("GRAY 04% (SHADOW TOE)", 0.04),
            ("GRAY 06% (SHADOW TOE)", 0.06),
            ("GRAY 08% (SHADOW TOE)", 0.08),
            ("GRAY 10%", 0.10),
            ("GRAY 15%", 0.15),
            ("GRAY 20%", 0.20),
            ("GRAY 30%", 0.30),
            ("GRAY 40%", 0.40),
            ("GRAY 50%", 0.50),
            ("GRAY 60%", 0.60),
            ("GRAY 70%", 0.70),
            ("GRAY 80%", 0.80),
            ("GRAY 90%", 0.90),
            ("GRAY 95% (SHOULDER)", 0.95),
            ("GRAY 98% (SHOULDER)", 0.98),
        ],
    };

    for &(label, v) in gray_steps {
        let level = (v * 255.0).round() as u8;
        patches.push(PatchSpec {
            label,
            color: egui::Color32::from_rgb(level, level, level),
            exact_rgb: [v, v, v],
            kind: PatchKind::Gray(v),
        });
    }

    // 2. Separate R, G, B Primary Sub-Ramps (20%, 40%, 60%, 80%)
    if matches!(preset, PatchPreset::Extended33 | PatchPreset::Studio54) {
        let sub_ramps: &[(&'static str, usize, f64)] = &[
            ("RED SUB-RAMP 20%", 0, 0.20),
            ("RED SUB-RAMP 40%", 0, 0.40),
            ("RED SUB-RAMP 60%", 0, 0.60),
            ("RED SUB-RAMP 80%", 0, 0.80),
            ("GREEN SUB-RAMP 20%", 1, 0.20),
            ("GREEN SUB-RAMP 40%", 1, 0.40),
            ("GREEN SUB-RAMP 60%", 1, 0.60),
            ("GREEN SUB-RAMP 80%", 1, 0.80),
            ("BLUE SUB-RAMP 20%", 2, 0.20),
            ("BLUE SUB-RAMP 40%", 2, 0.40),
            ("BLUE SUB-RAMP 60%", 2, 0.60),
            ("BLUE SUB-RAMP 80%", 2, 0.80),
        ];
        for &(label, ch, level) in sub_ramps {
            let u = (level * 255.0).round() as u8;
            let (color, exact_rgb) = match ch {
                0 => (egui::Color32::from_rgb(u, 0, 0), [level, 0.0, 0.0]),
                1 => (egui::Color32::from_rgb(0, u, 0), [0.0, level, 0.0]),
                _ => (egui::Color32::from_rgb(0, 0, u), [0.0, 0.0, level]),
            };
            patches.push(PatchSpec {
                label,
                color,
                exact_rgb,
                kind: PatchKind::PrimaryRamp { channel: ch, level },
            });
        }
    }

    // 3. Saturation Sweeps (25%, 50%, 75%) & ColorChecker Verification Patches
    if matches!(preset, PatchPreset::Studio54) {
        let verif_specs: &[(&'static str, [u8; 3])] = &[
            // Primary & Secondary Saturation / Luminance Sweeps
            ("SAT RED 50% (R+W MIX)", [255, 128, 128]),
            ("SAT GREEN 50% (G+W MIX)", [128, 255, 128]),
            ("SAT BLUE 50% (B+W MIX)", [128, 128, 255]),
            ("CYAN SECONDARY 100%", [0, 255, 255]),
            ("CYAN SAT 50%", [128, 255, 255]),
            ("CYAN DRIVE 50%", [0, 128, 128]),
            ("MAGENTA SECONDARY 100%", [255, 0, 255]),
            ("MAGENTA SAT 50%", [255, 128, 255]),
            ("MAGENTA DRIVE 50%", [128, 0, 128]),
            ("YELLOW SECONDARY 100%", [255, 255, 0]),
            ("YELLOW SAT 50%", [255, 255, 128]),
            ("YELLOW DRIVE 50%", [128, 128, 0]),
            // Classic ColorChecker Reference Patches (sRGB 8-bit)
            ("CC01 // DARK SKIN", [115, 82, 68]),
            ("CC02 // LIGHT SKIN", [194, 150, 130]),
            ("CC03 // BLUE SKY", [98, 122, 157]),
            ("CC04 // FOLIAGE", [87, 108, 67]),
            ("CC05 // BLUE FLOWER", [133, 128, 177]),
            ("CC06 // BLUISH GREEN", [103, 189, 170]),
            ("CC07 // ORANGE", [214, 126, 44]),
            ("CC12 // ORANGE YELLOW", [224, 163, 46]),
            ("CC18 // CYAN", [8, 133, 161]),
        ];

        for &(label, rgb) in verif_specs {
            let stimulus_rgb = [
                rgb[0] as f64 / 255.0,
                rgb[1] as f64 / 255.0,
                rgb[2] as f64 / 255.0,
            ];
            patches.push(PatchSpec {
                label,
                color: egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
                exact_rgb: stimulus_rgb,
                kind: PatchKind::Verification {
                    label,
                    stimulus_rgb,
                },
            });
        }
    }

    patches
}

/// Render an optical calibration patch with optional 10-bit/12-bit Spatio-Temporal FRC Dithering.
///
/// Because `egui::Color32` quantizes RGB stimuli to 8-bit integers (`0..=255`), fractional patches
/// such as `2% = 5.10/255`, `4% = 10.20/255`, `6% = 15.30/255`, `8% = 20.40/255`, `15% = 38.25/255`,
/// `95% = 242.25/255`, and `98% = 249.90/255` suffer up to `0.40 LSB` quantization error in 8-bit mode.
/// When `enable_frc` is enabled, this function fills the patch with `floor(c * 255)` and overlays a
/// 4x4 Bayer spatial + temporal phase-shifted micro-grid inside the center sensor aperture so the
/// Spyder photodiode integrates the exact floating-point stimulus over its 714 ms integration window.
fn draw_frc_dithered_patch(
    painter: &egui::Painter,
    patch_rect: egui::Rect,
    exact_rgb: [f64; 3],
    fallback_color: egui::Color32,
    frame_parity: u64,
    enable_frc: bool,
) -> bool {
    if !enable_frc {
        painter.rect_filled(patch_rect, 0.0, fallback_color);
        return false;
    }

    let scaled = [
        (exact_rgb[0].clamp(0.0, 1.0) * 255.0),
        (exact_rgb[1].clamp(0.0, 1.0) * 255.0),
        (exact_rgb[2].clamp(0.0, 1.0) * 255.0),
    ];
    let lo = [
        scaled[0].floor() as u8,
        scaled[1].floor() as u8,
        scaled[2].floor() as u8,
    ];
    let hi = [
        (lo[0] as u16 + 1).min(255) as u8,
        (lo[1] as u16 + 1).min(255) as u8,
        (lo[2] as u16 + 1).min(255) as u8,
    ];
    let frac = [
        scaled[0] - (lo[0] as f64),
        scaled[1] - (lo[1] as f64),
        scaled[2] - (lo[2] as f64),
    ];

    let has_fractional = frac.iter().any(|&f| f > 0.01 && f < 0.99);
    if !has_fractional {
        painter.rect_filled(patch_rect, 0.0, fallback_color);
        return false;
    }

    let lo_color = egui::Color32::from_rgb(lo[0], lo[1], lo[2]);
    painter.rect_filled(patch_rect, 0.0, lo_color);

    // 4x4 ordered Bayer matrix thresholds in 0..16
    const BAYER_4X4: [[u8; 4]; 4] = [
        [0, 8, 2, 10],
        [12, 4, 14, 6],
        [3, 11, 1, 9],
        [15, 7, 13, 5],
    ];

    // Subdivide the center 240x240 sensor aperture into a 24x24 micro-cell grid (10x10 px per cell)
    // phase-shifted by `frame_parity` each frame so both spatial and temporal integration average out.
    let aperture = egui::Rect::from_center_size(patch_rect.center(), egui::vec2(240.0, 240.0))
        .intersect(patch_rect);
    let grid_n = 24usize;
    let cell_w = aperture.width() / (grid_n as f32);
    let cell_h = aperture.height() / (grid_n as f32);
    let phase_x = (frame_parity as usize) & 3;
    let phase_y = ((frame_parity as usize) >> 2) & 3;

    for gy in 0..grid_n {
        for gx in 0..grid_n {
            let bayer_val = BAYER_4X4[(gy + phase_y) & 3][(gx + phase_x) & 3] as f64;
            let threshold = (bayer_val + 0.5) / 16.0;
            let r_u8 = if frac[0] > threshold { hi[0] } else { lo[0] };
            let g_u8 = if frac[1] > threshold { hi[1] } else { lo[1] };
            let b_u8 = if frac[2] > threshold { hi[2] } else { lo[2] };
            if r_u8 != lo[0] || g_u8 != lo[1] || b_u8 != lo[2] {
                let cell_rect = egui::Rect::from_min_max(
                    egui::pos2(
                        aperture.left() + (gx as f32) * cell_w,
                        aperture.top() + (gy as f32) * cell_h,
                    ),
                    egui::pos2(
                        aperture.left() + ((gx + 1) as f32) * cell_w,
                        aperture.top() + ((gy + 1) as f32) * cell_h,
                    ),
                );
                painter.rect_filled(cell_rect, 0.0, egui::Color32::from_rgb(r_u8, g_u8, b_u8));
            }
        }
    }
    true
}

/// Probe the active Wayland / KWin / `colord` display color management state.
///
/// Checks `WAYLAND_DISPLAY`, `XDG_CURRENT_DESKTOP`, and queries `kscreen-doctor -o` or `colormgr`
/// if available on the host to warn when an existing ICC profile is active during calibration.
fn probe_wayland_color_environment() -> String {
    let wayland_disp = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| String::from("unset"));
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| String::from("unknown"));
    let session_type =
        std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| String::from("unknown"));

    // 1. Check KWin / KDE Plasma outputs via `kscreen-doctor -o` if installed
    if let Ok(output) = std::process::Command::new("kscreen-doctor")
        .arg("-o")
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut icc_active = false;
            for line in stdout.lines() {
                let lower = line.to_ascii_lowercase();
                if lower.contains("icc profile:") && !lower.contains("none") {
                    icc_active = true;
                    break;
                }
            }
            let icc_note = if icc_active {
                "WARNING: Active KWin ICC profile detected! Set Color Profile to 'None' before measuring."
            } else {
                "KWin ICC Bypass Verified (No active ICC profile reported by kscreen-doctor)."
            };
            return format!(
                "WAYLAND [{wayland_disp}] // DESKTOP [{desktop}] // {icc_note}"
            );
        }
    }

    // 2. Check colord display devices via `colormgr` if installed
    if let Ok(output) = std::process::Command::new("colormgr")
        .arg("get-devices-by-kind")
        .arg("display")
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let dev_count = stdout.lines().filter(|l| l.contains("Object Path:")).count();
            return format!(
                "WAYLAND [{wayland_disp}] // SESSION [{session_type}/{desktop}] // colord active ({dev_count} display device(s))"
            );
        }
    }

    format!(
        "WAYLAND [{wayland_disp}] // SESSION [{session_type}/{desktop}] // Ensure compositor ICC profile is set to 'None' before calibration"
    )
}

/// Copy a generated `.icc` profile into the standard XDG / `colord` user ICC directories
/// (`~/.local/share/icc/` and `~/.color/icc/`) and import it into `colord` via `colormgr` if present.
fn install_and_register_icc_profile(
    profile_path: &std::path::Path,
    display: DisplayTarget,
) -> Result<String, String> {
    let file_name = profile_path
        .file_name()
        .ok_or_else(|| String::from("Invalid ICC profile filename"))?;

    let home = std::env::var("HOME").map_err(|_| String::from("HOME environment variable not set"))?;
    let home_path = PathBuf::from(home);
    let xdg_icc_dir = home_path.join(".local/share/icc");
    let legacy_icc_dir = home_path.join(".color/icc");

    std::fs::create_dir_all(&xdg_icc_dir)
        .map_err(|e| format!("Failed to create {}: {e}", xdg_icc_dir.display()))?;
    let dest_xdg = xdg_icc_dir.join(file_name);
    std::fs::copy(profile_path, &dest_xdg)
        .map_err(|e| format!("Failed to copy ICC to {}: {e}", dest_xdg.display()))?;

    if std::fs::create_dir_all(&legacy_icc_dir).is_ok() {
        let _ = std::fs::copy(profile_path, legacy_icc_dir.join(file_name));
    }

    // Attempt `colormgr import-profile <dest_xdg>` if colord CLI is available
    let colormgr_note = match std::process::Command::new("colormgr")
        .arg("import-profile")
        .arg(&dest_xdg)
        .output()
    {
        Ok(out) if out.status.success() => String::from("colord: imported via colormgr"),
        Ok(_) => String::from("colord: indexed in ~/.local/share/icc"),
        Err(_) => String::from("installed to ~/.local/share/icc (colormgr not on PATH)"),
    };

    Ok(format!(
        "INSTALLED {} FOR {} -> {} ({})",
        file_name.to_string_lossy(),
        display.short_code().to_ascii_uppercase(),
        dest_xdg.display(),
        colormgr_note
    ))
}

/// Build the default custom export path (`~/Documents/<filename>.icc` or `~/<filename>.icc`).
fn default_icc_export_path(source_profile: &std::path::Path) -> String {
    let fname = source_profile
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("yutani_calibrated.icc");
    if let Ok(home) = std::env::var("HOME") {
        let home_dir = PathBuf::from(&home);
        let docs_dir = home_dir.join("Documents");
        if docs_dir.is_dir() {
            return docs_dir.join(fname).to_string_lossy().into_owned();
        }
        return home_dir.join(fname).to_string_lossy().into_owned();
    }
    String::from(fname)
}

/// Open a native KDE Plasma (`kdialog`) or FreeDesktop (`zenity`) Save File dialog to let the
/// user pick where to save a copy of the `.icc` profile. Returns `Ok(Some(path))` if a destination
/// was chosen, `Ok(None)` if the user canceled the dialog, or `Err` if neither dialog tool is installed.
fn pick_icc_save_path_native_dialog(default_path: &str) -> Result<Option<PathBuf>, String> {
    // 1. Try KDE Plasma native `kdialog --getsavefilename`
    if let Ok(out) = std::process::Command::new("kdialog")
        .arg("--title")
        .arg("Yutani — Save ICC Color Profile Copy")
        .arg("--getsavefilename")
        .arg(default_path)
        .arg("ICC Color Profiles (*.icc *.icm)")
        .output()
    {
        if out.status.success() {
            let chosen = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !chosen.is_empty() {
                return Ok(Some(PathBuf::from(chosen)));
            }
        }
        return Ok(None);
    }

    // 2. Fallback to GTK/GNOME `zenity --file-selection --save`
    if let Ok(out) = std::process::Command::new("zenity")
        .arg("--file-selection")
        .arg("--save")
        .arg("--confirm-overwrite")
        .arg("--title=Yutani — Save ICC Color Profile Copy")
        .arg(format!("--filename={default_path}"))
        .arg("--file-filter=ICC Color Profiles (*.icc, *.icm) | *.icc *.icm")
        .output()
    {
        if out.status.success() {
            let chosen = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !chosen.is_empty() {
                return Ok(Some(PathBuf::from(chosen)));
            }
        }
        return Ok(None);
    }

    Err(String::from(
        "Neither 'kdialog' nor 'zenity' found on PATH; use the Custom Save Path box below.",
    ))
}

/// Copy the generated `.icc` profile to a user-specified file or directory path (expanding leading `~/`)
/// and return the canonical/resolved destination `PathBuf`.
fn save_icc_profile_copy(
    source_profile: &std::path::Path,
    raw_dest: &str,
) -> Result<PathBuf, String> {
    let trimmed = raw_dest.trim();
    if trimmed.is_empty() {
        return Err(String::from("Destination path is empty."));
    }

    // Expand leading `~/` to `$HOME/`
    let expanded = if let Some(rest) = trimmed.strip_prefix("~/") {
        let home =
            std::env::var("HOME").map_err(|_| String::from("HOME environment variable not set"))?;
        PathBuf::from(home).join(rest)
    } else if trimmed == "~" {
        let home =
            std::env::var("HOME").map_err(|_| String::from("HOME environment variable not set"))?;
        PathBuf::from(home)
    } else {
        PathBuf::from(trimmed)
    };

    let source_fname = source_profile
        .file_name()
        .ok_or_else(|| String::from("Invalid source ICC profile filename"))?;

    // If destination is an existing directory or ends with `/`, append the source `.icc` filename
    let mut final_dest = if expanded.is_dir() || trimmed.ends_with('/') {
        expanded.join(source_fname)
    } else {
        expanded
    };

    // Ensure `.icc` or `.icm` extension is present
    let has_icc_ext = final_dest
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let lower = e.to_ascii_lowercase();
            lower == "icc" || lower == "icm"
        })
        .unwrap_or(false);
    if !has_icc_ext {
        final_dest.set_extension("icc");
    }

    if let Some(parent) = final_dest.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directory {}: {e}", parent.display()))?;
        }
    }

    std::fs::copy(source_profile, &final_dest)
        .map_err(|e| format!("Failed to write ICC copy to {}: {e}", final_dest.display()))?;

    Ok(final_dest)
}

/// Interval between consecutive sensor reads in Live OSD Alignment mode.
const OSD_POLL_INTERVAL: Duration = Duration::from_millis(150);

/// Compute D65-normalized linear RGB balance `(r_pct, g_pct, b_pct)`, Correlated Color
/// Temperature `cct_kelvin`, and CIEDE2000 `delta_e00` relative to CIE D65 whitepoint.
fn compute_d65_alignment_metrics(reading: &XyzReading) -> ([f64; 3], f64, f64) {
    let (cx, cy, lum) = reading.to_xyy();
    if lum <= 1e-4 {
        return ([100.0, 100.0, 100.0], 6504.0, 99.9);
    }

    // Standard CIE 1931 XYZ -> Linear BT.709 / sRGB (D65 white => R = G = B = Y)
    let x = reading.x;
    let y = reading.y;
    let z = reading.z;
    let r_lin = (3.2404542 * x - 1.5371385 * y - 0.4985314 * z).max(0.0);
    let g_lin = (-0.9692660 * x + 1.8760108 * y + 0.0415560 * z).max(0.0);
    let b_lin = (0.0556434 * x - 0.2040259 * y + 1.0572252 * z).max(0.0);

    let avg = ((r_lin + g_lin + b_lin) / 3.0).max(1e-6);
    let rgb_pct = [
        (r_lin / avg) * 100.0,
        (g_lin / avg) * 100.0,
        (b_lin / avg) * 100.0,
    ];

    // McCamy's cubic approximation for Correlated Color Temperature (CCT in Kelvin)
    let denom = (0.1858 - cy).abs().max(1e-6) * if 0.1858 - cy >= 0.0 { 1.0 } else { -1.0 };
    let n = (cx - 0.3320) / denom;
    let cct = (449.0 * n.powi(3) + 3525.0 * n.powi(2) + 6823.3 * n + 5520.33).clamp(1500.0, 25000.0);

    // Perceptual CIEDE2000 (ΔE00) chromaticity difference against CIE D65 at equal luminance
    let xn = y * (0.3127 / 0.3290);
    let zn = y * ((1.0 - 0.3127 - 0.3290) / 0.3290);
    let lab_meas = xyz_to_lab(x, y, z, xn, y, zn);
    let delta_e00 = ciede2000((100.0, 0.0, 0.0), lab_meas);

    (rgb_pct, cct, delta_e00)
}

/// Draw a horizontal center-zero deviation gauge bar.
fn draw_deviation_bar(
    ui: &mut egui::Ui,
    label: &str,
    pct_diff: f64,
    max_span_pct: f64,
    bar_color: egui::Color32,
    hint: &str,
) {
    let amber = egui::Color32::from_rgb(255, 180, 40);
    let dim_amber = egui::Color32::from_rgb(90, 60, 15);
    let bg_dark = egui::Color32::from_rgb(16, 14, 10);

    ui.horizontal(|ui| {
        ui.monospace(format!("{label:<14}"));
        let (rect, _) = ui.allocate_exact_size(egui::vec2(280.0, 18.0), egui::Sense::hover());
        let painter = ui.painter();

        painter.rect_filled(rect, 0.0, bg_dark);
        painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.0_f32, dim_amber));

        let center_x = rect.center().x;
        let norm = (pct_diff / max_span_pct).clamp(-1.0, 1.0) as f32;
        let fill_x = center_x + norm * (rect.width() * 0.5);

        let bar_rect = if fill_x >= center_x {
            egui::Rect::from_min_max(
                egui::pos2(center_x, rect.top() + 2.0),
                egui::pos2(fill_x, rect.bottom() - 2.0),
            )
        } else {
            egui::Rect::from_min_max(
                egui::pos2(fill_x, rect.top() + 2.0),
                egui::pos2(center_x, rect.bottom() - 2.0),
            )
        };
        painter.rect_filled(bar_rect, 0.0, bar_color);

        // Center target tick line
        painter.line_segment(
            [
                egui::pos2(center_x, rect.top()),
                egui::pos2(center_x, rect.bottom()),
            ],
            egui::Stroke::new(2.0_f32, amber),
        );

        ui.monospace(format!(" {:+6.1}%  {}", pct_diff, hint));
    });
}

/// Draw the Yutani EOTF Tracking Curve (Target vs. Measured).
fn draw_eotf_graph(
    ui: &mut egui::Ui,
    measurements: &CalibrationMeasurements,
    eotf: EotfConfig,
) {
    let amber = egui::Color32::from_rgb(255, 180, 40);
    let dim_amber = egui::Color32::from_rgb(70, 48, 12);
    let grid_amber = egui::Color32::from_rgb(35, 24, 8);
    let meas_cyan = egui::Color32::from_rgb(90, 235, 190);
    let bg_dark = egui::Color32::from_rgb(10, 9, 6);

    ui.vertical(|ui| {
        ui.monospace("EOTF LUMINANCE // TARGET (AMBER) vs MEASURED (CYAN)");
        let (rect, _) = ui.allocate_exact_size(egui::vec2(430.0, 235.0), egui::Sense::hover());
        let painter = ui.painter();

        painter.rect_filled(rect, 0.0, bg_dark);
        painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.0_f32, dim_amber));

        let pad = 28.0;
        let plot = egui::Rect::from_min_max(
            egui::pos2(rect.left() + pad, rect.top() + 12.0),
            egui::pos2(rect.right() - 12.0, rect.bottom() - 24.0),
        );

        // 10x10 phosphor grid
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            let x = plot.left() + t * plot.width();
            let y = plot.bottom() - t * plot.height();
            painter.line_segment(
                [egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())],
                egui::Stroke::new(1.0_f32, grid_amber),
            );
            painter.line_segment(
                [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
                egui::Stroke::new(1.0_f32, grid_amber),
            );
        }
        painter.rect_stroke(plot, 0.0, egui::Stroke::new(1.0_f32, dim_amber));

        let lw = measurements.white.y.max(1.0);
        let lb = measurements.black.y.clamp(0.0, lw * 0.2);
        let span = (lw - lb).max(1e-6);

        let to_screen = |u: f64, y_norm: f64| -> egui::Pos2 {
            let px = plot.left() + (u.clamp(0.0, 1.0) as f32) * plot.width();
            let py = plot.bottom() - (y_norm.clamp(0.0, 1.0) as f32) * plot.height();
            egui::pos2(px, py)
        };

        // 1. Target EOTF curve (64 segments)
        let mut target_pts = Vec::with_capacity(65);
        for i in 0..=64 {
            let v = i as f64 / 64.0;
            let l = eotf_target_luminance(v, lw, lb, eotf);
            let y_norm = (l - lb) / span;
            target_pts.push(to_screen(v, y_norm));
        }
        for win in target_pts.windows(2) {
            painter.line_segment([win[0], win[1]], egui::Stroke::new(2.0_f32, amber));
        }

        // 2. Measured grayscale points & polyline
        if measurements.white.y > 0.5 && !measurements.grayscale.is_empty() {
            let mut meas_pts = Vec::with_capacity(measurements.grayscale.len());
            for &(v, ref r) in &measurements.grayscale {
                let y_norm = ((r.y - lb) / span).clamp(0.0, 1.0);
                let pt = to_screen(v, y_norm);
                meas_pts.push(pt);
            }
            for win in meas_pts.windows(2) {
                painter.line_segment([win[0], win[1]], egui::Stroke::new(1.5_f32, meas_cyan));
            }
            for pt in meas_pts {
                painter.circle_filled(pt, 3.0, meas_cyan);
            }
        }

        // 3. Optional separate R, G, B primary sub-ramp curves
        let prim_peaks = [
            (measurements.red.y - lb).max(1e-6),
            (measurements.green.y - lb).max(1e-6),
            (measurements.blue.y - lb).max(1e-6),
        ];
        let prim_colors = [
            egui::Color32::from_rgba_unmultiplied(245, 75, 65, 190),
            egui::Color32::from_rgba_unmultiplied(75, 230, 95, 190),
            egui::Color32::from_rgba_unmultiplied(85, 155, 255, 190),
        ];
        for c in 0..3 {
            if !measurements.primary_ramps[c].is_empty() {
                let mut ch_pts = Vec::with_capacity(measurements.primary_ramps[c].len() + 2);
                ch_pts.push(to_screen(0.0, 0.0));
                for &(u, ref r) in &measurements.primary_ramps[c] {
                    let y_norm = ((r.y - lb).max(0.0) / prim_peaks[c]).clamp(0.0, 1.0);
                    ch_pts.push(to_screen(u, y_norm));
                }
                ch_pts.push(to_screen(1.0, 1.0));
                for win in ch_pts.windows(2) {
                    painter.line_segment([win[0], win[1]], egui::Stroke::new(1.1_f32, prim_colors[c]));
                }
            }
        }

        // Axis labels
        painter.text(
            egui::pos2(plot.left(), rect.bottom() - 10.0),
            egui::Align2::LEFT_CENTER,
            "0% IN",
            egui::FontId::monospace(10.0),
            amber,
        );
        painter.text(
            egui::pos2(plot.center().x, rect.bottom() - 10.0),
            egui::Align2::CENTER_CENTER,
            "50% SIGNAL",
            egui::FontId::monospace(10.0),
            amber,
        );
        painter.text(
            egui::pos2(plot.right(), rect.bottom() - 10.0),
            egui::Align2::RIGHT_CENTER,
            "100%",
            egui::FontId::monospace(10.0),
            amber,
        );
        painter.text(
            egui::pos2(rect.left() + 4.0, plot.top() + 6.0),
            egui::Align2::LEFT_CENTER,
            "1.0",
            egui::FontId::monospace(10.0),
            amber,
        );
        painter.text(
            egui::pos2(rect.left() + 4.0, plot.bottom() - 6.0),
            egui::Align2::LEFT_CENTER,
            "0.0",
            egui::FontId::monospace(10.0),
            amber,
        );
    });
}

/// Draw the CIE 1931 xy Chromaticity Diagram with Spectral Locus, Target Gamut (Rec.709 / P3-D65),
/// secondary reference gamut, measured panel triangle, and all measured patch coordinates
/// (R, G, B primaries, Peak White, and all grayscale steps).
fn draw_cie_gamut_graph(
    ui: &mut egui::Ui,
    measurements: &CalibrationMeasurements,
    gamut: ColorGamut,
) {
    let amber = egui::Color32::from_rgb(255, 180, 40);
    let dim_amber = egui::Color32::from_rgb(80, 55, 14);
    let faint_ref = egui::Color32::from_rgb(65, 45, 14);
    let grid_amber = egui::Color32::from_rgb(35, 24, 8);
    let locus_amber = egui::Color32::from_rgb(120, 82, 22);
    let meas_cyan = egui::Color32::from_rgb(90, 235, 190);
    let gray_dot = egui::Color32::from_rgb(255, 230, 140);
    let bg_dark = egui::Color32::from_rgb(10, 9, 6);

    // Approximate CIE 1931 2-degree spectral locus (380nm..700nm) in (x, y)
    const LOCUS_XY: &[(f64, f64)] = &[
        (0.1741, 0.0050),
        (0.1733, 0.0048),
        (0.1703, 0.0058),
        (0.1644, 0.0109),
        (0.1510, 0.0227),
        (0.1241, 0.0578),
        (0.0913, 0.1327),
        (0.0454, 0.2950),
        (0.0082, 0.5384),
        (0.0139, 0.7502),
        (0.0743, 0.8338),
        (0.1547, 0.8059),
        (0.2296, 0.7543),
        (0.3016, 0.6923),
        (0.3731, 0.6245),
        (0.4441, 0.5547),
        (0.5125, 0.4866),
        (0.5752, 0.4242),
        (0.6270, 0.3725),
        (0.6658, 0.3340),
        (0.7006, 0.2993),
        (0.7260, 0.2740),
        (0.7347, 0.2653),
    ];

    ui.vertical(|ui| {
        ui.monospace(format!(
            "CIE 1931 xy GAMUT // {} TARGET vs MEASURED PATCH LOCATIONS",
            gamut.short_code().to_ascii_uppercase()
        ));
        let (rect, _) = ui.allocate_exact_size(egui::vec2(430.0, 235.0), egui::Sense::hover());
        let painter = ui.painter();

        painter.rect_filled(rect, 0.0, bg_dark);
        painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.0_f32, dim_amber));

        let pad = 28.0;
        let plot = egui::Rect::from_min_max(
            egui::pos2(rect.left() + pad, rect.top() + 12.0),
            egui::pos2(rect.right() - 12.0, rect.bottom() - 24.0),
        );

        for i in 0..=8 {
            let tx = i as f32 / 8.0;
            let x = plot.left() + tx * plot.width();
            painter.line_segment(
                [egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())],
                egui::Stroke::new(1.0_f32, grid_amber),
            );
        }
        for i in 0..=9 {
            let ty = i as f32 / 9.0;
            let y = plot.bottom() - ty * plot.height();
            painter.line_segment(
                [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
                egui::Stroke::new(1.0_f32, grid_amber),
            );
        }
        painter.rect_stroke(plot, 0.0, egui::Stroke::new(1.0_f32, dim_amber));

        // Map CIE x in [0.0..0.8], y in [0.0..0.9] -> screen coordinates
        let to_screen = |cx: f64, cy: f64| -> egui::Pos2 {
            let nx = (cx / 0.8).clamp(0.0, 1.0) as f32;
            let ny = (cy / 0.9).clamp(0.0, 1.0) as f32;
            egui::pos2(
                plot.left() + nx * plot.width(),
                plot.bottom() - ny * plot.height(),
            )
        };

        // 1. Draw CIE 1931 Spectral Locus horseshoe + purple line
        let locus_pts: Vec<egui::Pos2> = LOCUS_XY.iter().map(|&(x, y)| to_screen(x, y)).collect();
        for win in locus_pts.windows(2) {
            painter.line_segment([win[0], win[1]], egui::Stroke::new(1.0_f32, locus_amber));
        }
        if let (Some(&first), Some(&last)) = (locus_pts.first(), locus_pts.last()) {
            painter.line_segment([last, first], egui::Stroke::new(1.0_f32, locus_amber));
        }

        // 2. Draw secondary comparison triangle (faint) + active Target Gamut triangle (bright amber)
        let secondary_xy = match gamut {
            ColorGamut::Rec709 => ColorGamut::DisplayP3.reference_primaries_xy(),
            ColorGamut::DisplayP3 | ColorGamut::Native => ColorGamut::Rec709.reference_primaries_xy(),
        };
        let sec_r = to_screen(secondary_xy[0].0, secondary_xy[0].1);
        let sec_g = to_screen(secondary_xy[1].0, secondary_xy[1].1);
        let sec_b = to_screen(secondary_xy[2].0, secondary_xy[2].1);
        painter.line_segment([sec_r, sec_g], egui::Stroke::new(1.0_f32, faint_ref));
        painter.line_segment([sec_g, sec_b], egui::Stroke::new(1.0_f32, faint_ref));
        painter.line_segment([sec_b, sec_r], egui::Stroke::new(1.0_f32, faint_ref));

        let target_xy = gamut.reference_primaries_xy();
        let tgt_r = to_screen(target_xy[0].0, target_xy[0].1);
        let tgt_g = to_screen(target_xy[1].0, target_xy[1].1);
        let tgt_b = to_screen(target_xy[2].0, target_xy[2].1);
        painter.line_segment([tgt_r, tgt_g], egui::Stroke::new(1.5_f32, amber));
        painter.line_segment([tgt_g, tgt_b], egui::Stroke::new(1.5_f32, amber));
        painter.line_segment([tgt_b, tgt_r], egui::Stroke::new(1.5_f32, amber));

        // Hollow rings on target primary vertices
        for &pt in &[tgt_r, tgt_g, tgt_b] {
            painter.circle_stroke(pt, 3.5, egui::Stroke::new(1.2_f32, amber));
        }

        // 3. Measured primaries triangle & primary reading markers
        let valid_prim =
            (measurements.red.y + measurements.green.y + measurements.blue.y) > 0.5;
        let (rx, ry) = if valid_prim {
            let (x, y, _) = measurements.red.to_xyy();
            (x, y)
        } else {
            target_xy[0]
        };
        let (gx, gy) = if valid_prim {
            let (x, y, _) = measurements.green.to_xyy();
            (x, y)
        } else {
            target_xy[1]
        };
        let (bx, by) = if valid_prim {
            let (x, y, _) = measurements.blue.to_xyy();
            (x, y)
        } else {
            target_xy[2]
        };

        let r_pt = to_screen(rx, ry);
        let g_pt = to_screen(gx, gy);
        let b_pt = to_screen(bx, by);
        painter.line_segment([r_pt, g_pt], egui::Stroke::new(1.5_f32, meas_cyan));
        painter.line_segment([g_pt, b_pt], egui::Stroke::new(1.5_f32, meas_cyan));
        painter.line_segment([b_pt, r_pt], egui::Stroke::new(1.5_f32, meas_cyan));

        // Error vectors from target primary vertices to measured primary points
        painter.line_segment([tgt_r, r_pt], egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(245, 70, 60)));
        painter.line_segment([tgt_g, g_pt], egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(70, 235, 90)));
        painter.line_segment([tgt_b, b_pt], egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(80, 150, 255)));

        painter.circle_filled(r_pt, 4.0, egui::Color32::from_rgb(245, 70, 60));
        painter.circle_filled(g_pt, 4.0, egui::Color32::from_rgb(70, 235, 90));
        painter.circle_filled(b_pt, 4.0, egui::Color32::from_rgb(80, 150, 255));

        painter.text(
            egui::pos2(r_pt.x + 6.0, r_pt.y),
            egui::Align2::LEFT_CENTER,
            "R",
            egui::FontId::monospace(10.0),
            egui::Color32::from_rgb(255, 110, 100),
        );
        painter.text(
            egui::pos2(g_pt.x + 6.0, g_pt.y - 4.0),
            egui::Align2::LEFT_CENTER,
            "G",
            egui::FontId::monospace(10.0),
            egui::Color32::from_rgb(100, 245, 120),
        );
        painter.text(
            egui::pos2(b_pt.x + 6.0, b_pt.y),
            egui::Align2::LEFT_CENTER,
            "B",
            egui::FontId::monospace(10.0),
            egui::Color32::from_rgb(115, 175, 255),
        );

        // 4. D65 Target crosshair
        let d65_pt = to_screen(0.3127, 0.3290);
        painter.line_segment(
            [
                egui::pos2(d65_pt.x - 5.5, d65_pt.y),
                egui::pos2(d65_pt.x + 5.5, d65_pt.y),
            ],
            egui::Stroke::new(1.5_f32, amber),
        );
        painter.line_segment(
            [
                egui::pos2(d65_pt.x, d65_pt.y - 5.5),
                egui::pos2(d65_pt.x, d65_pt.y + 5.5),
            ],
            egui::Stroke::new(1.5_f32, amber),
        );

        // 5. Plot every measured grayscale patch location inside the gamut diagram
        let mut gray_screen_pts = Vec::new();
        for &(v, ref reading) in &measurements.grayscale {
            let (gx_i, gy_i, glum) = reading.to_xyy();
            // Skip near-zero dark patch if luminance is too low for stable chromaticity
            if glum > 0.05 && gx_i > 0.01 && gy_i > 0.01 {
                let pt = to_screen(gx_i, gy_i);
                gray_screen_pts.push((v, pt));
            }
        }
        for win in gray_screen_pts.windows(2) {
            painter.line_segment(
                [win[0].1, win[1].1],
                egui::Stroke::new(1.0_f32, egui::Color32::from_rgba_unmultiplied(255, 225, 130, 140)),
            );
        }
        for &(_v, pt) in &gray_screen_pts {
            painter.circle_filled(pt, 2.2, gray_dot);
        }

        // 6. Plot separate R, G, B sub-ramp chromaticity points and Verification / ColorChecker patches
        let sub_colors = [
            egui::Color32::from_rgb(245, 95, 85),
            egui::Color32::from_rgb(85, 235, 105),
            egui::Color32::from_rgb(95, 165, 255),
        ];
        for c in 0..3 {
            for &(_u, ref r) in &measurements.primary_ramps[c] {
                let (px, py, plum) = r.to_xyy();
                if plum > 0.05 && px > 0.01 && py > 0.01 {
                    painter.circle_filled(to_screen(px, py), 2.3, sub_colors[c]);
                }
            }
        }
        for verif in &measurements.verification {
            let (vx, vy, vlum) = verif.reading.to_xyy();
            if vlum > 0.05 && vx > 0.01 && vy > 0.01 {
                let v_pt = to_screen(vx, vy);
                painter.circle_stroke(
                    v_pt,
                    2.8,
                    egui::Stroke::new(1.2_f32, egui::Color32::from_rgb(235, 130, 255)),
                );
            }
        }

        // Peak White (100%) outer ring marker
        let (wx, wy, wlum) = measurements.white.to_xyy();
        if wlum > 0.5 {
            let w_pt = to_screen(wx, wy);
            painter.circle_stroke(w_pt, 4.5, egui::Stroke::new(1.5_f32, meas_cyan));
            painter.text(
                egui::pos2(w_pt.x + 6.0, w_pt.y - 6.0),
                egui::Align2::LEFT_CENTER,
                "W",
                egui::FontId::monospace(9.5),
                meas_cyan,
            );
        }

        // Legend & Axis labels
        painter.text(
            egui::pos2(plot.left() + 6.0, plot.top() + 8.0),
            egui::Align2::LEFT_CENTER,
            "o:Target  *:Meas RGB  ·:Gray 10-90%  O:White  +:D65",
            egui::FontId::monospace(9.5),
            amber,
        );
        painter.text(
            egui::pos2(plot.left(), rect.bottom() - 10.0),
            egui::Align2::LEFT_CENTER,
            "x=0.0",
            egui::FontId::monospace(10.0),
            amber,
        );
        painter.text(
            egui::pos2(plot.center().x, rect.bottom() - 10.0),
            egui::Align2::CENTER_CENTER,
            "CIE 1931 x",
            egui::FontId::monospace(10.0),
            amber,
        );
        painter.text(
            egui::pos2(plot.right(), rect.bottom() - 10.0),
            egui::Align2::RIGHT_CENTER,
            "x=0.8",
            egui::FontId::monospace(10.0),
            amber,
        );
        painter.text(
            egui::pos2(rect.left() + 4.0, plot.top() + 6.0),
            egui::Align2::LEFT_CENTER,
            "0.9",
            egui::FontId::monospace(10.0),
            amber,
        );
        painter.text(
            egui::pos2(rect.left() + 4.0, plot.bottom() - 6.0),
            egui::Align2::LEFT_CENTER,
            "0.0",
            egui::FontId::monospace(10.0),
            amber,
        );
    });
}

/// Draw the 1D LUT (`vcgt`) Calibration Correction graphs:
/// 1) Left chart: Absolute VCGT Transfer Curves (`Output Drive` vs `Input Signal`) for R, G, B.
/// 2) Right chart: Per-Channel Drive Delta (`Δ% = (VCGT_out - Signal_in) * 100%`) showing the exact
///    white-balance and EOTF correction applied across 0%..100%.
fn draw_vcgt_correction_graph(ui: &mut egui::Ui, vcgt_summary: &VcgtSummary) {
    let amber = egui::Color32::from_rgb(255, 180, 40);
    let dim_amber = egui::Color32::from_rgb(75, 50, 14);
    let grid_amber = egui::Color32::from_rgb(35, 24, 8);
    let bg_dark = egui::Color32::from_rgb(10, 9, 6);
    let ch_colors = [
        egui::Color32::from_rgb(245, 75, 65),  // Red LUT
        egui::Color32::from_rgb(75, 230, 95),  // Green LUT
        egui::Color32::from_rgb(85, 155, 255), // Blue LUT
    ];

    ui.horizontal_wrapped(|ui| {
        // Chart A: Absolute 1D LUT Curves (R, G, B output vs linear diagonal)
        ui.vertical(|ui| {
            ui.monospace("1D VCGT LUT TRANSFER CURVES // R, G, B OUTPUT vs UNCORRECTED");
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(430.0, 210.0), egui::Sense::hover());
            let painter = ui.painter();

            painter.rect_filled(rect, 0.0, bg_dark);
            painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.0_f32, dim_amber));

            let pad = 28.0;
            let plot = egui::Rect::from_min_max(
                egui::pos2(rect.left() + pad, rect.top() + 12.0),
                egui::pos2(rect.right() - 12.0, rect.bottom() - 24.0),
            );

            for i in 0..=10 {
                let t = i as f32 / 10.0;
                let x = plot.left() + t * plot.width();
                let y = plot.bottom() - t * plot.height();
                painter.line_segment(
                    [egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())],
                    egui::Stroke::new(1.0_f32, grid_amber),
                );
                painter.line_segment(
                    [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
                    egui::Stroke::new(1.0_f32, grid_amber),
                );
            }
            painter.rect_stroke(plot, 0.0, egui::Stroke::new(1.0_f32, dim_amber));

            // Identity diagonal (uncorrected 1:1 ramp)
            painter.line_segment(
                [
                    egui::pos2(plot.left(), plot.bottom()),
                    egui::pos2(plot.right(), plot.top()),
                ],
                egui::Stroke::new(1.0_f32, dim_amber),
            );

            // R, G, B VCGT curves sampled at 65 points
            for c in 0..3 {
                let mut pts = Vec::with_capacity(65);
                for i in 0..=64 {
                    let v = i as f64 / 64.0;
                    let rgb = vcgt_summary.sample_rgb(v);
                    let px = plot.left() + (v as f32) * plot.width();
                    let py = plot.bottom() - (rgb[c].clamp(0.0, 1.0) as f32) * plot.height();
                    pts.push(egui::pos2(px, py));
                }
                for win in pts.windows(2) {
                    painter.line_segment([win[0], win[1]], egui::Stroke::new(1.6_f32, ch_colors[c]));
                }
            }

            painter.text(
                egui::pos2(plot.left(), rect.bottom() - 10.0),
                egui::Align2::LEFT_CENTER,
                "0% IN",
                egui::FontId::monospace(10.0),
                amber,
            );
            painter.text(
                egui::pos2(plot.center().x, rect.bottom() - 10.0),
                egui::Align2::CENTER_CENTER,
                "LUT OUTPUT (R/G/B)",
                egui::FontId::monospace(10.0),
                amber,
            );
            painter.text(
                egui::pos2(plot.right(), rect.bottom() - 10.0),
                egui::Align2::RIGHT_CENTER,
                "100%",
                egui::FontId::monospace(10.0),
                amber,
            );
        });

        ui.add_space(12.0);

        // Chart B: VCGT Correction Delta (%) = (VCGT_c(v) - v) * 100%
        ui.vertical(|ui| {
            ui.monospace("APPLIED VCGT CORRECTION DELTA // ΔDRIVE (%) PER CHANNEL");
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(430.0, 210.0), egui::Sense::hover());
            let painter = ui.painter();

            painter.rect_filled(rect, 0.0, bg_dark);
            painter.rect_stroke(rect, 0.0, egui::Stroke::new(1.0_f32, dim_amber));

            let pad = 34.0;
            let plot = egui::Rect::from_min_max(
                egui::pos2(rect.left() + pad, rect.top() + 12.0),
                egui::pos2(rect.right() - 12.0, rect.bottom() - 24.0),
            );

            // Determine max percentage deviation so small or large corrections remain clearly visible
            let mut max_abs_pct = 5.0_f64;
            for i in 0..=64 {
                let v = i as f64 / 64.0;
                let rgb = vcgt_summary.sample_rgb(v);
                for c in 0..3 {
                    let diff_pct = ((rgb[c] - v) * 100.0).abs();
                    if diff_pct > max_abs_pct {
                        max_abs_pct = diff_pct;
                    }
                }
            }
            let span_pct = (max_abs_pct * 1.15).clamp(5.0, 35.0);

            for i in 0..=10 {
                let t = i as f32 / 10.0;
                let x = plot.left() + t * plot.width();
                let y = plot.bottom() - t * plot.height();
                painter.line_segment(
                    [egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())],
                    egui::Stroke::new(1.0_f32, grid_amber),
                );
                painter.line_segment(
                    [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
                    egui::Stroke::new(1.0_f32, grid_amber),
                );
            }
            painter.rect_stroke(plot, 0.0, egui::Stroke::new(1.0_f32, dim_amber));

            // Center 0.0% baseline
            let zero_y = plot.center().y;
            painter.line_segment(
                [
                    egui::pos2(plot.left(), zero_y),
                    egui::pos2(plot.right(), zero_y),
                ],
                egui::Stroke::new(1.5_f32, amber),
            );

            // Plot R, G, B delta curves
            for c in 0..3 {
                let mut pts = Vec::with_capacity(65);
                for i in 0..=64 {
                    let v = i as f64 / 64.0;
                    let rgb = vcgt_summary.sample_rgb(v);
                    let diff_pct = (rgb[c] - v) * 100.0;
                    let norm_y = (diff_pct / span_pct).clamp(-1.0, 1.0) as f32;
                    let px = plot.left() + (v as f32) * plot.width();
                    let py = zero_y - norm_y * (plot.height() * 0.5);
                    pts.push(egui::pos2(px, py));
                }
                for win in pts.windows(2) {
                    painter.line_segment([win[0], win[1]], egui::Stroke::new(1.8_f32, ch_colors[c]));
                }
            }

            painter.text(
                egui::pos2(rect.left() + 3.0, plot.top() + 6.0),
                egui::Align2::LEFT_CENTER,
                format!("+{:.0}%", span_pct),
                egui::FontId::monospace(9.5),
                amber,
            );
            painter.text(
                egui::pos2(rect.left() + 3.0, zero_y),
                egui::Align2::LEFT_CENTER,
                " 0%",
                egui::FontId::monospace(9.5),
                amber,
            );
            painter.text(
                egui::pos2(rect.left() + 3.0, plot.bottom() - 6.0),
                egui::Align2::LEFT_CENTER,
                format!("-{:.0}%", span_pct),
                egui::FontId::monospace(9.5),
                amber,
            );
            painter.text(
                egui::pos2(plot.left(), rect.bottom() - 10.0),
                egui::Align2::LEFT_CENTER,
                "0% IN",
                egui::FontId::monospace(10.0),
                amber,
            );
            painter.text(
                egui::pos2(plot.center().x, rect.bottom() - 10.0),
                egui::Align2::CENTER_CENTER,
                "CORRECTION OFFSET (R/G/B)",
                egui::FontId::monospace(10.0),
                amber,
            );
            painter.text(
                egui::pos2(plot.right(), rect.bottom() - 10.0),
                egui::Align2::RIGHT_CENTER,
                "100%",
                egui::FontId::monospace(10.0),
                amber,
            );
        });
    });
}

/// Render the animated Yutani CRT terminal startup sequence (without emblem/logo).
fn draw_boot_sequence(
    ui: &mut egui::Ui,
    progress: f32,
    sensor_status: &str,
) -> bool {
    let mut skip = false;
    let rect = ui.max_rect();
    let center = rect.center();
    let painter = ui.painter();

    let bg_black = egui::Color32::from_rgb(6, 6, 5);
    let amber = egui::Color32::from_rgb(255, 180, 40);
    let bright_amber = egui::Color32::from_rgb(255, 225, 120);
    let dim_amber = egui::Color32::from_rgb(95, 65, 16);
    let faint_grid = egui::Color32::from_rgb(22, 16, 6);

    painter.rect_filled(rect, 0.0, bg_black);

    // Subtle horizontal CRT scanlines + sweeping phosphor beam
    let scan_step = 6.0;
    let mut y_scan = rect.top();
    while y_scan < rect.bottom() {
        painter.line_segment(
            [egui::pos2(rect.left(), y_scan), egui::pos2(rect.right(), y_scan)],
            egui::Stroke::new(1.0_f32, faint_grid),
        );
        y_scan += scan_step;
    }

    let beam_y = rect.top() + ((progress * 2.3).fract()) * rect.height();
    painter.line_segment(
        [egui::pos2(rect.left(), beam_y), egui::pos2(rect.right(), beam_y)],
        egui::Stroke::new(2.0_f32, egui::Color32::from_rgba_unmultiplied(255, 190, 50, 55)),
    );

    // Outer CRT telemetry corner brackets
    let m = 24.0;
    let blen = 36.0;
    let corners = [
        (
            egui::pos2(rect.left() + m, rect.top() + m),
            egui::vec2(blen, 0.0),
            egui::vec2(0.0, blen),
        ),
        (
            egui::pos2(rect.right() - m, rect.top() + m),
            egui::vec2(-blen, 0.0),
            egui::vec2(0.0, blen),
        ),
        (
            egui::pos2(rect.left() + m, rect.bottom() - m),
            egui::vec2(blen, 0.0),
            egui::vec2(0.0, -blen),
        ),
        (
            egui::pos2(rect.right() - m, rect.bottom() - m),
            egui::vec2(-blen, 0.0),
            egui::vec2(0.0, -blen),
        ),
    ];
    for (origin, dx, dy) in corners {
        painter.line_segment([origin, origin + dx], egui::Stroke::new(2.0_f32, amber));
        painter.line_segment([origin, origin + dy], egui::Stroke::new(2.0_f32, amber));
    }

    // Clean Yutani terminal header banner
    let header_y = rect.top() + 120.0;
    painter.text(
        egui::pos2(center.x, header_y),
        egui::Align2::CENTER_CENTER,
        "Y U T A N I   W A Y L A N D",
        egui::FontId::monospace(24.0),
        bright_amber,
    );
    painter.text(
        egui::pos2(center.x, header_y + 28.0),
        egui::Align2::CENTER_CENTER,
        "OPTICAL COLORIMETRY & 1D LUT CALIBRATION SUBSYSTEM",
        egui::FontId::monospace(13.0),
        amber,
    );
    painter.line_segment(
        [
            egui::pos2(center.x - 310.0, header_y + 48.0),
            egui::pos2(center.x + 310.0, header_y + 48.0),
        ],
        egui::Stroke::new(1.0_f32, dim_amber),
    );

    // Teletype boot diagnostic lines revealed sequentially with progress
    let boot_lines = [
        (0.06_f32, "> INITIALISING YUTANI COLORIMETRY ENGINE [REV 2026.10]... OK"),
        (0.20_f32, "> ENGAGING KDE PLASMA WAYLAND COLOR MANAGEMENT BRIDGE... OK"),
        (0.35_f32, "> LOADING DISPLAY PROFILES: ASUS PA32UCDM // PA279CRV // MQ16AHE... OK"),
        (0.50_f32, "> LOADING GAMUT SPACES: REC.709 // DISPLAY P3 (D65) // NATIVE... OK"),
        (0.65_f32, "> CALIBRATING ITU-R BT.1886 & 3x1024 VCGT LUT SOLVER... OK"),
        (0.78_f32, "> PROBING USB BUS (VID 0x085C) FOR DATACOLOR INSTRUMENTS..."),
    ];

    let log_left = center.x - 310.0;
    let mut log_y = header_y + 82.0;

    for &(threshold, text) in &boot_lines {
        if progress >= threshold {
            let local_t = ((progress - threshold) / 0.13).clamp(0.0, 1.0);
            let char_count = ((text.len() as f32) * local_t).ceil() as usize;
            let slice: String = text.chars().take(char_count).collect();
            painter.text(
                egui::pos2(log_left, log_y),
                egui::Align2::LEFT_CENTER,
                slice,
                egui::FontId::monospace(12.5),
                amber,
            );
            log_y += 24.0;
        }
    }

    if progress >= 0.86 {
        let blink = ((progress * 18.0) as i32) % 2 == 0;
        let cursor = if blink { " █" } else { "" };
        painter.text(
            egui::pos2(log_left, log_y),
            egui::Align2::LEFT_CENTER,
            format!("> {}{}", sensor_status, cursor),
            egui::FontId::monospace(12.5),
            bright_amber,
        );
    }

    // Bottom progress bar + hex memory counter
    let bar_w = 620.0;
    let bar_rect = egui::Rect::from_center_size(
        egui::pos2(center.x, rect.bottom() - 78.0),
        egui::vec2(bar_w, 16.0),
    );
    painter.rect_stroke(bar_rect, 0.0, egui::Stroke::new(1.0_f32, amber));
    let fill_rect = egui::Rect::from_min_max(
        egui::pos2(bar_rect.left() + 2.0, bar_rect.top() + 2.0),
        egui::pos2(
            bar_rect.left() + 2.0 + (bar_w - 4.0) * progress.clamp(0.0, 1.0),
            bar_rect.bottom() - 2.0,
        ),
    );
    painter.rect_filled(fill_rect, 0.0, amber);

    let mem_addr = (progress.clamp(0.0, 1.0) * 65535.0) as u16;
    painter.text(
        egui::pos2(center.x, rect.bottom() - 48.0),
        egui::Align2::CENTER_CENTER,
        format!(
            "SYSTEM INTEGRITY {:>3.0}% // MEM 0x{:04X} // [ PRESS SPACE OR CLICK TO ENTER CONSOLE ]",
            progress * 100.0,
            mem_addr
        ),
        egui::FontId::monospace(11.5),
        dim_amber,
    );

    if ui.rect_contains_pointer(rect) && ui.input(|i| i.pointer.any_click()) {
        skip = true;
    }

    skip
}

enum AppPhase {
    /// Animated Yutani CRT terminal startup sequence.
    BootSequence { started_at: Instant },
    /// Main Yutani control console.
    Dashboard,
    /// Fullscreen target reticle for physical sensor placement on the monitor.
    PlacementReticle,
    /// Live continuous OSD Whitepoint (D65) & Luminance (Nits) alignment meter.
    OsdAlignment {
        last_poll: Instant,
        paused: bool,
        reading: Option<XyzReading>,
        read_count: usize,
    },
    /// Active fullscreen patch display and hardware measurement sequence.
    Measuring {
        patches: Vec<PatchSpec>,
        current_index: usize,
        patch_shown_at: Instant,
        awaiting_read: bool,
        readings: Vec<(PatchSpec, XyzReading)>,
    },
    /// Completed calibration telemetry report.
    Report {
        measurements: CalibrationMeasurements,
        vcgt_summary: VcgtSummary,
        saved_paths: Vec<PathBuf>,
        install_status: Option<String>,
        custom_save_path: String,
        export_status: Option<String>,
    },
}

struct YutaniApp {
    session: Option<SpyderWorker>,
    status_log: String,
    wayland_env_status: String,
    last_single_read: Option<XyzReading>,
    display_target: DisplayTarget,
    color_gamut: ColorGamut,
    eotf: EotfConfig,
    patch_preset: PatchPreset,
    oled_windowed_apl: bool,
    apply_panel_ccmx: bool,
    dark_patch_averaging: bool,
    ten_bit_frc_dither: bool,
    frame_counter: u64,
    target_nits: f64,
    phase: AppPhase,
    theme_applied: bool,
}

impl YutaniApp {
    fn new() -> Self {
        let default_display = DisplayTarget::default();
        let mut app = Self {
            session: None,
            status_log: String::from("SYSTEM READY. INITIALISING INSTRUMENT LINK..."),
            wayland_env_status: probe_wayland_color_environment(),
            last_single_read: None,
            display_target: default_display,
            color_gamut: ColorGamut::default(),
            eotf: EotfConfig::default(),
            patch_preset: PatchPreset::default(),
            oled_windowed_apl: default_display.is_oled(),
            apply_panel_ccmx: false,
            dark_patch_averaging: true,
            ten_bit_frc_dither: true,
            frame_counter: 0,
            target_nits: default_display.default_target_nits(),
            phase: AppPhase::BootSequence {
                started_at: Instant::now(),
            },
            theme_applied: false,
        };
        app.connect_sensor();
        app
    }

    fn connect_sensor(&mut self) {
        match SpyderWorker::spawn() {
            Ok(s) => {
                self.status_log = format!(
                    "ONLINE (ASYNC WORKER): {} [{:04x}:{:04x} // {}] // FW: {} // SN: {} // INT: {} ms",
                    s.model_name,
                    s.vid_pid.0,
                    s.vid_pid.1,
                    s.cmd_set.name(),
                    s.firmware_version,
                    s.serial_number,
                    s.integration_ms
                );
                self.session = Some(s);
            }
            Err(e) => {
                self.status_log = format!("OFFLINE: {e:#}");
                self.session = None;
            }
        }
    }

    fn finalize_measurements(
        &mut self,
        ctx: &egui::Context,
        readings: Vec<(PatchSpec, XyzReading)>,
    ) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));

        let mut black = XyzReading::default();
        let mut white = XyzReading::default();
        let mut red = XyzReading::default();
        let mut green = XyzReading::default();
        let mut blue = XyzReading::default();
        let mut grayscale = Vec::new();
        let mut primary_ramps: [Vec<(f64, XyzReading)>; 3] =
            [Vec::new(), Vec::new(), Vec::new()];
        let mut verification = Vec::new();

        for (spec, reading) in readings {
            match spec.kind {
                PatchKind::Black => {
                    black = reading;
                    grayscale.push((0.0, reading));
                }
                PatchKind::White => {
                    white = reading;
                    grayscale.push((1.0, reading));
                }
                PatchKind::Red => red = reading,
                PatchKind::Green => green = reading,
                PatchKind::Blue => blue = reading,
                PatchKind::Gray(v) => grayscale.push((v, reading)),
                PatchKind::PrimaryRamp { channel, level } => {
                    if channel < 3 {
                        primary_ramps[channel].push((level, reading));
                    }
                }
                PatchKind::Verification {
                    label,
                    stimulus_rgb,
                } => {
                    verification.push(VerificationPatchReading {
                        label,
                        stimulus_rgb,
                        reading,
                    });
                }
            }
        }

        grayscale.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        for ch_ramp in &mut primary_ramps {
            ch_ramp.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        }

        let measurements = CalibrationMeasurements {
            black,
            white,
            red,
            green,
            blue,
            grayscale,
            primary_ramps,
            verification,
        };

        match generate_profile_from_measurements(
            &measurements,
            self.display_target,
            self.color_gamut,
            self.eotf,
        ) {
            Ok((saved_paths, vcgt_summary)) => {
                self.status_log = format!(
                    "CALIBRATION COMPLETE // SAVED {} ICC PROFILE(S) WITH 3x1024 VCGT",
                    saved_paths.len()
                );
                let custom_save_path = saved_paths
                    .first()
                    .map(|p| default_icc_export_path(p))
                    .unwrap_or_else(|| String::from("~/Documents/yutani_calibrated.icc"));
                self.phase = AppPhase::Report {
                    measurements,
                    vcgt_summary,
                    saved_paths,
                    install_status: None,
                    custom_save_path,
                    export_status: None,
                };
            }
            Err(e) => {
                self.status_log = format!("ICC GENERATION ERROR: {e:#}");
                self.phase = AppPhase::Dashboard;
            }
        }
    }
}

impl eframe::App for YutaniApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frame_counter = self.frame_counter.wrapping_add(1);
        if !self.theme_applied {
            configure_yutani_theme(ctx);
            self.theme_applied = true;
        }

        // Global ESC key aborts fullscreen placement, measurement, or boot sequence
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if !matches!(self.phase, AppPhase::Dashboard) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                self.phase = AppPhase::Dashboard;
            }
        }

        let current_eotf = self.eotf;
        let current_display = self.display_target;
        let current_gamut = self.color_gamut;
        let current_preset = self.patch_preset;
        let current_windowed_apl = self.oled_windowed_apl;
        let current_ccmx = self.apply_panel_ccmx;
        let current_dark_avg = self.dark_patch_averaging;
        let current_frc_dither = self.ten_bit_frc_dither;
        let current_frame_parity = self.frame_counter;

        match &mut self.phase {
            AppPhase::BootSequence { started_at } => {
                let elapsed = started_at.elapsed();
                let progress = (elapsed.as_secs_f32() / BOOT_SEQUENCE_DURATION.as_secs_f32())
                    .clamp(0.0, 1.0);
                let skip_key = ctx.input(|i| {
                    i.key_pressed(egui::Key::Space) || i.key_pressed(egui::Key::Enter)
                });

                let mut finish_boot = skip_key || elapsed >= BOOT_SEQUENCE_DURATION;
                egui::CentralPanel::default().show(ctx, |ui| {
                    if draw_boot_sequence(ui, progress, &self.status_log) {
                        finish_boot = true;
                    }
                });

                if finish_boot {
                    self.phase = AppPhase::Dashboard;
                    ctx.request_repaint();
                } else {
                    ctx.request_repaint_after(Duration::from_millis(16));
                }
            }

            AppPhase::Dashboard => {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.heading("YUTANI WAYLAND // COLORIMETRY SUBSYSTEM");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(" [ REPLAY BOOT ] ").clicked() {
                                self.phase = AppPhase::BootSequence {
                                    started_at: Instant::now(),
                                };
                            }
                        });
                    });
                    ui.label("YUTANI DISPLAY PROFILER // BUILDING BETTER COLORS");
                    ui.separator();

                    ui.add_space(4.0);
                    ui.label("SELECT DISPLAY TARGET:");
                    ui.horizontal_wrapped(|ui| {
                        for target in DisplayTarget::ALL {
                            let is_sel = self.display_target == target;
                            if ui.selectable_label(is_sel, target.button_label()).clicked() {
                                self.display_target = target;
                                self.target_nits = target.default_target_nits();
                                self.oled_windowed_apl = target.is_oled();
                            }
                        }
                    });

                    ui.add_space(4.0);
                    ui.label("SELECT TARGET COLOR GAMUT SPACE:");
                    ui.horizontal_wrapped(|ui| {
                        for gamut in ColorGamut::ALL {
                            let is_sel = self.color_gamut == gamut;
                            if ui.selectable_label(is_sel, gamut.button_label()).clicked() {
                                self.color_gamut = gamut;
                            }
                        }
                    });

                    ui.add_space(4.0);
                    ui.label("SELECT CALIBRATION PATCH SUITE & OLED APL MODE:");
                    ui.horizontal_wrapped(|ui| {
                        for preset in PatchPreset::ALL {
                            let is_sel = self.patch_preset == preset;
                            if ui.selectable_label(is_sel, preset.button_label()).clicked() {
                                self.patch_preset = preset;
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.checkbox(
                            &mut self.oled_windowed_apl,
                            "10% Windowed Patch + 20% Neutral Gray APL Stabilizer (Prevents OLED ABL Dimming)",
                        );
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(
                            &mut self.apply_panel_ccmx,
                            format!("Apply {}", self.display_target.ccmx_label()),
                        );
                        ui.checkbox(
                            &mut self.dark_patch_averaging,
                            "Adaptive 3-Sample Dark-Patch Averaging (Raw <= 35)",
                        );
                        ui.checkbox(
                            &mut self.ten_bit_frc_dither,
                            "10-Bit Spatio-Temporal FRC Dithering (4x4 Bayer + Frame Phase)",
                        );
                    });

                    ui.add_space(6.0);
                    ui.label(format!(
                        "DISPLAY TARGET : {} — {}",
                        self.display_target.full_name(),
                        self.display_target.panel_spec()
                    ));
                    ui.label(format!("COLOR GAMUT    : {}", self.color_gamut.display_label()));
                    ui.label(format!("TARGET EOTF    : {}", self.eotf.display_label()));
                    ui.label(format!(
                        "OUTPUT PROFILE : {}",
                        self.eotf.filename_for(self.display_target, self.color_gamut)
                    ));
                    ui.label("TARGET WHITE   : CIE D65 (x = 0.3127, y = 0.3290)");
                    ui.label(format!("WAYLAND / CMS  : {}", self.wayland_env_status));
                    ui.label(format!("SENSOR STATUS  : {}", self.status_log));

                    if let Some(s) = &self.session {
                        ui.add_space(4.0);
                        ui.label(format!(
                            "PRIMARY MAT Y  : [{:+.6}, {:+.6}, {:+.6}] (setup_off = {:?})",
                            s.matrix[1][0], s.matrix[1][1], s.matrix[1][2], s.setup_off
                        ));
                        if s.cmd_set == CmdSet::SpyderX2 {
                            ui.label(format!(
                                "SECONDARY MAT Y: [{:+.6}, {:+.6}, {:+.6}] (sec_off = {:?})",
                                s.secondary_matrix[1][0],
                                s.secondary_matrix[1][1],
                                s.secondary_matrix[1][2],
                                s.secondary_setup_off
                            ));
                        }
                        let bcal_str = if s.black_calibrated {
                            format!(
                                "ZEROED (CAP-ON VERIFIED) // bcal = [{:.2}, {:.2}, {:.2}] counts",
                                s.black_off[0], s.black_off[1], s.black_off[2]
                            )
                        } else {
                            String::from("UNCALIBRATED (FACTORY SETUP_OFF ONLY — CLOSE LENS CAP & CLICK ZERO DARK OFFSET)")
                        };
                        ui.label(format!("DARK OFFSET    : {bcal_str}"));
                    }

                    ui.add_space(8.0);
                    ui.separator();
                    ui.label("SELECT TARGET GAMMA / EOTF CURVE:");
                    ui.horizontal_wrapped(|ui| {
                        let is_bt1886_24 = matches!(self.eotf, EotfConfig::Bt1886 { gamma } if (gamma - 2.4).abs() < 0.005);
                        if ui.selectable_label(is_bt1886_24, " [ BT.1886 (γ 2.40) ] ").clicked() {
                            self.eotf = EotfConfig::Bt1886 { gamma: 2.4 };
                        }

                        let is_bt1886_22 = matches!(self.eotf, EotfConfig::Bt1886 { gamma } if (gamma - 2.2).abs() < 0.005);
                        if ui.selectable_label(is_bt1886_22, " [ BT.1886 (γ 2.20) ] ").clicked() {
                            self.eotf = EotfConfig::Bt1886 { gamma: 2.2 };
                        }

                        let is_pure_22 = matches!(self.eotf, EotfConfig::PurePower { gamma } if (gamma - 2.2).abs() < 0.005);
                        if ui.selectable_label(is_pure_22, " [ PURE GAMMA 2.20 ] ").clicked() {
                            self.eotf = EotfConfig::PurePower { gamma: 2.2 };
                        }

                        let is_pure_24 = matches!(self.eotf, EotfConfig::PurePower { gamma } if (gamma - 2.4).abs() < 0.005);
                        if ui.selectable_label(is_pure_24, " [ PURE GAMMA 2.40 ] ").clicked() {
                            self.eotf = EotfConfig::PurePower { gamma: 2.4 };
                        }

                        let is_pure_26 = matches!(self.eotf, EotfConfig::PurePower { gamma } if (gamma - 2.6).abs() < 0.005);
                        if ui.selectable_label(is_pure_26, " [ DCI GAMMA 2.60 ] ").clicked() {
                            self.eotf = EotfConfig::PurePower { gamma: 2.6 };
                        }

                        let is_srgb = matches!(self.eotf, EotfConfig::Srgb);
                        if ui.selectable_label(is_srgb, " [ sRGB PIECEWISE ] ").clicked() {
                            self.eotf = EotfConfig::Srgb;
                        }
                    });

                    // Fine-grained custom gamma slider & black-compensation mode toggle
                    if !matches!(self.eotf, EotfConfig::Srgb) {
                        let mut g = self.eotf.gamma_value();
                        let mut use_bt1886 = matches!(self.eotf, EotfConfig::Bt1886 { .. });
                        ui.horizontal(|ui| {
                            ui.label("CUSTOM GAMMA (γ):");
                            let slider_changed = ui
                                .add(
                                    egui::Slider::new(&mut g, 1.60..=2.80)
                                        .step_by(0.05)
                                        .fixed_decimals(2),
                                )
                                .changed();
                            let mode_changed = ui
                                .checkbox(
                                    &mut use_bt1886,
                                    "BT.1886 Black-Level Lift Compensation",
                                )
                                .changed();
                            if slider_changed || mode_changed {
                                self.eotf = if use_bt1886 {
                                    EotfConfig::Bt1886 { gamma: g }
                                } else {
                                    EotfConfig::PurePower { gamma: g }
                                };
                            }
                        });
                    }

                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("TARGET PEAK LUMINANCE (OSD):");
                        for &preset in &[80.0, 100.0, 120.0, 160.0, 200.0] {
                            let selected = (self.target_nits - preset).abs() < 0.5;
                            if ui
                                .selectable_label(selected, format!(" {:.0} cd/m² ", preset))
                                .clicked()
                            {
                                self.target_nits = preset;
                            }
                        }
                        ui.add(
                            egui::Slider::new(&mut self.target_nits, 40.0..=300.0)
                                .step_by(5.0)
                                .suffix(" cd/m²")
                                .fixed_decimals(0),
                        );
                    });

                    ui.add_space(10.0);
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        if ui.button(" [ RE-SCAN USB INSTRUMENT ] ").clicked() {
                            self.connect_sensor();
                        }
                        if ui.button(" [ PROBE WAYLAND / COLORD ] ").clicked() {
                            self.wayland_env_status = probe_wayland_color_environment();
                        }

                        if self.session.is_some() {
                            if ui.button(" [ ZERO DARK OFFSET (CAP ON) ] ").clicked() {
                                if let Some(s) = &mut self.session {
                                    match s.calibrate_black_sync() {
                                        Ok(bcal) => {
                                            self.status_log = format!(
                                                "DARK OFFSET ZEROED: bcal = [{:.2}, {:.2}, {:.2}] counts",
                                                bcal[0], bcal[1], bcal[2]
                                            );
                                        }
                                        Err(e) => {
                                            self.status_log = format!("DARK ZERO FAILED: {e}");
                                        }
                                    }
                                }
                            }

                            let is_busy = self.session.as_ref().map(|s| s.is_busy()).unwrap_or(false);
                            let read_btn_label = if is_busy {
                                " [ READING SENSOR (ASYNC)... ] "
                            } else {
                                " [ SINGLE READ (TEST SENSOR) ] "
                            };
                            if ui.button(read_btn_label).clicked() && !is_busy {
                                if let Some(s) = &mut self.session {
                                    if s.request_measure(current_dark_avg) {
                                        self.status_log =
                                            String::from("ACQUIRING OPTICAL SAMPLE ON WORKER THREAD...");
                                    }
                                }
                            }

                            if ui.button(" [ LIVE OSD WHITE & NITS METER ] ").clicked() {
                                self.wayland_env_status = probe_wayland_color_environment();
                                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                                self.phase = AppPhase::OsdAlignment {
                                    last_poll: Instant::now() - OSD_POLL_INTERVAL,
                                    paused: false,
                                    reading: self.last_single_read,
                                    read_count: 0,
                                };
                            }

                            if ui.button(" [ INITIALIZE CALIBRATION SEQUENCE ] ").clicked() {
                                self.wayland_env_status = probe_wayland_color_environment();
                                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                                self.phase = AppPhase::PlacementReticle;
                            }
                        }
                    });

                    // Poll non-blocking single read if in flight on Dashboard
                    if let Some(s) = &mut self.session {
                        if let Some(res) = s.poll_measure() {
                            match res {
                                Ok(raw_r) => {
                                    let r = if current_ccmx {
                                        raw_r.apply_ccmx(current_display.ccmx_matrix())
                                    } else {
                                        raw_r
                                    };
                                    let (x, y, lum) = r.to_xyy();
                                    self.status_log = format!(
                                        "READ OK ({}x avg): {:.3} cd/m² (x={:.4}, y={:.4}) raw={:?}",
                                        r.sample_count, lum, x, y, r.raw_counts
                                    );
                                    self.last_single_read = Some(r);
                                }
                                Err(e) => {
                                    self.status_log = format!("READ FAILED: {e}");
                                }
                            }
                        } else if s.is_busy() {
                            ctx.request_repaint_after(Duration::from_millis(16));
                        }
                    }

                    if let Some(r) = self.last_single_read {
                        let (cx, cy, lum) = r.to_xyy();
                        ui.add_space(10.0);
                        ui.separator();
                        ui.label("TELEMETRY // LAST SPOT MEASUREMENT:");
                        ui.label(format!(
                            "  Luminance (Y) : {:.4} cd/m² (nits) [Samples: {}x // CCMX: {}]",
                            lum,
                            r.sample_count,
                            if current_ccmx { "ON" } else { "OFF" }
                        ));
                        ui.label(format!(
                            "  CIE 1931 xy   : x = {:.4}, y = {:.4}",
                            cx, cy
                        ));
                        ui.label(format!(
                            "  CIE 1931 XYZ  : X = {:.4}, Y = {:.4}, Z = {:.4}",
                            r.x, r.y, r.z
                        ));
                        ui.label(format!(
                            "  Raw Counts    : R={} G={} B={} (Net: {:.1}, {:.1}, {:.1}) | Sec Raw={:?}",
                            r.raw_counts[0],
                            r.raw_counts[1],
                            r.raw_counts[2],
                            r.net_counts[0],
                            r.net_counts[1],
                            r.net_counts[2],
                            r.secondary_raw_counts
                        ));
                    }
                });
            }

            AppPhase::PlacementReticle => {
                let start_now = ctx.input(|i| i.key_pressed(egui::Key::Space));
                let open_osd = ctx.input(|i| i.key_pressed(egui::Key::O));
                let mut begin_sequence = start_now;
                let mut launch_osd = open_osd;
                let mut cancel = false;

                egui::CentralPanel::default()
                    .frame(egui::Frame::default().fill(egui::Color32::from_rgb(8, 8, 8)))
                    .show(ctx, |ui| {
                        let rect = ui.max_rect();
                        let center = rect.center();
                        let painter = ui.painter();
                        let amber = egui::Color32::from_rgb(255, 180, 40);

                        // Draw target reticle in exact screen center for hanging the Spyder sensor
                        let box_size = egui::vec2(260.0, 260.0);
                        let target_rect = egui::Rect::from_center_size(center, box_size);
                        painter.rect_stroke(
                            target_rect,
                            0.0,
                            egui::Stroke::new(2.0_f32, amber),
                        );
                        painter.circle_stroke(center, 70.0, egui::Stroke::new(1.5_f32, amber));
                        painter.line_segment(
                            [
                                egui::pos2(center.x - 100.0, center.y),
                                egui::pos2(center.x + 100.0, center.y),
                            ],
                            egui::Stroke::new(1.0_f32, amber),
                        );
                        painter.line_segment(
                            [
                                egui::pos2(center.x, center.y - 100.0),
                                egui::pos2(center.x, center.y + 100.0),
                            ],
                            egui::Stroke::new(1.0_f32, amber),
                        );

                        ui.vertical_centered(|ui| {
                            ui.add_space(32.0);
                            ui.heading(format!(
                                "ALIGN SPYDER SENSOR ON {} CENTER RETICLE",
                                current_display.full_name().to_ascii_uppercase()
                            ));
                            ui.label(format!(
                                "1. Remove sensor lens cap and place flush against the {} panel.",
                                current_display.full_name()
                            ));
                            ui.label(format!(
                                "2. Wayland/CMS Check: {}",
                                self.wayland_env_status
                            ));
                            ui.add_space(16.0);
                            ui.horizontal(|ui| {
                                if ui.button(" [ LIVE OSD WHITE & NITS METER (O) ] ").clicked() {
                                    launch_osd = true;
                                }
                                if ui
                                    .button(format!(
                                        " [ ENGAGE {} SEQUENCE (SPACE) ] ",
                                        current_preset.short_label()
                                    ))
                                    .clicked()
                                {
                                    begin_sequence = true;
                                }
                                if ui.button(" [ ABORT (ESC) ] ").clicked() {
                                    cancel = true;
                                }
                            });
                        });
                    });

                if cancel {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                    self.phase = AppPhase::Dashboard;
                } else if launch_osd {
                    self.phase = AppPhase::OsdAlignment {
                        last_poll: Instant::now() - OSD_POLL_INTERVAL,
                        paused: false,
                        reading: None,
                        read_count: 0,
                    };
                    ctx.request_repaint();
                } else if begin_sequence {
                    let patches = build_calibration_patches(current_preset);
                    let cap = patches.len();
                    self.phase = AppPhase::Measuring {
                        patches,
                        current_index: 0,
                        patch_shown_at: Instant::now(),
                        awaiting_read: false,
                        readings: Vec::with_capacity(cap),
                    };
                    ctx.request_repaint();
                }
            }

            AppPhase::OsdAlignment {
                last_poll,
                paused,
                reading,
                read_count,
            } => {
                let start_cal = ctx.input(|i| i.key_pressed(egui::Key::Space));
                let mut proceed_to_cal = start_cal;
                let mut exit_to_dash = false;
                let mut trigger_manual_read = false;

                egui::CentralPanel::default()
                    .frame(egui::Frame::default().fill(egui::Color32::from_rgb(8, 8, 8)))
                    .show(ctx, |ui| {
                        let rect = ui.max_rect();
                        let center = rect.center();
                        let painter = ui.painter();
                        let amber = egui::Color32::from_rgb(255, 180, 40);

                        // Pure 100% White (255, 255, 255) patch in the exact center for the Spyder sensor
                        let patch_size = egui::vec2(300.0, 300.0);
                        let patch_rect = egui::Rect::from_center_size(center, patch_size);
                        painter.rect_filled(patch_rect, 0.0, egui::Color32::WHITE);
                        painter.rect_stroke(
                            patch_rect.expand(4.0),
                            0.0,
                            egui::Stroke::new(2.0_f32, amber),
                        );

                        // Top telemetry HUD (outside the center optical patch)
                        ui.vertical(|ui| {
                            ui.heading(
                                "YUTANI WAYLAND // LIVE OSD WHITE-POINT (D65) & LUMINANCE ALIGNMENT",
                            );
                            ui.label(format!(
                                "Adjust {} OSD Brightness & RGB Gain while sensor reads center white patch.",
                                current_display.full_name()
                            ));
                            ui.separator();

                            ui.horizontal(|ui| {
                                ui.label(format!(
                                    "TARGET LUMINANCE: {:.0} cd/m²  |  TARGET WHITE: CIE D65 (x=0.3127, y=0.3290, 6504K)  |  SAMPLES: {}",
                                    self.target_nits, *read_count
                                ));
                            });

                            if let Some(r) = *reading {
                                let (cx, cy, lum) = r.to_xyy();
                                let (rgb_pct, cct, delta_e) = compute_d65_alignment_metrics(&r);
                                let lum_diff_nits = lum - self.target_nits;
                                let lum_diff_pct =
                                    (lum_diff_nits / self.target_nits.max(1.0)) * 100.0;

                                let lum_hint = if lum_diff_nits.abs() <= 2.0 {
                                    "[ LUMINANCE LOCKED ]"
                                } else if lum_diff_nits > 0.0 {
                                    "LOWER OSD BRIGHTNESS"
                                } else {
                                    "RAISE OSD BRIGHTNESS"
                                };

                                draw_deviation_bar(
                                    ui,
                                    &format!("Y ({:.1} nit)", lum),
                                    lum_diff_pct,
                                    30.0,
                                    egui::Color32::from_rgb(255, 190, 60),
                                    lum_hint,
                                );

                                let r_diff = rgb_pct[0] - 100.0;
                                let g_diff = rgb_pct[1] - 100.0;
                                let b_diff = rgb_pct[2] - 100.0;

                                draw_deviation_bar(
                                    ui,
                                    &format!("RED ({:.1}%)", rgb_pct[0]),
                                    r_diff,
                                    15.0,
                                    egui::Color32::from_rgb(235, 65, 55),
                                    if r_diff.abs() <= 1.0 {
                                        "[ R LOCKED ]"
                                    } else if r_diff > 0.0 {
                                        "LOWER OSD RED GAIN"
                                    } else {
                                        "RAISE OSD RED GAIN"
                                    },
                                );
                                draw_deviation_bar(
                                    ui,
                                    &format!("GRN ({:.1}%)", rgb_pct[1]),
                                    g_diff,
                                    15.0,
                                    egui::Color32::from_rgb(65, 220, 85),
                                    if g_diff.abs() <= 1.0 {
                                        "[ G LOCKED ]"
                                    } else if g_diff > 0.0 {
                                        "LOWER OSD GREEN GAIN"
                                    } else {
                                        "RAISE OSD GREEN GAIN"
                                    },
                                );
                                draw_deviation_bar(
                                    ui,
                                    &format!("BLU ({:.1}%)", rgb_pct[2]),
                                    b_diff,
                                    15.0,
                                    egui::Color32::from_rgb(70, 145, 255),
                                    if b_diff.abs() <= 1.0 {
                                        "[ B LOCKED ]"
                                    } else if b_diff > 0.0 {
                                        "LOWER OSD BLUE GAIN"
                                    } else {
                                        "RAISE OSD BLUE GAIN"
                                    },
                                );

                                let lock_status =
                                    if delta_e < 1.0 && lum_diff_nits.abs() <= 2.0 {
                                        "STATUS: [ D65 & LUMINANCE LOCKED // READY FOR 14-PATCH SWEEP ]"
                                    } else if delta_e < 2.0 {
                                        "STATUS: [ NEAR D65 SPEC (ΔE < 2.0) // FINE-TUNE OSD ]"
                                    } else {
                                        "STATUS: [ ADJUSTING OSD // ALIGN BARS TO CENTER TICK ]"
                                    };

                                ui.monospace(format!(
                                    "CIE xy = ({:.4}, {:.4})  |  CCT = {:.0} K  |  ΔE(D65) = {:.2}  |  {}",
                                    cx, cy, cct, delta_e, lock_status
                                ));
                            } else {
                                ui.label("ACQUIRING INITIAL SENSOR READING FROM CENTER WHITE PATCH...");
                            }

                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                let pause_label = if *paused {
                                    " [ RESUME LIVE POLLING ] "
                                } else {
                                    " [ PAUSE LIVE POLLING ] "
                                };
                                if ui.button(pause_label).clicked() {
                                    *paused = !*paused;
                                }
                                if *paused && ui.button(" [ SINGLE READ ] ").clicked() {
                                    trigger_manual_read = true;
                                }
                                if ui
                                    .button(format!(
                                        " [ START {} CALIBRATION (SPACE) ] ",
                                        current_preset.short_label()
                                    ))
                                    .clicked()
                                {
                                    proceed_to_cal = true;
                                }
                                if ui.button(" [ EXIT TO CONSOLE (ESC) ] ").clicked() {
                                    exit_to_dash = true;
                                }
                            });
                        });
                    });

                if exit_to_dash {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                    self.phase = AppPhase::Dashboard;
                } else if proceed_to_cal {
                    let patches = build_calibration_patches(current_preset);
                    let cap = patches.len();
                    self.phase = AppPhase::Measuring {
                        patches,
                        current_index: 0,
                        patch_shown_at: Instant::now(),
                        awaiting_read: false,
                        readings: Vec::with_capacity(cap),
                    };
                    ctx.request_repaint();
                } else if let Some(s) = &mut self.session {
                    if let Some(res) = s.poll_measure() {
                        if let Ok(raw_r) = res {
                            let r = if current_ccmx {
                                raw_r.apply_ccmx(current_display.ccmx_matrix())
                            } else {
                                raw_r
                            };
                            *reading = Some(r);
                            self.last_single_read = Some(r);
                            *read_count += 1;
                        }
                        *last_poll = Instant::now();
                        if !*paused {
                            ctx.request_repaint_after(OSD_POLL_INTERVAL);
                        }
                    } else if s.is_busy() {
                        ctx.request_repaint_after(Duration::from_millis(16));
                    } else if (!*paused && last_poll.elapsed() >= OSD_POLL_INTERVAL)
                        || trigger_manual_read
                    {
                        let _ = s.request_measure(false);
                        ctx.request_repaint_after(Duration::from_millis(16));
                    } else if !*paused {
                        ctx.request_repaint_after(OSD_POLL_INTERVAL - last_poll.elapsed());
                    }
                }
            }

            AppPhase::Measuring {
                patches,
                current_index,
                patch_shown_at,
                awaiting_read,
                readings,
            } => {
                let spec = patches[*current_index];

                // When OLED Windowed APL mode is active, surround a 10% center patch window (360x360)
                // with a constant ~20% APL neutral gray surround (RGB 118, 118, 118) so OLED/QD-OLED
                // Auto Brightness Limiter (ABL) stays locked across all patches.
                let bg_fill = if current_windowed_apl {
                    egui::Color32::from_rgb(118, 118, 118)
                } else {
                    spec.color
                };

                egui::CentralPanel::default()
                    .frame(egui::Frame::default().fill(bg_fill))
                    .show(ctx, |ui| {
                        let screen_rect = ui.max_rect();
                        let center = screen_rect.center();
                        let patch_rect = if current_windowed_apl {
                            egui::Rect::from_center_size(center, egui::vec2(360.0, 360.0))
                        } else {
                            screen_rect
                        };
                        let dither_active = draw_frc_dithered_patch(
                            ui.painter(),
                            patch_rect,
                            spec.exact_rgb,
                            spec.color,
                            current_frame_parity,
                            current_frc_dither,
                        );

                        // Small top-left corner status outside the center sensor zone
                        egui::Frame::default()
                            .fill(egui::Color32::from_rgba_unmultiplied(0, 0, 0, 200))
                            .inner_margin(8.0)
                            .show(ui, |ui| {
                                ui.label(format!(
                                    "YUTANI // PATCH {}/{} : {} [{} // {}]  [ESC TO ABORT]",
                                    *current_index + 1,
                                    patches.len(),
                                    spec.label,
                                    if current_windowed_apl {
                                        "10% WINDOWED APL"
                                    } else {
                                        "FULLSCREEN"
                                    },
                                    if dither_active {
                                        "10-BIT FRC DITHER"
                                    } else {
                                        "8-BIT INTEGER"
                                    }
                                ));
                            });
                    });

                let elapsed = patch_shown_at.elapsed();
                if elapsed < PATCH_SETTLE_DURATION {
                    // Keep repainting at 60 Hz during settle if FRC temporal dithering is active
                    if current_frc_dither {
                        ctx.request_repaint_after(Duration::from_millis(16));
                    } else {
                        ctx.request_repaint_after(PATCH_SETTLE_DURATION - elapsed);
                    }
                } else if !*awaiting_read {
                    // Panel has settled: dispatch non-blocking read to the Spyder worker thread
                    if let Some(s) = &mut self.session {
                        if s.request_measure(current_dark_avg) {
                            *awaiting_read = true;
                            ctx.request_repaint_after(Duration::from_millis(16));
                        } else if s.is_busy() {
                            *awaiting_read = true;
                            ctx.request_repaint_after(Duration::from_millis(16));
                        } else {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                            self.status_log =
                                String::from("MEASUREMENT ERROR: Spyder worker disconnected");
                            self.phase = AppPhase::Dashboard;
                        }
                    } else {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                        self.status_log = String::from("MEASUREMENT ERROR: Spyder session offline");
                        self.phase = AppPhase::Dashboard;
                    }
                } else {
                    // Poll non-blocking worker thread for completed optical measurement
                    let poll_res = if let Some(s) = &mut self.session {
                        s.poll_measure()
                    } else {
                        Some(Err(String::from("Spyder session disconnected")))
                    };

                    match poll_res {
                        Some(Ok(raw_reading)) => {
                            let reading = if current_ccmx {
                                raw_reading.apply_ccmx(current_display.ccmx_matrix())
                            } else {
                                raw_reading
                            };
                            readings.push((spec, reading));
                            *current_index += 1;
                            if *current_index >= patches.len() {
                                let completed = std::mem::take(readings);
                                self.finalize_measurements(ctx, completed);
                            } else {
                                *patch_shown_at = Instant::now();
                                *awaiting_read = false;
                                ctx.request_repaint();
                            }
                        }
                        Some(Err(e)) => {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                            self.status_log = format!("MEASUREMENT ERROR ON {}: {e}", spec.label);
                            self.phase = AppPhase::Dashboard;
                        }
                        None => {
                            // Still integrating on the background worker thread; keep UI responsive
                            ctx.request_repaint_after(Duration::from_millis(16));
                        }
                    }
                }
            }

            AppPhase::Report {
                measurements,
                vcgt_summary,
                saved_paths,
                install_status,
                custom_save_path,
                export_status,
            } => {
                let mut back_to_dash = false;
                let mut trigger_install = false;
                let mut trigger_browse_save = false;
                let mut trigger_save_custom = false;
                egui::CentralPanel::default().show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.heading("YUTANI WAYLAND // CALIBRATION TELEMETRY REPORT");
                        ui.separator();

                        let lw = measurements.white.y;
                        let lb = measurements.black.y;
                        let contrast = if lb > 1e-5 { lw / lb } else { 0.0 };
                        let (a, b) = bt1886_params(lw, lb, current_eotf.gamma_value());
                        let (wx, wy, _) = measurements.white.to_xyy();
                        let (rx, ry, _) = measurements.red.to_xyy();
                        let (gx, gy, _) = measurements.green.to_xyy();
                        let (bx, by, _) = measurements.blue.to_xyy();
                        let tgt_prim = current_gamut.reference_primaries_xy();
                        let err_r = ((rx - tgt_prim[0].0).powi(2) + (ry - tgt_prim[0].1).powi(2)).sqrt();
                        let err_g = ((gx - tgt_prim[1].0).powi(2) + (gy - tgt_prim[1].1).powi(2)).sqrt();
                        let err_b = ((bx - tgt_prim[2].0).powi(2) + (by - tgt_prim[2].1).powi(2)).sqrt();

                        ui.label(format!(
                            "DISPLAY TARGET  : {} ({})",
                            current_display.full_name(),
                            current_display.panel_spec()
                        ));
                        ui.label(format!(
                            "COLOR GAMUT     : {}",
                            current_gamut.display_label()
                        ));
                        ui.label(format!("TARGET EOTF     : {}", current_eotf.display_label()));
                        ui.label(format!(
                            "PEAK WHITE (Lw) : {:.3} cd/m²  (CIE xy = {:.4}, {:.4})",
                            lw, wx, wy
                        ));
                        ui.label(format!("BLACK LEVEL (Lb): {:.4} cd/m²", lb));
                        if contrast > 0.0 {
                            ui.label(format!("CONTRAST RATIO  : {:.0} : 1", contrast));
                        } else {
                            ui.label("CONTRAST RATIO  : ∞ : 1 (OLED Zero-Black)");
                        }
                        if let EotfConfig::Bt1886 { gamma } = current_eotf {
                            ui.label(format!(
                                "BT.1886 PARAMS  : a = {:.4}, b = {:.6} (γ = {:.2})",
                                a, b, gamma
                            ));
                        }
                        ui.label(format!(
                            "MEASURED RGB xy : R({:.4}, {:.4})  G({:.4}, {:.4})  B({:.4}, {:.4})",
                            rx, ry, gx, gy, bx, by
                        ));
                        ui.label(format!(
                            "GAMUT Δxy vs {:<6}: ΔR = {:.4}  |  ΔG = {:.4}  |  ΔB = {:.4}",
                            current_gamut.short_code(),
                            err_r,
                            err_g,
                            err_b
                        ));
                        ui.label(format!(
                            "D65 VCGT GAINS  : R = {:.1}% | G = {:.1}% | B = {:.1}% (Inversion: {} // RGB Sub-Ramps: {})",
                            vcgt_summary.d65_rgb_gains[0] * 100.0,
                            vcgt_summary.d65_rgb_gains[1] * 100.0,
                            vcgt_summary.d65_rgb_gains[2] * 100.0,
                            if vcgt_summary.used_measured_inversion {
                                "ACTIVE"
                            } else {
                                "BASELINE FALLBACK"
                            },
                            if vcgt_summary.used_rgb_subramps {
                                "BLENDED"
                            } else {
                                "GRAY-ONLY"
                            }
                        ));
                        ui.label(format!(
                            "VCGT @ 50% IN   : R_out = {:.3} ({:+.1}%) | G_out = {:.3} ({:+.1}%) | B_out = {:.3} ({:+.1}%)",
                            vcgt_summary.midtone_50_vcgt[0],
                            (vcgt_summary.midtone_50_vcgt[0] - 0.5) * 100.0,
                            vcgt_summary.midtone_50_vcgt[1],
                            (vcgt_summary.midtone_50_vcgt[1] - 0.5) * 100.0,
                            vcgt_summary.midtone_50_vcgt[2],
                            (vcgt_summary.midtone_50_vcgt[2] - 0.5) * 100.0,
                        ));

                        ui.add_space(8.0);
                        ui.separator();
                        ui.horizontal_wrapped(|ui| {
                            draw_eotf_graph(ui, measurements, current_eotf);
                            ui.add_space(12.0);
                            draw_cie_gamut_graph(ui, measurements, current_gamut);
                        });

                        ui.add_space(8.0);
                        ui.separator();
                        draw_vcgt_correction_graph(ui, vcgt_summary);

                        ui.add_space(8.0);
                        ui.separator();
                        let mut gray_de00_sum = 0.0f64;
                        let mut gray_de00_max = 0.0f64;
                        let mut gray_de00_count = 0usize;
                        for (v, reading) in &measurements.grayscale {
                            if *v > 0.005 {
                                let (_, de00_full) =
                                    evaluate_grayscale_de00(*v, reading, lw, lb, current_eotf);
                                gray_de00_sum += de00_full;
                                if de00_full > gray_de00_max {
                                    gray_de00_max = de00_full;
                                }
                                gray_de00_count += 1;
                            }
                        }
                        let gray_de00_mean = if gray_de00_count > 0 {
                            gray_de00_sum / (gray_de00_count as f64)
                        } else {
                            0.0
                        };
                        ui.label(format!(
                            "GRAYSCALE EOTF TRACKING & 8-BIT VCGT LUT // UNCORRECTED MEAN ΔE00 = {:.2} (MAX {:.2}):",
                            gray_de00_mean, gray_de00_max
                        ));
                        for (v, reading) in &measurements.grayscale {
                            let target_y = eotf_target_luminance(*v, lw, lb, current_eotf);
                            let (gx, gy, meas_y) = reading.to_xyy();
                            let dy = meas_y - target_y;
                            let (de00_ch, de00_full) =
                                evaluate_grayscale_de00(*v, reading, lw, lb, current_eotf);
                            let in_8bit = (v * 255.0).round() as i32;
                            let lut_rgb = vcgt_summary.sample_rgb(*v);
                            let r_8bit = (lut_rgb[0] * 255.0).round() as i32;
                            let g_8bit = (lut_rgb[1] * 255.0).round() as i32;
                            let b_8bit = (lut_rgb[2] * 255.0).round() as i32;
                            ui.monospace(format!(
                                "  {:>3.0}% (In {:>3}) | Meas:{:>6.2} Tgt:{:>6.2} (ΔY {:+5.2}) | xy:({:.4},{:.4}) ΔE00:{:>4.2}(C:{:>4.2}) | LUT R:{:>3}({:+3}) G:{:>3}({:+3}) B:{:>3}({:+3})",
                                v * 100.0,
                                in_8bit,
                                meas_y,
                                target_y,
                                dy,
                                gx,
                                gy,
                                de00_full,
                                de00_ch,
                                r_8bit,
                                r_8bit - in_8bit,
                                g_8bit,
                                g_8bit - in_8bit,
                                b_8bit,
                                b_8bit - in_8bit,
                            ));
                        }

                        if measurements.primary_ramps.iter().any(|r| !r.is_empty()) {
                            ui.add_space(8.0);
                            ui.separator();
                            ui.label("PER-CHANNEL PRIMARY SUB-RAMP TELEMETRY (R / G / B EOTF):");
                            let ch_names = ["RED  ", "GREEN", "BLUE "];
                            let peak_ch = [
                                measurements.red.y.max(1e-6),
                                measurements.green.y.max(1e-6),
                                measurements.blue.y.max(1e-6),
                            ];
                            for c in 0..3 {
                                for &(u, ref r) in &measurements.primary_ramps[c] {
                                    let (px, py, plum) = r.to_xyy();
                                    let rel_pct = ((plum - lb).max(0.0) / (peak_ch[c] - lb).max(1e-6)) * 100.0;
                                    ui.monospace(format!(
                                        "  {} {:>3.0}% | Lum: {:>6.2} cd/m² ({:>5.1}% of ch peak) | CIE xy = ({:.4}, {:.4})",
                                        ch_names[c],
                                        u * 100.0,
                                        plum,
                                        rel_pct,
                                        px,
                                        py
                                    ));
                                }
                            }
                        }

                        if !measurements.verification.is_empty() {
                            ui.add_space(8.0);
                            ui.separator();
                            let mut sum_de00 = 0.0f64;
                            let mut max_de00 = 0.0f64;
                            let mut rows = Vec::with_capacity(measurements.verification.len());
                            for vpatch in &measurements.verification {
                                let (de00, tgt_xy) = evaluate_verification_patch_de00(
                                    vpatch,
                                    measurements,
                                    current_gamut,
                                    current_eotf,
                                );
                                sum_de00 += de00;
                                if de00 > max_de00 {
                                    max_de00 = de00;
                                }
                                rows.push((vpatch, de00, tgt_xy));
                            }
                            let mean_de00 = sum_de00 / (rows.len() as f64);
                            ui.label(format!(
                                "SATURATION SWEEP & COLORCHECKER VERIFICATION ({} PATCHES) // MEAN ΔE00 = {:.2} | MAX ΔE00 = {:.2}:",
                                rows.len(),
                                mean_de00,
                                max_de00
                            ));
                            for (vpatch, de00, tgt_xy) in rows {
                                let (mx, my, mlum) = vpatch.reading.to_xyy();
                                ui.monospace(format!(
                                    "  {:<26} | Lum: {:>6.2} cd/m² | Meas xy:({:.4},{:.4}) vs Tgt:({:.4},{:.4}) | ΔE00 = {:>5.2}",
                                    vpatch.label, mlum, mx, my, tgt_xy.0, tgt_xy.1, de00
                                ));
                            }
                        }

                        ui.add_space(8.0);
                        ui.separator();
                        ui.label("GENERATED ICC PROFILES:");
                        for p in saved_paths.iter() {
                            ui.monospace(format!("  -> {}", p.display()));
                        }
                        if let Some(status) = install_status.as_ref() {
                            ui.monospace(format!("  WAYLAND / COLORD INSTALL: {status}"));
                        }
                        if let Some(exp) = export_status.as_ref() {
                            ui.monospace(format!("  EXPORT COPY STATUS      : {exp}"));
                        }
                        ui.label(
                            "KWin Wayland: System Settings -> Display & Monitor -> Display Configuration -> Color Profile -> Assign ICC Profile.",
                        );

                        ui.add_space(10.0);
                        ui.label("EXPORT / SAVE A COPY OF THIS .ICC PROFILE TO CUSTOM LOCATION:");
                        let default_fname = saved_paths
                            .first()
                            .and_then(|p| p.file_name())
                            .and_then(|s| s.to_str())
                            .unwrap_or("yutani_calibrated.icc");
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Destination Path:");
                            let text_edit = egui::TextEdit::singleline(custom_save_path)
                                .desired_width(440.0);
                            ui.add(text_edit);
                            if ui.button(" [ BROWSE... ] ").clicked() {
                                trigger_browse_save = true;
                            }
                            if ui.button(" [ SAVE COPY TO PATH ] ").clicked() {
                                trigger_save_custom = true;
                            }
                        });
                        ui.horizontal_wrapped(|ui| {
                            ui.label("Quick Folders:");
                            for (label, prefix) in [
                                ("~/Desktop", "~/Desktop"),
                                ("~/Documents", "~/Documents"),
                                ("~/Downloads", "~/Downloads"),
                                ("~/.local/share/icc", "~/.local/share/icc"),
                            ] {
                                if ui.button(format!(" [ {label} ] ")).clicked() {
                                    *custom_save_path = format!("{prefix}/{default_fname}");
                                }
                            }
                        });

                        ui.add_space(12.0);
                        ui.horizontal_wrapped(|ui| {
                            if ui
                                .button(" [ INSTALL TO ~/.local/share/icc & REGISTER WITH COLORD ] ")
                                .clicked()
                            {
                                trigger_install = true;
                            }
                            if ui
                                .button(" [ SAVE .ICC COPY AS... (FILE DIALOG) ] ")
                                .clicked()
                            {
                                trigger_browse_save = true;
                            }
                            if ui.button(" [ RETURN TO YUTANI CONSOLE ] ").clicked() {
                                back_to_dash = true;
                            }
                        });
                    });
                });

                if trigger_browse_save {
                    if let Some(first_path) = saved_paths.first().cloned() {
                        match pick_icc_save_path_native_dialog(custom_save_path) {
                            Ok(Some(chosen_path)) => {
                                let chosen_str = chosen_path.to_string_lossy().into_owned();
                                *custom_save_path = chosen_str.clone();
                                match save_icc_profile_copy(&first_path, &chosen_str) {
                                    Ok(final_dest) => {
                                        if !saved_paths.contains(&final_dest) {
                                            saved_paths.push(final_dest.clone());
                                        }
                                        let msg = format!(
                                            "SAVED .ICC COPY -> {}",
                                            final_dest.display()
                                        );
                                        self.status_log = msg.clone();
                                        *export_status = Some(msg);
                                    }
                                    Err(err) => {
                                        let msg = format!("SAVE ERROR: {err}");
                                        self.status_log = msg.clone();
                                        *export_status = Some(msg);
                                    }
                                }
                            }
                            Ok(None) => {
                                // User canceled native file dialog; keep current custom_save_path intact
                            }
                            Err(err) => {
                                *export_status = Some(err);
                            }
                        }
                    }
                }

                if trigger_save_custom {
                    if let Some(first_path) = saved_paths.first().cloned() {
                        match save_icc_profile_copy(&first_path, custom_save_path) {
                            Ok(final_dest) => {
                                *custom_save_path = final_dest.to_string_lossy().into_owned();
                                if !saved_paths.contains(&final_dest) {
                                    saved_paths.push(final_dest.clone());
                                }
                                let msg = format!("SAVED .ICC COPY -> {}", final_dest.display());
                                self.status_log = msg.clone();
                                *export_status = Some(msg);
                            }
                            Err(err) => {
                                let msg = format!("SAVE ERROR: {err}");
                                self.status_log = msg.clone();
                                *export_status = Some(msg);
                            }
                        }
                    }
                }

                if trigger_install {
                    if let Some(first_path) = saved_paths.first() {
                        match install_and_register_icc_profile(first_path, current_display) {
                            Ok(msg) => {
                                self.status_log = msg.clone();
                                *install_status = Some(msg);
                            }
                            Err(err) => {
                                self.status_log = format!("ICC INSTALL ERROR: {err}");
                                *install_status = Some(format!("ERROR: {err}"));
                            }
                        }
                    }
                }
                if back_to_dash {
                    self.phase = AppPhase::Dashboard;
                }
            }
        }
    }
}

