//! 发文件：要发的文件怎么列出来、收到的文件怎么安全地落到磁盘上。连网的部分在 net 里。
//!
//! · 发：选中的文件和文件夹展开成“相对路径 + 大小”的清单，文件夹里的东西保持原来的层次。
//! · 收：只认清单里的相对路径（不许绝对路径、不许 ..），写进收件文件夹。Windows 上不能用的字符换成 _。
//!   最上层的名字一开始就占好（和已有的、和同时在收的重名就在后面加“ (1)”）；每个文件先写成
//!   “…一苇-接收中”的临时文件，收全了才改成正式的名字，改名时绝不覆盖已有的文件；中途断了只删没收完的那个。

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

/// 一次发的东西：每项是（磁盘上的文件，对方那边的相对路径，大小）
pub type Manifest = Vec<(PathBuf, String, u64)>;

/// 一次最多发这么多个文件（清单要装在一条消息里）
pub const MAX_FILES: usize = 100_000;

/// 把选中的文件、文件夹展开成清单。系统自己生成的 .DS_Store、._*、Thumbs.db、desktop.ini 不发；
/// 文件夹里的快捷方式（符号链接）不跟进去，免得绕圈；读不了的子文件夹跳过
pub fn collect(paths: &[PathBuf]) -> io::Result<Manifest> {
    let mut out = Vec::new();
    for p in paths {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).filter(|n| !n.is_empty()).unwrap_or_else(|| "文件".into());
        let meta = std::fs::metadata(p)?;                                  // 用户直接选的：是链接也跟过去
        walk(p, &meta, &name, &mut out)?;
        if out.len() > MAX_FILES { return Err(io::Error::new(io::ErrorKind::Other, format!("文件太多（一次最多 {MAX_FILES} 个）"))) }
    }
    Ok(out)
}
/// 复制的文件、文件夹合起来数出来的结果：够小可以带、太大（或者太多）、一个文件都没有（空文件夹、路径不在了、读不了）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit { Yes, TooBig, Nothing }
/// 这些文件、文件夹合起来有没有超过 max_bytes 字节、max_files 个。跳过的东西和 collect 一样。
/// 一超就停，不把大文件夹走完（复制的文件要不要跟着指针带过去，穿屏的那一下就要知道）
pub fn fits(paths: &[PathBuf], max_bytes: u64, max_files: usize) -> Fit {
    fn add(p: &Path, meta: &std::fs::Metadata, left: &mut (u64, usize)) -> bool {
        if meta.is_dir() {
            let Ok(dir) = std::fs::read_dir(p) else { return true };
            for e in dir.filter_map(|e| e.ok()) {
                let n = e.file_name().to_string_lossy().into_owned();
                if n == ".DS_Store" || n.starts_with("._") || n == "Thumbs.db" || n == "desktop.ini" { continue }
                let Ok(m) = std::fs::symlink_metadata(e.path()) else { continue };
                if !m.file_type().is_symlink() && !add(&e.path(), &m, left) { return false }
            }
        } else if meta.is_file() {
            if meta.len() > left.0 || left.1 == 0 { return false }
            left.0 -= meta.len(); left.1 -= 1;
        }
        true
    }
    let mut left = (max_bytes, max_files);
    for p in paths {
        let Ok(m) = std::fs::metadata(p) else { continue };
        if !add(p, &m, &mut left) { return Fit::TooBig }
    }
    if left.1 < max_files { Fit::Yes } else { Fit::Nothing }
}
fn walk(p: &Path, meta: &std::fs::Metadata, rel: &str, out: &mut Manifest) -> io::Result<()> {
    if rel.len() > 4000 { return Err(io::Error::new(io::ErrorKind::Other, format!("名字太长：{rel}"))) }
    if meta.is_dir() {
        let Ok(dir) = std::fs::read_dir(p) else { return Ok(()) };
        let mut items: Vec<_> = dir.filter_map(|e| e.ok()).collect();
        items.sort_by_key(|e| e.file_name());
        for e in items {
            let n = e.file_name().to_string_lossy().into_owned();
            if n == ".DS_Store" || n.starts_with("._") || n == "Thumbs.db" || n == "desktop.ini" { continue }
            let Ok(m) = std::fs::symlink_metadata(e.path()) else { continue };
            if m.file_type().is_symlink() { continue }
            walk(&e.path(), &m, &format!("{rel}/{n}"), out)?;
            if out.len() > MAX_FILES { return Ok(()) }
        }
    } else if meta.is_file() {
        out.push((p.to_path_buf(), rel.to_string(), meta.len()));
    }
    Ok(())
}

