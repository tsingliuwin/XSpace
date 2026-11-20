//! 欢迎页：无扫描结果时的中央引导区域。
//! 文案面向用户价值（解决什么问题），技术实现与版权信息不在此展示。

use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, Hsla, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, px
};

// 内容固定宽度：items_center 布局链不提供宽度，必须显式指定，否则文本坍缩为单字宽
const CONTENT_W: f32 = 540.;

pub fn render_welcome(cx: &App) -> AnyElement {
    let muted = cx.theme().muted_foreground;

    v_flex()
        .id("welcome_scroll")
        .size_full()
        .items_center()
        .justify_center()
        .px_8()
        .py_6()
        .overflow_y_scroll()
        .child(
            v_flex()
                .w(px(CONTENT_W))
                .items_center()
                .gap_1()
                .child(
                    gpui_kit::div()
                        .text_size(px(26.))
                        .font_weight(gpui_kit::FontWeight::BOLD)
                        .child("欢迎使用 XSpace"),
                )
                .child(div_muted(muted).child("磁盘又满了？几秒钟看清空间都去哪了。"))
                .child(div_muted(muted).child(
                    "揪出最占空间的大文件、清理无用的空文件夹、按名字找到任何文件——释放空间，就是这么简单。",
                )),
        )
        .child(
            v_flex()
                .w(px(CONTENT_W))
                .gap_2()
                .pt_8()
                .child(strong_line("🚀 快速开始："))
                .child(step_line("①", "勾选要扫描的磁盘分区。"))
                .child(step_line("②", "选择扫描类型，按需调整阈值或关键字。"))
                .child(step_line("③", "点击【开始扫描】，结果实时呈现。")),
        )
        .into_any_element()
}

fn div_muted(color: Hsla) -> gpui_kit::Div {
    gpui_kit::div().text_color(color)
}

fn strong_line(s: &str) -> gpui_kit::Div {
    gpui_kit::div()
        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
        .child(s.to_string())
}

// 步骤行：序号 + 说明（说明占满剩余宽度，避免文本坍缩）
fn step_line(num: &str, text: &str) -> gpui_kit::Div {
    h_flex()
        .gap_2()
        .items_start()
        .child(
            gpui_kit::div()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .child(num.to_string()),
        )
        .child(gpui_kit::div().flex_1().min_w_0().child(text.to_string()))
}
