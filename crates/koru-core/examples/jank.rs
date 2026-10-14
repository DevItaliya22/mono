// SPDX-License-Identifier: MPL-2.0

//! Proves the stall watch writes what it claims. `KORU_JANK=1 cargo run -p koru-core --example jank`

fn main() {
    koru_core::timing::start();
    println!("floor: {:?}", koru_core::timing::stall_floor());
    {
        let _w = koru_core::timing::Watch::start("key press");
        std::thread::sleep(std::time::Duration::from_millis(120));
    }
    {
        let _w = koru_core::timing::Watch::start("redraw");
        std::thread::sleep(std::time::Duration::from_millis(70));
    }
    {
        let _w = koru_core::timing::Watch::start("pointer move");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    koru_core::timing::report_stalls();
}
