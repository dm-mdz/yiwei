#!/bin/bash
# 打 macOS 发布包：两个 dmg——一苇（截图、键鼠共享）和一苇输入法，各自按需安装。
# 正式签名 + 加固运行 + 苹果公证。需要仓库根目录的 local.env（不进仓库）里有：
#   YIWEI_SIGN_ID="Developer ID Application: 你的名字 (TEAMID)"
#   YIWEI_NOTARY_PROFILE=配置名   # 用 xcrun notarytool store-credentials 存进钥匙串的；不写就只签名不公证
# 发布包里不能带雾凇拼音词库（GPL-3.0）：third_party/rime-ice 在就停下，请在 tools/make_public.sh 导出的公开副本里打包。
# 只在 build/release/ 里生成文件，不往任何地方上传。
set -e
cd "$(dirname "$0")/.."
[ -f local.env ] && . ./local.env
case "$YIWEI_SIGN_ID" in "Developer ID Application:"*) ;; *) echo "local.env 里的 YIWEI_SIGN_ID 要是 Developer ID Application 证书"; exit 1;; esac
[ ! -e third_party/rime-ice ] || { echo "third_party/rime-ice 在：发布包不能带雾凇拼音词库（GPL-3.0）"; exit 1; }
VER=$(/usr/libexec/PlistBuddy -c 'Print CFBundleShortVersionString' apps/yiwei-macos/Info.plist)
OUT=build/release; rm -rf "$OUT"; mkdir -p "$OUT"
export YIWEI_SIGN_ID YIWEI_TIMESTAMP=1
apps/yiwei-macos/build.sh
apps/ime-macos/build.sh
for a in apps/yiwei-macos/build/Yiwei.app apps/ime-macos/build/Yiwei.app; do
  codesign --verify --strict --deep "$a"
  codesign -d --verbose=2 "$a" 2>&1 | grep -q 'flags=.*runtime' || { echo "$a 没有正式签名或没开加固运行"; exit 1; }
done
# mk <文件名> <卷名> <app> <快捷方式名> <快捷方式指向>：打开 dmg 后把 Yiwei.app 拖到旁边的快捷方式上就装好了
mk() {
  S="$OUT/stage"; rm -rf "$S"; mkdir -p "$S"; ditto "$3" "$S/Yiwei.app"; ln -s "$5" "$S/$4"
  hdiutil create -quiet -volname "$2" -srcfolder "$S" -fs HFS+ -format UDZO "$OUT/$1-$VER.dmg"; rm -rf "$S"
  codesign --force --timestamp --sign "$YIWEI_SIGN_ID" "$OUT/$1-$VER.dmg"
}
mk Yiwei "一苇" apps/yiwei-macos/build/Yiwei.app "应用程序" /Applications
mk Yiwei-IME "一苇输入法" apps/ime-macos/build/Yiwei.app "输入法" "/Library/Input Methods"
if [ -n "$YIWEI_NOTARY_PROFILE" ]; then
  for d in "$OUT"/*.dmg; do
    xcrun notarytool submit "$d" --keychain-profile "$YIWEI_NOTARY_PROFILE" --wait > "$d.notary.log" 2>&1 || true
    grep -q "status: Accepted" "$d.notary.log" || { cat "$d.notary.log"; echo "公证没通过：$d（详情：xcrun notarytool log <上面的 id> --keychain-profile 配置名）"; exit 1; }
    xcrun stapler staple -q "$d"
    spctl -a -t open --context context:primary-signature "$d"
  done
fi
(cd "$OUT" && shasum -a 256 *.dmg)
