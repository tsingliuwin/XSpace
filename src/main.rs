//! XSpace 入口：创建 GPUI 应用、初始化 gpui-kit 与浅色主题、打开主窗口。
//!
//! 中文字体由 gpui-kit 的系统字体回退栈处理（Windows: Microsoft YaHei 等），
//! 无需手动注册字体；exe 图标仍由 build.rs 的 winres 嵌入。

mod app;
mod scan;
mod scanner;
mod titlebar_color;
mod types;
mod ui;
mod utils;

use gpui_kit::{AppContext, Bounds, TitlebarOptions, WindowBounds, WindowOptions, open_window, px, size};

use crate::app::XSpaceApp;

fn main() {
    gpui_kit::application().run(|cx| {
        // 使用任何 gpui-kit 组件功能前必须先调用
        gpui_kit::init(cx);
        // 浅色主题（对齐旧版 egui 版本的观感）
        ui::theme::init(cx);

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: Default::default(),
                size: size(px(960.), px(620.)),
            })),
            window_min_size: Some(size(px(750.), px(620.))),
            titlebar: Some(TitlebarOptions {
                title: Some("XSpace - 极速磁盘空间探测器".into()),
                ..TitlebarOptions::default()
            }),
            ..WindowOptions::default()
        };

        open_window(options, cx, |window, cx| {
            let view = cx.new(|cx| XSpaceApp::new(window, cx));
            // 原生标题栏配色与页面主题一致（Windows DWM；其他平台空实现）
            titlebar_color::apply(window, cx);
            view
        })
        .expect("failed to open window");
    });
}
