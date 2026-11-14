//! 运行记录：真机上出问题时看发生了什么。外壳调 ywk_set_log 给一个文件路径，之后核心和外壳的关键事件都写进去。
//! 每行：时间（时:分:秒.毫秒，世界时）+ 内容。接着上次的往后写（重启不清空，才能对着两台电脑的记录看同一件事）；
//! 超过 5MB 就从头重写，不会越长越大。
use std::io::Write;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX: u64 = 5 * 1024 * 1024;
static LOG: Mutex<Option<(std::fs::File, std::path::PathBuf, u64)>> = Mutex::new(None);

/// 从 1970-01-01 起的天数换成年月日
fn date(days: u64) -> (u64, u64, u64) {
    let z = days + 719468;
    let (era, doe) = (z / 146097, z % 146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    (yoe + era * 400 + (m <= 2) as u64, m, d)
}

pub fn open(path: &std::path::Path) {
    if let Some(d) = path.parent() { let _ = std::fs::create_dir_all(d); }
    let old = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let file = if old > MAX { std::fs::File::create(path).map(|f| (f, 0)) }
               else { std::fs::OpenOptions::new().create(true).append(true).open(path).map(|f| (f, old)) };
    if let Ok((f, n)) = file { *LOG.lock().unwrap() = Some((f, path.to_path_buf(), n)); OPEN.store(true, std::sync::atomic::Ordering::Relaxed); }
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (y, m, d) = date(secs / 86400);
    line(&format!("—— 开始记录 {y}-{m:02}-{d:02}（世界时；一苇核心 {}，协议第 {} 版）", env!("CARGO_PKG_VERSION"), crate::wire::PROTOCOL));
}

pub fn line(s: &str) {
    let mut g = LOG.lock().unwrap();
    let Some((f, path, n)) = g.as_mut() else { return };
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let sec = (ms / 1000) % 86400;
    let t = format!("{:02}:{:02}:{:02}.{:03} {s}\n", sec / 3600, sec / 60 % 60, sec % 60, ms % 1000);
    if *n > MAX { if let Ok(nf) = std::fs::File::create(&*path) { *f = nf; *n = 0 } }
    *n += t.len() as u64;
    let _ = f.write_all(t.as_bytes());
    let _ = f.flush();
}

// —— 轨迹（排查“卡不卡”用）——
// 光看汇总看不出鼠标是不是一顿一顿的。打开运行记录时，把一次控制 / 被控期间每个鼠标事件的时间和数值先记在内存里，
// 结束时接着写进运行记录旁边的“一苇轨迹.csv”（超过 8MB 从头重写）。每行：微秒时间,种类,六个数。种类：
//   1 macOS 监听到的移动（鼠标在对面时）：系统给的移动量 x、y，指针离停靠点 x、y，事件送到回调晚了多少毫秒，是不是正等着“挪回去的那一段”
//   2 后台线程发出的一批移动：移动量 x、y，从回调到处理等了多少毫秒
//   3 收到并照做的鼠标位置：x、y，照做花了多少毫秒
//   4 Windows 钩子看到的移动（鼠标在对面时）：移动量 x、y，指针离停靠点 x、y，0，这一下是不是因为指针不在停靠点而丢掉了
static OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static TRACE: Mutex<Vec<(u64, u8, [f64; 6])>> = Mutex::new(Vec::new());
static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
const TRACE_MAX: usize = 60000;

pub fn trace(tag: u8, v: [f64; 6]) {
    if !OPEN.load(std::sync::atomic::Ordering::Relaxed) { return }
    let t = T0.get_or_init(std::time::Instant::now).elapsed().as_micros() as u64;
    let mut g = TRACE.lock().unwrap();
    if g.capacity() == 0 { g.reserve_exact(TRACE_MAX) }
    if g.len() < TRACE_MAX { g.push((t, tag, v)) }
}
/// 把攒着的轨迹写进文件（另起一个线程写，不耽误调用的人）
pub fn trace_dump(title: &str) {
    let rows = std::mem::take(&mut *TRACE.lock().unwrap());
    let Some(path) = LOG.lock().unwrap().as_ref().and_then(|l| {
        // 轨迹文件跟着运行记录的名字走（“一苇日志-电脑名.txt”旁边是“一苇轨迹-电脑名.csv”）：几台电脑从同一个文件夹运行也不混
        let tail = l.1.file_stem().and_then(|s| s.to_str()).and_then(|s| s.split_once('-')).map_or(String::new(), |(_, t)| format!("-{t}"));
        l.1.parent().map(|d| d.join(format!("一苇轨迹{tail}.csv")))
    }) else { return };
    if rows.is_empty() { return }
    let title = title.to_string();
    std::thread::spawn(move || {
        let big = std::fs::metadata(&path).map(|m| m.len() > 8 * 1024 * 1024).unwrap_or(false);
        let file = if big { std::fs::File::create(&path) } else { std::fs::OpenOptions::new().create(true).append(true).open(&path) };
        let Ok(file) = file else { return };
        let mut w = std::io::BufWriter::new(file);
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) % 86400;
        let _ = writeln!(w, "# {title} 结束于 {:02}:{:02}:{:02}，{} 行", secs / 3600, secs / 60 % 60, secs % 60, rows.len());
        for (t, tag, v) in rows { let _ = writeln!(w, "{t},{tag},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2}", v[0], v[1], v[2], v[3], v[4], v[5]); }
    });
}

/// 记一行（没打开记录时什么都不做）
#[macro_export]
macro_rules! ylog { ($($t:tt)*) => { $crate::log::line(&format!($($t)*)) } }

#[cfg(test)]
mod tests {
    #[test]
    fn 天数换成年月日() {
        assert_eq!(super::date(0), (1970, 1, 1));
        assert_eq!(super::date(59), (1970, 3, 1));
        assert_eq!(super::date(19782), (2024, 2, 29));
        assert_eq!(super::date(20769), (2026, 11, 12));
    }
}
