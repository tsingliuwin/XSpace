use jwalk::ClientState;

// 1. 定义自定义 ClientState 结构体，用于在多线程中缓存文件大小
#[derive(Default, Clone, Debug)]
pub struct FileSizeState;

impl ClientState for FileSizeState {
    type ReadDirState = ();
    type DirEntryState = u64; // 将文件大小（u64）作为状态存储在每个 DirEntry 中
}

// 递归计算目录的总大小与内部文件数（使用项目已有的 walkdir 依赖进行并发安全计算）
pub fn get_dir_stats(path: &std::path::Path) -> (u64, usize) {
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
