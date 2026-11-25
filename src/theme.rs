use eframe::egui::{self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Frame, Margin, Shadow, Stroke, Ui, Vec2, Visuals};
use std::sync::Arc;

/// Load crisp Windows system fonts (Segoe UI + Cascadia Mono / Consolas) for modern typography.
/// `text_scale` enlarges every glyph (also text with an explicit size), leaving icons and spacing alone.
pub fn install_fonts(ctx: &egui::Context, text_scale: f32) {
    let mut fonts = FontDefinitions::default();

    let ui_font_paths = [
        r"C:\Windows\Fonts\segoeui.ttf",
        r"C:\Windows\Fonts\arial.ttf",
    ];
    for p in ui_font_paths {
        if let Ok(bytes) = std::fs::read(p) {
            fonts.font_data.insert("sys_ui".into(), Arc::new(FontData::from_owned(bytes)));
            fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "sys_ui".into());
            break;
        }
    }

    let mono_font_paths = [
        r"C:\Windows\Fonts\CascadiaMono.ttf",
        r"C:\Windows\Fonts\consola.ttf",
    ];
    for p in mono_font_paths {
        if let Ok(bytes) = std::fs::read(p) {
            fonts.font_data.insert("sys_mono".into(), Arc::new(FontData::from_owned(bytes)));
            fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "sys_mono".into());
            break;
        }
    }

    if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\segoeuib.ttf") {
        fonts.font_data.insert("sys_bold".into(), Arc::new(FontData::from_owned(bytes)));
        fonts.families.insert(FontFamily::Name("Bold".into()), vec!["sys_bold".into()]);
    }

    for fd in fonts.font_data.values_mut() {
        Arc::make_mut(fd).tweak.scale *= text_scale;
    }
    ctx.set_fonts(fonts);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeKind {
    Dark,
    Light,
    Nord,
    Sepia,
    Oled,
}

impl ThemeKind {
    pub const ALL: [ThemeKind; 5] = [
        ThemeKind::Dark,
        ThemeKind::Light,
        ThemeKind::Nord,
        ThemeKind::Sepia,
        ThemeKind::Oled,
    ];

