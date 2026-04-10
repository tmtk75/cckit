# Window Hover Popover Design

## Summary

`cckit app` のウィンドウ上で、セッション行に 500ms 以上 hover するとその行に対応する「最後の assistant 応答」をフロート popover で表示する。長めのやり取りをウィンドウ側のレイアウトを崩さずに確認できるようにする。

## Goals

- Mission Control / Classic テーマで、行への hover 停止 500ms 後に「最後の assistant 応答の末尾（最大 10 行）」をフロート popover 表示する。
- hover 行が変わったら即座に差し替え、hover が外れたら即座に消す。
- トランスクリプト I/O はバックグラウンドで行い、UI の描画スレッドを止めない。
- テスト可能なロジック（hit-test / 抽出処理）を `window.rs` 本体から切り出す。

## Non-Goals (YAGNI)

明示的にやらないこと:

- Notch テーマでの hover 対応（行レイアウトが特殊で hit-test コストに見合わない）。
- 応答以外の詳細メタデータの表示（cwd フルパス・token 使用率・last tool・session id 等）。
- 最後の 1 ターン以外の表示（会話履歴の複数ターン、user 発話、pretty-printed tool 呼び出し等）。
- popover 内でのスクロール・インタラクション（クリック／テキスト選択／コピー）。
- クリック or 右クリックで開く明示操作モード（hover のみ）。
- 他のウィンドウ要素（ヘッダ・空行）への hover 対応。
- Codex セッションのトランスクリプト対応（現状 `transcript_path` が Claude Code 前提）。
- トランスクリプト内容の永続キャッシュ（プロセス内メモリキャッシュのみ）。

## UX 仕様

### トリガー

- 行の bounding rect に入って **500ms 静止**すると popover が表示される。
- 行から外れた瞬間に popover が消える。
- マウスが別の行に移動した場合、古い popover は即座に消え、新しい行で 500ms カウントが始まる。
- ウィンドウがキーウィンドウでない状態でも hover は受ける（`NSTrackingAreaOptions.activeAlways`）。

### 表示位置

- popover は hover 中の行の**右外側**に、縦位置を行の上辺に合わせて固定表示する。
- 画面右端から 16pt 未満しか余白がない場合は、行の**左外側**に flip する。
- 画面下端からはみ出す場合は下端ぎりぎりにクランプする。
- 行スクロール等は現状ないので、hover 中にウィンドウがリサイズ／再描画されたら popover も再配置する。

### 見た目

- 幅固定 480pt、高さは内容に応じて可変（最大 10 行 + パディング）。
- 背景: `color_surface()` ベースに不透明度 0.95 + 角丸 8pt + 1pt ボーダー（`color_border()`）。
- 本文フォント: `monospacedSystemFontOfSize(FONT_SIZE, 0.0)`（既存の window 描画と揃える）。
- 本文色: `color_text()`。
- 末尾省略の場合は行末に `…` を付与する。
- popover は `NSWindow.level = .floating`、影あり、フォーカスを奪わない。

### 内容

- 対象セッションの `transcript_path` をパースし、末尾から見て **直近の assistant ターンを 1 つ**取り出す。
- そのテキストを改行単位で上から 10 行だけ残し、11 行目以降があれば 10 行目の末尾に `…` を付ける。
- 1 行が 480pt 幅に収まらない場合は通常の折り返し表示（NSString draw の自動折り返し）に任せる（折り返し後の行数で数える）。
- assistant ターンが存在しない／`transcript_path` が `None` or 読めない場合は popover を表示しない（静かに何もしない）。

## アーキテクチャ

### モジュール構成

```
src/monitor/
├── window.rs           既存。NSTrackingArea 登録と event forwarding を追加
├── window_hover.rs     新規。HoverTracker / HoverPopover / 純ロジック
└── mod.rs              pub mod window_hover; を追加
```

### 主要コンポーネント

#### `HoverTracker`（純ロジック、テスト可能）

```rust
pub struct HoverTracker {
    current: Option<HoverState>,
    version: u64,
}

struct HoverState {
    session_idx: usize,
    session_key: String,          // 比較用に session.key() を保持
    started_at: Instant,
    version: u64,                 // 非同期読み込みとの整合性判定に利用
}

pub enum HoverEvent {
    Entered(usize, String),       // (idx, session_key)
    Unchanged,                    // 同じ行 hover 継続
    Cleared,                      // 範囲外に出た or 行が消えた
}

impl HoverTracker {
    pub fn on_mouse(&mut self, hit: Option<HoverHit>) -> HoverEvent { ... }
    pub fn elapsed(&self) -> Option<Duration> { ... }
    pub fn current_version(&self) -> Option<u64> { ... }
    pub fn clear(&mut self) { ... }
}
```

#### `hit_test_mission_control` / `hit_test_classic`（純関数）

```rust
pub struct HoverHit {
    pub idx: usize,
    pub row_rect: NSRect, // popover 位置決めに使う
}

pub fn hit_test_mission_control(
    point: NSPoint,
    view_width: CGFloat,
    session_count: usize,
) -> Option<HoverHit>;

pub fn hit_test_classic(
    point: NSPoint,
    view_width: CGFloat,
    session_count: usize,
) -> Option<HoverHit>;
```

