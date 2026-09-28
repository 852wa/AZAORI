#![cfg(not(target_arch = "wasm32"))]

use azaori::{interpreter::Host, native_host::StdioHost};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn scratch() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/native-cli-tests")
        .join(format!(
            "{}-{stamp}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn cli(args: &[&str], input: &str, cwd: &Path) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_azaori"))
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn text(output: &[u8]) -> String {
    String::from_utf8(output.to_vec())
        .unwrap()
        .replace("\r\n", "\n")
}

#[test]
fn inline_code_help_and_argument_errors() {
    let cwd = scratch();
    let out = cli(&["-e", "1 足 2"], "", &cwd);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), "3\n");
    for args in [&[][..], &["--help"], &["-h"]] {
        let out = cli(args, "", &cwd);
        assert!(out.status.success());
        assert!(text(&out.stdout).contains("AZAORI (Rust)"));
    }
    for args in [
        &["-e"][..],
        &["--files-dir"],
        &["--unknown"],
        &["-e", "1", "-e", "2"],
        &["--check"],
        &["--case-jsonl", "-e", "1"],
    ] {
        assert_eq!(cli(args, "", &cwd).status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn option_like_source_and_directory_are_not_options() {
    let cwd = scratch();
    for source in ["--help", "--case-jsonl", "--check"] {
        let out = cli(&["-e", source], "", &cwd);
        assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
        assert!(!text(&out.stdout).contains("AZAORI (Rust)"));
    }
    let out = cli(&["-e", "1", "--files-dir", "--help"], "", &cwd);
    assert_eq!(text(&out.stdout), "1\n");
}

#[test]
fn files_bom_and_end_of_options() {
    let cwd = scratch();
    fs::write(cwd.join("sample.azo"), "\u{feff}2 掛 3").unwrap();
    fs::write(cwd.join("--sample.azo"), "7").unwrap();
    assert_eq!(text(&cli(&["sample.azo"], "", &cwd).stdout), "6\n");
    assert_eq!(text(&cli(&["--", "--sample.azo"], "", &cwd).stdout), "7\n");
    assert_eq!(
        cli(&["sample.azo", "--sample.azo"], "", &cwd).status.code(),
        Some(2)
    );
}

#[test]
fn check_and_rendering_options() {
    let cwd = scratch();
    let out = cli(&["-e", "1 割 0", "--check"], "", &cwd);
    assert!(out.status.success());
    assert!(out.stdout.is_empty());
    let out = cli(&["-e", "1 2 総足", "--kaki", "--tenkai"], "", &cwd);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let stdout = text(&out.stdout);
    assert!(stdout.contains("=== 書き下し文 ==="));
    assert!(stdout.contains("=== 展開"));
    assert!(stdout.ends_with("=== 結果 ===\n3\n"));
    let out = cli(&["-e", "1 割 0", "--check", "--kaki", "--tenkai"], "", &cwd);
    assert!(out.status.success());
    assert!(!text(&out.stdout).contains("=== 結果 ==="));
}

#[test]
fn error_location_and_eof() {
    let cwd = scratch();
    let out = cli(&["-e", "1 割 0"], "", &cwd);
    assert_eq!(out.status.code(), Some(1));
    let stderr = text(&out.stderr);
    assert!(stderr.contains("1行目"));
    assert!(stderr.contains("  1 割 0\n    ^^"));
    let out = cli(&["-e", "「入力？」 待"], "", &cwd);
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("入力が終了しました"));
}

#[test]
fn text_choice_retry_and_approval() {
    let cwd = scratch();
    let source = "「名前は？」 待\n「赤」 「青」 どれ\n「続ける？」 認";
    let out = cli(&["-e", source], "太郎\n0\n2\nyes\n", &cwd);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let stdout = text(&out.stdout);
    assert!(stdout.contains("名前は？ > 太郎\n"));
    assert!(stdout.contains("  1. 赤\n  2. 青\n"));
    assert!(stdout.contains("1〜2 の番号を入れてください"));
    assert!(stdout.contains("番号 > 青\n"));
    assert!(stdout.ends_with("続ける？（はい／いいえ） > 1\n"));
    let out = cli(&["-e", "「続ける？」 認 真 1 偽 終\n99"], "いいえ\n", &cwd);
    assert!(out.status.success());
    assert!(text(&out.stdout).contains("[終 1行]"));
    assert!(!text(&out.stdout).contains("99\n"));
}

#[test]
fn file_round_trip_and_runtime_relative_default() {
    let cwd = scratch();
    let files = cwd.join("data files");
    let source = "「第一行」 「第二行」 外書 \"sub/result.txt\"\n\"sub/result.txt\" 外読";
    let out = cli(
        &["-e", source, "--files-dir", files.to_str().unwrap()],
        "",
        &cwd,
    );
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        fs::read_to_string(files.join("sub/result.txt")).unwrap(),
        "第一行\n第二行\n"
    );
    assert_eq!(
        text(&out.stdout),
        "「第一行」 「第二行」\n「第一行」 「第二行」\n"
    );
    let out = cli(&["-e", "「保存」 外書 \"default.txt\""], "", &cwd);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        fs::read_to_string(cwd.join("files/default.txt")).unwrap(),
        "保存\n"
    );
}

