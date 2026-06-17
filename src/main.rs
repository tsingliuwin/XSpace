#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use jwalk::{ClientState, WalkDirGeneric};
use std::time::{Instant, Duration};
use std::cmp::Ordering;
use sysinfo::Disks;
use std::sync::{Mutex, Arc};
use std::sync::atomic::{AtomicUsize, AtomicBool, Ordering as AtomicOrdering};
use rayon::prelude::*;
use std::sync::mpsc;
use eframe::egui;

// 扫描模式
#[derive(PartialEq, Clone, Copy, Debug)]
enum ScanMode {
    BigFiles,       // 找大文件
    EmptyFiles,     // 找空文件
    CustomFileName, // 找指定文件
    CustomDirName,  // 找指定目录
}

// 记录大文件或目录信息
#[derive(Clone, Debug, Eq, PartialEq)]
struct FileInfo {
    size: u64,
    path: String,
    is_dir: bool,
    file_count: Option<usize>,
}

// 排序规则：按大小对比（最大堆）
impl Ord for FileInfo {
    fn cmp(&self, other: &Self) -> Ordering {
        self.size.cmp(&other.size)
    }
}
impl PartialOrd for FileInfo {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

// 1. 定义自定义 ClientState 结构体，用于在多线程中缓存文件大小
#[derive(Default, Clone, Debug)]
struct FileSizeState;

impl ClientState for FileSizeState {
    type ReadDirState = ();
    type DirEntryState = u64; // 将文件大小（u64）作为状态存储在每个 DirEntry 中
}

// 硬盘选择信息
struct DiskInfo {
    mount_point: String,
    is_selected: bool,
}

// 扫描参数配置
#[derive(Clone, Debug)]
struct ScanConfig {
    selected_roots: Vec<String>,
    scan_mode: ScanMode,
    min_size_bytes: u64,
    query: String,
}

// 线程间通信消息
enum ScanMessage {
    Progress {
        files: usize,
        dirs: usize,
        current_path: String,
    },
    BigFile(FileInfo),
    Finished {
        files: usize,
        dirs: usize,
        elapsed: Duration,
        was_cancelled: bool,
    },
}

// App 主状态
struct SpaceProbeApp {
    disks: Vec<DiskInfo>,
    min_size_mb: f64,
    scan_mode: ScanMode,
    file_query: String,
    dir_query: String,
    top_n: usize,
    
    // 扫描状态
    is_scanning: bool,
    scanned_files: usize,
    scanned_dirs: usize,
    current_path: String,
    big_files: Vec<FileInfo>,
    scan_duration: Option<Duration>,
    was_cancelled: bool,
    
    // 线程控制
    rx: Option<mpsc::Receiver<ScanMessage>>,
    cancel_flag: Option<Arc<AtomicBool>>,
}

// 递归计算目录的总大小与内部文件数（使用项目已有的 walkdir 依赖进行并发安全计算）
fn get_dir_stats(path: &std::path::Path) -> (u64, usize) {
    let mut total_size = 0;
    let mut file_count = 0;
    for entry in walkdir::WalkDir::new(path).into_iter().flatten() {
        if let Ok(metadata) = entry.metadata() {
            if metadata.is_file() {
                total_size += metadata.len();
                file_count += 1;
            }
        }
    }
    (total_size, file_count)
}

impl SpaceProbeApp {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let disks_list = Disks::new_with_refreshed_list();
        let mut disks = Vec::new();
        for disk in &disks_list {
            if let Some(path_str) = disk.mount_point().to_str() {
                disks.push(DiskInfo {
                    mount_point: path_str.to_string(),
                    is_selected: true, // 默认全选
                });
            }
        }
        
