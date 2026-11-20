//! 应用根实体：全局状态、扫描对接（轮询通道驱动 UI）、整体布局
//! （顶栏 + 预设 Tabs + 左侧参数面板/中央内容 + 底部状态栏）。

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use gpui_kit::component::input::{InputEvent, InputState, NumberInputEvent};
use gpui_kit::component::select::{SelectEvent, SelectState};
use gpui_kit::component::tab::TabBar;
use gpui_kit::component::table::TableState;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::{
    AnyElement, App, AppContext, Context, Entity, IntoElement, ParentElement, Render,
    SharedString, Styled, Window, px,
};
use sysinfo::Disks;

use crate::scan;
use crate::types::{
    Aggregate, DiskInfo, FileInfo, Filter, Limit, ScanConfig, ScanMessage, ScanPreset, SortBy,
    Target,
};
use crate::ui::results::ResultsTable;
use crate::utils::validate_query;

pub const COMPANY: &str = "苏州遇码科技有限公司";
const VERSION: &str = env!("CARGO_PKG_VERSION");

// 顶部 Tabs 的预设顺序（下标 <-> ScanPreset 互转）
const PRESETS: [ScanPreset; 5] = [
    ScanPreset::BigFiles,
    ScanPreset::EmptyFiles,
    ScanPreset::EmptyDirs,
    ScanPreset::FindFiles,
    ScanPreset::FindDirs,
];
const PRESET_LABELS: [&str; 5] = ["🔍 大文件", "📄 空文件", "📁 空目录", "🗂 搜文件", "📂 搜目录"];

fn preset_index(p: ScanPreset) -> usize {
    PRESETS.iter().position(|x| *x == p).unwrap_or(0)
}

// FindDirs 的"统计维度"子选项（仅 UI 用，构建 ScanConfig 时映射到 Aggregate）
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum DirMetric {
    Size,
    FileCount,
    Both,
}

// FindDirs 的"排序键"子选项（仅 metric=Both 时可选；其余跟随 metric）
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum DirSort {
    Size,
    FileCount,
}

const METRIC_LABELS: [&str; 3] = ["按大小", "按文件数", "大小+文件数"];
const SORT_LABELS: [&str; 2] = ["按大小", "按文件数"];

pub struct XSpaceApp {
    pub disks: Vec<DiskInfo>,
    pub preset: ScanPreset,
    pub exact_match: bool, // 精确 vs 包含（Name* 预设用）
    pub min_size_mb: f64,  // Largest 用（由 NumberInput 事件同步）
    pub top_n: usize,      // Limit::TopN（由 NumberInput 事件同步）
    pub show_all: bool,    // true → Limit::All
    pub dir_metric: DirMetric,
    pub dir_sort: DirSort,

    // 组件实体状态
    pub query_input: Entity<InputState>,
    pub min_size_input: Entity<InputState>,
    pub top_n_input: Entity<InputState>,
    pub dir_metric_select: Entity<SelectState<Vec<SharedString>>>,
    pub dir_sort_select: Entity<SelectState<Vec<SharedString>>>,
    pub table: Entity<TableState<ResultsTable>>,

    // 扫描状态
    pub is_scanning: bool,
    pub scanned_files: usize,
    pub scanned_dirs: usize,
    pub current_path: String,
    pub scan_duration: Option<Duration>,
    pub scan_start: Option<Instant>, // 扫描中实时计耗时用；结束/中止时清空
    pub was_cancelled: bool,

    // 当前扫描的排序/上限，供结果收集逻辑使用（start_scan 时从 config 拷出）
    pub current_sort_by: SortBy,
    pub current_limit: Limit,

    pub cancel_flag: Option<Arc<AtomicBool>>,
    pub validation_error: Option<String>,
    // 上次计算的路径列宽（变化超过阈值才刷新表格列组，避免每帧 refresh）
    pub last_path_width: Option<f32>,
}

