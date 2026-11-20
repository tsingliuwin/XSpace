//! 结果区：DataTable（虚拟滚动）+ 工具栏（复制全部 / 下载 TXT / 单行复制 / 定位）。

use std::sync::{Arc, Mutex};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Size, Sizable as _};
use gpui_kit::component::label::Label;
use gpui_kit::component::table::{Column, DataTable, TableDelegate, TableState};
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, ClickEvent, ClipboardItem, Context, InteractiveElement, IntoElement,
    ParentElement, Rgba, StatefulInteractiveElement, Styled, Window, px, rgb,
};

use crate::app::XSpaceApp;
use crate::types::{FileInfo, ScanPreset};
use crate::utils::{downloads_dir, format_size, reveal_in_file_manager};

// 与旧版一致的强调色
fn rank_top_color() -> Rgba {
    rgb(0xf59e0b) // 金色 top 3
}
fn size_color() -> Rgba {
    rgb(0x0284c7)
}
fn count_color() -> Rgba {
    rgb(0x10b981)
}

// 卡片容器：白底 + 细边框 + 圆角（对齐旧版 egui 卡片观感）
pub fn card_style(el: gpui_kit::Div, cx: &App) -> gpui_kit::Div {
    el.bg(cx.theme().popover)
        .border_1()
        .border_color(cx.theme().border)
        .rounded_lg()
}

// 结果表数据（app 与 delegate 共享同一份，避免高频拷贝）。
// 注意：不设 loading 标志——DataTable 在 loading 态下会用加载占位替换整个行区域，
// 扫描中实时出结果的需求要求行区域始终渲染（扫描状态由顶部进度卡表达）。
#[derive(Default)]
pub struct TableData {
    pub rows: Vec<FileInfo>,
    pub show_size: bool,
    pub show_count: bool,
    // 路径列宽（DataTable 列宽为固定像素、无 flex 概念，由 app 按视口宽度动态算出，
    // 使"完整路径"列尽可能占满剩余空间；用户仍可拖拽微调，下次窗口尺寸变化时重算）
    pub path_col_width: f32,
}

pub struct ResultsTable {
    pub data: Arc<Mutex<TableData>>,
}

impl ResultsTable {
    pub fn new() -> Self {
        Self {
            data: Arc::new(Mutex::new(TableData::default())),
        }
    }
}

// 列语义（列数随 show_size / show_count 自适应）
#[derive(Clone, Copy, PartialEq)]
enum Col {
    Rank,
    Size,
    Count,
    Path,
    Actions,
}

impl ResultsTable {
    fn cols(&self) -> Vec<Col> {
        let d = self.data.lock().unwrap();
        let mut v = vec![Col::Rank];
        if d.show_size {
            v.push(Col::Size);
        }
        if d.show_count {
            v.push(Col::Count);
        }
        v.push(Col::Path);
        v.push(Col::Actions);
        v
    }

    fn col_at(&self, col_ix: usize) -> Option<Col> {
        self.cols().get(col_ix).copied()
    }
}

impl TableDelegate for ResultsTable {
    fn columns_count(&self, _cx: &App) -> usize {
        self.cols().len()
    }

    fn rows_count(&self, _cx: &App) -> usize {
        self.data.lock().unwrap().rows.len()
    }

    fn column(&self, col_ix: usize, _cx: &App) -> Column {
        let path_w = self.data.lock().unwrap().path_col_width;
        match self.col_at(col_ix) {
            Some(Col::Rank) => Column::new("rank", "排名").width(px(56.)).text_center(),
            Some(Col::Size) => Column::new("size", "占用大小").width(px(84.)).text_right(),
            Some(Col::Count) => Column::new("count", "内含文件数").width(px(84.)).text_right(),
            Some(Col::Path) => {
                Column::new("path", "完整路径").width(px(path_w.max(180.)))
            }
            Some(Col::Actions) => Column::new("actions", "操作").width(px(68.)).text_center(),
            None => Column::new("empty", ""),
        }
    }

