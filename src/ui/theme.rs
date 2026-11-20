//! 主题初始化：锁定浅色主题（对齐旧版 egui 的浅色观感）。
//! gpui-kit 默认浅色主题已按语义 token 打磨，无需逐项覆写。

use gpui_kit::App;
use gpui_kit::component::{Theme, ThemeMode};

pub fn init(cx: &mut App) {
    Theme::change(ThemeMode::Light, None, cx);
}
