// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // 日志初始化移至 lib（见 `pilotdesk_lib::init_logging`）：除 stdout 外还会落盘到
    // `<配置根>/logs/app.log` —— release 构建没有可见终端，不落盘就无从取证。
    pilotdesk_lib::init_logging();
    pilotdesk_lib::run()
}
