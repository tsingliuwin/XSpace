#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod types;
mod utils;
mod scanner;
mod app;

use eframe::egui;
use app::SpaceProbeApp;

// 配置自定义样式，打造高雅的浅色系风格
fn configure_custom_style(ctx: &egui::Context) {
    // 寻找系统中的中文字体以支持多平台（Windows / macOS / Linux）中文显示
    let font_paths = [
        // Windows
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\msyh.ttf",
        "C:\\Windows\\Fonts\\simsun.ttc",
        // macOS
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
        "/System/Library/Fonts/STHeiti Medium.ttc",
        "/Library/Fonts/Arial Unicode.ttf",
        // Linux
        "/usr/share/fonts/wqy-zenhei/wqy-zenhei.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/NotoSansCJK.ttc",
    ];

    for path in &font_paths {
        if let Ok(font_data) = std::fs::read(path) {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(
                "custom_chinese".to_owned(),
                egui::FontData::from_owned(font_data),
            );
            fonts.families.entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "custom_chinese".to_owned());
            fonts.families.entry(egui::FontFamily::Monospace)
                .or_default()
                .insert(0, "custom_chinese".to_owned());
            ctx.set_fonts(fonts);
            break;
        }
    }

    let mut style = (*ctx.style()).clone();
    
    // 增加间距和舒适度
    style.spacing.item_spacing = egui::vec2(12.0, 10.0);
    style.spacing.button_padding = egui::vec2(14.0, 8.0);
    style.visuals.window_rounding = 8.0.into();
    style.visuals.widgets.noninteractive.rounding = 8.0.into();
    style.visuals.widgets.inactive.rounding = 6.0.into();
    style.visuals.widgets.hovered.rounding = 6.0.into();
    style.visuals.widgets.active.rounding = 6.0.into();
    
    // 设置高贵雅致的浅色系配色
    let mut visuals = egui::Visuals::light();
    visuals.panel_fill = egui::Color32::from_rgb(0xF3, 0xF4, 0xF6); // 主面板浅灰底色
    visuals.window_fill = egui::Color32::from_rgb(0xFF, 0xFF, 0xFF); // 容器卡片纯白底色
    visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(0xF9, 0xFA, 0xFB);
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(0xE5, 0xE7, 0xEB); // 组件未选中填充
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(0xD1, 0xD5, 0xDB);
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(0x9C, 0xA3, 0xAF);
    
    // 文字颜色微调
    visuals.widgets.noninteractive.fg_stroke.color = egui::Color32::from_rgb(0x1F, 0x29, 0x37); // 深色文字
    visuals.widgets.inactive.fg_stroke.color = egui::Color32::from_rgb(0x37, 0x41, 0x51);
    
    // 主题高亮强调色
    visuals.selection.bg_fill = egui::Color32::from_rgb(0xDB, 0xEA, 0xFE); // 浅蓝色选择背景
    
    style.visuals = visuals;
    ctx.set_style(style);
}

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 620.0])
            .with_min_inner_size([750.0, 520.0])
            .with_title("🔍 XSpace - 极速磁盘空间探测器"),
        default_theme: eframe::Theme::Light,
        ..Default::default()
    };
    
    eframe::run_native(
        "xspace",
        options,
        Box::new(|cc| {
            configure_custom_style(&cc.egui_ctx);
            Box::new(SpaceProbeApp::new(cc))
        }),
    )
}