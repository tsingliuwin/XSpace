use std::path::Path;

/// 判断扫描时是否应跳过该路径（目录/文件）。
///
/// 跳过两类目录：
/// 1. 虚拟/系统目录：/dev、/proc、/sys 等；从根 / 扫描时还需跳过
///    /Volumes、/System/Volumes、/mnt、/media 等外部/内部重定向挂载。
/// 2. 云端"按需文件"同步目录：~/Library/CloudStorage（OneDrive/iCloud/Google Drive
///    等）。这些目录里的文件多为云端占位符，访问时 macOS File Provider 扩展会去
///    联系云服务，导致 read_dir/metadata 长时间阻塞甚至挂起，扫描会卡死。
pub fn should_skip_path(path: &Path, root: &str) -> bool {
    // 1. 虚拟/系统目录（任何根下都跳过）
    if path.starts_with("/dev") || path.starts_with("/proc")
        || path.starts_with("/sys") || path.starts_with("/run")
    {
        return true;
    }

    // 2. 云端按需文件同步目录：任何根下都跳过（避免卡死）
    //    ~/Library/CloudStorage 下挂载了 OneDrive、iCloud Drive、Google Drive 等。
    if path.starts_with("/Users") {
        // /Users/<name>/Library/CloudStorage
        if let Ok(rel) = path.strip_prefix("/Users") {
            let comps: Vec<_> = rel.components().collect();
            if comps.len() >= 3 {
                // comps: [<user>, "Library", "CloudStorage", ...]
                if comps[1].as_os_str() == "Library" && comps[2].as_os_str() == "CloudStorage" {
                    return true;
                }
            }
        }
    }

    // 3. 从根 / 扫描时，跳过其他外部挂载卷与内部重定向卷，避免重复扫描
    if root == "/" {
        if path.starts_with("/Volumes") || path.starts_with("/System/Volumes") {
            return true;
        }
        if path.starts_with("/mnt") || path.starts_with("/media") {
            return true;
        }
    }

    false
}

// 格式化文件大小
pub fn format_size(size: u64) -> String {
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

// 在系统的文件管理器中定位并选中该文件/目录
#[cfg(target_os = "macos")]
pub fn reveal_in_file_manager(path: &str) {
    // `open -R` 在 Finder 中打开父目录并选中目标
    let _ = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn();
}

#[cfg(target_os = "windows")]
pub fn reveal_in_file_manager(path: &str) {
    let _ = std::process::Command::new("explorer")
        .arg(format!("/select,{}", path))
        .spawn();
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn reveal_in_file_manager(path: &str) {
    let _ = std::process::Command::new("xdg-open")
        .arg(path)
        .spawn();
}

// 校验关键字合法性
pub fn validate_query(query: &str) -> Result<(), String> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Err("请输入关键字".to_string());
    }
    // Windows 文件/文件夹非法字符: \ / : * ? " < > |
    let invalid_chars = ['\\', '/', ':', '*', '?', '"', '<', '>', '|'];
    for c in trimmed.chars() {
        if invalid_chars.contains(&c) {
            return Err(format!("包含非法字符 '{}'", c));
        }
    }
    Ok(())
}
