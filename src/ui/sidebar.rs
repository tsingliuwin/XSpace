//! 左侧参数面板：扫描分区（含全选不确定态）、参数配置（按预设自适应）、扫描按钮。

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, NumberInput};
use gpui_kit::component::select::Select;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Disableable as _, h_flex, v_flex};
use gpui_kit::{
    AnyElement, Context, FontWeight, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, Window, px, rgb,
};

use crate::app::{DirMetric, XSpaceApp};
use crate::types::ScanPreset;
use crate::ui::results::card_style;

impl XSpaceApp {
    pub(crate) fn render_sidebar(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .id("sidebar_scroll")
            .w(px(crate::ui::SIDEBAR_W))
            .flex_shrink_0()
            .h_full()
            .min_h_0()
            .p_3()
            .gap_3()
            .overflow_y_scroll()
            .child(self.render_disks_card(cx))
            .child(self.render_params_card(cx))
            .child(self.render_scan_button(cx))
            .into_any_element()
    }

    // ===== 扫描分区（全选 + 各分区，全选用 0.10.0 不确定态） =====
    fn render_disks_card(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let all = self.disks.iter().all(|d| d.is_selected);
        let any = self.disks.iter().any(|d| d.is_selected);

        let mut list = v_flex().gap_1();
        for ix in 0..self.disks.len() {
            let checked = self.disks[ix].is_selected;
            let name = self.disks[ix].name.clone();
            list = list.child(
                Checkbox::new(("disk", ix))
                    .checked(checked)
                    .label(name)
                    .on_change(cx.listener(move |this, checked: &bool, _, cx| {
                        this.disks[ix].is_selected = *checked;
                        cx.notify();
                    })),
            );
        }

        card_style(
            v_flex()
                .gap_2()
                .p_3()
                .child(
                    gpui_kit::div()
                        .font_weight(FontWeight::BOLD)
                        .child("💾 扫描分区"),
                )
                .child(
                    Checkbox::new("disks_all")
                        .label("全选")
                        .checked(all)
                        .indeterminate(any && !all) // 部分选中 → 不确定态（0.10.0 新特性）
                        .on_change(cx.listener(|this, checked: &bool, _, cx| {
                            for d in this.disks.iter_mut() {
                                d.is_selected = *checked;
                            }
                            cx.notify();
                        })),
                ),
            cx,
        )
        .child(list)
        .into_any_element()
    }