        Self {
            disks,
            min_size_mb: 100.0,
            scan_mode: ScanMode::BigFiles,
            file_query: String::new(),
            dir_query: String::new(),
            top_n: 20,
            is_scanning: false,
            scanned_files: 0,
            scanned_dirs: 0,
            current_path: String::new(),
            big_files: Vec::new(),
            scan_duration: None,
            was_cancelled: false,
            rx: None,
            cancel_flag: None,
        }
    }

    // 开启扫描后台线程
    fn start_scan(&mut self, ctx: egui::Context) {
        self.is_scanning = true;
        self.scanned_files = 0;
        self.scanned_dirs = 0;
        self.current_path.clear();
        self.big_files.clear();
        self.scan_duration = None;
        self.was_cancelled = false;

        let selected_roots: Vec<String> = self.disks.iter()
            .filter(|d| d.is_selected)
            .map(|d| d.mount_point.clone())
            .collect();

        if selected_roots.is_empty() {
            self.is_scanning = false;
            return;
        }

        let min_size_bytes = (self.min_size_mb * 1024.0 * 1024.0) as u64;
        let query = match self.scan_mode {
            ScanMode::CustomFileName => self.file_query.trim().to_lowercase(),
            ScanMode::CustomDirName => self.dir_query.trim().to_lowercase(),
            _ => String::new(),
        };

        let config = ScanConfig {
            selected_roots,
            scan_mode: self.scan_mode,
            min_size_bytes,
            query,
        };

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
                let walk = WalkDirGeneric::<FileSizeState>::new(&root)
                    .process_read_dir({
                        let cancel = cancel_inner.clone();
                        let config = config_inner.clone();
                        move |_depth, _path, _state, children| {
                            if cancel.load(AtomicOrdering::Relaxed) {
                                children.clear();
                                return;
                            }
                            for entry_result in children.iter_mut() {
                                if let Ok(entry) = entry_result {
                                    if !entry.file_type.is_dir() {
                                        // 仅在大文件模式下并发预读大小以提高速度
                                        if config.scan_mode == ScanMode::BigFiles {
                                            if let Ok(metadata) = entry.metadata() {
                                                entry.client_state = metadata.len();
                                            }
                                        }
                                    }
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

                    let file_name = entry.file_name.to_string_lossy().to_lowercase();
                    let path = entry.path().display().to_string();

                    let mut matched = false;
                    let mut entry_size = 0;
                    let mut entry_file_count = None;

                    match config_inner.scan_mode {
                        ScanMode::BigFiles => {
                            if !is_dir {
                                entry_size = entry.client_state;
                                if entry_size >= config_inner.min_size_bytes {
                                    matched = true;
                                }
                            }
                        }
                        ScanMode::EmptyFiles => {
                            if !is_dir {
                                if let Ok(metadata) = entry.metadata() {
                                    if metadata.len() == 0 {
                                        matched = true;
                                        entry_size = 0;
                                    }
                                }
                            }
                        }
                        ScanMode::CustomFileName => {
                            if !is_dir && file_name.contains(&config_inner.query) {
                                if let Ok(metadata) = entry.metadata() {
                                    entry_size = metadata.len();
                                    matched = true;
                                }
                            }
                        }
                        ScanMode::CustomDirName => {
                            if is_dir && file_name.contains(&config_inner.query) {
                                let (dir_size, file_count) = get_dir_stats(&entry.path());
                                entry_size = dir_size;
                                entry_file_count = Some(file_count);
                                matched = true;
                            }
                        }
                    }

                    if matched {
                        let _ = tx_clone.send(ScanMessage::BigFile(FileInfo {
                            size: entry_size,
                            path,
                            is_dir,
                            file_count: entry_file_count,
                        }));
                    }

                    // 控制汇报频率在 100ms 左右
                    if let Ok(mut last) = last_report.try_lock() {
                        if last.elapsed() >= Duration::from_millis(100) {
                            *last = Instant::now();
                            let _ = tx_clone.send(ScanMessage::Progress {
                                files: total_files.load(AtomicOrdering::Relaxed),
                                dirs: total_dirs.load(AtomicOrdering::Relaxed),
                                current_path: entry.path().display().to_string(),
                            });
                            ctx.request_repaint(); // 刷新 UI 帧
                        }
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

    fn stop_scan(&mut self) {
        if let Some(flag) = &self.cancel_flag {
            flag.store(true, AtomicOrdering::Relaxed);
        }
    }
}

// 格式化文件大小
fn format_size(size: u64) -> String {
    let kb = size as f64 / 1024.0;
    let mb = kb / 1024.0;
    let gb = mb / 1024.0;
    if gb >= 1.0 {
        format!("{:.2} GB", gb)
    } else if mb >= 1.0 {
        format!("{:.2} MB", mb)
    } else {
        format!("{:.2} KB", kb)
    }
}

// 在 Windows 资源管理器中打开并选中该文件
fn open_in_explorer(path: &str) {
    let _ = std::process::Command::new("explorer")
        .arg(format!("/select,{}", path))
        .spawn();
}

impl eframe::App for SpaceProbeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 1. 处理后台线程发来的消息
        let mut finished_msg = None;
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
                            let idx = match self.big_files.binary_search_by(|probe| file.size.cmp(&probe.size).reverse()) {
                                Ok(idx) => idx,
                                Err(idx) => idx,
                            };
                            if idx < self.top_n {
                                self.big_files.insert(idx, file);
                                self.big_files.truncate(self.top_n); 
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
        }

        // 2. 绘制 UI
        egui::SidePanel::left("control_panel")
            .resizable(false)
            .default_width(260.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            ui.heading("🔍 空间探针");
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(egui::RichText::new("SpaceProbe").weak().size(10.0));
                            });
                        });
                        
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(6.0);
                        
                        // 1. 分区选择
                        ui.strong("扫描分区：");
                        ui.add_space(4.0);
                        ui.add_enabled_ui(!self.is_scanning, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                for disk in &mut self.disks {
                                    ui.checkbox(&mut disk.is_selected, &disk.mount_point);
                                }
                            });
                        });
                        