/// 对方给的相对路径，转成本机可以用的、一定落在收件文件夹里面的路径。
/// 绝对路径、..、空名字直接不收；Windows 上不能用的字符（\ : * ? " < > | 和控制字符、能把名字倒过来显示的字符）换成 _，
/// 末尾的空格和点去掉，CON、COM1 这类设备名前面加 _
pub fn safe_rel(rel: &str) -> Option<PathBuf> {
    if rel.is_empty() || rel.len() > 4096 || rel.starts_with('/') { return None }
    let mut out = PathBuf::new();
    for part in rel.split('/') {
        if part.is_empty() || part == "." || part == ".." { return None }
        let mut clean: String = part.chars().map(|c| {
            let bidi = matches!(c as u32, 0x200E | 0x200F | 0x202A..=0x202E | 0x2066..=0x2069);
            if (c as u32) < 0x20 || c == '\u{7F}' || bidi || "\\:*?\"<>|".contains(c) { '_' } else { c }
        }).collect();
        while clean.ends_with(' ') || clean.ends_with('.') { clean.pop(); }
        if clean.is_empty() { clean = "_".into() }
        let stem = clean.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
        let reserved = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&stem.as_str())
            || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4 && stem.as_bytes()[3].is_ascii_digit());
        if reserved { clean.insert(0, '_') }
        out.push(clean);
    }
    if out.components().any(|c| !matches!(c, Component::Normal(_))) { return None }
    Some(out)
}

/// dir 里还没被占用的名字：“照片” → “照片 (1)”；“a.txt” → “a (1).txt”
pub fn unique(dir: &Path, name: &str) -> String {
    if !dir.join(name).exists() { return name.to_string() }
    let (stem, ext) = match name.rfind('.') { Some(i) if i > 0 => (&name[..i], &name[i..]), _ => (name, "") };
    (1..).map(|n| format!("{stem} ({n}){ext}")).find(|n| !dir.join(n).exists()).unwrap()
}

/// 正在收的一批文件
pub struct Incoming {
    dir: PathBuf,
    files: Vec<(PathBuf, u64)>,              // 每个文件落在哪（已经换好不重名的最上层名字）、多大
    pub total: u64,
    pub got: u64,
    cur: usize,
    cur_got: u64,
    out: Option<(File, PathBuf)>,            // 正在写的临时文件
    pub first: PathBuf,                      // 收下的第一个最上层的东西（通知里“打开”用）
    pub count: usize,
}