    // 尾部填充列：默认渲染 12px 宽的空白格（视觉上像多了一列空表头），改为零宽
    fn render_last_empty_col(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        gpui_kit::div().w_0()
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        // 注意：取数用独立作用域，锁在取完即释放——
        // 后续 col_at() 内部还要加同一把锁，若 Guard 存活到这里会自死锁（Mutex 不可重入）
        let f = { self.data.lock().unwrap().rows.get(row_ix).cloned() };
        let col = self.col_at(col_ix);
        let Some(f) = f else {
            return gpui_kit::div().into_any_element();
        };
        match col {
            Some(Col::Rank) => {
                let is_top3 = row_ix < 3;
                let color: gpui_kit::Hsla =
                    if is_top3 { rank_top_color().into() } else { cx.theme().foreground };
                gpui_kit::div()
                    .flex()
                    .w_full()
                    .justify_center()
                    .text_color(color)
                    .font_weight(if is_top3 {
                        gpui_kit::FontWeight::BOLD
                    } else {
                        gpui_kit::FontWeight::NORMAL
                    })
                    .child(format!("{}", row_ix + 1))
                    .into_any_element()
            }
            Some(Col::Size) => gpui_kit::div()
                .flex()
                .w_full()
                .justify_end()
                .text_color(size_color())
                .child(format_size(f.size))
                .into_any_element(),
            Some(Col::Count) => {
                let text = match f.file_count {
                    Some(c) => format!("{} 个", c),
                    None => "-".to_string(),
                };
                gpui_kit::div()
                    .flex()
                    .w_full()
                    .justify_end()
                    .text_color(count_color())
                    .child(text)
                    .into_any_element()
            }
            Some(Col::Path) => {
                // 纯文本路径（无图标）；hover 显示完整路径 tooltip
                let full_path = f.path.clone();
                gpui_kit::div()
                    .id(("path_cell", row_ix))
                    .min_w_0()
                    .flex_1()
                    .overflow_hidden()
                    .truncate()
                    .child(f.path.clone())
                    .tooltip(move |window, cx| {
                        Tooltip::new(full_path.clone()).build(window, cx)
                    })
                    .into_any_element()
            }
            Some(Col::Actions) => {
                let path_copy = f.path.clone();
                let path_reveal = f.path.clone();
                // XSmall 图标按钮（20×20）：紧凑排布，避免超出列宽
                h_flex()
                    .gap_1()
                    .child(
                        Button::new(("cell_copy", row_ix))
                            .ghost()
                            .with_size(Size::XSmall)
                            .label("📋")
                            .on_click(move |_, _, cx: &mut App| {
                                cx.write_to_clipboard(ClipboardItem::new_string(path_copy.clone()));
                            }),
                    )
                    .child(
                        Button::new(("cell_reveal", row_ix))
                            .ghost()
                            .with_size(Size::XSmall)
                            .label("📂")
                            .on_click(move |_, _, _| {
                                reveal_in_file_manager(&path_reveal);
                            }),
                    )
                    .into_any_element()
            }
            None => gpui_kit::div().into_any_element(),
        }
    }
}

// ===== 中央面板（进度 / 摘要 + 结果表） =====

impl XSpaceApp {
    pub(crate) fn render_central(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if !self.is_scanning && self.result_count(cx) == 0 && self.scan_duration.is_none() {
            return crate::ui::welcome::render_welcome(cx);
        }

        v_flex()
            .size_full()
            .min_h_0()
            .p_2()
            .child(self.render_table_card(window, cx))
            .into_any_element()
    }

