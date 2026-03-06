# 3バイナリ → 単一バイナリ統合プラン

## Context

現在 `cckit`, `cckit-app`, `cckit-window` の3バイナリが生成される。これを `cckit` 1つに統合し、CLIとしてもmacOSアプリとしても動くようにする。

## 方針

`app` と `window` をトップレベルのサブコマンドとして追加。macOS App Bundle起動時は `.app/Contents/MacOS/` パスを検出して自動でmenubarモードに入る。

```
cckit app              # menubar app を起動
cckit window           # window app を起動
cckit session menubar  # 既存コマンド（保持）
cckit ls / prune / ... # 既存CLI機能（変更なし）
```

App Bundle からの起動（引数なし）→ 実行パスに `.app/Contents/MacOS/` を検出 → menubar app モード。

## 変更ファイル一覧

### 1. `Cargo.toml` — `[[bin]]` セクション削除
- L8-14 の `cckit-app` と `cckit-window` の `[[bin]]` 定義を削除

### 2. `src/cli.rs` — Commands enum に `App` / `Window` 追加

```rust
/// Run as menubar app (macOS only)
App {
    #[arg(long, default_value = "500")]
    poll_interval: u64,
},

/// Run as window app (macOS only)
Window,
```

`run()` 関数に対応するマッチアーム追加（`#[cfg(target_os = "macos")]` で分岐）。

### 3. `src/main.rs` — App Bundle検出ロジック追加

```rust
fn main() {
    // macOS App Bundle から起動された場合（引数なし）→ menubar app
    if std::env::args().len() == 1 && is_in_app_bundle() {
        // run_menubar_app(500)
    } else {
        cckit::cli::run();
    }
}

fn is_in_app_bundle() -> bool {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().contains(".app/Contents/MacOS/"))
        .unwrap_or(false)
}
```

### 4. `src/bin/cckit_app.rs`, `src/bin/cckit_window.rs` — 削除

### 5. `scripts/macos/build_app.sh` — バイナリ名変更

`BIN_NAME="cckit-app"` → `BIN_NAME="cckit"`
`cargo build --release --bin "$BIN_NAME"` → `cargo build --release --bin cckit`

### 6. `macos/Info.plist` — 実行ファイル名変更

`<string>cckit-app</string>` → `<string>cckit</string>`

### 7. ドキュメント更新
- `README.md` — ビルド・実行コマンド例を更新
- `CLAUDE.md` — アーキテクチャセクション更新（2バイナリ→1バイナリ）

## 検証方法

```bash
cargo build --release --bins     # バイナリが cckit のみ生成されることを確認
cargo test                        # 既存テスト通過
cckit app                         # menubar 起動確認（macOS）
cckit window                      # window 起動確認（macOS）
cckit session menubar             # 既存コマンド動作確認
mise run build-app                # App Bundle ビルド確認
open dist/CCKit.app               # App Bundle 起動→menubar モード確認
```
