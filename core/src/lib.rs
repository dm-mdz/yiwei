//! 一苇 · 键鼠共享的核心（macOS、Windows、Linux 共用）
//!
//! 这里只放和系统无关的东西：两台电脑之间说什么话（`wire`）、几台电脑怎么摆（`layout`）、
//! 按键在不同系统之间怎么对应（`keys`）。监听按键、移动鼠标、读写剪贴板这些和系统打交道的事，
//! 由各系统自己的外壳去做。

pub mod clipflow;
pub mod control;
pub mod ffi;
pub mod files;
pub mod front;
pub mod input;
pub mod keys;
pub mod layout;
pub mod log;
pub mod net;
pub mod secure;
pub mod wire;

/// 把当前线程标成“用户正等着它”的：键鼠经过的几个线程都调一下，系统忙的时候也先排它们，鼠标才不会一顿一顿
pub fn urgent_thread() {
    #[cfg(target_os = "macos")]
    {
        extern "C" { fn pthread_set_qos_class_self_np(qos: u32, relative_priority: i32) -> i32; }
        unsafe { pthread_set_qos_class_self_np(0x21, 0); }                       // QOS_CLASS_USER_INTERACTIVE
    }
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" { fn GetCurrentThread() -> isize; fn SetThreadPriority(thread: isize, priority: i32) -> i32; }
        unsafe { SetThreadPriority(GetCurrentThread(), 2); }                     // THREAD_PRIORITY_HIGHEST
    }
}