impl Incoming {
    /// 对方报了清单：检查、换好名字。清单不合规矩就拒收
    pub fn begin(dir: &Path, list: &[(String, u64)]) -> io::Result<Incoming> {
        std::fs::create_dir_all(dir)?;
        let bad = || io::Error::new(io::ErrorKind::InvalidData, "文件名不合规矩");
        if list.len() > MAX_FILES { return Err(io::Error::new(io::ErrorKind::InvalidData, "文件太多")) }
        // 最上层的名字 → 换成不重名的（不分大小写比较，Mac、Windows 的磁盘大多不分）。
        // 文件夹一开始就建好占住名字；文件在收完改名时再确认一次不覆盖
        let mut tops: HashMap<String, String> = HashMap::new();
        let mut used: HashSet<String> = HashSet::new();
        let mut files = Vec::with_capacity(list.len());
        let mut total = 0u64;
        for (rel, size) in list {
            let p = safe_rel(rel).ok_or_else(bad)?;
            let mut parts: Vec<String> = p.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
            let top = parts.remove(0);
            let is_dir = !parts.is_empty();
            let new_top = match tops.get(&top.to_lowercase()) { Some(t) => t.clone(), None => {
                let mut t = unique(dir, &top);
                let mut n = 1;
                while used.contains(&t.to_lowercase()) || (is_dir && std::fs::create_dir(dir.join(&t)).is_err()) {
                    t = unique(dir, &format!("{top} ({n})")); n += 1;
                    if n > 10_000 { return Err(bad()) }
                }
                used.insert(t.to_lowercase()); tops.insert(top.to_lowercase(), t.clone()); t } };
            let mut full = dir.join(new_top);
            for x in parts { full.push(x) }
            total = total.checked_add(*size).ok_or_else(bad)?;
            files.push((full, *size));
        }
        let first = files.first().map(|f| { let rel = f.0.strip_prefix(dir).unwrap_or(&f.0); dir.join(rel.components().next().map(|c| c.as_os_str()).unwrap_or_default()) }).unwrap_or_else(|| dir.to_path_buf());
        let mut me = Incoming { dir: dir.to_path_buf(), count: files.len(), files, total, got: 0, cur: 0, cur_got: 0, out: None, first };
        me.skip_empty()?;
        Ok(me)
    }
    fn open_cur(&mut self) -> io::Result<()> {
        let (path, _) = &self.files[self.cur];
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)? }
        let tmp = path.with_file_name(format!("{}.一苇-接收中", path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
        self.out = Some((File::create(&tmp)?, tmp));
        Ok(())
    }
    fn finish_cur(&mut self) -> io::Result<()> {
        if self.out.is_none() { self.open_cur()? }
        let (f, tmp) = self.out.take().unwrap();
        drop(f);                                                            // 不逐个强制写盘：系统自己会写，强制写会拖慢上千个小文件
        let mut want = self.files[self.cur].0.clone();
        // 改成正式名字，绝不覆盖已有的（比如同时从两台电脑收到同名文件）：撞名就换一个
        loop {
            match place(&tmp, &want) {
                Ok(()) => break,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    let parent = want.parent().map(Path::to_path_buf).unwrap_or_default();
                    let name = want.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    want = parent.join(unique(&parent, &name));
                    if self.cur == 0 || self.first == self.files[self.cur].0 { self.first = want.clone() }
                }
                Err(e) => { let _ = std::fs::remove_file(&tmp); return Err(e) }
            }
        }
        self.files[self.cur].0 = want;
        self.cur += 1; self.cur_got = 0;
        Ok(())
    }
    /// 大小为 0 的文件不会有内容块，直接建好
    fn skip_empty(&mut self) -> io::Result<()> {
        while self.cur < self.files.len() && self.files[self.cur].1 == 0 { self.finish_cur()? }
        Ok(())
    }
    /// 收到一块内容（可能跨好几个文件）
    pub fn data(&mut self, mut d: &[u8]) -> io::Result<()> {
        while !d.is_empty() {
            if self.cur >= self.files.len() { return Err(io::Error::new(io::ErrorKind::InvalidData, "内容比清单多")) }
            if self.out.is_none() { self.open_cur()? }
            let left = self.files[self.cur].1 - self.cur_got;
            let n = (d.len() as u64).min(left) as usize;
            self.out.as_mut().unwrap().0.write_all(&d[..n])?;
            self.cur_got += n as u64; self.got += n as u64; d = &d[n..];
            if self.cur_got == self.files[self.cur].1 { self.finish_cur()?; self.skip_empty()?; }
        }
        Ok(())
    }
    /// 对方说发完了：要正好收全
    pub fn end(&mut self) -> io::Result<()> {
        if self.cur == self.files.len() && self.got == self.total { Ok(()) } else { Err(io::Error::new(io::ErrorKind::UnexpectedEof, "没收全")) }
    }
    pub fn dir(&self) -> &Path { &self.dir }
    /// 收下的最上层的东西（文件或文件夹），按清单的顺序、不重复
    pub fn tops(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        for (f, _) in &self.files {
            let Some(top) = f.strip_prefix(&self.dir).ok().and_then(|r| r.components().next()) else { continue };
            let top = self.dir.join(top.as_os_str());
            if !out.contains(&top) { out.push(top) }
        }
        out
    }
}
/// 把收好的临时文件放到正式的名字上；那个名字已经有东西了就报 AlreadyExists，不覆盖。
/// 先试“硬链接再删临时文件”（不会覆盖）；磁盘不支持硬链接（比如 exFAT 的移动硬盘）就先看一眼再改名
fn place(tmp: &Path, want: &Path) -> io::Result<()> {
    match std::fs::hard_link(tmp, want) {
        Ok(()) => { let _ = std::fs::remove_file(tmp); Ok(()) }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(e),
        Err(_) => {
            if want.exists() { return Err(io::Error::new(io::ErrorKind::AlreadyExists, "重名")) }
            std::fs::rename(tmp, want)
        }
    }
}

