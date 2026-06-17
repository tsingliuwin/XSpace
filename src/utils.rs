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

// 在 Windows 资源管理器中打开并选中该文件
pub fn open_in_explorer(path: &str) {
    let _ = std::process::Command::new("explorer")
        .arg(format!("/select,{}", path))
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
