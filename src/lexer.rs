use crate::error::AzaError as Error;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token {
    pub kind: String,
    pub value: String,
    pub line: usize,
    pub col: usize,
    pub end: usize,
    pub quote: String,
}

/// col・end は正規化後の行の位置。元の行の位置に直して印を付ける
fn error(map: &[usize], message: impl Into<String>, line: usize, col: usize, end: usize) -> Error {
    let at = |k: usize| map.get(k).or(map.last()).copied().unwrap_or(k);
    let (col, end) = (at(col), at(end));
    Error::new(message)
        .at(line)
        .place(col, end.max(col + 1))
        .with_kind("syntax")
}

fn plain(text: &str) -> String {
    text.nfkc().collect::<String>().replace('−', "-")
}

fn is_quote(ch: char) -> bool {
    matches!(ch, '"' | '“' | '”' | '＂')
}

pub fn normalize_line(line: &str) -> String {
    normalize_line_mapped(line).0
}

/// 正規化の前後で位置がずれない文字のまとまり（基底の字＋結合する字・半角の濁点）に分ける
fn joins_previous(ch: char) -> bool {
    use unicode_normalization::char::canonical_combining_class;
    canonical_combining_class(ch) != 0
        || matches!(ch, '\u{ff9e}' | '\u{ff9f}' | '\u{3099}' | '\u{309a}')
}

/// 括弧の外を正規化した行と、正規化後の各文字が元の行の何文字目から来たか（最後に元の文字数）を返す
fn plain_mapped(raw: &[char], offset: usize, out: &mut String, map: &mut Vec<usize>) {
    let whole = plain(&raw.iter().collect::<String>());
    let mut pieces = String::new();
    let mut piece_map = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let mut j = i + 1;
        while j < raw.len() && joins_previous(raw[j]) {
            j += 1;
        }
        let text = plain(&raw[i..j].iter().collect::<String>());
        piece_map.extend(std::iter::repeat_n(offset + i, text.chars().count()));
        pieces.push_str(&text);
        i = j;
    }
    if pieces == whole {
        map.extend(piece_map);
    } else {
        // まとまりごとの正規化が行全体と違うとき（まれ）は、位置を比例で割り当てる
        let n = whole.chars().count().max(1);
        map.extend((0..whole.chars().count()).map(|k| offset + k * raw.len() / n));
    }
    out.push_str(&whole);
}

pub fn normalize_line_mapped(line: &str) -> (String, Vec<usize>) {
    let chars: Vec<char> = line.chars().collect();
    let (mut out, mut map, mut raw_start, mut i) = (String::new(), Vec::new(), 0, 0);
    while i < chars.len() {
        let ch = chars[i];
        if matches!(ch, '「' | '『' | '｢') || is_quote(ch) {
            let close = match ch {
                '「' => '」',
                '｢' => '｣',
                _ => '』',
            };
            let end = (i + 1..chars.len()).find(|&j| {
                if is_quote(ch) {
                    is_quote(chars[j])
                } else {
                    chars[j] == close
                }
            });
            plain_mapped(&chars[raw_start..i], raw_start, &mut out, &mut map);
            let Some(end) = end else {
                out.extend(chars[i..].iter());
                map.extend(i..chars.len());
                map.push(chars.len());
                return (out, map);
            };
            if is_quote(ch) || ch == '｢' {
                // 半角の ｢｣ は「」と同じ。中身は書いたとおりに残す
                let (open, close) = if ch == '｢' {
                    ('「', '」')
                } else {
                    ('"', '"')
                };
                out.push(open);
                out.extend(chars[i + 1..end].iter());
                out.push(close);
            } else {
                out.extend(chars[i..=end].iter());
            }
            map.extend(i..=end);
            i = end + 1;
            raw_start = i;
        } else {
            i += 1;
        }
    }
    plain_mapped(&chars[raw_start..], raw_start, &mut out, &mut map);
    map.push(chars.len());
    (out, map)
}

fn char_kind(ch: char) -> u8 {
    // Match the reference implementation's Unicode 15 CJK unified names.
    match ch {
        '\u{3040}'..='\u{309f}' => 1,
        '\u{30fb}' => 0, // ・（中黒）は名前の字ではない
        '\u{30a0}'..='\u{30ff}' => 2,
        '々'
        | '\u{3400}'..='\u{4dbf}'
        | '\u{4e00}'..='\u{9fff}'
        | '\u{20000}'..='\u{2a6df}'
        | '\u{2a700}'..='\u{2b739}'
        | '\u{2b740}'..='\u{2b81d}'
        | '\u{2b820}'..='\u{2cea1}'
        | '\u{2ceb0}'..='\u{2ebe0}'
        | '\u{30000}'..='\u{3134a}'
        | '\u{31350}'..='\u{323af}' => 3,
        _ if ch.is_ascii_alphabetic() || ch == '_' => 4,
        _ => 0,
    }
}

fn is_space(ch: char) -> bool {
    ch.is_whitespace() || matches!(ch, '\u{001c}'..='\u{001f}')
}

