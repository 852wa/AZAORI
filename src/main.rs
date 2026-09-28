use azaori::{
    error::AzaError,
    interpreter::Interpreter,
    native_host::{default_files, StdioHost},
};
use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
};
use unicode_width::UnicodeWidthStr;

const USAGE: &str = "AZAORI (Rust)\nazaori FILE.azo [--kaki] [--tenkai] [--check] [--emit-rust] [--files-dir DIR]\nazaori -e SOURCE [--kaki] [--tenkai] [--check] [--emit-rust] [--files-dir DIR]\nazaori --case-jsonl\nazaori --help";

enum Source {
    Inline(String),
    File(PathBuf),
}

#[derive(Default)]
struct Options {
    source: Option<Source>,
    files: Option<PathBuf>,
    kaki: bool,
    tenkai: bool,
    check: bool,
    emit_rust: bool,
    jsonl: bool,
    help: bool,
}

fn options(args: &[String]) -> Result<Options, String> {
    let mut options = Options {
        help: args.is_empty(),
        ..Options::default()
    };
    let mut args = args.iter();
    let mut positional = false;
    while let Some(arg) = args.next() {
        let source = if positional {
            Some(Source::File(arg.into()))
        } else {
            match arg.as_str() {
                "--help" | "-h" => {
                    options.help = true;
                    None
                }
                "--kaki" => {
                    options.kaki = true;
                    None
                }
                "--tenkai" => {
                    options.tenkai = true;
                    None
                }
                "--check" => {
                    options.check = true;
                    None
                }
                "--emit-rust" => {
                    options.emit_rust = true;
                    None
                }
                "--case-jsonl" => {
                    options.jsonl = true;
                    None
                }
                "--files-dir" => {
                    options.files = Some(PathBuf::from(
                        args.next()
                            .ok_or("--files-dir の後ろにフォルダを指定してください")?,
                    ));
                    None
                }
                "-e" => Some(Source::Inline(
                    args.next()
                        .ok_or("-e の後ろにプログラムを指定してください")?
                        .clone(),
                )),
                "--" => {
                    positional = true;
                    None
                }
                arg if arg.starts_with('-') => {
                    return Err(format!("知らないオプションです: {arg}"))
                }
                _ => Some(Source::File(arg.into())),
            }
        };
        if let Some(source) = source {
            if options.source.is_some() {
                return Err("プログラムはファイルか -e のどちらか1つだけ指定してください".into());
            }
            options.source = Some(source);
        }
    }
    if options.jsonl
        && (options.source.is_some()
            || options.files.is_some()
            || options.kaki
            || options.tenkai
            || options.check
            || options.emit_rust)
    {
        return Err("--case-jsonl は単独で指定してください".into());
    }
    if !options.help && !options.jsonl && options.source.is_none() {
        return Err("プログラムのファイルか -e SOURCE を指定してください".into());
    }
    Ok(options)
}

/// 構文木をたどる処理（書き下し・展開・実行）は再帰するので、WASM と同じ 64MB のスタックで動かす
const STACK_SIZE: usize = 64 << 20;

fn main() {
    // `azaori ... | head` のように読む側が先に閉じたら、エラーにせず静かに終える
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| info.payload().downcast_ref::<&str>().copied())
            .unwrap_or("");
        if message.contains("failed printing to stdout") && message.contains("Broken pipe") {
            std::process::exit(0);
        }
        default_hook(info);
    }));
    let worker = std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(real_main)
        .unwrap_or_else(|error| fail(&error.to_string()));
    if let Err(payload) = worker.join() {
        std::panic::resume_unwind(payload);
    }
}

fn real_main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let options = options(&args).unwrap_or_else(|error| fail(&error));
    if options.help {
        println!("{USAGE}");
        return;
    }
    if options.jsonl {
        if let Err(error) = run_jsonl() {
            if error.kind() != io::ErrorKind::BrokenPipe {
                fail(&error.to_string());
            }
        }
        return;
    }
    let source = match options.source.unwrap() {
        Source::Inline(source) => source,
        Source::File(path) => std::fs::read_to_string(&path)
            .unwrap_or_else(|error| fail(&format!("{}: {error}", path.display()))),
    };
    let source = source.trim_start_matches('\u{feff}');
    let result = (|| -> Result<(), AzaError> {
        let program = azaori::parser::parse(source)?;
        if options.emit_rust {
            print!("{}", azaori::codegen::emit_rust(&program)?);
            return Ok(());
        }
        if options.kaki {
            println!("=== 書き下し文 ===");
            for (line, text) in azaori::readings::read_program(&program) {
                println!("{line:>3}行  {text}");
            }
            println!();
        }
        if options.tenkai {
            println!("=== 展開（主語の省略を書き出したもの。「とは」の中の主語は これ） ===");
            for (line, text) in azaori::expand::expand_program(&program) {
                println!("{line:>3}行  {text}");
            }
            println!();
        }
        if !options.check {
            if options.kaki || options.tenkai {
                println!("=== 結果 ===");
            }
            Interpreter::new(Box::new(StdioHost {
                files: options.files.unwrap_or_else(default_files),
            }))
            .run(&program)?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        print_error(source, &error);
        std::process::exit(1);
    }
}

fn run_jsonl() -> io::Result<()> {
    let stdout = io::stdout();
    let mut output = io::BufWriter::new(stdout.lock());
    for line in io::stdin().lock().lines() {
        let line = line?;
        let result = match serde_json::from_str(&line) {
            Ok(case) => azaori::conformance::run_case(&case),
            Err(error) => {
                serde_json::json!({"error":{"kind":"syntax","message":format!("JSONを読めませんでした: {error}")}})
            }
        };
        writeln!(output, "{result}")?;
        output.flush()?;
    }
    Ok(())
}

fn print_error(source: &str, error: &AzaError) {
    eprintln!("エラー: {error}");
    let Some(number) = error.line else {
        return;
    };
    let lines = azaori::lexer::source_lines(source);
    let Some(line) = lines.get(number.saturating_sub(1)) else {
        return;
    };
    eprintln!("  {line}");
    if let Some(col) = error.col {
        let before: String = line.chars().take(col).collect();
        let end = error
            .end
            .and_then(|end| usize::try_from(end).ok())
            .unwrap_or(col + 1);
        let marked: String = line
            .chars()
            .skip(col)
            .take(end.saturating_sub(col).max(1))
            .collect();
        eprintln!(
            "  {}{}",
            " ".repeat(before.width()),
            "^".repeat(marked.width().max(1))
        );
    }
}

fn fail(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(2);
}
