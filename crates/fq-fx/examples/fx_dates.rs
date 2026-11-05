//! Print spot and forward value dates: `cargo run -p fq-fx --example fx_dates -- EURUSD 2026-10-13 1M 3M`
use fq_core::time::{Date, Period};
use fq_fx::CurrencyPair;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let pair: CurrencyPair = args[1].parse().expect("pair");
    let trade: Date = args[2].parse().expect("date");
    println!("spot {}", pair.spot_date(trade).expect("spot"));
    for t in &args[3..] {
        let tenor: Period = t.parse().expect("tenor");
        println!("{t} {}", pair.forward_date(trade, tenor).expect("forward date"));
    }
}