/// 行に分ける（エラーの表示もこの分け方に合わせる）
pub fn source_lines(source: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut chars = source.chars().peekable();
    while let Some(ch) = chars.next() {
        if matches!(
            ch,
            '\n' | '\r' | '\u{000b}' | '\u{000c}' | '\u{001c}'
                ..='\u{001e}' | '\u{0085}' | '\u{2028}' | '\u{2029}'
        ) {
            lines.push(std::mem::take(&mut line));
            if ch == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
        } else {
            line.push(ch);
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

pub fn tokenize(source: &str) -> Result<Vec<Token>, Error> {
    const WORDS: &[(&str, &str)] = &[
        ("とは", "TOWA"),
        ("これ", "KORE"),
        ("それ", "SORE"),
        ("あれ", "ARE"),
        ("ここ", "KOKO"),
        ("どれ", "FN"),
        ("そこ", "RESERVED"),
        ("どこ", "RESERVED"),
        ("は", "WA"),
    ];
    let mut tokens = Vec::new();
    let lines = source_lines(source);
    for (index, line) in lines.iter().enumerate() {
        let line_no = index + 1;
        let (text, map) = normalize_line_mapped(line);
        let chars: Vec<char> = text.chars().collect();
        let initial = tokens.len();
        let mut i = 0;
        while i < chars.len() {
            let ch = chars[i];
            if is_space(ch) {
                i += 1;
                continue;
            }
            if ch == '※' {
                break;
            }
            let start = i;
            let (kind, value, end, quote): (&str, String, usize, String);
            if matches!(ch, '「' | '『' | '｢') {
                let close = match ch {
                    '「' => '」',
                    '｢' => '｣',
                    _ => '』',
                };
                let j = (i + 1..chars.len())
                    .find(|&j| chars[j] == close)
                    .ok_or_else(|| {
                        error(
                            &map,
                            format!("{ch} に対応する {close} がありません"),
                            line_no,
                            i,
                            chars.len(),
                        )
                    })?;
                kind = "STR";
                value = chars[i + 1..j].iter().collect();
                end = j + 1;
                quote = format!("{ch}{close}");
            } else if ch == '【' {
                let j = (i + 1..chars.len())
                    .find(|&j| chars[j] == '】')
                    .ok_or_else(|| {
                        error(
                            &map,
                            "【 に対応する 】がありません",
                            line_no,
                            i,
                            chars.len(),
                        )
                    })?;
                kind = "NAME";
                value = chars[i + 1..j]
                    .iter()
                    .collect::<String>()
                    .trim_matches(is_space)
                    .to_owned();
                end = j + 1;
                quote = String::new();
                if value.is_empty() {
                    return Err(error(&map, "【】の中に名前がありません", line_no, i, end));
                }
            } else if ch == '。' || ch == '、' {
                kind = if ch == '。' { "PERIOD" } else { "COMMA" };
                value = ch.to_string();
                end = i + 1;
                quote = String::new();
            } else if ch == '(' || ch == ')' {
                kind = if ch == '(' { "LP" } else { "RP" };
                value = if ch == '(' { "（" } else { "）" }.to_owned();
                end = i + 1;
                quote = String::new();
            } else if ch.is_ascii_digit()
                || (ch == '-'
                    && chars.get(i + 1).is_some_and(char::is_ascii_digit)
                    && (i == 0 || is_space(chars[i - 1]) || chars[i - 1] == '('))
            {
                let mut j = i + 1;
                while chars.get(j).is_some_and(char::is_ascii_digit) {
                    j += 1;
                }
                if chars.get(j) == Some(&'.') && chars.get(j + 1).is_some_and(char::is_ascii_digit)
                {
                    j += 1;
                    while chars.get(j).is_some_and(char::is_ascii_digit) {
                        j += 1;
                    }
                }
                kind = "NUM";
                value = chars[i..j].iter().collect();
                end = j;
                quote = String::new();
            } else if "+-−×*÷/^".contains(ch) {
                kind = "OP";
                value = match ch {
                    '*' => "×".into(),
                    '/' => "÷".into(),
                    '−' => "-".into(),
                    _ => ch.to_string(),
                };
                end = i + 1;
                quote = String::new();
            } else if is_quote(ch) {
                let j = (i + 1..chars.len())
                    .find(|&j| is_quote(chars[j]))
                    .ok_or_else(|| {
                        error(
                            &map,
                            format!("{ch} に対応する閉じの \" がありません"),
                            line_no,
                            i,
                            chars.len(),
                        )
                    })?;
                kind = "EXT";
                value = chars[i + 1..j].iter().collect();
                end = j + 1;
                quote = String::new();
            } else if "<>〈〉《》〔〕{}[]".contains(ch) {
                return Err(error(
                    &map,
                    format!("「{ch}」は将来のために取ってある括弧です（まだ使えません）"),
                    line_no,
                    i,
                    i + 1,
                ));
            } else if char_kind(ch) == 3 {
                let mut j = i + 1;
                while chars.get(j).is_some_and(|&c| char_kind(c) == 3) {
                    j += 1;
                }
                kind = "FN";
                value = chars[i..j].iter().collect();
                end = j;
                quote = String::new();
            } else if char_kind(ch) == 1 {
                let mut j = i + 1;
                while chars.get(j).is_some_and(|&c| char_kind(c) == 1) {
                    j += 1;
                }
                let mut k = i;
                while k < j {
                    let remaining: String = chars[k..j].iter().collect();
                    let Some(&(word, word_kind)) =
                        WORDS.iter().find(|(word, _)| remaining.starts_with(word))
                    else {
                        return Err(error(&map, format!("「{remaining}」は使えません。ひらがなで書ける言葉は「は」「とは」「これ」「それ」「あれ」「ここ」「どれ」です"), line_no, k, j));
                    };
                    let next = k + word.chars().count();
                    if word_kind == "RESERVED" {
                        return Err(error(
                            &map,
                            format!("「{word}」は将来のために取ってある言葉です（まだ使えません）"),
                            line_no,
                            k,
                            next,
                        ));
                    }
                    tokens.push(Token {
                        kind: word_kind.to_owned(),
                        value: word.to_owned(),
                        line: line_no,
                        col: k,
                        end: next,
                        quote: String::new(),
                    });
                    k = next;
                }
                i = j;
                continue;
            } else if matches!(char_kind(ch), 2 | 4) {
                let mut j = i + 1;
                while chars.get(j).is_some_and(|&c| matches!(char_kind(c), 2 | 4)) {
                    j += 1;
                }
                let word: String = chars[i..j].iter().collect();
                return Err(error(
                    &map,
                    format!("名前は【】で囲んでください（例: 【{word}】）"),
                    line_no,
                    i,
                    j,
                ));
            } else {
                return Err(error(
                    &map,
                    format!("「{ch}」は使えない文字です"),
                    line_no,
                    i,
                    i + 1,
                ));
            }
            tokens.push(Token {
                kind: kind.to_owned(),
                value,
                line: line_no,
                col: start,
                end,
                quote,
            });
            i = end;
        }
        // 位置は元の行の文字で数える（エラーの印やエディタの表示が元の文字に合うように）
        let original = map[chars.len()];
        let at = |k: usize| map.get(k).copied().unwrap_or(original);
        for token in &mut tokens[initial..] {
            let col = at(token.col);
            token.end = at(token.end).max(col + usize::from(token.end > token.col));
            token.col = col;
        }
        if tokens.len() > initial {
            tokens.push(Token {
                kind: "NEWLINE".into(),
                value: "改行".into(),
                line: line_no,
                col: original,
                end: original,
                quote: String::new(),
            });
        }
    }
    tokens.push(Token {
        kind: "EOF".into(),
        value: "ファイルの終わり".into(),
        line: lines.len(),
        col: 0,
        end: 0,
        quote: String::new(),
    });
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::{normalize_line, tokenize};

    #[test]
    fn normalization_preserves_quoted_content() {
        assert_eq!(
            normalize_line("１２ （−３） 「？Ａ」 『Ｂ』 ＂Ｃ＂"),
            "12 (-3) 「？Ａ」 『Ｂ』 \"Ｃ\""
        );
    }

    #[test]
    fn unicode_15_kanji_are_one_token() {
        let tokens = tokenize("足\u{31350}\u{323af}").unwrap();
        assert_eq!(tokens[0].kind, "FN");
        assert_eq!(tokens[0].end, 3);
        assert!(tokenize("\u{2b73a}")
            .unwrap_err()
            .message
            .contains("使えない文字"));
    }

    #[test]
    fn control_whitespace_separates_negative_values() {
        let tokens = tokenize("1\u{001f}-2").unwrap();
        assert_eq!(tokens[1].kind, "NUM");
        assert_eq!(tokens[1].value, "-2");
        assert_eq!(
            tokenize("【\u{001f}名前\u{001f}】").unwrap()[0].value,
            "名前"
        );
    }

    #[test]
    fn positions_point_into_the_original_line() {
        // 半角の濁点は正規化で1字にまとまるが、位置は元の行で数える
        let tokens = tokenize("【ﾀﾞﾀﾞ】 足 1").unwrap();
        assert_eq!(tokens[0].value, "ダダ");
        assert_eq!((tokens[0].col, tokens[0].end), (0, 6));
        assert_eq!((tokens[1].col, tokens[1].end), (7, 8));
        // 1字が2字に広がる字でも、うしろの位置はずれない
        let tokens = tokenize("㍻ 足 1").unwrap();
        assert_eq!((tokens[0].col, tokens[0].end), (0, 1));
        assert_eq!(tokens[1].col, 2);
    }

    #[test]
    fn source_positions_count_characters() {
        let tokens = tokenize("「𠮷」 総足\r\n3").unwrap();
        assert_eq!((tokens[1].col, tokens[1].end), (4, 6));
        assert_eq!((tokens[3].line, tokens[3].col), (2, 0));
    }
}
