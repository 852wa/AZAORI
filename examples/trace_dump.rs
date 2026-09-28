//! 中身の記録を確かめるための道具。  cargo run --release --example trace_dump -- "ソース"
use azaori::{interpreter::Interpreter, parser::parse};

fn main() {
    let source = std::env::args().nth(1).expect("ソースを渡してください");
    let mut interpreter = Interpreter::default();
    interpreter.trace = Some(Default::default());
    let outcome = interpreter.run(&parse(&source).expect("parse"));
    if let Err(error) = outcome {
        println!("エラー: {}", error.message);
    }
    for (line, value) in &interpreter.results {
        println!(
            "結果 {line}行: {}",
            azaori::values::format_value(value).replace('\n', " / ")
        );
    }
    let trace = interpreter.finish_trace().unwrap();
    for r in &trace.rows {
        println!(
            "{}行 {} {}回目 ({}行から) {:?}",
            r.line, r.place, r.call, r.from, r.names
        );
    }
}
