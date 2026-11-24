//! Tray / menu bar icon: a status-colored ring around a stamina pie (same
//! color), drawn with tiny-skia. On macOS the label text is shown next to the
//! icon (`set_title`); Windows and Linux trays only show icons, so the text
//! goes in the tooltip.

use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

/// Rendered at 2x the logical size for sharp HiDPI icons.
const SIZE: u32 = 36;

pub struct Tray {
    icon: TrayIcon,
    last: Option<(u32, Option<i32>, String)>,
}

impl Tray {
    pub fn new() -> anyhow::Result<Self> {
        let icon = TrayIconBuilder::new()
            .with_icon(render(0x8e8e93, Some(0.0))?)
            .with_tooltip("BitCraft Tracker")
            // Left click opens our popup instead of a menu.
            .with_menu_on_left_click(false)
            .build()?;
        Ok(Self { icon, last: None })
    }

    /// Updates the icon/text only when something changed.
    pub fn update(&mut self, rgb: u32, stamina: Option<f64>, text: &str) {
        // Pie quantized to whole percents to avoid redrawing every tick.
        let key = (rgb, stamina.map(|f| (f * 100.0).round() as i32), text.to_string());
        if self.last.as_ref() == Some(&key) {
            return;
        }
        if let Ok(icon) = render(rgb, stamina) {
            let _ = self.icon.set_icon(Some(icon));
        }
        #[cfg(target_os = "macos")]
        self.icon.set_title(Some(format!(" {text}")));
        let tooltip = if text.is_empty() { "BitCraft Tracker".to_string() } else { format!("BitCraft Tracker\n{text}") };
        let _ = self.icon.set_tooltip(Some(tooltip));
        self.last = Some(key);
    }
}

fn color(rgb: u32, alpha: f32) -> Color {
    let c = |shift: u32| ((rgb >> shift) & 0xff) as f32 / 255.0;
    Color::from_rgba(c(16), c(8), c(0), alpha).unwrap_or(Color::BLACK)
}

/// Ring + pie. `stamina` None → solid dot. Pie starts at 12 o'clock and
/// fills counter-clockwise (user preference in the macOS app).
fn render(rgb: u32, stamina: Option<f64>) -> anyhow::Result<Icon> {
    let mut pixmap = Pixmap::new(SIZE, SIZE).ok_or_else(|| anyhow::anyhow!("pixmap"))?;
    let s = SIZE as f32;
    let center = s / 2.0;
    let ring_width = s * 1.5 / 16.0;
    let gap = s * 1.5 / 16.0;
    let mut paint = Paint { anti_alias: true, ..Default::default() };

    // Ring.
    paint.set_color(color(rgb, 1.0));
    if let Some(ring) = PathBuilder::from_circle(center, center, center - ring_width / 2.0) {
        let stroke = Stroke { width: ring_width, ..Default::default() };
        pixmap.stroke_path(&ring, &paint, &stroke, Transform::identity(), None);
    }

    // Pie: faint track, then the filled wedge.
    let radius = center - ring_width - gap;
    paint.set_color(color(rgb, if stamina.is_some() { 0.3 } else { 1.0 }));
    if let Some(track) = PathBuilder::from_circle(center, center, radius) {
        pixmap.fill_path(&track, &paint, FillRule::Winding, Transform::identity(), None);
    }
    if let Some(fraction) = stamina.filter(|f| *f > 0.0) {
        paint.set_color(color(rgb, 1.0));
        let path = if fraction >= 1.0 {
            PathBuilder::from_circle(center, center, radius)
        } else {
            let mut pb = PathBuilder::new();
            pb.move_to(center, center);
            // Screen coords (y down): 12 o'clock = -90°; counter-clockwise =
            // decreasing angle.
            let steps = 48;
            let sweep = std::f32::consts::TAU * fraction as f32;
            for i in 0..=steps {
                let a = -std::f32::consts::FRAC_PI_2 - sweep * i as f32 / steps as f32;
                pb.line_to(center + radius * a.cos(), center + radius * a.sin());
            }
            pb.close();
            pb.finish()
        };
        if let Some(path) = path {
            pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
        }
    }

    // tiny-skia stores premultiplied RGBA; tray icons want straight RGBA.
    let rgba = pixmap
        .pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    Ok(Icon::from_rgba(rgba, SIZE, SIZE)?)
}
