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
# 把本机的文件夹路径从程序里换掉（Rust 和 Swift 都会把源码路径写进去），发出去的程序不带本机路径
ROOT="$(cd ../.. && pwd)"; CH="${CARGO_HOME:-$HOME/.cargo}"
RUSTFLAGS="$RUSTFLAGS --remap-path-prefix=$CH=/cargo --remap-path-prefix=$ROOT=/yiwei" \
  "${CARGO:-cargo}" build --release --quiet --manifest-path "$CORE/Cargo.toml" --target-dir "$CORE/target"
swiftc -O -swift-version 5 -target arm64-apple-macos14.0 -file-prefix-map "$ROOT=/yiwei" main.swift App.swift Overlay.swift OCR.swift LongShot.swift Share.swift \
  -import-objc-header "$CORE/include/yiwei_core.h" -L "$CORE/target/release" -lyiwei_core \
  -framework Cocoa -framework Carbon -framework ScreenCaptureKit -framework Vision -framework ServiceManagement -framework ApplicationServices -framework UserNotifications \
  -o "$APP/Contents/MacOS/YiweiMain"
# 签名。想用自己固定的开发者身份（这样重新构建后不用反复重新授权屏幕录制、辅助功能），
# 在仓库根目录建一个 local.env（不进仓库），写一行 YIWEI_SIGN_ID="你的签名身份"；没有就用临时签名
[ -f ../../local.env ] && . ../../local.env
# 用固定身份签名时一律打开加固运行（hardened runtime），和发布包一样；发布时 YIWEI_TIMESTAMP=1 再加上苹果的时间戳（公证要求）
OPTS=(--force --options runtime --sign "$YIWEI_SIGN_ID"); [ -n "$YIWEI_TIMESTAMP" ] && OPTS+=(--timestamp)
if [ -n "$YIWEI_SIGN_ID" ] && codesign "${OPTS[@]}" "$APP" 2>/dev/null; then :; else codesign --force --sign - "$APP"; fi
echo "built: $APP"