impl Drop for Incoming {
    /// 没收完就没了（取消、断开）：删掉那个写了一半的临时文件
    fn drop(&mut self) { if let Some((f, tmp)) = self.out.take() { drop(f); let _ = std::fs::remove_file(tmp); } }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf { let d = std::env::temp_dir().join(format!("yiwei-files-{name}-{}", std::process::id())); let _ = std::fs::remove_dir_all(&d); std::fs::create_dir_all(&d).unwrap(); d }

    #[test]
    fn 不合规矩的路径不收() {
        for bad in ["", "/etc/passwd", "../x", "a/../b", "a//b", "./a"] { assert!(safe_rel(bad).is_none(), "{bad}") }
        assert_eq!(safe_rel("照片/2024/一.jpg"), Some(PathBuf::from("照片").join("2024").join("一.jpg")));
        // Windows 上不能用的换掉，不整批拒收
        assert_eq!(safe_rel("报告?.docx"), Some(PathBuf::from("报告_.docx")));
        assert_eq!(safe_rel("C:/x"), Some(PathBuf::from("C_").join("x")));
        assert_eq!(safe_rel("a\\b"), Some(PathBuf::from("a_b")));
        assert_eq!(safe_rel("文件夹. /a"), Some(PathBuf::from("文件夹").join("a")));
        assert_eq!(safe_rel("aux.c"), Some(PathBuf::from("_aux.c")));
        assert_eq!(safe_rel("COM5"), Some(PathBuf::from("_COM5")));
        assert_eq!(safe_rel("x\u{202E}gpj.exe"), Some(PathBuf::from("x_gpj.exe")));
    }

