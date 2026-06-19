use jwalk::ClientState;

// 自定义 ClientState：在遍历时把每个文件的大小（u64）缓存到对应 DirEntry 中，
// 供文件类查询（Largest / Empty / Name*）直接读取，免去后续再次 metadata 系统调用。
#[derive(Default, Clone, Debug)]
pub struct FileSizeState;

impl ClientState for FileSizeState {
    type ReadDirState = ();
    type DirEntryState = u64; // 文件大小（u64），作为状态存储在每个 DirEntry 中
}
