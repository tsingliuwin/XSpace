use jwalk::WalkDir;
use std::time::Instant;
use std::collections::BinaryHeap;
use std::cmp::Ordering;
use sysinfo::Disks; // 引入磁盘查询库

// 记录大文件信息
#[derive(Eq, PartialEq)]
struct FileInfo {
    size: u64,
    path: String,
}

// 排序规则：按文件大小对比
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

fn main() {
    println!("🔍 正在探测系统中所有的硬盘分区...");
    
    // 1. 自动获取所有挂载的磁盘
    let disks = Disks::new_with_refreshed_list();
    let mut target_disks = Vec::new();
    
    for disk in &disks {
        // disk.mount_point() 在 Windows 下会返回如 "C:\"、"D:\" 的路径
        if let Some(path_str) = disk.mount_point().to_str() {
            target_disks.push(path_str.to_string());
        }
    }
    
    println!("找到以下磁盘: {:?}", target_disks);
    println!("🚀 开始全盘【多线程】并行扫描...");
    let start = Instant::now();

    let mut total_file_count = 0;
    let mut total_dir_count = 0;
    let mut big_files = BinaryHeap::new();

    // 2. 依次遍历每个盘符
    for root in target_disks {
        println!("正在扫描: {} ...", root);
        
        for entry in WalkDir::new(&root) {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue, // 自动跳过没有权限的系统文件夹（如 C:\System Volume Information）
            };

            if entry.file_type.is_dir() {
                total_dir_count += 1;
            } else {
                total_file_count += 1;
                
                if let Ok(metadata) = entry.metadata() {
                    let size = metadata.len();
                    // 只把大于 100MB 的文件送去排序，节省内存
                    if size > 100 * 1024 * 1024 { 
                        big_files.push(FileInfo {
                            size,
                            path: entry.path().display().to_string(),
                        });
                    }
                }
            }
        }
    }

    println!("\n-----------------------------------");
    println!("✨ 全盘扫描完成！总耗时: {:?}", start.elapsed());
    println!("全电脑共扫描目录: {} 个，文件: {} 个\n", total_dir_count, total_file_count);
    
    println!("💾 【全台电脑吃空间元凶 Top 10】:");
    println!("-----------------------------------");
    let mut count = 0;
    while let Some(file) = big_files.pop() {
        if count >= 10 { break; }
        let size_gb = file.size as f64 / 1024.0 / 1024.0 / 1024.0;
        println!("[#{}] {:.2} GB  -->  {}", count + 1, size_gb, file.path);
        count += 1;
    }
}