//! 目印付きの書き下しから目印を除くと、ふつうの書き下しと同じになるかを、共通テストの全ソースで確かめる。
//!     cargo run --release --example marked_readings_check
use azaori::{
    parser::parse,
    readings::{read_program, read_program_marked, strip_marks},
};
use serde_json::Value;

fn main() {
    let text = std::fs::read_to_string("conformance/cases.json").expect("cases.json");
    let corpus: Value = serde_json::from_str(&text).expect("json");
    let (mut checked, mut different) = (0, 0);
    for case in corpus["cases"].as_array().unwrap() {
        let Some(source) = case["source"].as_str() else {
            continue;
        };
        let Ok(program) = parse(source) else { continue };
        let plain = read_program(&program);
        let marked: Vec<_> = read_program_marked(&program)
            .into_iter()
            .map(|(line, text)| (line, strip_marks(&text)))
            .collect();
        checked += 1;
        if plain != marked {
            different += 1;
            if different <= 5 {
                println!("違う: {source:?}\n  ふつう: {plain:?}\n  目印除き: {marked:?}");
            }
        }
    }
    println!("{checked} 件を確かめ、違ったのは {different} 件");
}