    pub fn id(self) -> &'static str {
        match self {
            ThemeKind::Dark => "dark",
            ThemeKind::Light => "light",
            ThemeKind::Nord => "nord",
            ThemeKind::Sepia => "sepia",
            ThemeKind::Oled => "oled",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ThemeKind::Dark => "Studio Dark",
            ThemeKind::Light => "Porcelain Light",
            ThemeKind::Nord => "Nordic Midnight",
            ThemeKind::Sepia => "Warm Sepia",
            ThemeKind::Oled => "Pure OLED",
        }
    }

    pub fn from_id(id: &str) -> Self {
        match id {
            "light" => ThemeKind::Light,
            "nord" => ThemeKind::Nord,
            "sepia" => ThemeKind::Sepia,
            "oled" => ThemeKind::Oled,
            _ => ThemeKind::Dark,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ThemePalette {
    pub is_dark: bool,
    pub bg: Color32,
    pub panel: Color32,
    pub surface: Color32,
    pub surface_hover: Color32,
    pub line: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub green: Color32,
    pub red: Color32,
    pub amber: Color32,
    // Editor syntax colors
    pub syn_cmd: Color32,
    pub syn_env: Color32,
    pub syn_comment: Color32,
    pub syn_math: Color32,
    pub syn_brace: Color32,
    pub diff_add_bg: Color32,
    pub diff_del_bg: Color32,
}

impl ThemeKind {
    pub fn palette(self) -> ThemePalette {
        match self {
            ThemeKind::Dark => ThemePalette {
                is_dark: true,
                bg: Color32::from_rgb(18, 19, 24),
                panel: Color32::from_rgb(24, 26, 32),
                surface: Color32::from_rgb(31, 34, 42),
                surface_hover: Color32::from_rgb(40, 44, 55),
                line: Color32::from_rgb(46, 50, 62),
                text: Color32::from_rgb(232, 234, 240),
                muted: Color32::from_rgb(142, 148, 165),
                accent: Color32::from_rgb(94, 156, 255),
                green: Color32::from_rgb(56, 193, 114),
                red: Color32::from_rgb(240, 92, 92),
                amber: Color32::from_rgb(235, 179, 64),
                syn_cmd: Color32::from_rgb(130, 180, 255),
                syn_env: Color32::from_rgb(198, 148, 255),
                syn_comment: Color32::from_rgb(112, 120, 138),
                syn_math: Color32::from_rgb(110, 215, 165),
                syn_brace: Color32::from_rgb(235, 190, 110),
                diff_add_bg: Color32::from_rgba_unmultiplied(40, 160, 90, 45),
                diff_del_bg: Color32::from_rgba_unmultiplied(220, 70, 70, 45),
            },
            ThemeKind::Light => ThemePalette {
                is_dark: false,
                bg: Color32::from_rgb(247, 248, 250),
                panel: Color32::from_rgb(255, 255, 255),
                surface: Color32::from_rgb(240, 242, 246),
                surface_hover: Color32::from_rgb(229, 233, 240),
                line: Color32::from_rgb(218, 222, 230),
                text: Color32::from_rgb(26, 29, 36),
                muted: Color32::from_rgb(108, 115, 130),
                accent: Color32::from_rgb(37, 99, 235),
                green: Color32::from_rgb(22, 163, 74),
                red: Color32::from_rgb(220, 38, 38),
                amber: Color32::from_rgb(202, 138, 4),
                syn_cmd: Color32::from_rgb(29, 78, 216),
                syn_env: Color32::from_rgb(126, 34, 206),
                syn_comment: Color32::from_rgb(120, 130, 145),
                syn_math: Color32::from_rgb(15, 118, 110),
                syn_brace: Color32::from_rgb(180, 83, 9),
                diff_add_bg: Color32::from_rgba_unmultiplied(22, 163, 74, 30),
                diff_del_bg: Color32::from_rgba_unmultiplied(220, 38, 38, 30),
            },
            ThemeKind::Nord => ThemePalette {
                is_dark: true,
                bg: Color32::from_rgb(36, 41, 51),
                panel: Color32::from_rgb(46, 52, 64),
                surface: Color32::from_rgb(59, 66, 82),
                surface_hover: Color32::from_rgb(67, 76, 94),
                line: Color32::from_rgb(76, 86, 106),
                text: Color32::from_rgb(236, 239, 244),
                muted: Color32::from_rgb(165, 174, 192),
                accent: Color32::from_rgb(136, 192, 208),
                green: Color32::from_rgb(163, 190, 140),
                red: Color32::from_rgb(191, 97, 106),
                amber: Color32::from_rgb(235, 203, 139),
                syn_cmd: Color32::from_rgb(129, 161, 193),
                syn_env: Color32::from_rgb(180, 142, 173),
                syn_comment: Color32::from_rgb(115, 128, 150),
                syn_math: Color32::from_rgb(143, 188, 187),
                syn_brace: Color32::from_rgb(235, 203, 139),
                diff_add_bg: Color32::from_rgba_unmultiplied(163, 190, 140, 40),
                diff_del_bg: Color32::from_rgba_unmultiplied(191, 97, 106, 40),
            },
            ThemeKind::Sepia => ThemePalette {
                is_dark: false,
                bg: Color32::from_rgb(245, 238, 224),
                panel: Color32::from_rgb(251, 246, 236),
                surface: Color32::from_rgb(238, 229, 211),
                surface_hover: Color32::from_rgb(227, 216, 194),
                line: Color32::from_rgb(214, 201, 176),
                text: Color32::from_rgb(56, 46, 35),
                muted: Color32::from_rgb(128, 112, 93),
                accent: Color32::from_rgb(168, 86, 40),
                green: Color32::from_rgb(58, 125, 68),
                red: Color32::from_rgb(185, 56, 45),
                amber: Color32::from_rgb(176, 115, 20),
                syn_cmd: Color32::from_rgb(154, 68, 28),
                syn_env: Color32::from_rgb(120, 62, 138),
                syn_comment: Color32::from_rgb(146, 132, 112),
                syn_math: Color32::from_rgb(44, 114, 92),
                syn_brace: Color32::from_rgb(158, 104, 20),
                diff_add_bg: Color32::from_rgba_unmultiplied(58, 125, 68, 30),
                diff_del_bg: Color32::from_rgba_unmultiplied(185, 56, 45, 30),
            },
            ThemeKind::Oled => ThemePalette {
                is_dark: true,
                bg: Color32::from_rgb(0, 0, 0),
                panel: Color32::from_rgb(10, 11, 14),
                surface: Color32::from_rgb(20, 22, 27),
                surface_hover: Color32::from_rgb(32, 35, 43),
                line: Color32::from_rgb(38, 41, 50),
                text: Color32::from_rgb(240, 242, 248),
                muted: Color32::from_rgb(132, 138, 154),
                accent: Color32::from_rgb(105, 165, 255),
                green: Color32::from_rgb(52, 211, 153),
                red: Color32::from_rgb(248, 113, 113),
                amber: Color32::from_rgb(251, 191, 36),
                syn_cmd: Color32::from_rgb(138, 186, 255),
                syn_env: Color32::from_rgb(210, 160, 255),
                syn_comment: Color32::from_rgb(104, 112, 128),
                syn_math: Color32::from_rgb(94, 234, 212),
                syn_brace: Color32::from_rgb(252, 211, 77),
                diff_add_bg: Color32::from_rgba_unmultiplied(52, 211, 153, 38),
                diff_del_bg: Color32::from_rgba_unmultiplied(248, 113, 113, 38),
            },
        }
    }

    pub fn apply(self, ctx: &egui::Context) {
        let p = self.palette();
        let mut v = if p.is_dark { Visuals::dark() } else { Visuals::light() };
        v.panel_fill = p.panel;
        v.window_fill = p.panel;
        v.extreme_bg_color = p.bg;
        v.faint_bg_color = p.surface;
        v.code_bg_color = p.bg;
        v.override_text_color = Some(p.text);
        v.selection.bg_fill = p.accent.linear_multiply(0.32);
        v.selection.stroke = Stroke::new(1.0, p.accent);
        v.window_stroke = Stroke::new(1.0, p.glass_border_color());
        v.window_fill = p.glass_panel_color();
        v.window_corner_radius = CornerRadius::same(10);
        v.menu_corner_radius = CornerRadius::same(8);
        v.window_shadow = p.glass_shadow();
        v.popup_shadow = p.glass_shadow();
        v.text_edit_bg_color = Some(p.glass_card_color());

        // Minimal: borderless controls at rest, a soft fill on hover, accent only when pressed.
        let r = CornerRadius::same(6);
        let quiet = Stroke::new(1.0, p.line.gamma_multiply(0.7));
        v.widgets.noninteractive.bg_fill = p.panel;
        v.widgets.noninteractive.weak_bg_fill = p.panel;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.line.gamma_multiply(0.6));
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
        v.widgets.noninteractive.corner_radius = r;

        v.widgets.inactive.bg_fill = p.surface;
        v.widgets.inactive.weak_bg_fill = p.glass_card_color();
        v.widgets.inactive.bg_stroke = quiet;
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text);
        v.widgets.inactive.corner_radius = r;
        v.widgets.inactive.expansion = 0.0;

        v.widgets.hovered.bg_fill = p.surface_hover;
        v.widgets.hovered.weak_bg_fill = p.surface_hover;
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, p.line);
        v.widgets.hovered.fg_stroke = Stroke::new(1.0, p.text);
        v.widgets.hovered.corner_radius = r;
        v.widgets.hovered.expansion = 0.0;

        v.widgets.active.bg_fill = p.accent;
        v.widgets.active.weak_bg_fill = p.accent.gamma_multiply(0.85);
        v.widgets.active.bg_stroke = Stroke::new(1.0, p.accent);
        v.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        v.widgets.active.corner_radius = r;
        v.widgets.active.expansion = 0.0;

        v.widgets.open.weak_bg_fill = p.surface_hover;
        v.widgets.open.bg_stroke = Stroke::new(1.0, p.line);
        v.widgets.open.corner_radius = r;

        ctx.set_visuals(v);

        let mut style = (*ctx.global_style()).clone();
        style.spacing.item_spacing = Vec2::new(8.0, 6.0);
        style.spacing.button_padding = Vec2::new(10.0, 4.0);
        style.spacing.menu_margin = Margin::same(6);
        style.spacing.window_margin = Margin::same(16);
        style.spacing.interact_size.y = 26.0;
        style.animation_time = 0.18;
        ctx.set_global_style(style);
    }
}

