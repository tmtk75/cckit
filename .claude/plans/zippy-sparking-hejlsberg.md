# Plan: `cckit config` サブコマンドの追加

## Context

`~/.claude.json` は Claude Code が自動更新する設定ファイルで、4000行超のJSONになっている。
中身を確認したいとき `cat` や `jq` では見づらいため、cckit のサブコマンドとして見通しよく表示する機能を追加する。

## 変更対象

- `src/cli.rs` — Commands enum にバリアント追加、ハンドラ関数追加、run() にディスパッチ追加

## 実装内容

### 1. Commands enum に `Config` を追加 (L26付近)

```rust
/// Show ~/.claude.json contents in a readable format
Config {
    /// Key path to inspect (e.g., "projects", "tipsHistory")
    key: Option<String>,

    /// Show raw JSON output (pretty-printed)
    #[arg(long)]
    raw: bool,
},
```

### 2. `config_command(key, raw)` 関数を追加

- `~/.claude.json` を読み込み `serde_json::Value` にパース
- **引数なし**: トップレベルキーの一覧をテーブル形式で表示
  - キー名 / 型 (string, number, bool, object, array, null) / サイズまたは値のプレビュー
  - object/array は要素数、string は文字数、number/bool はそのまま値表示
- **`--raw`**: `serde_json::to_string_pretty` でJSON全体を出力
- **`<key>` 指定**: そのキーの値を表示
  - スカラー: 値をそのまま
  - object: キー一覧（サブキーのサマリー）
  - array: 先頭5件 + `... (残り N 件)` で省略表示
  - ネストキー対応: `projects` のようなトップレベルキー指定

### 3. `run()` にディスパッチ追加 (L1520付近の `Some(Commands::Status)` の前あたり)

```rust
Some(Commands::Config { key, raw }) => {
    config_command(key, raw);
}
```

## 既存パターンの再利用

- `dirs::home_dir()` でホーム取得（status_command と同パターン）
- `serde_json::Value` での動的JSON処理（prune_command, status_command で既に使用）
- `colored::Colorize` でカラー出力（全コマンドで共通）

## 検証方法

```bash
cargo build && ./target/debug/cckit config
cargo build && ./target/debug/cckit config tipsHistory
cargo build && ./target/debug/cckit config --raw | head -20
cargo build && ./target/debug/cckit config nonexistent_key
```
