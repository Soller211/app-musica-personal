// Evita abrir una consola extra en Windows en modo release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    bluewhale_lib::run()
}
