//! Print weekday holidays of a calendar, one ISO date per line.
//! `cargo run -p fq-core --example dump_holidays -- TARGET 2000 2060`
use fq_core::time::{Calendar, Date};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cal: Calendar = args[1].parse().expect("calendar");
    let (y0, y1): (i32, i32) = (args[2].parse().expect("year"), args[3].parse().expect("year"));
    let from = Date::from_ymd(y0, 1, 1).expect("date");
    let to = Date::from_ymd(y1, 12, 31).expect("date");
    for d in cal.holidays(from, to) {
        println!("{d}");
    }
}
