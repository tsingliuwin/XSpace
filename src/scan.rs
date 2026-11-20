//! 扫描引擎：与 UI 框架完全解耦。
//!
//! 输入一份 [`ScanConfig`]，后台多线程遍历文件系统，通过 mpsc channel 流式产出
//! [`ScanMessage`]（进度 / 命中条目 / 结束），并返回取消旗标。
//! UI 侧负责轮询 channel 并驱动重绘（见 app.rs 的 cx.spawn 轮询任务）。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
use std::time::{Duration, Instant};
use std::sync::mpsc;

use rayon::prelude::*;
use jwalk::WalkDirGeneric;

use crate::scanner::FileSizeState;
use crate::types::{Aggregate, Filter, ScanConfig, ScanMessage, Target, FileInfo};
use crate::utils::should_skip_path;

/// 启动一次扫描。返回消息接收端与取消旗标。
///
/// 内部为每个扫描根开一个 rayon 任务（多盘并行），每个根内再用 jwalk 独立线程池
/// 做目录树并行遍历（双层并行，互不抢线程池，详见 app 层历史注释）。
pub fn spawn_scan(
    config: ScanConfig,
) -> (mpsc::Receiver<ScanMessage>, Arc<AtomicBool>) {
    let (tx, rx) = mpsc::channel();
    let cancel_flag = Arc::new(AtomicBool::new(false));

    let config_clone = config.clone();
    let cancel_clone = cancel_flag.clone();

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
            // root 自身不是任何目录的子项，单独计入目录数
            total_dirs.fetch_add(1, AtomicOrdering::Relaxed);

            // 每根局部状态（单线程 Serial 遍历，无需锁）：
            //  - empty_dirs: 零成本空目录集合（process_read_dir 即可知某目录无子项）
            //  - agg_map: 单遍目录聚合（path → 子树累计 size / 文件数）
            //  - candidates: 目录聚合查询的候选目录（root 扫描结束后查 map 再发送）
            let need_empty_dirs =
                config_inner.target == Target::Directory && config_inner.filter == Filter::Empty;
            let aggregate_enabled =
                config_inner.target == Target::Directory && config_inner.aggregate != Aggregate::None;
            // 文件大小预读：仅目录聚合查询需要（消费端沿祖先链累加）。
            // 文件类查询（Largest/Empty）的 stat 改在下面的并行过滤阶段做。
            let preread_sizes = aggregate_enabled;

            let empty_dirs: Arc<Mutex<HashSet<PathBuf>>> = Arc::new(Mutex::new(HashSet::new()));
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
                    let config = config_inner.clone();
                    let total_files = total_files.clone();
                    let total_dirs = total_dirs.clone();
                    move |_depth, path, _state, children| {
                        if cancel.load(AtomicOrdering::Relaxed) {
                            children.clear();
                            return;
                        }

                        // 过滤系统/重复挂载目录，并在此时统计文件/目录数（计数前移到并行阶段，
                        // 这样后续可安全丢弃不命中文件而计数仍准确）。
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
                                if let Ok(entry) = &children[read_idx] {
                                    if entry.file_type.is_dir() {
                                        total_dirs.fetch_add(1, AtomicOrdering::Relaxed);
                                    } else {
                                        total_files.fetch_add(1, AtomicOrdering::Relaxed);
                                    }
                                }
                                if write_idx != read_idx {
                                    children.swap(write_idx, read_idx);
                                }
                                write_idx += 1;
                            }
                        }
                        children.truncate(write_idx);

                        // 预读文件大小（仅目录聚合查询付出此代价）。
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

                        // 文件类查询：在并行阶段提前过滤，只保留命中文件 + 全部目录。
                        // stat（Largest/Empty）在此并行完成；Name* 仅按名匹配不 stat。
                        // 消费端因此不再遍历海量非命中文件。
                        if config.target == Target::File {
                            let query = &config.query;
                            let min_size = config.min_size_bytes;
                            let mut w = 0;
                            for r in 0..children.len() {
                                if r & 0xFFF == 0 && cancel.load(AtomicOrdering::Relaxed) {
                                    children.truncate(w);
                                    return;
                                }
                                let keep = if let Ok(entry) = &mut children[r] {
                                    if entry.file_type.is_dir() {
                                        true
                                    } else {
                                        match config.filter {
                                            Filter::Largest => {
                                                let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
                                                entry.client_state = len;
                                                len >= min_size
                                            }
                                            Filter::Empty => {
                                                // 读取失败的文件不计为空（与旧版语义一致）
                                                entry.metadata().map(|m| m.len()).unwrap_or(u64::MAX) == 0
                                            }
                                            Filter::NameContains => entry
                                                .file_name
                                                .to_string_lossy()
                                                .to_lowercase()
                                                .contains(query),
                                            Filter::NameExact => {
                                                entry.file_name.to_string_lossy().to_lowercase() == *query
                                            }
                                        }
                                    }
                                } else {
                                    true
                                };
                                if keep {
                                    if w != r {
                                        children.swap(w, r);
                                    }
                                    w += 1;
                                }
                            }
                            children.truncate(w);
                        }

                        // 零成本空文件夹检测：过滤后无任何子项，即记为空目录。
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
                // 计数已在 process_read_dir 完成（root 在 for_each 开头计入）。

                // 单遍目录聚合：目录建 map 键（pre-order 先于其内容被 yield），
                // 文件则把 size/count 沿祖先链累加进所有已存在的目录键。
                if aggregate_enabled {
                    let entry_path = entry.path();
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

                // 匹配/发送。
                // 文件类：process_read_dir 已过滤，此处见到的文件都是命中的，直接发送。
                // file_name 与 entry_path 均按需计算（避免为海量非命中条目分配字符串）。
                match (config_inner.target, config_inner.filter) {
                    (Target::File, Filter::Largest) => {
                        if !is_dir {
                            let p = entry.path();
                            send_file(&tx_clone, entry.client_state, &p, false, None);
                        }
                    }
                    (Target::File, Filter::Empty) => {
                        if !is_dir {
                            let p = entry.path();
                            send_file(&tx_clone, 0, &p, false, None);
                        }
                    }
                    (Target::File, Filter::NameContains) | (Target::File, Filter::NameExact) => {
                        if !is_dir {
                            let p = entry.path();
                            // 命中文件少，按需取大小
                            let size = entry.metadata().ok().map(|m| m.len()).unwrap_or(0);
                            send_file(&tx_clone, size, &p, false, None);
                        }
                    }
                    (Target::Directory, Filter::Empty) => {
                        if is_dir {
                            let p = entry.path();
                            let is_empty = empty_dirs
                                .lock()
                                .map(|s| s.contains(p.as_path()))
                                .unwrap_or(false);
                            if is_empty {
                                send_file(&tx_clone, 0, &p, true, None);
                            }
                        }
                    }
                    (Target::Directory, Filter::NameContains)
                    | (Target::Directory, Filter::NameExact) => {
                        if is_dir {
                            let fname = entry.file_name.to_string_lossy().to_lowercase();
                            let matched = match config_inner.filter {
                                Filter::NameContains => fname.contains(&config_inner.query),
                                Filter::NameExact => fname == config_inner.query,
                                _ => false,
                            };
                            if matched {
                                candidates.push(entry.path());
                            }
                        }
                    }
                    // (Directory, Largest) 不由任何预设产生；无操作
                    (Target::Directory, Filter::Largest) => {}
                }

                // 控制汇报频率在 100ms 左右（按需算当前路径）
                if let Ok(mut last) = last_report.try_lock() {
                    if last.elapsed() >= Duration::from_millis(100) {
                        *last = Instant::now();
                        let current_path = entry.path().display().to_string();
                        let _ = tx_clone.send(ScanMessage::Progress {
                            files: total_files.load(AtomicOrdering::Relaxed),
                            dirs: total_dirs.load(AtomicOrdering::Relaxed),
                            current_path,
                        });
                    }
                }
            }

            // 目录聚合结果延迟发送：此时该 root 的 map 已完整，候选目录的子树总量可读。
            // 取消则跳过候选发送（已扫描的其余 root 结果不受影响）。
            if aggregate_enabled && !cancel_inner.load(AtomicOrdering::Relaxed) {
                for dir_path in candidates {
                    let (size, count) = agg_map.get(&dir_path).map(|v| (v.0, v.1)).unwrap_or((0, 0));
                    let _ = tx_clone.send(ScanMessage::BigFile(FileInfo {
                        size,
                        path: dir_path.display().to_string(),
                        is_dir: true,
                        file_count: Some(count),
                    }));
                }
            }
        });

        // 扫描结束（drop tx 前发送 Finished，drop 使 rx 的迭代自然终止）
        let elapsed = start_time.elapsed();
        let _ = tx.send(ScanMessage::Finished {
            files: total_files.load(AtomicOrdering::Relaxed),
            dirs: total_dirs.load(AtomicOrdering::Relaxed),
            elapsed,
            was_cancelled: cancel_clone.load(AtomicOrdering::Relaxed),
        });
    });

    (rx, cancel_flag)
}

// 把命中条目投递给 UI（统一构造 FileInfo，避免各 match 分支重复样板）
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