#[test]
fn jsonl_accepts_multiple_cases_and_reports_invalid_json() {
    let cwd = scratch();
    let out = cli(
        &["--case-jsonl"],
        "{\"source\":\"1 足 2\"}\nnot-json\n{\"source\":\"4\"}\n",
        &cwd,
    );
    assert!(out.status.success());
    let lines: Vec<serde_json::Value> = text(&out.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["results"][0]["text"], "3");
    assert_eq!(lines[1]["error"]["kind"], "syntax");
    assert_eq!(lines[2]["results"][0]["text"], "4");
}

#[test]
fn sandbox_rejects_parent_absolute_and_drive_paths() {
    let cwd = scratch();
    let mut host = StdioHost {
        files: cwd.join("files"),
    };
    fs::write(cwd.join("outside.txt"), "unchanged").unwrap();
    let absolute = cwd.join("outside.txt");
    for name in [
        "../outside.txt",
        "..\\outside.txt",
        "/outside.txt",
        "C:\\outside.txt",
        "\\\\server\\share\\file",
        "C:outside.txt",
        absolute.to_str().unwrap(),
    ] {
        assert!(host.read_file(name).is_err(), "read {name}");
        assert!(host.write_file(name, "changed").is_err(), "write {name}");
    }
    assert_eq!(
        fs::read_to_string(cwd.join("outside.txt")).unwrap(),
        "unchanged"
    );
}

#[test]
fn file_limits_and_invalid_utf8_are_reported() {
    let cwd = scratch();
    let files = cwd.join("files");
    fs::create_dir_all(&files).unwrap();
    fs::write(files.join("invalid.txt"), [0xff, 0xfe]).unwrap();
    fs::File::create(files.join("too-large.txt"))
        .unwrap()
        .set_len(azaori::interpreter::MAX_FILE_BYTES as u64 + 1)
        .unwrap();
    let mut host = StdioHost { files };
    assert!(host.read_file("invalid.txt").is_err());
    assert!(host.read_file("missing.txt").is_err());
    assert!(host
        .read_file("too-large.txt")
        .unwrap_err()
        .message
        .contains("10MB"));
}

#[test]
fn outside_services_use_local_time_and_known_names() {
    let mut host = StdioHost { files: scratch() };
    let time = azaori::values::text_of(&host.outside("時刻").unwrap(), "test").unwrap();
    assert_eq!(time.len(), 8);
    assert_eq!(&time[2..3], ":");
    assert_eq!(&time[5..6], ":");
    let date = azaori::values::text_of(&host.outside("日付").unwrap(), "test").unwrap();
    assert_eq!(date.len(), 10);
    assert_eq!(&date[4..5], "-");
    let weekday = azaori::values::text_of(&host.outside("曜日").unwrap(), "test").unwrap();
    assert!(["月", "火", "水", "木", "金", "土", "日"].contains(&weekday.as_str()));
    let seconds = host.outside("秒").unwrap();
    let seconds = azaori::values::number_f64(&seconds.data[0], "test").unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    assert!((seconds - now).abs() < 5.0);
    assert!(host.outside("知らない名前").is_err());
}

#[cfg(windows)]
fn junction(link: &Path, target: &Path) -> bool {
    let result = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link.to_string_lossy().replace('/', "\\"))
        .arg(target.to_string_lossy().replace('/', "\\"))
        .output();
    match result {
        Ok(output) if output.status.success() => true,
        Ok(output) => {
            eprintln!(
                "junction setup: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            false
        }
        Err(error) => {
            eprintln!("junction setup: {error}");
            false
        }
    }
}

#[test]
fn sandbox_rejects_existing_and_dangling_links() {
    let cwd = scratch();
    let files = cwd.join("files");
    let outside = cwd.join("outside");
    fs::create_dir_all(&files).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("sentinel.txt"), "unchanged").unwrap();
    let link = files.join("escape");
    #[cfg(windows)]
    let linked = junction(&link, &outside);
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&outside, &link).is_ok();
    if !linked {
        eprintln!("SKIP: directory links are unavailable in this environment");
        return;
    }
    let mut host = StdioHost {
        files: files.clone(),
    };
    assert!(host.read_file("escape/sentinel.txt").is_err());
    assert!(host.write_file("escape/sentinel.txt", "changed").is_err());
    assert!(host.write_file("escape/new.txt", "changed").is_err());
    assert_eq!(
        fs::read_to_string(outside.join("sentinel.txt")).unwrap(),
        "unchanged"
    );
    assert!(!outside.join("new.txt").exists());

    let missing_target = outside.join("missing.txt");
    let file_link = files.join("dangling.txt");
    #[cfg(windows)]
    let linked_file = std::os::windows::fs::symlink_file(&missing_target, &file_link).is_ok();
    #[cfg(unix)]
    let linked_file = std::os::unix::fs::symlink(&missing_target, &file_link).is_ok();
    if linked_file {
        assert!(host.write_file("dangling.txt", "changed").is_err());
        assert!(!missing_target.exists());
    } else {
        eprintln!("SKIP: file symbolic links require additional OS privileges");
    }

    #[cfg(windows)]
    {
        let missing_directory = outside.join("missing-directory");
        if junction(&files.join("dangling-directory"), &missing_directory) {
            assert!(host
                .write_file("dangling-directory/new.txt", "changed")
                .is_err());
            assert!(!missing_directory.exists());
        } else {
            eprintln!("SKIP: dangling junction could not be created");
        }
    }
}
