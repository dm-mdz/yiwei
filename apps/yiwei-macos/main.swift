// 一苇 · 入口
import Cocoa

let nsapp = NSApplication.shared
if let i = CommandLine.arguments.firstIndex(of: "--snapshot"), i + 1 < CommandLine.arguments.count {
    runSnapshots(CommandLine.arguments[i + 1])
    exit(0)
}
if CommandLine.arguments.contains("--selftest") {      // 不需要屏幕的自检
    let stitch = runStitchSelfTest(), clip = Share.clipSelfTest(), ocr = TextFinder.selfTest()
    exit(stitch && clip && ocr ? 0 : 1)
}
let delegate = AppDelegate()
nsapp.delegate = delegate
nsapp.setActivationPolicy(.accessory)
nsapp.run()
