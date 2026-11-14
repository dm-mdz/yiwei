//! 给 exe 带上图标、清单（按真实像素算坐标、不要求管理员权限）和版本信息
use std::process::Command;
fn main() {
    println!("cargo:rerun-if-changed=res");
    if !std::env::var("TARGET").unwrap_or_default().contains("windows") { return }
    let windres = std::env::var("WINDRES").unwrap_or_else(|_| "x86_64-w64-mingw32-windres".into());
    let obj = format!("{}/yiwei_res.o", std::env::var("OUT_DIR").unwrap());
    let ok = Command::new(&windres).current_dir("res").args(["-O", "coff", "-i", "yiwei.rc", "-o", &obj]).status().map_or(false, |s| s.success());
    if ok { println!("cargo:rustc-link-arg-bins={obj}") } else { println!("cargo:warning=没找到 windres，exe 不带图标和清单") }
}
