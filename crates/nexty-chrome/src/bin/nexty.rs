//! `nexty` 可执行入口：启动浏览器窗口。

fn main() {
    if let Err(error) = nexty_chrome::run() {
        eprintln!("nexty: {error}");
        std::process::exit(1);
    }
}
