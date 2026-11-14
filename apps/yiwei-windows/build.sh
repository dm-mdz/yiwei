#!/bin/bash
# 在 Mac 上编出 Windows 版一苇（x86_64）。需要：
#   · Rust 的 x86_64-pc-windows-gnullvm 标准库（和本机 rustc 同一版本）
#   · llvm-mingw（提供 Windows 的链接器和系统库）：解压到哪，就把 LLVM_MINGW 设成哪
#     （可以写在仓库根目录的 local.env 里，那个文件不进仓库）；没设就找 ~/llvm-mingw
set -e
cd "$(dirname "$0")"
[ -f ../../local.env ] && . ../../local.env
MINGW="${LLVM_MINGW:-$HOME/llvm-mingw}"
export CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_LINKER="$MINGW/bin/x86_64-w64-mingw32-clang"
export WINDRES="$MINGW/bin/x86_64-w64-mingw32-windres"
# 运行库静态链进去：只交一个 exe，Windows 10/11 上不用另装东西
# 把本机的文件夹路径从 exe 里换掉（Rust 会把源码路径写进报错信息里），发出去的 exe 不带本机路径
ROOT="$(cd ../.. && pwd)"; CH="${CARGO_HOME:-$HOME/.cargo}"
export CARGO_TARGET_X86_64_PC_WINDOWS_GNULLVM_RUSTFLAGS="-C target-feature=+crt-static --remap-path-prefix=$CH=/cargo --remap-path-prefix=$ROOT=/yiwei"
"${CARGO:-cargo}" build --release --offline --target x86_64-pc-windows-gnullvm "$@"
OUT=target/x86_64-pc-windows-gnullvm/release/Yiwei.exe
echo "built: $OUT ($(wc -c < "$OUT") bytes)"