impl ThemePalette {
    pub fn glass_panel_color(self) -> Color32 {
        let [r, g, b, _] = self.panel.to_array();
        Color32::from_rgba_unmultiplied(r, g, b, if self.is_dark { 208 } else { 222 })
    }

    pub fn glass_card_color(self) -> Color32 {
        let [r, g, b, _] = self.surface.to_array();
        Color32::from_rgba_unmultiplied(r, g, b, if self.is_dark { 175 } else { 210 })
    }

    pub fn glass_border_color(self) -> Color32 {
        if self.is_dark {
            Color32::from_rgba_unmultiplied(255, 255, 255, 30)
        } else {
            Color32::from_rgba_unmultiplied(255, 255, 255, 200)
        }
    }

    pub fn glass_shadow(self) -> Shadow {
        Shadow {
            offset: [0, 6],
            blur: 18,
            spread: 0,
            color: Color32::from_rgba_unmultiplied(0, 0, 0, if self.is_dark { 75 } else { 24 }),
        }
    }

    pub fn glass_panel_frame(self, margin: Margin) -> Frame {
        Frame::new()
            .fill(self.glass_panel_color())
            .stroke(Stroke::new(1.0, self.glass_border_color()))
            .inner_margin(margin)
    }
}

/// Paint subtle ambient gradient orbs behind translucent glass panels.
pub fn paint_glass_backdrop(ui: &Ui, pal: ThemePalette) {
    let rect = ui.max_rect();
    let painter = ui.painter();
    let orb1 = rect.left_top() + Vec2::new(rect.width() * 0.18, rect.height() * 0.22);
    let orb2 = rect.right_bottom() - Vec2::new(rect.width() * 0.16, rect.height() * 0.20);
    let alpha1 = if pal.is_dark { 22 } else { 28 };
    let alpha2 = if pal.is_dark { 18 } else { 24 };
    painter.circle_filled(orb1, rect.width().max(rect.height()) * 0.34, Color32::from_rgba_unmultiplied(pal.accent.r(), pal.accent.g(), pal.accent.b(), alpha1));
    painter.circle_filled(orb2, rect.width().max(rect.height()) * 0.30, Color32::from_rgba_unmultiplied(pal.syn_env.r(), pal.syn_env.g(), pal.syn_env.b(), alpha2));
}
