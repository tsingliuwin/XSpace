# 下载与安装

> 官网：<https://xspace.xi-n.com>　|　使用方法见[使用指南](/guide/)

## 下载

::: tip 最新版本
**XSpace 极速磁盘空间探测器**（macOS，zip 内含 `.app` + `README`）

⬇ **[立即下载（123 云盘）](https://1860052934.share.123865.com/123pan/bSOzvd-YoBmh)**
:::

## 系统要求

- **macOS** 11+（Apple Silicon / Intel）
- **Windows** 10 / 11
- **Linux**：常见发行版（需有图形环境）

## macOS：下载 zip 后如何运行

下载 `XSpace-<版本>.zip` 解压后，得到 `XSpace-<版本>.app`。由于未做 Apple 公证签名，首次运行需执行一次以下命令（解除隔离 + 赋予可执行权限）：

```bash
# 把路径换成实际解压位置与版本
xattr -cr ~/Downloads/XSpace-0.1.0.app
chmod +x ~/Downloads/XSpace-0.1.0.app/Contents/MacOS/xspace
```

然后**双击 `XSpace-0.1.0.app`** 即可启动（不再弹出终端）。

::: tip 也可以用图形界面解除拦截
若双击时系统提示「无法打开」或「已损坏」：打开「系统设置 → 隐私与安全性」，点击「仍要打开」即可。
:::

## 获取 XSpace

### 方式一：从源码构建

需要 [Rust](https://www.rust-lang.org/) 工具链（`rustup` 安装的 stable 即可）。

```bash
# 克隆仓库
git clone <repo-url> XSpace
cd XSpace

# 调试运行
cargo run

# 正式发布构建（优化）
cargo build --release
```

构建产物：

- 调试版：`target/debug/xspace`
- 正式版：`target/release/xspace`（约 5MB，优化编译）

直接运行该二进制即可：

```bash
./target/release/xspace
```

### 方式二：打包为可分发的应用

| 平台 | 命令 | 产物 |
| --- | --- | --- |
| macOS | `./scripts/package-macos.sh` | `dist/XSpace-<版本>.app` + `dist/XSpace-<版本>.zip`（含图标，双击启动不弹终端；zip 内附 README） |
| Windows | `cargo build --release` | `target\release\xspace.exe`（图标已嵌入，无控制台窗口） |

## 平台说明

| 平台 | 说明 |
| --- | --- |
| macOS | `package-macos.sh` 生成 `.app`：由 logo 生成 `AppIcon.icns`，写入版本号与「苏州遇码科技有限公司」署名，ad-hoc 签名（本机可用；正式分发需 Apple 签名+公证）。 |
| Windows | 用 MSVC 工具链（`stable-x86_64-pc-windows-msvc`）`cargo build --release`：`build.rs` 自动把 `assets/icon.ico` 嵌入 .exe，且 `windows_subsystem="windows"` 隐藏控制台。 |
| Linux | 自动加载系统中文字体；`xdg-open` 用于「定位」操作。 |

## 许可证

MIT License。© 苏州遇码科技有限公司