定数（`HEADER_HEIGHT` / `CARD_HEIGHT` / `CARD_SPACING` / `LEFT_PAD` 等）は `window.rs` で定義済みなので `pub(crate)` 化して共有する。

#### `extract_last_assistant_truncated`（純関数）

```rust
pub fn extract_last_assistant_truncated(
    record: &SessionRecord,
    max_lines: usize,
) -> Option<String>;
```

`src/history/loader.rs` の `SessionRecord` / `Role` を再利用する。`Role` が現状 `pub(crate)` なら `pub` に上げる。

#### `HoverPopover`（NSWindow ラッパ、main thread 専用）

```rust
pub struct HoverPopover {
    window: Option<Retained<NSWindow>>,
}

impl HoverPopover {
    pub fn show(&mut self, text: &str, anchor: NSRect, screen: &NSScreen);
    pub fn hide(&mut self);
    pub fn is_visible(&self) -> bool;
}
```

内部では borderless / non-activating な `NSPanel` を再利用し、`contentView` 内で `NSString.draw(in:withAttributes:)` により直接描画する（既存 window.rs と同じ CG 直描きスタイル）。

### データフロー

```
[NSView]  mouseEntered / mouseMoved / mouseExited
   │
   ▼
VIEW_CLASS のメソッドが NSEvent.locationInWindow → view 座標に変換
   │
   ▼
hit_test_<theme>(point, view_width, session_count) -> Option<HoverHit>
   │
   ▼
HoverTracker::on_mouse(hit) -> HoverEvent
   │
   ├ Entered(idx, key) 時:
   │     - 既存 popover を hide
   │     - 500ms 後に fire するワンショット `dispatch_after(DISPATCH_TIME_NOW + 500ms, main_queue, ...)`
   │       をセット（tracker の version を capture）
   │
   ├ Unchanged 時:
   │     - 何もしない（タイマ継続）
   │
   └ Cleared 時:
         - タイマ破棄
         - popover hide
         - tracker clear

[500ms timer fire]
   │
   ▼
tracker の current.version が timer 起動時の version と一致していたら:
   │
   ├ session_key から session を引き、transcript_path を取得
   │
   └ std::thread::spawn で background 読み込み
         │
         ├ parse_session_file() → SessionRecord
         ├ extract_last_assistant_truncated(record, 10)
         │
         └ 結果を `dispatch_async(dispatch_get_main_queue(), ...)` で main thread に post

[main thread]
   │
   ▼
 - tracker.current_version() == 読み込み開始時の version の場合のみ popover.show()
 - 一致しない場合は破棄
```

### 同時実行制御

- `HoverTracker` / `HoverPopover` は `Mutex<HoverRuntime>` でグローバル static として保持（他の window 状態と同様）。
- バックグラウンド読み込みは `version` による stale チェックで競合を捨てる（ロックは main thread 通過時のみ）。

## エラーハンドリング

| ケース | 挙動 |
|---|---|
| `transcript_path` が `None` | popover 表示しない |
| transcript ファイルが開けない／parse 失敗 | popover 表示しない（エラーログ warn のみ） |
| parse 結果に assistant ターンなし | popover 表示しない |
| hover 中に session list が更新され idx がずれる | `session_key` 比較で stale を検出し popover hide |
| popover が画面右端からはみ出す | 行の左外側に flip |
| popover が画面下端からはみ出す | 下端ぎりぎりでクランプ |
| window が非 key 状態 | `activeAlways` オプションで hover 可、popover 表示可 |
| Notch テーマが選択されている | hover trigger 自体を無効化（NSTrackingArea を張らない） |

## テスト戦略

- `src/monitor/window_hover.rs` の単体テスト:
    - `hit_test_mission_control`: header 内クリック / 1行目中央 / 行間隔のギャップ / 末尾行の直下 / 空リスト
    - `hit_test_classic`: 同様
    - `extract_last_assistant_truncated`: 10 行以内 / 11 行以上で `…` 付与 / assistant ターンなし / 末尾 user ターンだけの場合 / 既存 fixture `assistant_text_and_tool_use.jsonl` 再利用
    - `HoverTracker::on_mouse`: None→Some = Entered / Some→同一 Some = Unchanged / Some→別 Some = Entered 再発火 / Some→None = Cleared / 連続 Entered で version インクリメント
- `HoverPopover` と NSTrackingArea 登録は macOS GUI 層なので手動確認のみ（既存 `window.rs` 同様）。

## 依存・互換性

- 新しい crate 依存は追加しない。
- `objc2` / `objc2-app-kit` / `objc2-foundation` は既に使用中。
- `src/history/loader.rs::SessionRecord` / `Role` を module 外から参照可能にする必要がある場合のみ可視性を上げる。
- 既存ウィンドウの draw_rect / rebuild_view_* ロジックには触らない（popover は別ウィンドウ）。

## 将来の拡張候補（本スペック外）

- Notch テーマ対応。
- popover 内での応答全文表示とスクロール。
- クリック/右クリックで開くピン留めモード、Cmd+C でのコピー。
- 応答以外（cwd フルパス / model / token / last tool 等）の表示切替。
- Codex セッションのトランスクリプト対応。