                        ui.add_space(6.0);
                        ui.separator();
                        ui.add_space(6.0);

                        // 2. 结果数量 (Top N) - 简化为一行，滑块和输入框放一排
                        ui.horizontal(|ui| {
                            ui.strong("Top N：");
                            ui.spacing_mut().slider_width = 110.0;
                            ui.add_enabled_ui(!self.is_scanning, |ui| {
                                ui.add(egui::Slider::new(&mut self.top_n, 5..=1000)
                                    .integer()
                                    .show_value(false)
                                );
                                ui.add(egui::DragValue::new(&mut self.top_n).clamp_range(5..=10000));
                            });
                        });
                        
                        ui.add_space(6.0);
                        ui.separator();
                        ui.add_space(6.0);

                        // 3. 扫描模式
                        ui.horizontal(|ui| {
                            ui.strong("扫描模式：");
                            ui.add_enabled_ui(!self.is_scanning, |ui| {
                                egui::ComboBox::from_id_source("scan_mode_combo")
                                    .width(140.0)
                                    .selected_text(match self.scan_mode {
                                        ScanMode::BigFiles => "🔍 大文件",
                                        ScanMode::EmptyFiles => "📄 空文件",
                                        ScanMode::CustomFileName => "🗂 搜文件",
                                        ScanMode::CustomDirName => "📁 搜目录",
                                    })
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(&mut self.scan_mode, ScanMode::BigFiles, "🔍 大文件");
                                        ui.selectable_value(&mut self.scan_mode, ScanMode::EmptyFiles, "📄 空文件");
                                        ui.selectable_value(&mut self.scan_mode, ScanMode::CustomFileName, "🗂 搜文件");
                                        ui.selectable_value(&mut self.scan_mode, ScanMode::CustomDirName, "📁 搜目录");
                                    });
                            });
                        });

                        // 4. 模式参数展示
                        ui.add_space(6.0);
                        match self.scan_mode {
                            ScanMode::BigFiles => {
                                ui.horizontal(|ui| {
                                    ui.strong("阈值：");
                                    ui.spacing_mut().slider_width = 100.0;
                                    ui.add_enabled_ui(!self.is_scanning, |ui| {
                                        ui.add(egui::Slider::new(&mut self.min_size_mb, 1.0..=10240.0)
                                            .logarithmic(true)
                                            .integer()
                                            .show_value(false)
                                        );
                                        ui.add(egui::DragValue::new(&mut self.min_size_mb)
                                            .clamp_range(1.0..=102400.0)
                                            .suffix(" MB")
                                        );
                                    });
                                });
                            }
                            ScanMode::EmptyFiles => {
                                ui.label(egui::RichText::new("ℹ️ 检索体积为 0 字节的空文件").weak().size(11.0));
                            }
                            ScanMode::CustomFileName => {
                                ui.strong("文件名关键字：");
                                ui.add_space(2.0);
                                ui.add_enabled_ui(!self.is_scanning, |ui| {
                                    ui.add(egui::TextEdit::singleline(&mut self.file_query).desired_width(f32::INFINITY));
                                });
                                if self.file_query.trim().is_empty() {
                                    ui.add_space(2.0);
                                    ui.label(egui::RichText::new("⚠️ 请输入关键字").color(egui::Color32::from_rgb(0xef, 0x44, 0x44)).size(10.0));
                                }
                            }
                            ScanMode::CustomDirName => {
                                ui.strong("目录名关键字：");
                                ui.add_space(2.0);
                                ui.add_enabled_ui(!self.is_scanning, |ui| {
                                    ui.add(egui::TextEdit::singleline(&mut self.dir_query).desired_width(f32::INFINITY));
                                });
                                if self.dir_query.trim().is_empty() {
                                    ui.add_space(2.0);
                                    ui.label(egui::RichText::new("⚠️ 请输入关键字").color(egui::Color32::from_rgb(0xef, 0x44, 0x44)).size(10.0));
                                }
                            }
                        }

                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(8.0);
                        
                        // 控制按钮
                        if self.is_scanning {
                            let btn = egui::Button::new(egui::RichText::new("⏹ 停止").color(egui::Color32::WHITE))
                                .fill(egui::Color32::from_rgb(0xef, 0x44, 0x44));
                            if ui.add_sized([ui.available_width(), 40.0], btn).clicked() {
                                self.stop_scan();
                            }
                        } else {
                            let has_selection = self.disks.iter().any(|d| d.is_selected);
                            let query_valid = match self.scan_mode {
                                ScanMode::CustomFileName => !self.file_query.trim().is_empty(),
                                ScanMode::CustomDirName => !self.dir_query.trim().is_empty(),
                                _ => true,
                            };
                            let can_start = has_selection && query_valid;
                            ui.add_enabled_ui(can_start, |ui| {
                                let btn = egui::Button::new(egui::RichText::new("▶ 开始扫描").color(egui::Color32::WHITE))
                                    .fill(egui::Color32::from_rgb(0x25, 0x63, 0xEB));
                                if ui.add_sized([ui.available_width(), 40.0], btn).clicked() {
                                    self.start_scan(ctx.clone());
                                }
                            });
                        }
                        
                        ui.add_space(10.0);
                        
                        // 状态概要
                        ui.strong("状态概要");
                        ui.add_space(4.0);
                        egui::Frame::none()
                            .fill(ui.visuals().window_fill)
                            .rounding(8.0)
                            .inner_margin(8.0)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());

                                let (status_text, status_color) = if self.is_scanning {
                                    ("⚡ 正在扫描...", egui::Color32::from_rgb(0x06, 0xB6, 0xD4))
                                } else if self.scan_duration.is_some() {
                                    if self.was_cancelled { 
                                        ("⏹ 手动停止", egui::Color32::from_rgb(0xf5, 0x9e, 0x0b))
                                    } else { 
                                        ("✨ 扫描已完成", egui::Color32::from_rgb(0x10, 0xB9, 0x81))
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
                // 进度条及当前扫描状态
                if self.is_scanning {
                    ui.strong("实时进度：");
                    ui.add_space(5.0);
                    // 模拟未定百分比的滚动进度条
                    ui.add(egui::ProgressBar::new(0.5)
                        .animate(true)
                        .text(format!("正在深度读取文件目录 (已发现大文件: {})...", self.big_files.len()))
                    );
                    ui.add_space(5.0);
                    
                    // 当前扫描路径展示，截断长路径避免溢出
                    let truncated_path = if self.current_path.chars().count() > 70 {
                        let count = self.current_path.chars().count();
                        let left: String = self.current_path.chars().take(25).collect();
                        let right: String = self.current_path.chars().skip(count - 42).collect();
                        format!("{}...{}", left, right)
                    } else {
                        self.current_path.clone()
                    };
                    ui.label(egui::RichText::new(format!("当前扫描: {}", truncated_path)).weak().size(11.0));
                    ui.add_space(10.0);
                    ui.separator();
                } else if let Some(dur) = self.scan_duration {
                    let title = match self.scan_mode {
                        ScanMode::BigFiles => format!("💾 【全台电脑空间元凶 Top {} 榜单】", self.top_n),
                        ScanMode::EmptyFiles => format!("📄 【全台电脑空文件 Top {} 榜单】", self.top_n),
                        ScanMode::CustomFileName => format!("🗂 【指定文件名匹配结果 Top {} 榜单】", self.top_n),
                        ScanMode::CustomDirName => format!("📁 【指定目录名匹配结果 Top {} 榜单】", self.top_n),
                    };
                    let title = if self.was_cancelled { format!("⏹ 扫描中断 - 以下为已扫描的 Top {} 榜单", self.top_n) } else { title };
                    ui.heading(title);
                    ui.add_space(2.0);
                    ui.label(egui::RichText::new(format!("共扫描了 {} 个目录，{} 个文件，耗时 {:.2} 秒", self.scanned_dirs, self.scanned_files, dur.as_secs_f32())).weak().size(12.0));
                    ui.add_space(10.0);
                    ui.separator();
                }

                ui.add_space(10.0);
                
                // 大文件数据表格
                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        let is_dir_mode = self.scan_mode == ScanMode::CustomDirName;
                        let available_width = ui.available_width();
                        let col0_width = 40.0;
                        let col1_width = 85.0;
                        let col2_width = if is_dir_mode { 90.0 } else { 0.0 };
                        let col4_width = 100.0;
                        let spacing = 12.0;
                        let num_spacings = if is_dir_mode { 4.0 } else { 3.0 };
                        let path_width = (available_width - (col0_width + col1_width + col2_width + col4_width + num_spacings * spacing + 4.0)).max(150.0);

                        egui::Grid::new("big_files_table")
                            .num_columns(if is_dir_mode { 5 } else { 4 })
                            .spacing([spacing, 10.0])
                            .striped(true)
                            .show(ui, |ui| {
                                // 表格头部
                                ui.add_sized([col0_width, 20.0], egui::Label::new(egui::RichText::new("排名").strong()));
                                ui.add_sized([col1_width, 20.0], egui::Label::new(egui::RichText::new("占用大小").strong()));
                                if is_dir_mode {
                                    ui.add_sized([col2_width, 20.0], egui::Label::new(egui::RichText::new("内含文件数").strong()));
                                }
                                ui.add_sized([path_width, 20.0], egui::Label::new(egui::RichText::new("完整路径").strong()));
                                
                                ui.allocate_ui([col4_width, 20.0].into(), |ui| {
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
                                    ui.add_sized([col0_width, 20.0], egui::Label::new(egui::RichText::new(rank_text).color(rank_color).strong()));

                                    // 大小高亮
                                    let size_str = format_size(file.size);
                                    ui.add_sized([col1_width, 20.0], egui::Label::new(egui::RichText::new(size_str).color(egui::Color32::from_rgb(0x02, 0x84, 0xC7)).strong()));

                                    // 文件数高亮 (仅目录模式)
                                    if is_dir_mode {
                                        let count_str = match file.file_count {
                                            Some(count) => format!("{} 个", count),
                                            None => "-".to_string(),
                                        };
                                        ui.add_sized([col2_width, 20.0], egui::Label::new(egui::RichText::new(count_str).color(egui::Color32::from_rgb(0x10, 0xB9, 0x81)).strong()));
                                    }

                                    // 路径（可选择复制，超长截断，左对齐）
                                    let display_path = if file.is_dir {
                                        format!("📁 {}", file.path)
                                    } else {
                                        format!("📄 {}", file.path)
                                    };
                                    ui.add_sized([path_width, 20.0], egui::Label::new(display_path).selectable(true).truncate(true));

                                    // 打开所在目录按钮 (靠右对齐)
                                    ui.allocate_ui([col4_width, 22.0].into(), |ui| {
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            if ui.add_sized([100.0, 22.0], egui::Button::new("📂 定位文件")).clicked() {
                                                open_in_explorer(&file.path);
                                            }
                                        });
                                    });
                                    ui.end_row();
                                }
                            });
                    });
            } else {
                // 欢迎及提示主页面
                ui.vertical_centered(|ui| {
                    ui.add_space(100.0);
                    ui.label(egui::RichText::new("💾").size(72.0));
                    ui.add_space(15.0);
                    ui.heading("欢迎使用全盘空间探针");
                    ui.add_space(10.0);
                    ui.label("这是一个并发效率极高、轻量级、跨平台的磁盘文件空间清理辅助工具。");
                    ui.label("利用 Rust 的多线程并行处理能力，可瞬间定位侵占您磁盘空间的“罪魁祸首”。");
                    ui.add_space(30.0);
                    
                    egui::Frame::none()
                        .fill(ui.visuals().widgets.inactive.bg_fill)
                        .rounding(8.0)
                        .inner_margin(16.0)
                        .show(ui, |ui| {
                            ui.set_max_width(400.0);
                            ui.strong("快速开始步骤：");
                            ui.add_space(6.0);
                            ui.label("1. 在左侧面板勾选您想要扫描的本地磁盘驱动器。");
                            ui.add_space(4.0);
                            ui.label("2. 设置您想过滤的最小文件体积（默认为 100 MB）。");
                            ui.add_space(4.0);
                            ui.label("3. 点击【开始全盘扫描】按钮，即刻并发检索大文件！");
                        });
                });
            }
        });
    }
}

// 配置自定义样式，打造高雅的暗色系风格
fn configure_custom_style(ctx: &egui::Context) {
    // 尝试从 Windows 系统目录加载微软雅黑字体以支持中文显示
    let font_path = "C:\\Windows\\Fonts\\msyh.ttc";
    if let Ok(font_data) = std::fs::read(font_path) {
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "msyh".to_owned(),
            egui::FontData::from_owned(font_data),
        );
        fonts.families.entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "msyh".to_owned());
        fonts.families.entry(egui::FontFamily::Monospace)
            .or_default()
            .insert(0, "msyh".to_owned());
        ctx.set_fonts(fonts);
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
            .with_title("🔍 全盘大文件空间探针 (SpaceProbe)"),
        default_theme: eframe::Theme::Light,
        ..Default::default()
    };
    
    eframe::run_native(
        "space_probe",
        options,
        Box::new(|cc| {
            configure_custom_style(&cc.egui_ctx);
            Box::new(SpaceProbeApp::new(cc))
        }),
    )
}