//! 链接期配置：主线程栈预留。
//!
//! 主线程跑完整布局管线：taffy 解算 + 片段装配的递归约 10 KB/帧 × 上限
//! 512 层 ≈ 5–6 MB（实测，debug 与 release 同量级），cascade / 场景下发 /
//! TextMap 另有小额递归。Windows PE 的主线程栈预留默认只有 1 MiB
//!（rustc 不默认设 /STACK），512 层页面会直接栈溢出拖垮进程——这是
//! 输入加固轮实机验收抓到的真实崩溃，故在此显式提高预留。渲染隔离线程
//! 与抓取线程不受影响（前者只光栅化平坦显示列表，后者解析是迭代式）。

fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("windows") && target.contains("msvc") {
        println!("cargo:rustc-link-arg-bins=/STACK:16777216");
    }
}
