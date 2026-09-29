//! Uses litestd next to std, which must not compile.

fn main() {
    let start = litestd::time::Instant::now();
    println!("{:?}", start.elapsed());
}
