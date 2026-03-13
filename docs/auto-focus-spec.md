# ウィンドウ前面表示（Auto-Focus）要求仕様

## 目的

Claudeがユーザー入力を必要とするとき、cckit appウィンドウを自動的に前面に出す。

## トリガーパターン（3種類）

| # | パターン | 状態遷移 | デフォルト遅延 | 設定キー |
|---|---------|---------|-------------|---------|
| 1 | **パーミッション要求** | → `AwaitingApproval` | **3秒** | 個別設定可 |
| 2 | **AskUserQuestion** | （検知方法未定） | **即座（0秒）** | 個別設定可 |
| 3 | **タスク完了（入力待ち）** | `Running`/`AwaitingApproval` → `WaitingInput` | **3秒** | 個別設定可 |

### 遅延の意図

- **パーミッション要求（3秒）**: 自動承認で通過するケースが多いため、待ってから表示
- **AskUserQuestion（0秒）**: Claudeが明示的に質問しており、即座に通知すべき
- **タスク完了（3秒）**: 短時間タスクで頻繁にウィンドウが前面に来るのを防ぐ

## 制御

- **粒度**: プロジェクト単位のON/OFF（現状維持）
- **遅延設定**: パターンごとに個別設定可能（設定ファイルで管理）
- **通知方法**: ウィンドウ前面表示のみ（サウンド・macOS通知は不要）

## 未実装事項（後回し）

- AskUserQuestionの検知方法（現在のHookイベントでは検知不可）
  - 将来的にClaude Code側のHook拡張、またはstdout監視等で対応検討

## 現在の実装状況（参考）

- `bring_window_to_front()`: `window.rs` L816-829 (`orderFrontRegardless` + `activateIgnoringOtherApps`)
- タイマー: 2秒ごとに `update_sessions_and_redraw()` で状態変化を検出
- AF無効化ストレージ: `~/.local/share/cckit/af_disabled.json`
- UIトグル: `f` キー（行選択時は個別、未選択時は一括）
