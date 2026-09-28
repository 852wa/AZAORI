//! 入力パレットと文法メモが、すべての演算と熟語の前の字を含むこと（字を足したときの入れ忘れを防ぐ）。
use serde_json::Value;
use std::collections::BTreeSet;

fn language() -> Value {
    serde_json::from_str(include_str!("../conformance/language.json")).unwrap()
}

fn kanji_in(groups: &Value) -> BTreeSet<String> {
    groups
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|g| g["items"].as_array().unwrap())
        .map(|i| i["kanji"].as_str().unwrap().to_string())
        .collect()
}

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object().unwrap().keys().cloned().collect()
}

#[test]
fn palette_and_reference_cover_every_operation_and_modifier() {
    let d = language();
    let aliases = keys(&d["aliases"]);
    let mut known: BTreeSet<String> = keys(&d["primitives"])
        .difference(&aliases)
        .cloned()
        .collect();
    known.extend(keys(&d["modifiers"]));
    let palette = kanji_in(&d["palette"]["groups"]);
    let reference = kanji_in(&d["reference"]["groups"]);
    let missing: Vec<_> = known.difference(&palette).collect();
    assert!(missing.is_empty(), "パレットに入っていない字: {missing:?}");
    let missing: Vec<_> = known.difference(&reference).collect();
    assert!(missing.is_empty(), "文法メモに入っていない字: {missing:?}");
    let markdown = include_str!("../reference.md")
        .split_once("## 字の一覧")
        .expect("reference.md に字の一覧がありません")
        .1;
    let markdown_entries: BTreeSet<String> = markdown
        .lines()
        .filter_map(|line| {
            line.strip_prefix("| ")
                .and_then(|row| row.split('|').next())
        })
        .map(|cell| cell.trim().trim_start_matches('★').to_string())
        .collect();
    let missing: Vec<_> = known.difference(&markdown_entries).collect();
    assert!(
        missing.is_empty(),
        "reference.md に載っていない字: {missing:?}"
    );
    // パレットには、演算・熟語の前の字のほかに、記号・かなの言葉・流れの字・（ などがある
    let mut allowed = known.clone();
    allowed.extend(
        d["control"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_string()),
    );
    allowed.extend(keys(&d["keyword_guide"]));
    for extra in [
        "+", "-", "×", "÷", "^", "これ", "それ", "あれ", "ここ", "内", "（",
    ] {
        allowed.insert(extra.into());
    }
    let unknown: Vec<_> = palette.difference(&allowed).collect();
    assert!(unknown.is_empty(), "パレットに知らない字: {unknown:?}");
}

#[test]
fn every_palette_entry_has_readings_for_typing() {
    // ローマ字とかなの読みがないと、Tab で入力できない
    let d = language();
    for group in d["palette"]["groups"].as_array().unwrap() {
        assert!(
            group["tab"].as_str().is_some(),
            "{} にタブがありません",
            group["name"]
        );
        for item in group["items"].as_array().unwrap() {
            let kanji = item["kanji"].as_str().unwrap();
            if kanji.chars().all(|c| !c.is_alphanumeric()) {
                continue; // 記号
            }
            assert!(
                !item["keys"].as_array().unwrap().is_empty(),
                "{kanji} にローマ字の読みがありません"
            );
            assert!(
                item["short"].as_str().is_some_and(|s| !s.is_empty()),
                "{kanji} に、字の下に出す短い意味（short）がありません"
            );
        }
    }
}

#[test]
fn every_modifier_has_rules() {
    // 熟語の前の字を足したら、うしろにとる演算と、できる熟語の値の数も決める
    let d = language();
    for m in keys(&d["modifiers"]) {
        let rule = &d["modifier_rules"]["rules"][&m];
        assert!(
            matches!(
                rule["arity"].as_str(),
                Some("monad" | "dyad" | "same" | "both")
            ),
            "{m} に arity（できた熟語がとる値の数）がありません"
        );
        assert!(
            matches!(rule["base"].as_str(), None | Some("monad" | "dyad")),
            "{m} の base は monad か dyad です"
        );
    }
}

#[test]
fn every_kanji_has_an_origin() {
    // 字を足したら、origins.md にその字を当てた由来も書く
    let d = language();
    let mut known: BTreeSet<String> = keys(&d["primitives"])
        .difference(&keys(&d["aliases"]))
        .cloned()
        .collect();
    known.extend(keys(&d["modifiers"]));
    known.extend(
        d["control"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_string()),
    );
    known.extend(keys(&d["keyword_guide"]));
    let written: BTreeSet<String> = include_str!("../origins.md")
        .lines()
        .filter_map(|line| {
            line.strip_prefix("| ")
                .and_then(|row| row.split('|').next())
        })
        .map(|cell| cell.trim().to_string())
        .collect();
    let missing: Vec<_> = known.difference(&written).collect();
    assert!(missing.is_empty(), "origins.md に由来のない字: {missing:?}");
}
