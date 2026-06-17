use std::sync::{Mutex, Arc};
use std::sync::atomic::{AtomicUsize, AtomicBool, Ordering as AtomicOrdering};
use std::time::{Instant, Duration};
use std::sync::mpsc;
use eframe::egui;
use rayon::prelude::*;
use sysinfo::Disks;
use jwalk::WalkDirGeneric;

use crate::types::{FileInfo, DiskInfo, ScanConfig, ScanMessage, ScanMode};
use crate::utils::{format_size, open_in_explorer, validate_query};
use crate::scanner::{FileSizeState, get_dir_stats};

// App 主状态
pub struct SpaceProbeApp {
    pub disks: Vec<DiskInfo>,
    pub min_size_mb: f64,
    pub scan_mode: ScanMode,
    pub file_query: String,
    pub dir_query: String,
    pub top_n: usize,
    
    // 扫描状态
    pub is_scanning: bool,
    pub scanned_files: usize,
    pub scanned_dirs: usize,
    pub current_path: String,
    pub big_files: Vec<FileInfo>,
    pub scan_duration: Option<Duration>,
    pub was_cancelled: bool,
    
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
            validation_error: None,
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

    pub fn stop_scan(&mut self) {
        if let Some(flag) = &self.cancel_flag {
            flag.store(true, AtomicOrdering::Relaxed);
        }
    }
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
                            ui.heading("🔍 XSpace");
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(egui::RichText::new("极速空间探测器").weak().size(10.0));
                            });
                        });
                        
                        ui.add_space(12.0);
                        
                        // 1. 分区选择卡片
                        egui::Frame::none()
                            .fill(ui.visuals().window_fill)
                            .rounding(8.0)
                            .inner_margin(10.0)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.strong("💾 扫描分区");
                                ui.add_space(4.0);
                                ui.separator();
                                ui.add_space(6.0);
                                ui.add_enabled_ui(!self.is_scanning, |ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        for disk in &mut self.disks {
                                            ui.checkbox(&mut disk.is_selected, &disk.mount_point);
                                        }
                                    });
                                });
                            });
                        
                        ui.add_space(10.0);

                        // 2. 参数配置卡片
                        egui::Frame::none()
                            .fill(ui.visuals().window_fill)
                            .rounding(8.0)
                            .inner_margin(10.0)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.strong("⚙️ 参数配置");
                                ui.add_space(4.0);
                                ui.separator();
                                ui.add_space(6.0);

                                // 结果数量 (Top N)
                                ui.horizontal(|ui| {
                                    ui.label("结果数量：");
                                    ui.spacing_mut().slider_width = 80.0;
                                    ui.add_enabled_ui(!self.is_scanning, |ui| {
                                        ui.add(egui::Slider::new(&mut self.top_n, 5..=1000)
                                            .integer()
                                            .show_value(false)
                                        );
                                        ui.add(egui::DragValue::new(&mut self.top_n).clamp_range(5..=10000));
                                    });
                                });
                                
                                ui.add_space(6.0);

                                // 扫描模式
                                ui.horizontal(|ui| {
                                    ui.label("扫描模式：");
                                    ui.add_enabled_ui(!self.is_scanning, |ui| {
                                        let prev_mode = self.scan_mode;
                                        egui::ComboBox::from_id_source("scan_mode_combo")
                                            .width(110.0)
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
                                        if self.scan_mode != prev_mode {
                                            self.validation_error = None;
                                        }
                                    });
                                });

                                // 模式参数展示
                                ui.add_space(6.0);
                                match self.scan_mode {
                                    ScanMode::BigFiles => {
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
                                        ui.horizontal(|ui| {
                                            ui.label("文件名：");
                                            ui.add_enabled_ui(!self.is_scanning, |ui| {
                                                let w = ui.available_width();
                                                if ui.add(egui::TextEdit::singleline(&mut self.file_query).desired_width(w)).changed() {
                                                    self.validation_error = None;
                                                }
                                            });
                                        });
                                    }
                                    ScanMode::CustomDirName => {
                                        ui.horizontal(|ui| {
                                            ui.label("目录名：");
                                            ui.add_enabled_ui(!self.is_scanning, |ui| {
                                                let w = ui.available_width();
                                                if ui.add(egui::TextEdit::singleline(&mut self.dir_query).desired_width(w)).changed() {
                                                    self.validation_error = None;
                                                }
                                            });
                                        });
                                    }
                                }
                            });

                        ui.add_space(10.0);
                        
                        // 3. 控制操作卡片
                        egui::Frame::none()
                            .fill(ui.visuals().window_fill)
                            .rounding(8.0)
                            .inner_margin(10.0)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.strong("🎯 扫描控制");
                                ui.add_space(4.0);
                                ui.separator();
                                ui.add_space(8.0);
                                if self.is_scanning {
                                    let btn = egui::Button::new(egui::RichText::new("⏹ 停止").color(egui::Color32::WHITE).strong())
                                        .fill(egui::Color32::from_rgb(0xef, 0x44, 0x44));
                                    if ui.add_sized([ui.available_width(), 40.0], btn).clicked() {
                                        self.stop_scan();
                                    }
                                } else {
                                    let has_selection = self.disks.iter().any(|d| d.is_selected);
                                    ui.add_enabled_ui(has_selection, |ui| {
                                        let btn = egui::Button::new(egui::RichText::new("▶ 开始扫描").color(egui::Color32::WHITE).strong())
                                            .fill(egui::Color32::from_rgb(0x25, 0x63, 0xEB));
                                        if ui.add_sized([ui.available_width(), 40.0], btn).clicked() {
                                            let mut valid = true;
                                            self.validation_error = None;
                                            match self.scan_mode {
                                                ScanMode::CustomFileName => {
                                                    if let Err(err) = validate_query(&self.file_query) {
                                                        self.validation_error = Some(err);
                                                        valid = false;
                                                    }
                                                }
                                                ScanMode::CustomDirName => {
                                                    if let Err(err) = validate_query(&self.dir_query) {
                                                        self.validation_error = Some(err);
                                                        valid = false;
                                                    }
                                                }
                                                _ => {}
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
                        
                        ui.add_space(10.0);

                        // 4. 当前状态卡片
                        egui::Frame::none()
                            .fill(ui.visuals().window_fill)
                            .rounding(8.0)
                            .inner_margin(8.0)
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());

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
                            ui.add(egui::ProgressBar::new(0.5)
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
                        } else if let Some(dur) = self.scan_duration {
                            let list_title = match self.scan_mode {
                                ScanMode::BigFiles => format!("全盘空间占用元凶 Top {} 榜单", self.top_n),
                                ScanMode::EmptyFiles => format!("全盘零字节空文件 Top {} 榜单", self.top_n),
                                ScanMode::CustomFileName => format!("文件名匹配结果 Top {} 榜单", self.top_n),
                                ScanMode::CustomDirName => format!("目录名匹配结果 Top {} 榜单", self.top_n),
                            };
                            let list_title = if self.was_cancelled { format!("已中止检索的 Top {} 结果", self.top_n) } else { list_title };
                            
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
                                    ui.label(egui::RichText::new(format!("{:.2} 秒", dur.as_secs_f32())).strong());
                                });
                            });
                        }
                    });

                ui.add_space(10.0);
                
                // 大文件数据表格卡片
                let is_dir_mode = self.scan_mode == ScanMode::CustomDirName;
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
                                let col0_width = 40.0;
                                let col1_width = 85.0;
                                let col2_width = if is_dir_mode { 90.0 } else { 0.0 };
                                let col4_width = 40.0;
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
                                                    if ui.add_sized([26.0, 22.0], egui::Button::new("📂")).clicked() {
                                                        open_in_explorer(&file.path);
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
                                        ui.label("在参数配置中选择所需的扫描模式及阈值。");
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
