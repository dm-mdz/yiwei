#!/bin/bash
# 构建一苇输入法（只在本目录的 build/ 里生成，不安装到系统）
set -e
cd "$(dirname "$0")"
ROOT="$(cd ../.. && pwd)"
RIME="$ROOT/third_party/librime/dist"
APP=build/Yiwei.app
rm -rf build
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Frameworks" "$APP/Contents/Resources/rime" "$APP/Contents/Resources/licenses"
cp Info.plist "$APP/Contents/Info.plist"
clang -O2 -c -arch arm64 -mmacosx-version-min=14.0 -ffile-prefix-map="$ROOT"=/yiwei -I "$RIME/include" yw_rime.c -o build/yw_rime.o
clang -O2 -c -arch arm64 -mmacosx-version-min=14.0 -ffile-prefix-map="$ROOT"=/yiwei yw_fix.c -o build/yw_fix.o
swiftc -O -swift-version 5 -target arm64-apple-macos14.0 -file-prefix-map "$ROOT=/yiwei" -import-objc-header yw_rime.h \
  main.swift build/yw_rime.o build/yw_fix.o -L "$RIME/lib" -lrime.1 \
  -Xlinker -rpath -Xlinker @executable_path/../Frameworks \
  -framework Cocoa -framework InputMethodKit \
  -o "$APP/Contents/MacOS/Yiwei"
cp "$RIME/lib/librime.1.17.0.dylib" "$APP/Contents/Frameworks/librime.1.dylib"
cp "$ROOT/assets/ime_icon.tiff" "$ROOT/assets/Yiwei.icns" "$APP/Contents/Resources/"
cp "$ROOT/data/rime/"*.yaml "$APP/Contents/Resources/rime/"
cp "$ROOT/third_party/rime-pinyin-simp/pinyin_simp.dict.yaml" "$APP/Contents/Resources/rime/"   # 纠错层的词频始终用它
# 词库：third_party/rime-ice 里有雾凇拼音就用它，没有就用自带的小词库（见 tools/make_dict.py）
python3 "$ROOT/tools/make_dict.py" "$APP/Contents/Resources/rime"
[ -f "$ROOT/third_party/rime-ice/base.dict.yaml" ] && cp "$ROOT/third_party/rime-ice/LICENSE" "$APP/Contents/Resources/licenses/rime-ice-LICENSE-GPL-3.0.txt"
cp "$ROOT/third_party/rime-pinyin-simp/LICENSE" "$APP/Contents/Resources/licenses/rime-pinyin-simp-LICENSE.txt"
cp "$ROOT/third_party/rime-pinyin-simp/AUTHORS" "$APP/Contents/Resources/licenses/rime-pinyin-simp-AUTHORS.txt"
# 签名：仓库根目录的 local.env（不进仓库）里写了 YIWEI_SIGN_ID 就用它，没有就用临时签名
[ -f "$ROOT/local.env" ] && . "$ROOT/local.env"
# 用固定身份签名时一律打开加固运行，和发布包一样；发布时 YIWEI_TIMESTAMP=1 再加上苹果的时间戳（公证要求）。
# 加固运行下 librime 必须和输入法同一个身份签名才加载得了，所以两者要么都用固定身份，要么都用临时签名。
OPTS=(--force --options runtime --sign "$YIWEI_SIGN_ID"); [ -n "$YIWEI_TIMESTAMP" ] && OPTS+=(--timestamp)
if [ -n "$YIWEI_SIGN_ID" ] && codesign "${OPTS[@]}" "$APP/Contents/Frameworks/librime.1.dylib" 2>/dev/null && codesign "${OPTS[@]}" "$APP" 2>/dev/null; then :; else
  codesign --force --sign - "$APP/Contents/Frameworks/librime.1.dylib"; codesign --force --sign - "$APP"
fi
echo "built: $APP"
