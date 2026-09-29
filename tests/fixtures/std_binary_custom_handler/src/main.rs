//! Uses litestd next to std with litestd's panic handler compiled out.

fn main() {
    let start = litestd::time::Instant::now();
    println!("{:?}", start.elapsed());
}
