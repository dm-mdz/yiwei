#!/bin/bash
# 打包 Windows 版：先编译，再把 exe 和使用说明压成一个 zip，放在项目的 build/ 里
set -e
cd "$(dirname "$0")"
./build.sh
OUT=../../build/yiwei-windows; rm -rf "$OUT"; mkdir -p "$OUT/一苇"
cp target/x86_64-pc-windows-gnullvm/release/Yiwei.exe "$OUT/一苇/Yiwei.exe"
# 记事本打开不乱码：带 BOM、Windows 换行
printf '\xEF\xBB\xBF' > "$OUT/一苇/使用说明.txt"
sed 's/$/\r/' 使用说明.txt >> "$OUT/一苇/使用说明.txt"
# 用 Python 打包：不带 Mac 的隐藏文件，中文文件名标成 UTF-8，Windows 自带的解压认得
(cd "$OUT" && python3 -c 'import zipfile,os
with zipfile.ZipFile("一苇-Windows.zip","w",zipfile.ZIP_DEFLATED) as z:
    for f in sorted(os.listdir("一苇")): z.write(os.path.join("一苇",f))' && rm -rf 一苇)
echo "packed: $OUT/一苇-Windows.zip ($(wc -c < "$OUT/一苇-Windows.zip") bytes)"
