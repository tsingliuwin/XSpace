#!/usr/bin/env bash
# 把 release 二进制打包成 macOS .app（避免双击时弹出终端，并带上应用图标）。
# 产物：dist/XSpace.app
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
APP_NAME="XSpace"
BUNDLE_ID="com.xspace.app"
# 版本号取自 Cargo.toml，避免硬编码不同步
VERSION="$(grep -m1 '^version' "$ROOT_DIR/Cargo.toml" | sed -E 's/.*"([^"]+)".*/\1/')"
# 构建号：用 git 提交计数（递增整数）；无 git 时回退 1
BUILD="$(git -C "$ROOT_DIR" rev-list --count HEAD 2>/dev/null || echo 1)"
ICON_SRC="$ROOT_DIR/assets/logo.png"
BIN="$ROOT_DIR/target/release/xspace"
DIST="$ROOT_DIR/dist"
# bundle 目录名带版本号（如 XSpace-0.1.0.app），便于区分多版本
APP="$DIST/$APP_NAME-$VERSION.app"

if [[ ! -f "$BIN" ]]; then
  echo "未找到 release 二进制，先执行：cargo build --release"
  exit 1
fi
if [[ ! -f "$ICON_SRC" ]]; then
  echo "未找到图标源：$ICON_SRC"
  exit 1
fi

echo "==> 构建目录结构（清空 dist）"
rm -rf "$DIST"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

echo "==> 拷贝二进制并 strip"
cp "$BIN" "$APP/Contents/MacOS/xspace"
strip "$APP/Contents/MacOS/xspace" || true
chmod +x "$APP/Contents/MacOS/xspace"

echo "==> 生成图标 (icns)"
ICONSET="$(mktemp -d)/AppIcon.iconset"
mkdir -p "$ICONSET"
for spec in \
  "16:icon_16x16" "32:icon_16x16@2x" \
  "32:icon_32x32" "64:icon_32x32@2x" \
  "128:icon_128x128" "256:icon_128x128@2x" \
  "256:icon_256x256" "512:icon_256x256@2x" \
  "512:icon_512x512" "1024:icon_512x512@2x"; do
  size="${spec%%:*}"; name="${spec##*:}"
  sips -z "$size" "$size" "$ICON_SRC" --out "$ICONSET/$name.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"

echo "==> 写入 Info.plist"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>${APP_NAME}</string>
  <key>CFBundleDisplayName</key><string>${APP_NAME}</string>
  <key>CFBundleIdentifier</key><string>${BUNDLE_ID}</string>
  <key>CFBundleVersion</key><string>${BUILD}</string>
  <key>CFBundleShortVersionString</key><string>${VERSION}</string>
  <key>CFBundleExecutable</key><string>xspace</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>10.11</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSUIElement</key><false/>
  <key>NSHumanReadableCopyright</key><string>© 苏州遇码科技有限公司</string>
  <key>CFBundleGetInfoString</key><string>${APP_NAME} ${VERSION}, © 苏州遇码科技有限公司</string>
</dict>
</plist>
PLIST

echo "==> PkgInfo"
printf 'APPL????' > "$APP/Contents/PkgInfo"

# 确保可执行权限
chmod +x "$APP/Contents/MacOS/xspace"

echo "==> ad-hoc 签名"
codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || echo "   (codesign 跳过)"

echo "==> 生成 README（引导用户去官网）"
README="$DIST/README.txt"
cat > "$README" <<README_EOF
XSpace - 极速磁盘空间探测器   v${VERSION}
© 苏州遇码科技有限公司

官网 / 使用指南：https://xspace.xi-n.com
（安装、功能特性、各扫描模式说明、结果操作等详见官网「使用指南」）

macOS 快速运行（解压后执行一次）：
  xattr -cr ~/Downloads/${APP_NAME}-${VERSION}.app
  chmod +x ~/Downloads/${APP_NAME}-${VERSION}.app/Contents/MacOS/xspace

然后双击 ${APP_NAME}-${VERSION}.app 启动。
首次若被系统拦截：打开「系统设置 → 隐私与安全性」→ 点击「仍要打开」。
README_EOF

echo "==> 打包分发 zip（ditto 打 .app，再追加 README）"
ZIP="$DIST/$APP_NAME-$VERSION.zip"
ZIP_NAME="$(basename "$ZIP")"
# ditto 保留资源叉/扩展属性（code signature 完整）
ditto -c -k --sequesterRsrc --keepParent "$APP" "$ZIP"
# 追加 README（与 .app 同级）
( cd "$DIST" && zip -q "$ZIP_NAME" README.txt )
rm -f "$README"

echo ""
echo "完成。产物："
echo "  $APP   (可双击运行，不再弹终端)"
echo "  $ZIP   (分发用；解压后为 $APP_NAME-$VERSION.app)"
echo ""
echo "给下载用户的说明（解压后执行一次即可运行）："
echo "  xattr -cr \"~/Downloads/$APP_NAME-$VERSION.app\""
echo "  chmod +x \"~/Downloads/$APP_NAME-$VERSION.app/Contents/MacOS/xspace\""

