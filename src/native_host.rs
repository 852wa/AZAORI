use crate::{
    error::AzaError,
    interpreter::{Event, Host, InputRequest, MAX_FILE_BYTES},
    values::{Arr, Elem},
};
use chrono::{Datelike, Local};
use std::{
    io::{self, Write},
    path::{Path, PathBuf},
};

pub struct StdioHost {
    pub files: PathBuf,
}
impl StdioHost {
    fn path(&self, name: &str) -> Result<PathBuf, AzaError> {
        crate::interpreter::validate_file_name(name)?;
        std::fs::create_dir_all(&self.files).map_err(io_error)?;
        let base = self.files.canonicalize().map_err(io_error)?;
        let full = base.join(name);
        // Resolve the nearest existing ancestor before allowing creation below it.
        let mut ancestor = full.as_path();
        loop {
            match std::fs::symlink_metadata(ancestor) {
                Ok(_) => break,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    ancestor = ancestor
                        .parent()
                        .ok_or_else(|| AzaError::new("ファイル名が不正です"))?;
                }
                Err(error) => return Err(io_error(error)),
            }
        }
        let resolved = ancestor.canonicalize().map_err(io_error)?;
        if !resolved.starts_with(&base) || full == base {
            return Err(AzaError::new("files フォルダの外は使えません"));
        }
        Ok(full)
    }
}
fn io_error(e: io::Error) -> AzaError {
    AzaError::new(format!("入出力エラー: {e}")).with_kind("host")
}
impl Host for StdioHost {
    fn input(&mut self, request: &InputRequest) -> Result<String, AzaError> {
        if request.kind == "choice" {
            println!("{}", request.prompt);
            for (i, item) in request.options.iter().enumerate() {
                println!("  {}. {}", i + 1, item);
            }
        }
        loop {
            let prompt = match request.kind.as_str() {
                "choice" => "番号".to_owned(),
                "yesno" => format!("{}（はい／いいえ）", request.prompt),
                _ => request.prompt.clone(),
            };
            print!("{prompt} > ");
            io::stdout().flush().map_err(io_error)?;
            let mut line = String::new();
            if io::stdin().read_line(&mut line).map_err(io_error)? == 0 {
                return Err(AzaError::new("入力が終了しました"));
            }
            let answer = line.trim_end_matches(['\r', '\n']);
            return Ok(match request.kind.as_str() {
                "choice" => match answer.trim().parse::<usize>() {
                    Ok(n) if n >= 1 && n <= request.options.len() => (n - 1).to_string(),
                    _ => {
                        println!("1〜{} の番号を入れてください", request.options.len());
                        continue;
                    }
                },
                "yesno" => {
                    if ["はい", "y", "yes", "1", "ok", "true"]
                        .contains(&answer.trim().to_lowercase().as_str())
                    {
                        "はい".into()
                    } else {
                        "いいえ".into()
                    }
                }
                _ => answer.into(),
            });
        }
    }
    fn outside(&mut self, name: &str) -> Result<Arr, AzaError> {
        let now = Local::now();
        Ok(match name {
            "時刻" => Arr::text(&now.format("%H:%M:%S").to_string()),
            "日付" => Arr::text(&now.format("%Y-%m-%d").to_string()),
            "曜日" => Arr::text(
                ["月", "火", "水", "木", "金", "土", "日"]
                    [now.weekday().num_days_from_monday() as usize],
            ),
            "秒" => Arr::scalar(Elem::Float(now.timestamp_millis() as f64 / 1000.0)),
            _ => return Err(AzaError::new(format!("外の世界の「{name}」は知りません"))),
        })
    }
    fn read_file(&mut self, name: &str) -> Result<String, AzaError> {
        let path = self.path(name)?;
        if std::fs::metadata(&path).map_err(io_error)?.len() > MAX_FILE_BYTES as u64 {
            return Err(AzaError::new("ファイルが大きすぎます（10MBまで）"));
        }
        std::fs::read_to_string(path).map_err(io_error)
    }
    fn write_file(&mut self, name: &str, text: &str) -> Result<(), AzaError> {
        let path = self.path(name)?;
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).map_err(io_error)?;
        }
        let checked = self.path(name)?;
        std::fs::write(checked, text).map_err(io_error)
    }
    fn event(&mut self, event: &Event) {
        match event.kind.as_str() {
            "結果" => println!("{}", event.text),
            "停" | "終" => println!("[{} {}行] {}", event.kind, event.line, event.text),
            _ => (),
        }
    }
}
pub fn default_files() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| Path::new(".").to_owned())
        .join("files")
}
