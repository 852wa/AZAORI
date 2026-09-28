# AZAORI の実装構成

言語の主実装は Rust です。CLI と WebAssembly は同じパーサー・値・実行機械を使い、外部入出力だけを `Host` で切り替えます。

| モジュール | 役割 |
| --- | --- |
| `src/lexer.rs`, `src/parser.rs`, `src/ast.rs` | 字句解析、構文解析、構文木 |
| `src/values.rs`, `src/int.rs`, `src/primitives.rs` | 値、任意精度整数、基本演算 |
| `src/interpreter.rs`, `src/interpreter/machine.rs` | 評価、修飾子、継続、外部入出力の契約 |
| `src/readings.rs`, `src/expand.rs`, `src/analysis.rs` | 書き下し、省略の展開、型・効果の解析 |
| `src/native_host.rs`, `src/main.rs` | CLI、端末、ファイル |
| `src/wasm.rs`, `playground/` | WASM API とブラウザ画面 |
| `src/conformance.rs`, `conformance/`, `tests/` | 共通回帰ケースと手書きの仕様例 |

## 主な境界

- `parser::parse(&str) -> Result<Node, AzaError>` が構文木を返します。`parser::parse_with_elisions` は、あわせて省略した主語の場所（エディタで薄く見せる字）も返します。WASM の `analyze` はこれを `elisions` として渡します。
- 字句解析は括弧の外を NFKC で正規化しますが、トークンとエラーの位置（列）は元の行の文字で数えます。行の分け方は `lexer::source_lines` にそろえます（CLI のエラー表示とプレイグラウンドも同じ分け方です）。
- `Arr { shape: Vec<usize>, data: Data, kind: String }` が配列の値です。`Data` は整数・小数・文字を型別に共有格納し、一般の要素には `Elem` を使います。以前の `Vec<Elem>` 一律格納ではありません。
- `primitives::apply(name, left, right)` が基本演算を評価します。
- `Interpreter::run(&Node)` が各行の結果を返します。表示や入力などは `Host` の `event`、`input`、`outside`、`read_file`、`write_file`、`sleep`、`stopped` に委ねます。
- `readings::read_program` と `expand::expand_program` は構文木から表示用の文を作ります。
- `conformance/language.json` は演算の説明、読み、パレットなどのメタデータです。熟語の前の字の決まり（`modifier_rules`）、記号と字の対応（`math_ops`・`math_precedence`）もここにあり、パーサーと実行はここを読みます。表示を変更するときは `reference.md` と合わせて更新します。字の由来は `origins.md` にまとめます。

言語の結果を変更するときは、先に `conformance/spec.json` に期待値を書きます。固定された回帰ケースの期待値を変更する場合は、理由と変更前後の値を `conformance/superseded.json` に記録します。

## 上限とスタック

構文木をたどる処理（書き下し・展開・解析・実行の一部）は再帰するので、深さに上限を設けています。

- パーサー: （）の入れ子は100段まで、1つの式の記号の計算は1000個まで（`parser::MAX_NEST`・`MAX_MATH_OPS`）。
- 値: 入れ子は1000段まで、入れ子の中まで数えた要素は1000万個まで（`values::MAX_NESTING`・`MAX_TOTAL`）。各・毎・表・累 で結果を集めるときも数えます。
- CLI は 64MB のスタックのスレッドで動きます。WASM も `.cargo/config.toml` で 64MB にしています。テストのスレッドも `RUST_MIN_STACK` で同じ大きさにしています。上限の確認は `tests/limits.rs` にあります。
