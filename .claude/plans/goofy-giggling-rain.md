# Unit Test 追加計画

## 現状
- `parse_frontmatter()` のみテストあり（src/cli.rs:1700-1739）
- 他のテストなし

## テスト追加対象

### 高優先度 - Pure Functions（副作用なし、テストしやすい）

#### src/cli.rs
- `normalize_git_url()` - git URL正規化
- `shorten_path()` - パス短縮（~置換）
- `is_path_disabled()` - globパターンマッチ
- `truncate_str()` - 文字列切り詰め
- `format_duration()` - 時間フォーマット
- `get_file_info()` - ファイル情報取得（存在チェック部分）

#### src/monitor/notification.rs
- `Position::parse()` - 位置指定パース
- `parse_hex_color()` - 16進数色パース

#### src/monitor/hook.rs
- `extract_tool_summary()` - ツール情報抽出
- `truncate()` - 文字列切り詰め（改行対応版）

#### src/monitor/session.rs
- `Session::project_name()` - プロジェクト名抽出
- `Session::short_cwd()` - パス短縮
- `SessionStatus` Display trait

#### src/monitor/setup.rs
- `has_cckit_hook()` - hook設定検索
- `create_hook_entry()` - hook JSON生成

#### src/monitor/tui.rs
- `App::select_next()` - 次選択
- `App::select_previous()` - 前選択

### 中優先度 - データ構造のシリアライズ
- `Session` - JSON serialize/deserialize
- `SessionStore` - JSON serialize/deserialize
- `HookInput` - JSON deserialize

## 実装方法

各モジュールに `#[cfg(test)] mod tests { }` ブロックを追加。

## ファイル変更

1. `src/cli.rs` - tests モジュール拡張
2. `src/monitor/notification.rs` - tests モジュール追加
3. `src/monitor/hook.rs` - tests モジュール追加
4. `src/monitor/session.rs` - tests モジュール追加
5. `src/monitor/setup.rs` - tests モジュール追加
6. `src/monitor/tui.rs` - tests モジュール追加

## 検証

```bash
cargo test
```
