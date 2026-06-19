use std::time::Duration;

// ===== 扫描正交维度（轴） =====

/// 扫描目标：文件 / 目录
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum Target {
    File,
    Directory,
}

/// 过滤维度
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum Filter {
    Largest,       // 大于等于阈值（仅文件）
    Empty,         // 空文件 / 空目录
    NameContains,  // 文件名/目录名包含关键字
    NameExact,     // 文件名/目录名精确匹配
}

/// 目录聚合维度（仅 Directory 有意义；文件类恒为 None）
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum Aggregate {
    None,
    Size,
    FileCount,
    SizeAndCount,
}

/// 结果排序维度
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum SortBy {
    Size,
    FileCount,
    Path,
}

/// 结果数量上限
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum Limit {
    TopN(usize),
    All,
}

// ===== 用户侧预设（驱动下拉框） =====

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum ScanPreset {
    BigFiles,   // File + Largest + Size + TopN
    EmptyFiles, // File + Empty + Size + All
    EmptyDirs,  // Directory + Empty + Path + All
    FindFiles,  // File + (Contains|Exact) + Size + All
    FindDirs,   // Directory + (Contains|Exact) + Aggregate + Sort + All/TopN
}

// 找到的文件/目录信息
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileInfo {
    pub size: u64,
    pub path: String,
    pub is_dir: bool,
    pub file_count: Option<usize>,
}
// 注意：移除了 Ord/PartialOrd。多键排序（按 size / file_count / path）时单一全序会产生
// 歧义，统一改由调用方按 SortBy 提供显式闭包比较器（见 app.rs 二分插入逻辑）。

// 硬盘选择信息
pub struct DiskInfo {
    pub name: String,
    pub mount_point: String,
    pub is_selected: bool,
}

// 扫描参数配置（轴驱动）
#[derive(Clone, Debug)]
pub struct ScanConfig {
    pub selected_roots: Vec<String>,
    pub target: Target,
    pub filter: Filter,
    pub aggregate: Aggregate, // 目录类查询才 != None
    pub sort_by: SortBy,
    pub limit: Limit,
    pub min_size_bytes: u64, // 仅 Largest 用
    pub query: String,       // 仅 Name* 用，已 trim+lowercase
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