impl XSpaceApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // ===== 分区列表（沿用旧版平台适配逻辑） =====
        let disks_list = Disks::new_with_refreshed_list();
        let mut disks = Vec::new();
        for disk in &disks_list {
            if let Some(path_str) = disk.mount_point().to_str() {
                // macOS 特有过滤：隐藏重复的 Data 卷、Preboot 卷及私有挂载点
                #[cfg(target_os = "macos")]
                {
                    if path_str.starts_with("/System/Volumes/") {
                        continue;
                    }
                    if path_str.starts_with("/private/var/") {
                        continue;
                    }
                    if path_str == "/" {
                        // macOS 下默认只勾选用户目录 /Users
                        disks.push(DiskInfo {
                            name: "🏠 用户目录 (/Users)".to_string(),
                            mount_point: "/Users".to_string(),
                            is_selected: true,
                        });
                        disks.push(DiskInfo {
                            name: format!("💻 完整系统 ({})", disk.name().to_string_lossy()),
                            mount_point: "/".to_string(),
                            is_selected: false,
                        });
                        continue;
                    }
                }

                let name = disk.name().to_string_lossy();
                let display_name = if name.is_empty() {
                    path_str.to_string()
                } else {
                    format!("{} ({})", name, path_str)
                };

                disks.push(DiskInfo {
                    name: display_name,
                    mount_point: path_str.to_string(),
                    is_selected: true, // 其他分区默认全选
                });
            }
        }

        // ===== 组件实体状态 =====
        let query_input = cx.new(|cx| InputState::new(window, cx).placeholder("输入文件名关键字…"));
        let min_size_input =
            cx.new(|cx| InputState::new(window, cx).default_value("100"));
        let top_n_input = cx.new(|cx| InputState::new(window, cx).default_value("20"));
        let dir_metric_select = cx.new(|cx| {
            SelectState::new(
                METRIC_LABELS.map(SharedString::from).to_vec(),
                Some(gpui_kit::component::IndexPath::default()),
                window,
                cx,
            )
        });
        let dir_sort_select = cx.new(|cx| {
            SelectState::new(
                SORT_LABELS.map(SharedString::from).to_vec(),
                Some(gpui_kit::component::IndexPath::default()),
                window,
                cx,
            )
        });
        let table = cx.new(|cx| {
            let state = TableState::new(ResultsTable::new(), window, cx);
            state.col_resizable(true).sortable(false).row_selectable(false)
        });

        // ===== 事件订阅 =====
        // 关键字输入：变更清除校验错误；回车直接开扫
        cx.subscribe(&query_input, |this, _, event: &InputEvent, cx| match event {
            InputEvent::Change => {
                this.validation_error = None;
                cx.notify();
            }
            InputEvent::PressEnter { .. } => {
                this.try_start_scan(cx);
            }
            _ => {}
        })
        .detach();

        // 数值输入（阈值 / TopN）：文本变化或步进后解析同步到字段
        for input in [&min_size_input, &top_n_input] {
            let is_min = input == &min_size_input;
            let entity = input.clone();
            cx.subscribe(&entity, move |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.sync_numeric(is_min, cx);
                }
            })
            .detach();
            cx.subscribe(&entity, move |this, _, _: &NumberInputEvent, cx| {
                this.sync_numeric(is_min, cx);
            })
            .detach();
        }

        // 统计维度 / 排序键下拉
        cx.subscribe(&dir_metric_select, |this, _, _: &SelectEvent<Vec<SharedString>>, cx| {
            let label = this
                .dir_metric_select
                .read(cx)
                .selected_value()
                .cloned()
                .unwrap_or_else(|| "按大小".into());
            this.dir_metric = match label.as_ref() {
                "按文件数" => DirMetric::FileCount,
                "大小+文件数" => DirMetric::Both,
                _ => DirMetric::Size,
            };
            this.validation_error = None;
            cx.notify();
        })
        .detach();
        cx.subscribe(&dir_sort_select, |this, _, _: &SelectEvent<Vec<SharedString>>, cx| {
            let label = this
                .dir_sort_select
                .read(cx)
                .selected_value()
                .cloned()
                .unwrap_or_else(|| "按大小".into());
            this.dir_sort = match label.as_ref() {
                "按文件数" => DirSort::FileCount,
                _ => DirSort::Size,
            };
            this.validation_error = None;
            cx.notify();
        })
        .detach();

        Self {
            disks,
            preset: ScanPreset::BigFiles,
            exact_match: false,
            min_size_mb: 100.0,
            top_n: 20,
            show_all: false,
            dir_metric: DirMetric::Size,
            dir_sort: DirSort::Size,
            query_input,
            min_size_input,
            top_n_input,
            dir_metric_select,
            dir_sort_select,
            table,
            is_scanning: false,
            scanned_files: 0,
            scanned_dirs: 0,
            current_path: String::new(),
            scan_duration: None,
            scan_start: None,
            was_cancelled: false,
            current_sort_by: SortBy::Size,
            current_limit: Limit::TopN(20),
            cancel_flag: None,
            validation_error: None,
            last_path_width: None,
        }
    }

    // 由 NumberInput 文本解析同步数值（非法输入保留上次有效值）
    fn sync_numeric(&mut self, is_min: bool, cx: &Context<Self>) {
        let state = if is_min { &self.min_size_input } else { &self.top_n_input };
        let text = state.read(cx).value().to_string();
        if is_min {
            if let Ok(v) = text.trim().parse::<f64>() {
                self.min_size_mb = v.clamp(1.0, 10240.0);
            }
        } else if let Ok(v) = text.trim().parse::<usize>() {
            self.top_n = v.clamp(5, 1000);
        }
    }

    // 当前关键字文本（读输入框实体）
    fn query_text(&self, cx: &Context<Self>) -> String {
        self.query_input.read(cx).value().to_string()
    }

    // 由 UI 状态构建扫描配置（预设 → 正交轴；与旧版逻辑一致）
    // query 由调用方读取输入框实体后传入（此处无 cx 可借用）
    fn build_config(&self, selected_roots: Vec<String>, query: String) -> ScanConfig {
        let (target, filter, aggregate, sort_by, limit) = match self.preset {
            ScanPreset::BigFiles => (
                Target::File,
                Filter::Largest,
                Aggregate::None,
                SortBy::Size,
                if self.show_all {
                    Limit::All
                } else {
                    Limit::TopN(self.top_n)
                },
            ),
            ScanPreset::EmptyFiles => (
                Target::File,
                Filter::Empty,
                Aggregate::None,
                SortBy::Size,
                Limit::All,
            ),
            ScanPreset::EmptyDirs => (
                Target::Directory,
                Filter::Empty,
                Aggregate::None,
                SortBy::Path,
                Limit::All,
            ),
            ScanPreset::FindFiles => (
                Target::File,
                if self.exact_match {
                    Filter::NameExact
                } else {
                    Filter::NameContains
                },
                Aggregate::None,
                SortBy::Size,
                if self.show_all {
                    Limit::All
                } else {
                    Limit::TopN(self.top_n)
                },
            ),
            ScanPreset::FindDirs => {
                let aggregate = match self.dir_metric {
                    DirMetric::Size => Aggregate::Size,
                    DirMetric::FileCount => Aggregate::FileCount,
                    DirMetric::Both => Aggregate::SizeAndCount,
                };
                // 单一维度时排序键跟随维度；两者都显示时由 dir_sort 决定
                let sort_by = match self.dir_metric {
                    DirMetric::Size => SortBy::Size,
                    DirMetric::FileCount => SortBy::FileCount,
                    DirMetric::Both => match self.dir_sort {
                        DirSort::Size => SortBy::Size,
                        DirSort::FileCount => SortBy::FileCount,
                    },
                };
                (
                    Target::Directory,
                    if self.exact_match {
                        Filter::NameExact
                    } else {
                        Filter::NameContains
                    },
                    aggregate,
                    sort_by,
                    if self.show_all {
                        Limit::All
                    } else {
                        Limit::TopN(self.top_n)
                    },
                )
            }
        };
        let query = match self.preset {
            ScanPreset::FindFiles | ScanPreset::FindDirs => query.trim().to_lowercase(),
            _ => String::new(),
        };
        ScanConfig {
            selected_roots,
            target,
            filter,
            aggregate,
            sort_by,
            limit,
            min_size_bytes: (self.min_size_mb * 1024.0 * 1024.0) as u64,
            query,
        }
    }

    // ===== 扫描启动 / 停止 =====

    pub fn try_start_scan(&mut self, cx: &mut Context<Self>) {
        if self.is_scanning {
            return;
        }
        // 关键字校验
        if matches!(self.preset, ScanPreset::FindFiles | ScanPreset::FindDirs) {
            if let Err(err) = validate_query(&self.query_text(cx)) {
                self.validation_error = Some(err);
                cx.notify();
                return;
            }
        }
        // 勾选分区去重（嵌套根只扫最短路径，与旧版一致）
        let mut selected_roots: Vec<String> = self
            .disks
            .iter()
            .filter(|d| d.is_selected)
            .map(|d| d.mount_point.clone())
            .collect();
        selected_roots.sort_by_key(|p| p.len());
        let mut deduplicated_roots = Vec::new();
        for path in selected_roots {
            let is_subpath = deduplicated_roots.iter().any(|parent: &String| {
                if parent == "/" {
                    true
                } else {
                    path.starts_with(&format!("{}/", parent))
                }
            });
            if !is_subpath {
                deduplicated_roots.push(path);
            }
        }
        if deduplicated_roots.is_empty() {
            self.validation_error = Some("请至少勾选一个分区".to_string());
            cx.notify();
            return;
        }

        let query = if matches!(self.preset, ScanPreset::FindFiles | ScanPreset::FindDirs) {
            self.query_text(cx)
        } else {
            String::new()
        };
        let config = self.build_config(deduplicated_roots, query);
        self.current_sort_by = config.sort_by;
        self.current_limit = config.limit;

        // 重置状态
        self.is_scanning = true;
        self.scanned_files = 0;
        self.scanned_dirs = 0;
        self.current_path.clear();
        self.scan_duration = None;
        self.scan_start = Some(Instant::now());
        self.was_cancelled = false;
        self.validation_error = None;
        {
            let show_size = self.current_show_size();
            let show_count = self.current_show_count();
            let mut data = self.table.read(cx).delegate().data.lock().unwrap();
            data.rows.clear();
            data.show_size = show_size;
            data.show_count = show_count;
        }
        // 数据变更后必须在 TableState 自己的上下文里 notify，表格才会重绘
        self.table.update(cx, |st, cx| {
            st.refresh(cx);
            cx.notify();
        });

        let (rx, cancel) = scan::spawn_scan(config);
        self.cancel_flag = Some(cancel);

        // 轮询通道：消费扫描消息并驱动重绘（替代旧版 egui 的 request_repaint 轮询）。
        // Context::spawn 把 WeakEntity 作为第一个参数传给 async 闭包。
        //
        // 消息按 50ms 一批合并后用一次实体更新应用（批量插入 + 单次 notify），
        // 避免海量命中（All 模式可达数十万条）逐条 update 拖死 UI 线程。
        cx.spawn(async move |this: gpui_kit::WeakEntity<Self>, cx: &mut gpui_kit::AsyncApp| {
            let mut hits: Vec<FileInfo> = Vec::new();
            let mut progress: Option<(usize, usize, String)> = None;
            let mut finished: Option<(usize, usize, Duration, bool)> = None;
            loop {
                hits.clear();
                // 排空通道：只做本地收集，不触碰实体
                // （progress / finished 在上一轮 update 闭包内被 take，此处无需重置）
                while let Ok(msg) = rx.try_recv() {
                    match msg {
                        ScanMessage::Progress {
                            files,
                            dirs,
                            current_path,
                        } => progress = Some((files, dirs, current_path)),
                        ScanMessage::BigFile(f) => hits.push(f),
                        ScanMessage::Finished {
                            files,
                            dirs,
                            elapsed,
                            was_cancelled,
                        } => finished = Some((files, dirs, elapsed, was_cancelled)),
                    }
                }
                let is_done = finished.is_some();
                if !hits.is_empty() || progress.is_some() || is_done {
                    let applied = this
                        .update(cx, |app, cx| {
                            if let Some((files, dirs, path)) = progress.take() {
                                app.apply_progress(files, dirs, path);
                            }
                            if !hits.is_empty() {
                                app.apply_hits(std::mem::take(&mut hits), cx);
                            }
                            match finished.take() {
                                Some((files, dirs, elapsed, was_cancelled)) => {
                                    app.finish_scan(files, dirs, elapsed, was_cancelled, cx);
                                }
                                None => cx.notify(),
                            }
                        })
                        .is_ok();
                    if is_done || !applied {
                        // 扫描结束，或实体已销毁（窗口关闭）→ 退出轮询
                        break;
                    }
                } else {
                    // 扫描期间持续重绘（实时计时 / 进度）
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
            }
        })
        .detach();

        cx.notify();
    }

    pub fn stop_scan(&mut self) {
        if let Some(flag) = &self.cancel_flag {
            flag.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    // ===== 扫描消息的批量应用（由轮询任务每 50ms 调用一次） =====

    // 应用动态路径列宽（render 期间 defer 调用，渲染后生效）
    pub(crate) fn apply_path_width(&mut self, w: f32, cx: &mut Context<Self>) {
        self.table.update(cx, |st, cx| {
            st.delegate_mut().data.lock().unwrap().path_col_width = w;
            st.refresh(cx);
            cx.notify();
        });
    }

    fn apply_progress(&mut self, files: usize, dirs: usize, current_path: String) {
        self.scanned_files = files;
        self.scanned_dirs = dirs;
        self.current_path = current_path;
    }

    fn apply_hits(&mut self, hits: Vec<FileInfo>, cx: &mut Context<Self>) {
        let n = match self.current_limit {
            Limit::TopN(n) => n,
            Limit::All => usize::MAX,
        };
        let sb = self.current_sort_by;
        // 一批命中合并为一次表格更新 + 单次 notify
        self.table.update(cx, |st, cx| {
            let mut d = st.delegate().data.lock().unwrap();
            if n == usize::MAX {
                d.rows.extend(hits);
            } else {
                // TopN：按排序键降序二分插入 + 截断（保持内存有界）
                let key = |f: &FileInfo| topn_key(f, sb);
                for file in hits {
                    let idx = d.rows.partition_point(|probe| key(probe) >= key(&file));
                    if idx < n {
                        d.rows.insert(idx, file);
                        d.rows.truncate(n);
                    }
                }
            }
            cx.notify();
        });
    }

    fn finish_scan(
        &mut self,
        files: usize,
        dirs: usize,
        elapsed: Duration,
        was_cancelled: bool,
        cx: &mut Context<Self>,
    ) {
        self.scanned_files = files;
        self.scanned_dirs = dirs;
        self.scan_duration = Some(elapsed);
        self.scan_start = None;
        self.was_cancelled = was_cancelled;
        self.is_scanning = false;
        self.cancel_flag = None;
        // All 结果做最终排序：Size / FileCount 降序，Path 升序
        if matches!(self.current_limit, Limit::All) {
            let sb = self.current_sort_by;
            self.table.update(cx, |st, _| {
                let mut d = st.delegate().data.lock().unwrap();
                d.rows.sort_by(|a, b| match sb {
                    SortBy::Size => b.size.cmp(&a.size),
                    SortBy::FileCount => {
                        b.file_count.unwrap_or(0).cmp(&a.file_count.unwrap_or(0))
                    }
                    SortBy::Path => a.path.cmp(&b.path),
                });
            });
        }
        self.table.update(cx, |st, cx| {
            st.refresh(cx);
            cx.notify();
        });
        cx.notify();
    }

    // ===== 读取辅助（表格列显示配置 / 标题） =====

    pub fn result_count(&self, cx: &App) -> usize {
        self.table
            .read(cx)
            .delegate()
            .data
            .lock()
            .unwrap()
            .rows
            .len()
    }

    pub fn current_show_size(&self) -> bool {
        match self.preset {
            ScanPreset::BigFiles | ScanPreset::EmptyFiles | ScanPreset::FindFiles => true,
            ScanPreset::EmptyDirs => false,
            ScanPreset::FindDirs => !matches!(self.dir_metric, DirMetric::FileCount),
        }
    }

    pub fn current_show_count(&self) -> bool {
        match self.preset {
            ScanPreset::FindDirs => matches!(self.dir_metric, DirMetric::Both | DirMetric::FileCount),
            _ => false,
        }
    }

    pub fn list_title(&self) -> String {
        if self.was_cancelled {
            return "已中止检索的结果".to_string();
        }
        match self.preset {
            ScanPreset::BigFiles => {
                if self.show_all {
                    "全盘空间占用清单".to_string()
                } else {
                    format!("全盘空间占用 Top {} 榜单", self.top_n)
                }
            }
            ScanPreset::EmptyFiles => "全盘零字节空文件清单".to_string(),
            ScanPreset::EmptyDirs => "全盘空文件夹清单".to_string(),
            ScanPreset::FindFiles => "文件名匹配结果清单".to_string(),
            ScanPreset::FindDirs => "目录名匹配结果清单".to_string(),
        }
    }

    // ===== 布局 =====

    // Tabs 行右侧后缀：扫描中显示当前扫描路径。
    // 外层 flex_1 占满 tabs 之后的剩余行宽，路径自身 truncate 截断（显示尽可能多的字符）。
    // 状态与耗时由底部状态栏承载，这里不再重复。
    fn render_tabs_suffix(&self, cx: &Context<Self>) -> AnyElement {
        if self.is_scanning && !self.current_path.is_empty() {
            return gpui_kit::div()
                .flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .justify_end()
                .pl_3()
                .pr_2()
                .overflow_hidden()
                .child(
                    gpui_kit::div()
                        .min_w_0()
                        .flex_1()
                        .overflow_hidden()
                        .truncate()
                        .text_size(px(12.))
                        .text_color(cx.theme().muted_foreground)
                        .child(self.current_path.clone()),
                )
                .into_any_element();
        }
        gpui_kit::div().flex_1().into_any_element()
    }

    fn render_preset_tabs(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let entity = cx.entity();
        TabBar::new("preset_tabs")
            .underline()
            .children(PRESET_LABELS)
            .selected_index(preset_index(self.preset))
            .on_click(move |ix: &usize, _window, app: &mut App| {
                entity.update(app, |this, cx| {
                    this.preset = PRESETS[*ix];
                    this.validation_error = None;
                    cx.notify();
                });
            })
            .suffix(self.render_tabs_suffix(cx))
            // 标签行背景与页面背景一致（默认 tab_bar 色与内容区有色差）
            .bg(cx.theme().background)
            .px_2()
            .into_any_element()
    }

    fn render_status_bar(&self, cx: &Context<Self>) -> AnyElement {
        let dur = self.scan_start.map(|s| s.elapsed()).or(self.scan_duration);
        let left = format!(
            "文件数: {}   目录数: {}   耗时: {}",
            self.scanned_files,
            self.scanned_dirs,
            dur.map(|d| format!("{:.2}s", d.as_secs_f32()))
                .unwrap_or_else(|| "—".to_string())
        );
        h_flex()
            .px_4()
            .py_1()
            .justify_between()
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().status_bar)
            .text_size(px(12.))
            .child(
                gpui_kit::div()
                    .text_color(cx.theme().muted_foreground)
                    .child(left),
            )
            .child(
                gpui_kit::div()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("© {} · v{}", COMPANY, VERSION)),
            )
            .into_any_element()
    }
}

// TopN 二分插入时使用的数值键（Size / FileCount；Path 不与 TopN 组合，返回 0）
fn topn_key(f: &FileInfo, sb: SortBy) -> u64 {
    match sb {
        SortBy::Size => f.size,
        SortBy::FileCount => f.file_count.unwrap_or(0) as u64,
        SortBy::Path => 0,
    }
}

impl Render for XSpaceApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .min_h_0()
            .child(self.render_preset_tabs(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_sidebar(window, cx))
                    .child(
                        gpui_kit::div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(self.render_central(window, cx)),
                    ),
            )
            .child(self.render_status_bar(cx))
    }
}
