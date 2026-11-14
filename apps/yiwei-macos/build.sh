#!/bin/bash
# 构建一苇主程序（菜单栏外壳 + 截图），只在本目录的 build/ 里生成
set -e
cd "$(dirname "$0")"
APP=build/Yiwei.app
rm -rf build
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp Info.plist "$APP/Contents/Info.plist"
cp ../../assets/Yiwei.icns ../../assets/status.png "../../assets/status@2x.png" "../../assets/status@3x.png" "$APP/Contents/Resources/"
# 键鼠共享的核心（Rust）先编成静态库，再和 Swift 外壳链在一起
CORE=../../core
"${CARGO:-cargo}" build --release --quiet --manifest-path "$CORE/Cargo.toml" --target-dir "$CORE/target"
swiftc -O -swift-version 5 -target arm64-apple-macos14.0 main.swift App.swift Overlay.swift OCR.swift LongShot.swift Share.swift \
  -import-objc-header "$CORE/include/yiwei_core.h" -L "$CORE/target/release" -lyiwei_core \
  -framework Cocoa -framework Carbon -framework ScreenCaptureKit -framework Vision -framework ServiceManagement -framework ApplicationServices -framework UserNotifications \
  -o "$APP/Contents/MacOS/YiweiMain"
# 签名。想用自己固定的开发者身份（这样重新构建后不用反复重新授权屏幕录制、辅助功能），
# 在仓库根目录建一个 local.env（不进仓库），写一行 YIWEI_SIGN_ID="你的签名身份"；没有就用临时签名
[ -f ../../local.env ] && . ../../local.env
if [ -n "$YIWEI_SIGN_ID" ] && codesign --force --sign "$YIWEI_SIGN_ID" "$APP" 2>/dev/null; then :; else codesign --force --sign - "$APP"; fi
echo "built: $APP"