    #[test]
    fn 合起来多大_一超就说不行() {
        let d = tmp("fits");
        std::fs::create_dir_all(d.join("夹/里")).unwrap();
        std::fs::write(d.join("夹/a.bin"), vec![0u8; 600]).unwrap();
        std::fs::write(d.join("夹/里/b.bin"), vec![0u8; 300]).unwrap();
        std::fs::write(d.join("夹/.DS_Store"), vec![0u8; 5000]).unwrap();
        std::fs::write(d.join("c.bin"), vec![0u8; 100]).unwrap();
        let all = [d.join("夹"), d.join("c.bin")];
        assert_eq!(fits(&all, 1000, 3), Fit::Yes, "正好 1000 字节、3 个；系统自己生成的不算");
        assert_eq!((fits(&all, 999, 3), fits(&all, 1000, 2)), (Fit::TooBig, Fit::TooBig));
        std::fs::create_dir_all(d.join("空的")).unwrap();
        assert_eq!((fits(&[d.join("没有这个")], 1000, 3), fits(&[], 1000, 3), fits(&[d.join("空的")], 1000, 3)), (Fit::Nothing, Fit::Nothing, Fit::Nothing), "一个文件都没有：没什么可带的，不是“太大”");
        assert_eq!(fits(&[d.join("没有这个"), d.join("c.bin")], 1000, 3), Fit::Yes, "不在了的那个跳过，别的照带");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn 重名自动加编号() {
        let d = tmp("uniq");
        std::fs::write(d.join("a.txt"), "x").unwrap(); std::fs::create_dir(d.join("照片")).unwrap();
        assert_eq!(unique(&d, "a.txt"), "a (1).txt");
        assert_eq!(unique(&d, "照片"), "照片 (1)");
        assert_eq!(unique(&d, "新的.mp4"), "新的.mp4");
        assert_eq!(unique(&d, ".bashrc"), ".bashrc");
    }

    #[test]
    fn 文件夹展开再原样收下() {
        let src = tmp("src"); let dst = tmp("dst");
        std::fs::create_dir_all(src.join("照片/子")).unwrap();
        std::fs::write(src.join("照片/一.jpg"), vec![7u8; 100_000]).unwrap();
        std::fs::write(src.join("照片/子/空.txt"), b"").unwrap();
        std::fs::write(src.join("照片/.DS_Store"), b"junk").unwrap();
        std::fs::write(src.join("说明.txt"), "你好").unwrap();
        std::fs::write(dst.join("说明.txt"), "原来就有").unwrap();
        let m = collect(&[src.join("照片"), src.join("说明.txt")]).unwrap();
        let list: Vec<(String, u64)> = m.iter().map(|f| (f.1.clone(), f.2)).collect();
        assert_eq!(list, vec![("照片/一.jpg".into(), 100_000), ("照片/子/空.txt".into(), 0), ("说明.txt".into(), 6)]);
        let mut inc = Incoming::begin(&dst, &list).unwrap();
        let mut all = Vec::new(); for f in &m { all.extend(std::fs::read(&f.0).unwrap()) }
        for chunk in all.chunks(30_000) { inc.data(chunk).unwrap() }      // 一块会跨文件
        inc.end().unwrap();
        assert_eq!(std::fs::read(dst.join("照片/一.jpg")).unwrap().len(), 100_000);
        assert!(dst.join("照片/子/空.txt").exists());
        assert_eq!(std::fs::read_to_string(dst.join("说明 (1).txt")).unwrap(), "你好");
        assert_eq!(std::fs::read_to_string(dst.join("说明.txt")).unwrap(), "原来就有");
        assert_eq!(inc.first, dst.join("照片"));
        assert!(inc.data(b"x").is_err(), "多出来的不要");
    }

    #[test]
    fn 半路断了删掉没收完的() {
        let dst = tmp("cut");
        let mut inc = Incoming::begin(&dst, &[("a.bin".into(), 10), ("b.bin".into(), 10)]).unwrap();
        inc.data(&[1; 15]).unwrap();
        assert!(inc.end().is_err());
        drop(inc);
        assert!(dst.join("a.bin").exists(), "收全的留着");
        let left: Vec<_> = std::fs::read_dir(&dst).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left, vec!["a.bin".to_string()], "写了一半的临时文件要删掉");
        assert!(Incoming::begin(&dst, &[("../逃出去".into(), 1)]).is_err());
    }

    #[test]
    fn 同时收同名的不互相覆盖() {
        let dst = tmp("same");
        // 两批都叫 IMG.jpg，还有只差大小写的文件夹：先开始、后收完的不能盖掉先收完的
        let mut a = Incoming::begin(&dst, &[("IMG.jpg".into(), 1), ("Pics/x".into(), 1)]).unwrap();
        let mut b = Incoming::begin(&dst, &[("IMG.jpg".into(), 1), ("pics/x".into(), 1)]).unwrap();
        b.data(&[2, 2]).unwrap(); b.end().unwrap();
        a.data(&[1, 1]).unwrap(); a.end().unwrap();
        let mut names: Vec<String> = std::fs::read_dir(&dst).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names.len(), 4, "{names:?}");
        assert_eq!(std::fs::read(dst.join("IMG.jpg")).unwrap(), vec![2]);
        assert_eq!(std::fs::read(dst.join("IMG (1).jpg")).unwrap(), vec![1]);
    }
}
