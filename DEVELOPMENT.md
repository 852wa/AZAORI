# AZAORI の開発

AZAORI をビルドしたり、処理系を変更したりするときの手順です。言語の使い方は [README](README.md) と [リファレンス](reference.md) を見てください。

## CLI

Rust の安定版が必要です。リポジトリのルートで実行します。

```sh
cargo run --release -- -e "1 2 3 総足"
cargo run --release -- samples/02_平均.azo --kaki --tenkai
cargo run --release -- --help
```

`--check` は構文だけを確認します。`--kaki` は書き下し文、`--tenkai` は省略された主語の展開を表示します。`待`・`認`・`どれ` は端末で入力できます。

`外読`・`外書` の既定の保存先は、実行時のカレントディレクトリの `files/` です。`--files-dir DIR` で変更できます。`files/` にはサンプル用の3ファイルがあり、それ以外の実行時ファイルは Git の対象外です。

## プレイグラウンド

Rust、`wasm32-unknown-unknown`、wasm-pack、Node.js 24 以降を使用します。

```sh
npm ci
rustup target add wasm32-unknown-unknown
wasm-pack build --target web --out-dir playground/pkg --release --no-opt
npm run build:web-data
npm run serve
```

表示されたローカル URL を開きます。ブラウザ内の構文解析と実行は Rust/WASM で行います。`playground/data.json` は `npm run build:web-data` で作る生成物ですが、CI が差分を確かめるのでリポジトリに含めます。収録する初期ファイルは `playground/fixtures/` のみです。CLI の `files/` や実行時に作ったファイルは Web 配布データへ入りません。

対話入力（`待`・`認`・`どれ`）には `SharedArrayBuffer` を使うので、HTTPS と COOP/COEP ヘッダーが必要です。ヘッダーがない場所（GitHub Pages など）でも、対話入力を使わないプログラムは動きます。ローカルサーバー `tools/serve.mjs` は必要なヘッダーを設定します。`file://` では動きません。ブラウザ内で保存したプログラムとファイルは、そのオリジンの保存領域に置かれます。

## 検証

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
npm run test:wasm-api
```

ブラウザも検証する場合は、上記の手順で WASM と Web データをビルドし、別の端末で `npm run serve` を起動します。

```sh
npx playwright install chromium
npm run test:web
```

`CHROME_PATH` を設定すると、Playwright の Chromium の代わりに指定した Chrome を使用できます。サーバーが 8767 番以外で起動した場合は `AZAORI_URL` を設定してください。

共通検証データは `conformance/manifest.json` に列挙されています。固定した回帰ケースが53,094件、実行機械の追加ケースが21件、手書きの仕様例が78件です。ブラウザ検証では合計53,193件を照合します。結果、エラー文、位置、入出力イベント、配列の形と値を比較します。回帰ケースとの一致は、すべてのプログラムの正しさを保証するものではありません。

## 言語を変更するとき

1. `conformance/spec.json` に期待する結果またはエラーを手で書きます。
2. Rust 実装を変更し、`cargo test` を実行します。
3. 演算の説明やパレットが変わる場合は `conformance/language.json` と `reference.md` を更新します。字を足したときは `origins.md` にその字の由来も書きます。
4. `npm run build:web-data` で配布データを作り直し、必要なら WASM も再ビルドします。
5. ブラウザに関わる変更は `npm run test:web` まで実行します。

既存の回帰ケースの期待値を変える場合は、その理由と変更前後の値を `conformance/superseded.json` に記録します。

## 構成

| 場所 | 内容 |
| --- | --- |
| `src/` | 言語処理系、CLI、WASM API |
| `conformance/` | 仕様例、回帰ケース、言語メタデータ |
| `samples/` | AZAORI のサンプル |
| `playground/` | ブラウザ画面と Web 専用の初期ファイル |
| `files/` | CLI のサンプルファイルとローカル実行時の保存先 |
| `tests/`, `tools/` | 検証とビルド用のツール |

内部の構成は [ARCHITECTURE.md](ARCHITECTURE.md) にまとめています。演算別の性能は `cargo run --release --example ops_bench` で測れます。

## Web サイトへの配備

`main` へ push すると、GitHub Actions（`.github/workflows/deploy.yml`）が検証のあと WASM とプレイグラウンドをビルドし、Cloudflare Pages へ送ります。リポジトリの Secrets に `CLOUDFLARE_API_TOKEN` と `CLOUDFLARE_ACCOUNT_ID` が必要です。

対話入力（`待`・`認`・`どれ`）には COOP/COEP ヘッダー（`Cross-Origin-Opener-Policy: same-origin`、`Cross-Origin-Embedder-Policy: require-corp`）が要ります。Cloudflare Pages では `playground/_headers` で設定しています。GitHub Pages のようにヘッダーを設定できない場所では、対話入力を使うプログラムだけ動きません。
