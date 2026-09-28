//! 演算ごとの速さを、プロセスの起動を含めずに測る。
//!     cargo run --release --example ops_bench
use azaori::{interpreter::Interpreter, parser::parse, values::format_value};
use std::{hint::black_box, time::Instant};

const CASES: &[&str] = &[
    "1000000 連 個",
    "1000000 連 足 1 個",
    "1000000 連 自掛 個",
    "1000000 連 総足",
    "1000000 連 累足 個",
    "1000000 連 自掛 総足",
    "1000000 連 各（足 1） 総足",
    "100 連 総足",
    "100 連 自掛 総足",
    "（1000 連 列 1000 1000） 転 個",
    "（1000 連 列 1000 1000） 前総足 個",
    "（1000 連 列 1000 1000） 毎移 -1 0 1 総足 個",
    "【f】とは 総足\n1000000 連 【f】",
    "100000 連 各（足 1） 個",
    "1000 連 表掛 （1000 連） 個",
];

fn main() {
    for source in CASES {
        let program = parse(source).expect("parse");
        let mut samples = Vec::with_capacity(9);
        let mut checksum = String::new();
        for run in 0..10 {
            let start = Instant::now();
            let mut interpreter = Interpreter::default();
            let results = black_box(interpreter.run(black_box(&program)).expect("run"));
            if let Some((_, value)) = results.last() {
                checksum = format_value(value);
            }
            let elapsed = start.elapsed().as_secs_f64();
            if run > 0 {
                samples.push(elapsed);
            }
        }
        samples.sort_by(f64::total_cmp);
        let median = samples[samples.len() / 2];
        println!(
            "{:8.2} ms [{:8.2}..{:8.2}]  {}  {}",
            median * 1000.0,
            samples[0] * 1000.0,
            samples[samples.len() - 1] * 1000.0,
            checksum,
            source.replace('\n', " / ")
        );
    }
}
