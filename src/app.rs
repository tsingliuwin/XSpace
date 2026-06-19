use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Mutex, Arc};
use std::sync::atomic::{AtomicUsize, AtomicBool, Ordering as AtomicOrdering};
use std::time::{Instant, Duration};
use std::sync::mpsc;
use eframe::egui;
use rayon::prelude::*;
use sysinfo::Disks;
use jwalk::WalkDirGeneric;

use crate::types::{
    FileInfo, DiskInfo, ScanConfig, ScanMessage,
    ScanPreset, Target, Filter, Aggregate, SortBy, Limit,
};
use crate::utils::{format_size, reveal_in_file_manager, validate_query, should_skip_path};
use crate::scanner::FileSizeState;

// 在 Grid 单元格内右对齐显示富文本（用于数字列，便于按位对齐扫读）
fn right_aligned_label(ui: &mut egui::Ui, text: egui::RichText, width: f32) {
    ui.allocate_ui([width, 20.0].into(), |ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(text);
        });
    });
}

// 把命中条目投递给 UI 线程（统一构造 FileInfo，避免各 match 分支重复样板）
fn send_file(
    tx: &mpsc::Sender<ScanMessage>,
    size: u64,
    path: &std::path::Path,
    is_dir: bool,
    file_count: Option<usize>,
) {
    let _ = tx.send(ScanMessage::BigFile(FileInfo {
        size,
        path: path.display().to_string(),
        is_dir,
        file_count,
    }));
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

// App 主状态
pub struct SpaceProbeApp {
    pub disks: Vec<DiskInfo>,
    pub preset: ScanPreset,
    pub query: String,        // 合并原 file_query / dir_query
    pub exact_match: bool,    // 精确 vs 包含（Name* 预设用）
    pub min_size_mb: f64,     // Largest 用
    pub top_n: usize,         // Limit::TopN
    pub show_all: bool,       // true → Limit::All
    pub dir_metric: DirMetric,// 仅 FindDirs：映射 Aggregate
    pub dir_sort: DirSort,    // 仅 FindDirs(metric=Both)：映射 SortBy

    // 扫描状态
    pub is_scanning: bool,
    pub scanned_files: usize,
    pub scanned_dirs: usize,
    pub current_path: String,
    pub big_files: Vec<FileInfo>,
    pub scan_duration: Option<Duration>,
    pub was_cancelled: bool,

    // 当前扫描的排序/上限，供 UI 收集逻辑使用（start_scan 时从 config 拷出）
    pub current_sort_by: SortBy,
    pub current_limit: Limit,

    // 线程控制
    pub rx: Option<mpsc::Receiver<ScanMessage>>,
    pub cancel_flag: Option<Arc<AtomicBool>>,
    pub validation_error: Option<String>,
}

impl SpaceProbeApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
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
                        // 在 macOS 下，默认只勾选用户目录 /Users，因为绝大部分可清理的无用大文件都在这里。
                        // 同时保留完整系统 / 作为可选项（默认不勾选）。
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

        Self {
            disks,
            preset: ScanPreset::BigFiles,
            query: String::new(),
            exact_match: false,
            min_size_mb: 100.0,
            top_n: 20,
            show_all: false,
            dir_metric: DirMetric::Size,
            dir_sort: DirSort::Size,
            is_scanning: false,
            scanned_files: 0,
            scanned_dirs: 0,
            current_path: String::new(),
            big_files: Vec::new(),
            scan_duration: None,
            was_cancelled: false,
            current_sort_by: SortBy::Size,
            current_limit: Limit::TopN(20),
            rx: None,
            cancel_flag: None,
            validation_error: None,
        }
    }

    // 由 UI 状态构建扫描配置（预设 → 正交轴）
    fn build_config(&self, selected_roots: Vec<String>) -> ScanConfig {
        let (target, filter, aggregate, sort_by, limit) = match self.preset {
            ScanPreset::BigFiles => (
                Target::File,
                Filter::Largest,
                Aggregate::None,
                SortBy::Size,
                if self.show_all { Limit::All } else { Limit::TopN(self.top_n) },
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
                if self.exact_match { Filter::NameExact } else { Filter::NameContains },
                Aggregate::None,
                SortBy::Size,
                Limit::All,
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
                    if self.exact_match { Filter::NameExact } else { Filter::NameContains },
                    aggregate,
                    sort_by,
                    if self.show_all { Limit::All } else { Limit::TopN(self.top_n) },
                )
            }
        };
        let query = match self.preset {
            ScanPreset::FindFiles | ScanPreset::FindDirs => self.query.trim().to_lowercase(),
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

    // 开启扫描后台线程
    pub fn start_scan(&mut self, ctx: egui::Context) {
        self.is_scanning = true;
        self.scanned_files = 0;
        self.scanned_dirs = 0;
        self.current_path.clear();
        self.big_files.clear();
        self.scan_duration = None;
        self.was_cancelled = false;

        let mut selected_roots: Vec<String> = self.disks.iter()
            .filter(|d| d.is_selected)
            .map(|d| d.mount_point.clone())
            .collect();

        // 过滤嵌套目录以防止重复扫描（例如如果同时勾选了 / 和 /Users，则只扫描较短的 / 根目录）
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
        let selected_roots = deduplicated_roots;

        if selected_roots.is_empty() {
            self.is_scanning = false;
            return;
        }

        let config = self.build_config(selected_roots);
        // 保存排序/上限供 UI 收集逻辑（二分插入 / 最终排序）使用
        self.current_sort_by = config.sort_by;
        self.current_limit = config.limit;

        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);

        let cancel_flag = Arc::new(AtomicBool::new(false));
        self.cancel_flag = Some(cancel_flag.clone());

        let config_clone = config.clone();
        let cancel_clone = cancel_flag.clone();

        // 创建后台工作线程
        std::thread::spawn(move || {
            let start_time = Instant::now();
            let total_files = Arc::new(AtomicUsize::new(0));
            let total_dirs = Arc::new(AtomicUsize::new(0));
            let last_report = Arc::new(Mutex::new(Instant::now()));

            let cancel_inner = cancel_clone.clone();
            let tx_clone = tx.clone();
            let config_inner = config_clone.clone();

            // 多盘符并行扫描
            config_clone.selected_roots.into_par_iter().for_each(|root| {
                let root_path = root.clone();

                // 每根局部状态（单线程 Serial 遍历，无需锁）：
                //  - empty_dirs: 零成本空目录集合（process_read_dir 即可知某目录无子项）
                //  - agg_map: 单遍目录聚合（path → 子树累计 size / 文件数）
                //  - candidates: 目录聚合查询的候选目录（root 扫描结束后查 map 再发送）
                let need_empty_dirs = config_inner.target == Target::Directory
                    && config_inner.filter == Filter::Empty;
                let aggregate_enabled = config_inner.target == Target::Directory
                    && config_inner.aggregate != Aggregate::None;
                // 文件大小预读：Largest 比较每个文件阈值；目录聚合需每个文件 size 累加到祖先
                let preread_sizes = config_inner.filter == Filter::Largest || aggregate_enabled;

                let empty_dirs: Arc<Mutex<HashSet<PathBuf>>> =
                    Arc::new(Mutex::new(HashSet::new()));
                let mut agg_map: HashMap<PathBuf, (u64, usize)> = HashMap::new();
                let mut candidates: Vec<PathBuf> = Vec::new();

                let walk = WalkDirGeneric::<FileSizeState>::new(&root)
                    // 并行遍历：jwalk 在目录层级并行（深目录树收益大；单目录巨量文件无收益）。
                    // 用 RayonNewPool 建独立线程池，避免与外层 into_par_iter 共用默认池触发
                    // busy_timeout 死锁检测而中止迭代。取消仍由 process_read_dir 顶部 + 每 4096
                    // 条目的 cancel 检查保证：worker 命中后清空 children 短路返回，缓冲里少量
                    // 在途条目排空即结束（取消延迟比串行略高，但保持可用）。
                    .parallelism(jwalk::Parallelism::RayonNewPool(
                        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4),
                    ))
                    .process_read_dir({
                        let cancel = cancel_inner.clone();
                        let root_path = root_path.clone();
                        let empty_dirs = empty_dirs.clone();
                        move |_depth, path, _state, children| {
                            if cancel.load(AtomicOrdering::Relaxed) {
                                children.clear();
                                return;
                            }

                            // 过滤不需要扫描的系统目录或重复挂载目录。
                            // 巨型目录下 children 可能有百万级条目，不能让整个 retain
                            // 长时间不检查 cancel。这里手动 retain，每隔 4096 个检查一次取消。
                            let mut write_idx = 0;
                            for read_idx in 0..children.len() {
                                if read_idx & 0xFFF == 0 && cancel.load(AtomicOrdering::Relaxed) {
                                    children.clear();
                                    return;
                                }
                                let keep = {
                                    let entry_result = &children[read_idx];
                                    if let Ok(entry) = entry_result {
                                        !should_skip_path(&entry.path(), &root_path)
                                    } else {
                                        true
                                    }
                                };
                                if keep {
                                    if write_idx != read_idx {
                                        children.swap(write_idx, read_idx);
                                    }
                                    write_idx += 1;
                                }
                            }
                            children.truncate(write_idx);

                            // 预读文件大小（仅 Largest / 目录聚合查询付出此代价）。
                            // 巨型目录下逐个 metadata() 系统调用会很久，必须每条都检查取消。
                            if preread_sizes {
                                for entry_result in children.iter_mut() {
                                    if cancel.load(AtomicOrdering::Relaxed) {
                                        break;
                                    }
                                    if let Ok(entry) = entry_result {
                                        if !entry.file_type.is_dir() {
                                            if let Ok(metadata) = entry.metadata() {
                                                entry.client_state = metadata.len();
                                            }
                                        }
                                    }
                                }
                            }

                            // 零成本空文件夹检测：过滤后无任何子项，即记为空目录。
                            // 无额外系统调用、无递归；仅目录空查询构造此集合。
                            if need_empty_dirs && children.is_empty() {
                                if let Ok(mut set) = empty_dirs.lock() {
                                    set.insert(path.to_path_buf());
                                }
                            }
                        }
                    });

                for entry in walk {
                    if cancel_inner.load(AtomicOrdering::Relaxed) {
                        break;
                    }
                    let entry = match entry {
                        Ok(e) => e,
                        Err(_) => continue,
                    };

                    let is_dir = entry.file_type.is_dir();
                    if is_dir {
                        total_dirs.fetch_add(1, AtomicOrdering::Relaxed);
                    } else {
                        total_files.fetch_add(1, AtomicOrdering::Relaxed);
                    }

                    let entry_path = entry.path();
                    let file_name = entry.file_name.to_string_lossy().to_lowercase();

                    // 单遍目录聚合：目录建 map 键（pre-order 先于其内容被 yield），
                    // 文件则把 size/count 沿祖先链累加进所有已存在的目录键。
                    if aggregate_enabled {
                        if is_dir {
                            agg_map.entry(entry_path.clone()).or_default();
                        } else {
                            let size = entry.client_state;
                            for ancestor in entry_path.ancestors() {
                                if let Some(v) = agg_map.get_mut(ancestor) {
                                    v.0 += size;
                                    v.1 += 1;
                                }
                            }
                        }
                    }

                    // 匹配真值表（文件类即时发送；目录聚合类收集候选延迟发送；空目录即时发送）
                    match (config_inner.target, config_inner.filter) {
                        (Target::File, Filter::Largest) => {
                            if !is_dir && entry.client_state >= config_inner.min_size_bytes {
                                send_file(&tx_clone, entry.client_state, &entry_path, false, None);
                            }
                        }
                        (Target::File, Filter::Empty) => {
                            // 显式 metadata：保留"读取失败的文件不计为空"的语义（与旧版一致）
                            if !is_dir
                                && let Ok(metadata) = entry.metadata()
                                && metadata.len() == 0
                            {
                                send_file(&tx_clone, 0, &entry_path, false, None);
                            }
                        }
                        (Target::File, Filter::NameContains) => {
                            if !is_dir
                                && file_name.contains(&config_inner.query)
                                && let Ok(metadata) = entry.metadata()
                            {
                                send_file(&tx_clone, metadata.len(), &entry_path, false, None);
                            }
                        }
                        (Target::File, Filter::NameExact) => {
                            if !is_dir
                                && file_name == config_inner.query
                                && let Ok(metadata) = entry.metadata()
                            {
                                send_file(&tx_clone, metadata.len(), &entry_path, false, None);
                            }
                        }
                        (Target::Directory, Filter::Empty) => {
                            if is_dir
                                && empty_dirs
                                    .lock()
                                    .map(|s| s.contains(entry_path.as_path()))
                                    .unwrap_or(false)
                            {
                                send_file(&tx_clone, 0, &entry_path, true, None);
                            }
                        }
                        (Target::Directory, Filter::NameContains) => {
                            if is_dir && file_name.contains(&config_inner.query) {
                                candidates.push(entry_path.clone());
                            }
                        }
                        (Target::Directory, Filter::NameExact) => {
                            if is_dir && file_name == config_inner.query {
                                candidates.push(entry_path.clone());
                            }
                        }
                        // (Directory, Largest) 不由任何预设产生；无操作
                        (Target::Directory, Filter::Largest) => {}
                    }

                    // 控制汇报频率在 100ms 左右
                    if let Ok(mut last) = last_report.try_lock() {
                        if last.elapsed() >= Duration::from_millis(100) {
                            *last = Instant::now();
                            let _ = tx_clone.send(ScanMessage::Progress {
                                files: total_files.load(AtomicOrdering::Relaxed),
                                dirs: total_dirs.load(AtomicOrdering::Relaxed),
                                current_path: entry_path.display().to_string(),
                            });
                            ctx.request_repaint(); // 刷新 UI 帧
                        }
                    }
                }

                // 目录聚合结果延迟发送：此时该 root 的 map 已完整，候选目录的子树总量可读。
                // 取消则跳过候选发送（已扫描的其余 root 结果不受影响）。
                if aggregate_enabled && !cancel_inner.load(AtomicOrdering::Relaxed) {
                    for dir_path in candidates {
                        let (size, count) = agg_map
                            .get(&dir_path)
                            .map(|v| (v.0, v.1))
                            .unwrap_or((0, 0));
                        let _ = tx_clone.send(ScanMessage::BigFile(FileInfo {
                            size,
                            path: dir_path.display().to_string(),
                            is_dir: true,
                            file_count: Some(count),
                        }));
                    }
                }
            });

            // 扫描结束
            let elapsed = start_time.elapsed();
            let _ = tx.send(ScanMessage::Finished {
                files: total_files.load(AtomicOrdering::Relaxed),
                dirs: total_dirs.load(AtomicOrdering::Relaxed),
                elapsed,
                was_cancelled: cancel_clone.load(AtomicOrdering::Relaxed),
            });
            ctx.request_repaint();
        });
    }

    pub fn stop_scan(&mut self) {
        if let Some(flag) = &self.cancel_flag {
            flag.store(true, AtomicOrdering::Relaxed);
        }
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

impl eframe::App for SpaceProbeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 1. 处理后台线程发来的消息
        let mut finished_msg = None;

        // 扫描期间定时重绘：保证持续消费消息、及时响应"停止"。
        if self.is_scanning {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        if self.is_scanning {
            if let Some(rx) = &self.rx {
                while let Ok(msg) = rx.try_recv() {
                    match msg {
                        ScanMessage::Progress { files, dirs, current_path } => {
                            self.scanned_files = files;
                            self.scanned_dirs = dirs;
                            self.current_path = current_path;
                        }
                        ScanMessage::BigFile(file) => {
                            match self.current_limit {
                                // TopN：按 sort_by 键降序二分插入 + 截断（保持内存有界）
                                Limit::TopN(n) => {
                                    let sb = self.current_sort_by;
                                    let idx = self.big_files.binary_search_by(|probe| {
                                        topn_key(&file, sb).cmp(&topn_key(probe, sb))
                                    });
                                    let idx = idx.unwrap_or_else(|i| i);
                                    if idx < n {
                                        self.big_files.insert(idx, file);
                                        self.big_files.truncate(n);
                                    }
                                }
                                // All：直接收集，Finished 时统一排序
                                Limit::All => {
                                    self.big_files.push(file);
                                }
                            }
                        }
                        ScanMessage::Finished { files, dirs, elapsed, was_cancelled } => {
                            finished_msg = Some((files, dirs, elapsed, was_cancelled));
                        }
                    }
                }
            }
        }

        if let Some((files, dirs, elapsed, was_cancelled)) = finished_msg {
            self.scanned_files = files;
            self.scanned_dirs = dirs;
            self.scan_duration = Some(elapsed);
            self.was_cancelled = was_cancelled;
            self.is_scanning = false;
            self.rx = None;
            self.cancel_flag = None;
            // All 结果做最终排序：Size / FileCount 降序，Path 升序
            if matches!(self.current_limit, Limit::All) {
                let sb = self.current_sort_by;
                self.big_files.sort_by(|a, b| match sb {
                    SortBy::Size => b.size.cmp(&a.size),
                    SortBy::FileCount => {
                        b.file_count.unwrap_or(0).cmp(&a.file_count.unwrap_or(0))
                    }
                    SortBy::Path => a.path.cmp(&b.path),
                });
            }
        }

        // 2. 绘制 UI
        egui::SidePanel::left("control_panel")
            .resizable(false)
            .default_width(260.0)
            .show(ctx, |ui| {
                ui.add_space(8.0);
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.heading("🔍 XSpace");
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(egui::RichText::new("极速空间探测器").weak().size(10.0));
                            });
                        });

                        ui.add_space(4.0);

                        // 1. 分区选择卡片
                        egui::Frame::none()
                            .fill(ui.visuals().window_fill)
                            .rounding(8.0)
                            .inner_margin(10.0)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.spacing_mut().item_spacing.y = 8.0;
                                ui.strong("💾 扫描分区");
                                ui.add_space(4.0);
                                ui.separator();
                                ui.add_space(6.0);
                                ui.add_enabled_ui(!self.is_scanning, |ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        for disk in &mut self.disks {
                                            ui.checkbox(&mut disk.is_selected, &disk.name);
                                        }
                                    });
                                });
                            });

                        // 2. 参数配置卡片
                        egui::Frame::none()
                            .fill(ui.visuals().window_fill)
                            .rounding(8.0)
                            .inner_margin(10.0)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.spacing_mut().item_spacing.y = 8.0;
                                ui.strong("⚙️ 参数配置");
                                ui.add_space(4.0);
                                ui.separator();
                                ui.add_space(6.0);

                                // 扫描类型（主下拉，5 个预设）
                                ui.horizontal(|ui| {
                                    ui.label("扫描类型：");
                                    ui.add_enabled_ui(!self.is_scanning, |ui| {
                                        let prev = self.preset;
                                        egui::ComboBox::from_id_source("preset_combo")
                                            .width(110.0)
                                            .selected_text(match self.preset {
                                                ScanPreset::BigFiles => "🔍 大文件",
                                                ScanPreset::EmptyFiles => "📄 空文件",
                                                ScanPreset::EmptyDirs => "📁 空目录",
                                                ScanPreset::FindFiles => "🗂 搜文件",
                                                ScanPreset::FindDirs => "📂 搜目录",
                                            })
                                            .show_ui(ui, |ui| {
                                                ui.selectable_value(&mut self.preset, ScanPreset::BigFiles, "🔍 大文件");
                                                ui.selectable_value(&mut self.preset, ScanPreset::EmptyFiles, "📄 空文件");
                                                ui.selectable_value(&mut self.preset, ScanPreset::EmptyDirs, "📁 空目录");
                                                ui.selectable_value(&mut self.preset, ScanPreset::FindFiles, "🗂 搜文件");
                                                ui.selectable_value(&mut self.preset, ScanPreset::FindDirs, "📂 搜目录");
                                            });
                                        if self.preset != prev {
                                            self.validation_error = None;
                                        }
                                    });
                                });

                                // 类型条件子控件
                                ui.add_space(6.0);
                                match self.preset {
                                    ScanPreset::BigFiles => {
                                        ui.horizontal(|ui| {
                                            ui.label("大小阈值：");
                                            ui.spacing_mut().slider_width = 80.0;
                                            ui.add_enabled_ui(!self.is_scanning, |ui| {
                                                ui.add(egui::Slider::new(&mut self.min_size_mb, 1.0..=10240.0)
                                                    .logarithmic(true)
                                                    .integer()
                                                    .show_value(false)
                                                );
                                                ui.add(egui::DragValue::new(&mut self.min_size_mb)
                                                    .clamp_range(1.0..=10240.0)
                                                    .suffix(" MB")
                                                );
                                            });
                                        });
                                    }
                                    ScanPreset::EmptyFiles => {
                                        ui.label(egui::RichText::new("ℹ️ 检索体积为 0 字节的空文件").weak().size(11.0));
                                    }
                                    ScanPreset::EmptyDirs => {
                                        ui.label(egui::RichText::new("ℹ️ 检索不含任何子项的空文件夹").weak().size(11.0));
                                    }
                                    ScanPreset::FindFiles | ScanPreset::FindDirs => {
                                        let label = match self.preset {
                                            ScanPreset::FindFiles => "文件名：",
                                            _ => "目录名：",
                                        };
                                        ui.horizontal(|ui| {
                                            ui.label(label);
                                            ui.add_enabled_ui(!self.is_scanning, |ui| {
                                                // 失焦时也保留可见边框，避免输入框"隐身"
                                                ui.visuals_mut().widgets.inactive.bg_stroke =
                                                    egui::Stroke::new(1.0, egui::Color32::from_rgb(0xC4, 0xCA, 0xD4));
                                                let w = ui.available_width();
                                                if ui.add(egui::TextEdit::singleline(&mut self.query).desired_width(w)).changed() {
                                                    self.validation_error = None;
                                                }
                                            });
                                        });
                                        ui.add_enabled_ui(!self.is_scanning, |ui| {
                                            ui.checkbox(&mut self.exact_match, "精确匹配（取消则包含匹配）");
                                        });

                                        // FindDirs：统计维度 / 排序键
                                        if self.preset == ScanPreset::FindDirs {
                                            ui.horizontal(|ui| {
                                                ui.label("统计维度：");
                                                ui.add_enabled_ui(!self.is_scanning, |ui| {
                                                    egui::ComboBox::from_id_source("dir_metric_combo")
                                                        .width(110.0)
                                                        .selected_text(match self.dir_metric {
                                                            DirMetric::Size => "按大小",
                                                            DirMetric::FileCount => "按文件数",
                                                            DirMetric::Both => "大小+文件数",
                                                        })
                                                        .show_ui(ui, |ui| {
                                                            ui.selectable_value(&mut self.dir_metric, DirMetric::Size, "按大小");
                                                            ui.selectable_value(&mut self.dir_metric, DirMetric::FileCount, "按文件数");
                                                            ui.selectable_value(&mut self.dir_metric, DirMetric::Both, "大小+文件数");
                                                        });
                                                });
                                            });
                                            // 排序键仅在两者都显示时可选；否则跟随统计维度
                                            if self.dir_metric == DirMetric::Both {
                                                ui.horizontal(|ui| {
                                                    ui.label("排序键：");
                                                    ui.add_enabled_ui(!self.is_scanning, |ui| {
                                                        egui::ComboBox::from_id_source("dir_sort_combo")
                                                            .width(110.0)
                                                            .selected_text(match self.dir_sort {
                                                                DirSort::Size => "按大小",
                                                                DirSort::FileCount => "按文件数",
                                                            })
                                                            .show_ui(ui, |ui| {
                                                                ui.selectable_value(&mut self.dir_sort, DirSort::Size, "按大小");
                                                                ui.selectable_value(&mut self.dir_sort, DirSort::FileCount, "按文件数");
                                                            });
                                                    });
                                                });
                                            }
                                        }
                                    }
                                }

                                // 结果数量 / 显示全部（非 Empty* 预设才可配；Empty* 固定 All）
                                let can_limit = !matches!(self.preset, ScanPreset::EmptyFiles | ScanPreset::EmptyDirs);
                                if can_limit {
                                    ui.add_space(6.0);
                                    ui.add_enabled_ui(!self.is_scanning, |ui| {
                                        ui.checkbox(&mut self.show_all, "显示全部结果");
                                    });
                                    if !self.show_all {
                                        ui.horizontal(|ui| {
                                            ui.label("结果数量：");
                                            ui.spacing_mut().slider_width = 80.0;
                                            ui.add_enabled_ui(!self.is_scanning, |ui| {
                                                ui.add(egui::Slider::new(&mut self.top_n, 5..=1000)
                                                    .integer()
                                                    .show_value(false)
                                                );
                                                ui.add(egui::DragValue::new(&mut self.top_n).clamp_range(5..=1000));
                                            });
                                        });
                                    }
                                }
                            });

                        // 3. 控制操作卡片
                        egui::Frame::none()
                            .fill(ui.visuals().window_fill)
                            .rounding(8.0)
                            .inner_margin(10.0)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.spacing_mut().item_spacing.y = 8.0;
                                ui.strong("🎯 扫描控制");
                                ui.add_space(4.0);
                                ui.separator();
                                ui.add_space(8.0);
                                if self.is_scanning {
                                    let btn = egui::Button::new(egui::RichText::new("⏹ 停止").color(egui::Color32::WHITE).strong())
                                        .fill(egui::Color32::from_rgb(0xef, 0x44, 0x44));
                                    if ui.add_sized([ui.available_width(), 40.0], btn).clicked() {
                                        self.stop_scan();
                                        ctx.request_repaint(); // 立即重绘，给出即时反馈
                                    }
                                } else {
                                    let has_selection = self.disks.iter().any(|d| d.is_selected);
                                    ui.add_enabled_ui(has_selection, |ui| {
                                        let btn = egui::Button::new(egui::RichText::new("▶ 开始扫描").color(egui::Color32::WHITE).strong())
                                            .fill(egui::Color32::from_rgb(0x25, 0x63, 0xEB));
                                        if ui.add_sized([ui.available_width(), 40.0], btn).clicked() {
                                            let mut valid = true;
                                            self.validation_error = None;
                                            // Name* 预设需校验关键字
                                            if matches!(self.preset, ScanPreset::FindFiles | ScanPreset::FindDirs) {
                                                if let Err(err) = validate_query(&self.query) {
                                                    self.validation_error = Some(err);
                                                    valid = false;
                                                }
                                            }
                                            if valid {
                                                self.start_scan(ctx.clone());
                                            }
                                        }
                                    });
                                }

                                if let Some(err) = &self.validation_error {
                                    ui.add_space(6.0);
                                    ui.label(egui::RichText::new(format!("⚠️ {}", err)).color(egui::Color32::from_rgb(0xef, 0x44, 0x44)).size(11.0));
                                }
                            });

                        // 利用剩余垂直空间，把"当前状态"卡片贴近面板底部，
                        // 充分填充面板高度，并避免出现滚动条。
                        let status_reserve = 190.0;
                        let gap = (ui.available_height() - status_reserve).max(0.0);
                        ui.add_space(gap);

                        // 4. 当前状态卡片
                        egui::Frame::none()
                            .fill(ui.visuals().window_fill)
                            .rounding(8.0)
                            .inner_margin(10.0)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.spacing_mut().item_spacing.y = 8.0;

                                let (status_text, status_color) = if self.is_scanning {
                                    ("⚡ 极速扫描中...", egui::Color32::from_rgb(0x06, 0xB6, 0xD4))
                                } else if self.scan_duration.is_some() {
                                    if self.was_cancelled {
                                        ("⏹ 扫描已中止", egui::Color32::from_rgb(0xf5, 0x9e, 0x0b))
                                    } else {
                                        ("✨ 扫描完成", egui::Color32::from_rgb(0x10, 0xB9, 0x81))
                                    }
                                } else {
                                    ("💤 就绪", egui::Color32::from_rgb(0x94, 0xA3, 0xB8))
                                };

                                ui.horizontal(|ui| {
                                    ui.label("当前状态:");
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        ui.label(egui::RichText::new(status_text).color(status_color).strong());
                                    });
                                });

                                ui.add_space(4.0);
                                ui.separator();
                                ui.add_space(4.0);

                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new("文件数量:").weak());
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        ui.label(egui::RichText::new(format!("{}", self.scanned_files)).strong());
                                    });
                                });

                                ui.add_space(2.0);

                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new("目录数量:").weak());
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        ui.label(egui::RichText::new(format!("{}", self.scanned_dirs)).strong());
                                    });
                                });

                                if let Some(dur) = self.scan_duration {
                                    ui.add_space(2.0);
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new("总共耗时:").weak());
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            ui.label(egui::RichText::new(format!("{:.2} 秒", dur.as_secs_f32())).color(egui::Color32::from_rgb(0x38, 0xBD, 0xF8)).strong());
                                        });
                                    });
                                }
                            });
                    });
            });

        // 右侧中央主显示面板
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(5.0);

            if self.is_scanning || !self.big_files.is_empty() {
                // 进度条及当前扫描状态卡片
                egui::Frame::none()
                    .fill(ui.visuals().window_fill)
                    .rounding(8.0)
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());

                        let (card_title, status_text, status_color) = if self.is_scanning {
                            ("⚡ 实时进度", "扫描中", egui::Color32::from_rgb(0x06, 0xB6, 0xD4))
                        } else {
                            let stat = if self.was_cancelled { "已中止" } else { "已完成" };
                            let color = if self.was_cancelled { egui::Color32::from_rgb(0xf5, 0x9e, 0x0b) } else { egui::Color32::from_rgb(0x10, 0xB9, 0x81) };
                            ("📊 扫描结果", stat, color)
                        };

                        ui.horizontal(|ui| {
                            ui.strong(card_title);
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(egui::RichText::new(status_text).color(status_color).strong());
                            });
                        });

                        ui.add_space(4.0);
                        ui.separator();
                        ui.add_space(6.0);

                        if self.is_scanning {
                            // 扫描总量未知，无法显示真实百分比；让进度在 0.08..0.9 之间平滑往返，
                            // 营造"持续加载"的观感，避免固定 0.5 看起来像"卡在 50%"。
                            let t = ui.ctx().input(|i| i.time) as f32;
                            let frac = 0.08 + 0.82 * 0.5 * (1.0 - (t * 1.5).cos());
                            ui.add(egui::ProgressBar::new(frac)
                                .animate(true)
                                .text(format!("XSpace 正在并发极速扫描中 (已检索出目标项: {})...", self.big_files.len()))
                            );
                            ui.add_space(6.0);

                            let truncated_path = if self.current_path.chars().count() > 70 {
                                let count = self.current_path.chars().count();
                                let left: String = self.current_path.chars().take(25).collect();
                                let right: String = self.current_path.chars().skip(count - 42).collect();
                                format!("{}...{}", left, right)
                            } else {
                                self.current_path.clone()
                            };
                            ui.label(egui::RichText::new(format!("当前扫描: {}", truncated_path)).weak().size(11.0));
                        } else if let Some(_dur) = self.scan_duration {
                            let is_topn = !matches!(self.preset, ScanPreset::EmptyFiles | ScanPreset::EmptyDirs)
                                && !self.show_all;
                            let list_title = match self.preset {
                                ScanPreset::BigFiles => {
                                    if is_topn {
                                        format!("全盘空间占用元凶 Top {} 榜单", self.top_n)
                                    } else {
                                        "全盘空间占用元凶清单".to_string()
                                    }
                                }
                                ScanPreset::EmptyFiles => "全盘零字节空文件清单".to_string(),
                                ScanPreset::EmptyDirs => "全盘空文件夹清单".to_string(),
                                ScanPreset::FindFiles => "文件名匹配结果清单".to_string(),
                                ScanPreset::FindDirs => "目录名匹配结果清单".to_string(),
                            };
                            let list_title = if self.was_cancelled {
                                "已中止检索的结果".to_string()
                            } else {
                                list_title
                            };

                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("分析模式:").weak());
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.label(egui::RichText::new(list_title).strong());
                                });
                            });
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("检索耗时:").weak());
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.label(egui::RichText::new(format!("{:.2} 秒", self.scan_duration.unwrap_or_default().as_secs_f32())).strong());
                                });
                            });
                        }
                    });

                ui.add_space(10.0);

                // 表格列随 aggregate / 预设自适应
                let (show_size, show_count) = match self.preset {
                    ScanPreset::BigFiles | ScanPreset::EmptyFiles | ScanPreset::FindFiles => (true, false),
                    ScanPreset::EmptyDirs => (false, false),
                    ScanPreset::FindDirs => match self.dir_metric {
                        DirMetric::Size => (true, false),
                        DirMetric::FileCount => (false, true),
                        DirMetric::Both => (true, true),
                    },
                };
                let num_columns = 3 + (if show_size { 1 } else { 0 }) + (if show_count { 1 } else { 0 });

                egui::Frame::none()
                    .fill(ui.visuals().window_fill)
                    .rounding(8.0)
                    .inner_margin(12.0)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.strong("📋 检索详情清单");
                        ui.add_space(4.0);
                        ui.separator();
                        ui.add_space(8.0);

                        egui::ScrollArea::vertical()
                            .auto_shrink([false; 2])
                            .show(ui, |ui| {
                                let available_width = ui.available_width();
                                let col0_width = 40.0; // 排名
                                let col_size_width = 85.0; // 占用大小
                                let col_count_width = 90.0; // 内含文件数
                                let col_action_width = 40.0; // 操作
                                let spacing = 12.0;
                                let num_spacings = (num_columns as f32) - 1.0;
                                let used = col0_width
                                    + (if show_size { col_size_width } else { 0.0 })
                                    + (if show_count { col_count_width } else { 0.0 })
                                    + col_action_width
                                    + num_spacings * spacing
                                    + 4.0;
                                let path_width = (available_width - used).max(150.0);

                                egui::Grid::new("big_files_table")
                                    .num_columns(num_columns)
                                    .spacing([spacing, 10.0])
                                    .striped(true)
                                    .show(ui, |ui| {
                                        // 表头
                                        right_aligned_label(ui, egui::RichText::new("排名").strong(), col0_width);
                                        if show_size {
                                            right_aligned_label(ui, egui::RichText::new("占用大小").strong(), col_size_width);
                                        }
                                        if show_count {
                                            right_aligned_label(ui, egui::RichText::new("内含文件数").strong(), col_count_width);
                                        }
                                        ui.add_sized([path_width, 20.0], egui::Label::new(egui::RichText::new("完整路径").strong()));

                                        ui.allocate_ui([col_action_width, 20.0].into(), |ui| {
                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                ui.strong("操作");
                                            });
                                        });
                                        ui.end_row();

                                        for (idx, file) in self.big_files.iter().enumerate() {
                                            // 排名高亮
                                            let rank_text = format!("#{}", idx + 1);
                                            let is_top3 = idx < 3;
                                            let rank_color = if is_top3 {
                                                egui::Color32::from_rgb(0xf5, 0x9e, 0x0b) // 金色 top 3
                                            } else {
                                                ui.visuals().widgets.noninteractive.fg_stroke.color
                                            };
                                            right_aligned_label(ui, egui::RichText::new(rank_text).color(rank_color).strong(), col0_width);

                                            // 大小列
                                            if show_size {
                                                let size_str = format_size(file.size);
                                                right_aligned_label(ui, egui::RichText::new(size_str).color(egui::Color32::from_rgb(0x02, 0x84, 0xC7)).strong(), col_size_width);
                                            }

                                            // 文件数列
                                            if show_count {
                                                let count_str = match file.file_count {
                                                    Some(count) => format!("{} 个", count),
                                                    None => "-".to_string(),
                                                };
                                                right_aligned_label(ui, egui::RichText::new(count_str).color(egui::Color32::from_rgb(0x10, 0xB9, 0x81)).strong(), col_count_width);
                                            }

                                            // 路径（可选择复制，超长截断，左对齐）
                                            let display_path = if file.is_dir {
                                                format!("📁 {}", file.path)
                                            } else {
                                                format!("📄 {}", file.path)
                                            };
                                            ui.add_sized([path_width, 20.0], egui::Label::new(display_path).selectable(true).truncate(true));

                                            // 定位文件/目录按钮 (靠右对齐)
                                            ui.allocate_ui([col_action_width, 22.0].into(), |ui| {
                                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                    if ui.add_sized([26.0, 22.0], egui::Button::new("📂")).clicked() {
                                                        reveal_in_file_manager(&file.path);
                                                    }
                                                });
                                            });
                                            ui.end_row();
                                        }
                                    });
                            });
                    });
            } else {
                // 欢迎及提示主页面卡片
                ui.vertical_centered(|ui| {
                    ui.add_space(50.0);
                    egui::Frame::none()
                        .fill(ui.visuals().window_fill)
                        .rounding(12.0)
                        .inner_margin(24.0)
                        .show(ui, |ui| {
                            ui.set_max_width(500.0);
                            ui.vertical_centered(|ui| {
                                ui.label(egui::RichText::new("💾").size(72.0));
                                ui.add_space(15.0);
                                ui.heading("欢迎使用 XSpace 极速空间探测器");
                                ui.add_space(10.0);
                                ui.label(egui::RichText::new("XSpace 是一款基于 Rust 开发的极速、轻量级磁盘分析与空间清理辅助工具。").weak());
                                ui.label(egui::RichText::new("依托多线程并行扫描引擎，能在数秒内深度检索全盘，精准定位侵占磁盘空间的“元凶”与无用文件。").weak());

                                ui.add_space(15.0);
                                ui.separator();
                                ui.add_space(15.0);

                                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                    ui.strong("🚀 快速开始：");
                                    ui.add_space(8.0);
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new("1️⃣").strong());
                                        ui.label("在左侧面板勾选需要扫描的本地磁盘分区。");
                                    });
                                    ui.add_space(6.0);
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new("2️⃣").strong());
                                        ui.label("在参数配置中选择所需的扫描类型及阈值。");
                                    });
                                    ui.add_space(6.0);
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new("3️⃣").strong());
                                        ui.label("点击【开始扫描】按钮，即刻开启极速并发检索！");
                                    });
                                });
                            });
                        });
                });
            }
        });
    }
}