    fn render_table_card(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // 动态路径列宽：视口宽 − 侧栏 − 卡片边距 − 其他固定列，让"完整路径"尽可能宽。
        // 渲染期不允许更新其他实体，defer 到本帧结束后应用。
        let scale = window.scale_factor();
        let viewport_w: f32 = f32::from(window.viewport_size().width / scale);
        let other_cols = 56.0 + 68.0
            + if self.current_show_size() { 84.0 } else { 0.0 }
            + if self.current_show_count() { 84.0 } else { 0.0 };
        // 侧栏 + 8×2 面板内边距 + 8×2 卡片内边距 + 少量冗余
        // （垂直滚动条为 overlay 不占布局宽，冗余仅留 4px 避免列总宽超出出现横向滚动）
        let path_w = (viewport_w - crate::ui::SIDEBAR_W - 16.0 - 16.0 - 4.0 - other_cols)
            .max(180.0);
        if self
            .last_path_width
            .map_or(true, |w| (w - path_w).abs() > 4.0)
        {
            self.last_path_width = Some(path_w);
            let w = path_w;
            cx.defer_in(window, move |this, _window, cx| {
                this.apply_path_width(w, cx);
            });
        }

        let title = match self.preset {
            ScanPreset::BigFiles => "大文件",
            ScanPreset::EmptyFiles => "空文件",
            ScanPreset::EmptyDirs => "空目录",
            ScanPreset::FindFiles => "搜文件",
            ScanPreset::FindDirs => "搜目录",
        };
        let show_size = self.current_show_size();
        let show_count = self.current_show_count();
        // 只捕获 Arc 引用（计数一下即可），点击时才加锁取数——
        // 避免扫描期间每帧克隆整个结果集（数万行时会把 UI 线程拖死）
        let data_arc = self.table.read(cx).delegate().data.clone();

        let copy_all = {
            let data = data_arc.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                let rows = &data.lock().unwrap().rows;
                let text = rows
                    .iter()
                    .map(|f| f.path.clone())
                    .collect::<Vec<_>>()
                    .join("\n");
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
        };

        let download = {
            let data = data_arc.clone();
            move |_: &ClickEvent, _: &mut Window, _cx: &mut App| {
                let rows = data.lock().unwrap().rows.clone();
                if let Some(path) = export_results_txt(title, &rows, show_size, show_count) {
                    reveal_in_file_manager(&path.display().to_string());
                }
            }
        };

        // 列表卡片：仅白底 + 圆角，不带外层边框（表格自带网格线）
        v_flex()
            .bg(cx.theme().popover)
            .rounded_lg()
            .flex_1()
            .min_h_0()
            .p_2()
            .gap_2()
            .child(
            h_flex()
                .justify_between()
                .child(Label::new(format!("📋 {}", self.list_title())))
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("download_txt")
                                .ghost()
                                .label("⬇ 下载TXT")
                                .on_click(download),
                        )
                        .child(
                            Button::new("copy_all")
                                .ghost()
                                .label("📋 复制全部")
                                .on_click(copy_all),
                        ),
                ),
        )
        .child(
            gpui_kit::div()
                .flex_1()
                .min_h_0()
                .child(DataTable::new(&self.table).stripe(true).bordered(true)),
        )
        .into_any_element()
    }
}

// 把结果清单导出为 TXT（保存到系统下载目录）。返回写入路径，便于在文件管理器中定位。
pub fn export_results_txt(
    title: &str,
    files: &[FileInfo],
    show_size: bool,
    show_count: bool,
) -> Option<std::path::PathBuf> {
    let dir = downloads_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // 文件名里只保留字母数字/下划线，避免非法字符
    let safe: String = title
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    let path = dir.join(format!("xspace_{}_{}.txt", safe, secs));

    let mut out = String::new();
    out.push_str("XSpace 检索结果清单\n");
    out.push_str(&format!("分析模式: {}\n", title));
    out.push_str(&format!("条目数量: {}\n", files.len()));
    out.push_str(&format!("导出时间(unix秒): {}\n\n", secs));

    if show_count {
        out.push_str("排名\t大小\t文件数\t路径\n");
    } else if show_size {
        out.push_str("排名\t大小\t路径\n");
    } else {
        out.push_str("排名\t路径\n");
    }

    for (i, f) in files.iter().enumerate() {
        let rank = i + 1;
        if show_count {
            let count = match f.file_count {
                Some(c) => format!("{} 个", c),
                None => "-".to_string(),
            };
            out.push_str(&format!(
                "{}\t{}\t{}\t{}\n",
                rank,
                format_size(f.size),
                count,
                f.path
            ));
        } else if show_size {
            out.push_str(&format!("{}\t{}\t{}\n", rank, format_size(f.size), f.path));
        } else {
            out.push_str(&format!("{}\t{}\n", rank, f.path));
        }
    }

    std::fs::write(&path, out).ok()?;
    Some(path)
}

