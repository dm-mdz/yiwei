#!/bin/bash
# 引擎层自测（先运行 build.sh）
set -e
cd "$(dirname "$0")"
RIME="$(cd ../.. && pwd)/third_party/librime/dist"
clang -O1 -arch arm64 -I "$RIME/include" test_engine.c yw_rime.c yw_fix.c -L "$RIME/lib" -lrime.1 -Xlinker -rpath -Xlinker "$RIME/lib" -o build/test_engine
rm -rf build/test_user && mkdir -p build/test_user
./build/test_engine build/Yiwei.app/Contents/Resources/rime build/test_user
