# Plan: macOS ウィンドウアプリ (`cckit-window`)

## Context

既存の `cckit-app` はメニューバーアプリ（Cmd+Tab に出ない）。Cmd+Tab でフォーカスして、キーボードでセッションを選んで activate したい。新しいバイナリ `cckit-window` として追加する。ウィンドウを閉じたら終了。

## 方針

- `notification.rs` の NSWindow 生成パターンと `menubar.rs` のイベントループ・action handler パターンを組み合わせる
- NSTableView は objc2 で DataSource/Delegate の実装が複雑すぎるため、**NSStackView + カスタム row view** で実装
- キーボードイベントは **NSView サブクラスで `keyDown:` をオーバーライド**して処理
- セッション一覧は Timer で定期更新（`menubar.rs` と同じパターン）

## 変更ファイル

| ファイル | 変更内容 |
|---|---|
| `Cargo.toml` | `[[bin]]` に `cckit-window` 追加、objc2-app-kit features 追加 |
| `src/bin/cckit_window.rs` | 新規: エントリポイント |
| `src/monitor/window.rs` | 新規: ウィンドウアプリ本体 |
| `src/monitor/mod.rs` | `pub mod window;` 追加 |

## 実装詳細

### 1. `Cargo.toml`

```toml
[[bin]]
name = "cckit-window"
path = "src/bin/cckit_window.rs"
```

objc2-app-kit の features に追加:
- `"NSStackView"` - セッション一覧のレイアウト
- `"NSScrollView"` - スクロール対応
- `"NSClipView"` - NSScrollView に必要
- `"NSEvent"` - キーボードイベント
- `"NSWorkspace"` - (既にあるが確認)

### 2. `src/bin/cckit_window.rs`

```rust
fn main() {
    cckit::monitor::window::run_window_app().unwrap();
}
```

### 3. `src/monitor/window.rs` — 構造

#### アプリ初期化
- `NSApplicationActivationPolicy::Regular` — Dock に表示、Cmd+Tab に出る
- NSWindow 作成 (タイトルバー + 閉じるボタン付き、`NSWindowStyleMask::Titled | Closable`)
- タイトル: "cckit sessions"

#### ウィンドウ構造
```
NSWindow (480x400)
└─ NSScrollView
   └─ content view (カスタム NSView サブクラス)
      └─ セッション行を動的に描画
```

#### カスタム NSView サブクラス (`CCKitSessionListView`)
- `ClassBuilder` で動的に作成（`menubar.rs` L273-304 のパターン）
- オーバーライドするメソッド:
  - `acceptsFirstResponder` → `true` (キーボードイベント受信)
  - `keyDown:` → 上下キー、Enter、Esc 処理
  - `drawRect:` → セッション行の描画

#### グローバル状態 (static Mutex)
```rust
static SESSION_LIST: Mutex<Vec<Session>> = ...;
static SELECTED_INDEX: Mutex<usize> = ...;
```

#### キーボード操作
| キー | アクション |
|---|---|
| `↑` / `k` | 前のセッション |
| `↓` / `j` | 次のセッション |
| `Enter` | 選択セッションの activate (focus.rs 再利用) |
| `q` / `Esc` | アプリ終了 |
| `1-9` | 番号でジャンプ |

#### セッション行の描画 (`drawRect:`)
- 各セッション: 高さ 36px
- 左から: ステータスアイコン ("●"/"○"/"?"/"×"), プロジェクト名, ツール名, 更新時刻
- 選択行: 青い背景色 (`NSColor::selectedContentBackgroundColor`)
- 非選択行: 透明

#### フォーカス処理 (Enter)
- `focus::focus_ghostty_tab_by_tty()` → フォールバック `focus::focus_ghostty_tab()`
- `tui.rs` L253-282 と同じロジック

#### 定期更新
- `NSTimer` で 2 秒ごとにセッション更新 (`menubar.rs` L616-623 のパターン)
- `Storage::load()` で最新セッションを取得
- `setNeedsDisplay(true)` で再描画

#### ウィンドウ閉じ → 終了
- `NSWindow` の delegate で `windowWillClose:` を実装
- `NSApplication::terminate()` を呼ぶ

### 4. 再利用するコード

| 機能 | ファイル | 関数/パターン |
|---|---|---|
| セッションデータ | `session.rs` | `Session`, `SessionStatus`, `SessionStore` |
| ストレージ | `storage.rs` | `Storage::load()` |
| ターミナルフォーカス | `focus.rs` | `focus_ghostty_tab_by_tty()`, `focus_ghostty_tab()` |
| 動的ObjCクラス | `menubar.rs:273-304` | `ClassBuilder` + `Once` パターン |
| NSTimer更新 | `menubar.rs:616-623` | Timer + block パターン |
| 経過時間表示 | `tui.rs` の `format_elapsed()` 相当 |

## 検証

```bash
cargo build --bin cckit-window
./target/debug/cckit-window

# 確認:
# 1. Cmd+Tab に表示される
# 2. セッション一覧が表示される
# 3. ↑↓ で選択が移動する
# 4. Enter でターミナルにフォーカスが移る
# 5. ウィンドウを閉じるとアプリ終了
```