    // ===== 参数配置（按预设自适应子控件） =====
    fn render_params_card(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let disabled = self.is_scanning;

        let mut body = v_flex().gap_2();

        // 预设专属参数
        match self.preset {
            ScanPreset::BigFiles => {
                body = body.child(
                    v_flex()
                        .gap_1()
                        .child(
                            gpui_kit::div()
                                .text_color(cx.theme().muted_foreground)
                                .child("大小阈值（MB）："),
                        )
                        .child(
                            h_flex().child(
                                NumberInput::new(&self.min_size_input)
                                    .stepper(true)
                                    .disabled(disabled)
                                    .w(px(140.)),
                            ),
                        ),
                );
            }
            ScanPreset::EmptyFiles => {
                body = body.child(hint_line("ℹ️ 检索体积为 0 字节的空文件", cx));
            }
            ScanPreset::EmptyDirs => {
                body = body.child(hint_line("ℹ️ 检索不含任何子项的空文件夹", cx));
            }
            ScanPreset::FindFiles | ScanPreset::FindDirs => {
                let label = if self.preset == ScanPreset::FindFiles {
                    "文件名："
                } else {
                    "目录名："
                };
                body = body
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                gpui_kit::div()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(label),
                            )
                            .child(
                                Input::new(&self.query_input)
                                    .disabled(disabled)
                                    .w_full(),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Switch::new("exact_match")
                                    .checked(self.exact_match)
                                    .disabled(disabled)
                                    .on_change(cx.listener(|this, checked: &bool, _, cx| {
                                        this.exact_match = *checked;
                                        this.validation_error = None;
                                        cx.notify();
                                    })),
                            )
                            // 短标签 + 问号图标，完整说明放 tooltip（侧栏窄，长文案会溢出）
                            .child(
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .child("精确匹配")
                                    .child(
                                        gpui_kit::div()
                                            .id("exact_help")
                                            .text_size(px(12.))
                                            .text_color(cx.theme().muted_foreground)
                                            .child("ⓘ")
                                            .tooltip(|window, cx| {
                                                Tooltip::new("开启后需全名一致，关闭则包含关键字即可")
                                                    .build(window, cx)
                                            }),
                                    ),
                            ),
                    );

                // FindDirs：统计维度 / 排序键（Select 带 0.10.0 匹配高亮）
                if self.preset == ScanPreset::FindDirs {
                    body = body.child(
                        v_flex()
                            .gap_1()
                            .child(
                                gpui_kit::div()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("统计维度："),
                            )
                            .child(
                                Select::new(&self.dir_metric_select)
                                    .highlight_matches(true) // 0.10.0 新特性
                                    .disabled(disabled)
                                    .w(px(160.)),
                            ),
                    );
                    if self.dir_metric == DirMetric::Both {
                        body = body.child(
                            v_flex()
                                .gap_1()
                                .child(
                                    gpui_kit::div()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("排序键："),
                                )
                                .child(
                                    Select::new(&self.dir_sort_select)
                                        .highlight_matches(true)
                                        .disabled(disabled)
                                        .w(px(160.)),
                                ),
                        );
                    }
                }
            }
        }

        // 结果数量（非 Empty* 预设可配）
        let can_limit = !matches!(self.preset, ScanPreset::EmptyFiles | ScanPreset::EmptyDirs);
        if can_limit {
            body = body.child(
                Checkbox::new("show_all")
                    .label("显示全部结果")
                    .checked(self.show_all)
                    .disabled(disabled)
                    .on_change(cx.listener(|this, checked: &bool, _, cx| {
                        this.show_all = *checked;
                        cx.notify();
                    })),
            );
            if !self.show_all {
                body = body.child(
                    v_flex()
                        .gap_1()
                        .child(
                            gpui_kit::div()
                                .text_color(cx.theme().muted_foreground)
                                .child("结果数量（Top N）："),
                        )
                        .child(
                            h_flex().child(
                                NumberInput::new(&self.top_n_input)
                                    .stepper(true)
                                    .disabled(disabled)
                                    .w(px(140.)),
                            ),
                        ),
                );
            }
        }

        card_style(
            v_flex()
                .gap_2()
                .p_3()
                .child(
                    gpui_kit::div()
                        .font_weight(FontWeight::BOLD)
                        .child("⚙️ 参数配置"),
                ),
            cx,
        )
        .child(body)
        .into_any_element()
    }

    // ===== 扫描按钮 + 校验错误 =====
    fn render_scan_button(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let mut col = v_flex().gap_1();
        if self.is_scanning {
            col = col.child(
                Button::new("stop_scan")
                    .danger()
                    .label("⏹ 停止")
                    .w_full()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.stop_scan();
                        cx.notify();
                    })),
            );
        } else {
            let has_selection = self.disks.iter().any(|d| d.is_selected);
            let btn = Button::new("start_scan")
                .primary()
                .label("▶ 开始扫描")
                .w_full()
                .disabled(!has_selection)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.try_start_scan(cx);
                }));
            col = col.child(if has_selection {
                btn.into_any_element()
            } else {
                btn.into_any_element()
            });
        }
        if let Some(err) = &self.validation_error {
            col = col.child(
                gpui_kit::div()
                    .text_color(rgb(0xef4444))
                    .text_size(px(12.))
                    .child(format!("⚠️ {}", err)),
            );
        }
        col.into_any_element()
    }
}

fn hint_line(text: &str, cx: &gpui_kit::App) -> gpui_kit::Div {
    gpui_kit::div()
        .text_color(cx.theme().muted_foreground)
        .text_size(px(12.))
        .child(text.to_string())
}
