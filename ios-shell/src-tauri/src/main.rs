// 桌面跑起来只是调试模板用的；这个壳真正的目标是 iOS。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    shipping_shell_lib::run()
}
