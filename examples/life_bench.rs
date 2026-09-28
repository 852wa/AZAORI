use azaori::{ast::Node, interpreter::Interpreter, parser::parse};
use serde_json::{json, Value};
use std::{
    hint::black_box,
    io::{self, Read},
    time::Instant,
};

fn execute(program: &Node) -> Value {
    let mut interpreter = Interpreter::default();
    black_box(
        interpreter
            .run(program)
            .expect("benchmark program must run"),
    );
    json!(interpreter
        .events
        .iter()
        .map(|e| &e.text)
        .collect::<Vec<_>>())
}

fn main() {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input).unwrap();
    let request: Value = serde_json::from_str(&input).unwrap();
    let source = request["source"].as_str().unwrap();
    let iterations = request["iterations"].as_u64().unwrap() as usize;
    let samples = request["samples"].as_u64().unwrap() as usize;
    let warmup = request["warmup"].as_u64().unwrap() as usize;
    let include_parse = request["include_parse"].as_bool().unwrap();
    let parsed = parse(source).expect("benchmark program must parse");
    let expected = execute(&parsed);
    let once = |program: &Node| {
        let mut interpreter = Interpreter::default();
        black_box(interpreter.run(black_box(program)).unwrap());
        black_box(interpreter.events);
    };
    let timed = || {
        if include_parse {
            once(&parse(black_box(source)).unwrap())
        } else {
            once(&parsed)
        }
    };
    for _ in 0..warmup {
        timed();
    }
    let mut elapsed = Vec::with_capacity(samples);
    for _ in 0..samples {
        let start = Instant::now();
        for _ in 0..iterations {
            timed();
        }
        elapsed.push(start.elapsed().as_secs_f64() / iterations as f64);
    }
    println!(
        "{}",
        json!({"seconds_per_run":elapsed,"output":expected,"build":"release"})
    );
}
