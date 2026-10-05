//! A throwaway file server with real range support, for demos and manual testing:
//!
//!     cargo run -p idmx-core --example origin -- --size 500M --mbps 200 --port 8099
//!
//! then `idmx get http://<your-ip>:8099/anything.bin`.
#[path = "../tests/common/mod.rs"]
mod common;

#[tokio::main]
async fn main() {
    let mut size = 256u64 << 20;
    let mut per_conn_bps = 0u64;
    let mut port = 8099u16;
    let mut ranges = true;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--size" => size = idmx_core::util::parse_size(&args.next().unwrap()).expect("bad size"),
            // Per-connection ceiling in megabits/s — like a server that throttles each socket.
            "--mbps" => per_conn_bps = (args.next().unwrap().parse::<f64>().unwrap() * 125_000.0) as u64,
            "--no-ranges" => ranges = false,
            "--port" => port = args.next().unwrap().parse().unwrap(),
            _ => {}
        }
    }
    let o = common::start_on(
        common::Config { size, per_conn_bps, ranges, ..Default::default() },
        format!("0.0.0.0:{port}"),
    )
    .await;
    println!("origin on {} — {} bytes, {} B/s per connection", o.addr, size, per_conn_bps);
    std::future::pending::<()>().await;
}
