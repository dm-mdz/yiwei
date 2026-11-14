#!/bin/bash
# 下载雾凇拼音词库（https://github.com/iDvel/rime-ice ，GPL-3.0）到 third_party/rime-ice/，之后重新构建输入法就会用上。
#   tools/get_dict.sh        基础：常用字表 + 基础词库，约 17 MB，54 万词
#   tools/get_dict.sh full   全套：再加扩展词库和腾讯词库，共约 46 MB，188 万词
# 想换回自带的小词库：删掉 third_party/rime-ice/ 再重新构建。
set -e
cd "$(dirname "$0")/../third_party"
mkdir -p rime-ice && cd rime-ice
BASE=https://raw.githubusercontent.com/iDvel/rime-ice/main
# 先下到临时文件，下完整了（末尾是完整的一行、有词库正文）才换上去：断线不会留下半截词库
get() {
  curl -fsSL --retry 3 -o "$2.tmp" "$BASE/$1"
  if [ "$3" = dict ]; then
    grep -q '^\.\.\.$' "$2.tmp" && [ -z "$(tail -c 1 "$2.tmp")" ] || { rm -f "$2.tmp"; echo "下载不完整：$2，请重试"; exit 1; }
  fi
  mv "$2.tmp" "$2"
}
get LICENSE LICENSE
FILES="8105 base"
if [ "$1" = full ]; then FILES="$FILES ext tencent"; else rm -f ext.dict.yaml tencent.dict.yaml; fi
for f in $FILES; do echo "下载 $f.dict.yaml"; get "cn_dicts/$f.dict.yaml" "$f.dict.yaml" dict; done
ls -la
