use std::cmp::Ordering;
use std::time::Duration;

// 扫描模式
#[derive(PartialEq, Clone, Copy, Debug)]
pub enum ScanMode {
    BigFiles,       // 找大文件
    EmptyFiles,     // 找空文件
    CustomFileName, // 找指定文件
    CustomDirName,  // 找指定目录
}

// 记录大文件或目录信息
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileInfo {
    pub size: u64,
    pub path: String,
    pub is_dir: bool,
    pub file_count: Option<usize>,
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

// 硬盘选择信息
pub struct DiskInfo {
    pub mount_point: String,
    pub is_selected: bool,
}

// 扫描参数配置
#[derive(Clone, Debug)]
pub struct ScanConfig {
    pub selected_roots: Vec<String>,
    pub scan_mode: ScanMode,
    pub min_size_bytes: u64,
    pub query: String,
}

// 线程间通信消息
pub enum ScanMessage {
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
