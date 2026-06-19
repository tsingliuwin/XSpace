// build.rs：仅在 Windows 目标把图标嵌入 .exe（macOS/Linux 上跳过）。
// 图标在资源管理器、任务栏、窗口标题栏显示。
fn main() {
    #[cfg(target_os = "windows")]
    {
        if let Err(e) = embed_icon() {
            println!("cargo:warning=winres embed failed: {e}");
        }
    }
}

#[cfg(target_os = "windows")]
fn embed_icon() -> std::io::Result<()> {
    let mut res = winres::WindowsResource::new();
    res.set_icon("assets/icon.ico");
    res.set("FileDescription", "XSpace - 极速磁盘空间探测器");
    res.set("ProductName", "XSpace");
    res.set("CompanyName", "苏州遇码科技有限公司");
    res.set("LegalCopyright", "© 苏州遇码科技有限公司");
    // 版本号取自 Cargo.toml（CARGO_PKG_VERSION），改版本无需动 build 脚本
    res.set("FileVersion", env!("CARGO_PKG_VERSION"));
    res.set("ProductVersion", env!("CARGO_PKG_VERSION"));
    res.compile()
}
