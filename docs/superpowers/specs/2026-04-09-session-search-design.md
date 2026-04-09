# Session Search Subcommand Design

## Summary

`cckit session search <TERMS...>` サブコマンドを新設し、`~/.claude/projects/**/*.jsonl` に記録された Claude Code の過去セッション群を全文検索して、該当するセッションのワーキングディレクトリを思い出せるようにする。ユースケースは「あの話題について話した directory、どこだっけ？」を解決すること。

## Goals

- 全プロジェクトの jsonl を横断して、ユーザー発話に含まれる複数キーワード（AND）を高速検索する。
- 結果から「どこで何を話していたか」を思い出せるだけの文脈を表示する（日時・cwd・git branch・最初と最後のやり取り）。
- TUI プレビューで、選んだセッションの最初 3 ターン／最後 3 ターンを素早く確認できるようにする。
- TUI から `cd` できるパス出力を提供する。

## Non-Goals (YAGNI)

明示的にやらないこと:

- Codex セッション (`~/.codex/sessions/`) の検索。
- OR／フレーズ／正規表現検索（AND のみ）。
- 日付フィルタ、プロジェクトフィルタ。
- インクリメンタル検索 TUI（起動時引数で固定）。
- インデックス化／永続化キャッシュ（毎回スキャンで十分速い）。
- assistant 発話の検索対象化（プレビュー表示にのみ利用）。

## CLI Interface

```
cckit session search <TERMS...> [options]
```

### Arguments

- `<TERMS...>` — 1 個以上のキーワード。全て AND 結合され、case-insensitive で部分一致マッチされる。

### Options

- `-i, --interactive` — TUI モード（デフォルトはワンショット出力）。
- `--limit <N>` — 表示件数上限（デフォルト 20、ワンショット／TUI 共通）。
- `--json` — 機械処理用の JSON 出力（ワンショットのみ。`-i` と併用不可）。

### Examples

```bash
cckit session search osrm jni
cckit session search -i rust async
cckit session search --json --limit 5 terraform
```

## Data Source

- ディレクトリ: `~/.claude/projects/`（`dirs::home_dir().join(".claude/projects")` で解決）
- 対象ファイル: 配下の `*.jsonl`（`walkdir` で再帰列挙）
- **除外**: `subagents/` 配下の補助 jsonl（親セッションの重複）、`tool-results/` 配下のファイル
- ディレクトリが存在しない場合は "No sessions found" を出して exit 0

## JSONL Schema

各行は独立した JSON オブジェクトで、次のフィールドを使う:

```jsonc
{
  "type": "user" | "assistant" | "progress" | "file-history-snapshot" | ...,
  "cwd": "/Users/tomotaka/.ghq/github.com/kiicorp/varanus-osrm",
  "sessionId": "531497e8-...",
  "timestamp": "2026-03-12T06:12:48.919Z",
  "gitBranch": "master",
  "message": {
    "role": "user" | "assistant",
    "content": "テキスト" | [
      { "type": "text", "text": "..." },
      { "type": "tool_use", ... },
      { "type": "tool_result", ... }
    ]
  }
}
```

## Turn Extraction Rules

パース時のテキスト抽出は以下のルールで行う。

### User turn

`type == "user"` かつ以下のいずれか:

- `message.content` が文字列 → そのまま採用。
- `message.content` がリスト → `type == "text"` のブロックの `text` のみ連結して採用。`tool_result` のみのリストは**スキップ**（ツール実行結果の reply であってユーザー発話ではない）。

### Assistant turn

`type == "assistant"` のとき `message.content` のリスト要素のうち `type == "text"` のテキストのみ連結して採用。`tool_use` ブロックは除外。

### 空ターン

抽出後のテキストが空なら、その行はターンとして記録しない。

### セッション全体

- 1 ファイル内で `cwd` は一貫していると想定し、**最初に出現した `cwd`** を `SessionRecord.cwd` として採用する。
- `started_at` / `ended_at` は記録されたターンの timestamp の最小／最大。
- テキストターンが 1 つも無いセッションは結果から除外する。

## Data Model (Rust)

```rust
enum Role { User, Assistant }

struct Turn {
    role: Role,
    timestamp: DateTime<Utc>,
    text: String,
}

struct SessionRecord {
    session_id: String,
    cwd: PathBuf,
    git_branch: Option<String>,
    file_path: PathBuf,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    turns: Vec<Turn>,
}

struct Hit {
    session: SessionRecord,
    matched_turn_indices: Vec<usize>, // user ターンのインデックスのみ
}
```

## Search Logic

- クエリは `Vec<String>`。全部 `to_lowercase()` 済みのものを用意。
- セッションごとに、`Turn::role == User` のターンを順に見る。各ターンの `text.to_lowercase()` に**すべてのタームが含まれる**ならヒット扱い。
- ヒットした user ターンの index を `matched_turn_indices` に積む。1 つでもあればそのセッションは結果に含める。
- 結果は `session.ended_at` 降順（新しい順）でソート。
- `--limit` を超えたら切り詰める。

## Output: One-shot (Default)

```
2026-03-12 15:12  /Users/tomotaka/.ghq/github.com/kiicorp/varanus-osrm  (master)
  session: 531497e8-e342-47bb-86f7-93ea5c85e8a3  turns: 245  matches: 25
  first> linuxのamd64用の.soを作ってるよね。macos用のバイナリ作れるかな？
  last>  テスト通ったね、コミットしといて

2026-02-28 09:04  /Users/tomotaka/.ghq/github.com/kiicorp/vrp-hub  (feature/osrm)
  session: 0bcf0a06-0844-444c-94f9-ddb471053bf1  turns: 512  matches: 35
  first> osrm のルーティングを jni 経由で呼び出す件、調べてほしい
  last>  PR 上げといて

Found 2 sessions (scanned 1247 jsonl files in 0.8s)
```

- 各エントリは 4 行で構成する。
- 1 行目: `started_at (YYYY-MM-DD HH:MM)  cwd  (gitBranch)`。branch が無ければ括弧ごと省略。
- 2 行目: `session: <uuid>  turns: <n>  matches: <m>`。
- 3 行目: `first> <セッション全体の最初のユーザー発話>`。
- 4 行目: `last>  <セッション全体の最後のユーザー発話>`。
- 1 エントリごとに空行 1 つ区切り。
- 末尾に統計行（ヒット件数／スキャンした jsonl ファイル数／経過秒）。
- `first>` / `last>` はいずれも **セッション全体の最初／最後の user ターン**（マッチした turn ではない）。
- 各テキスト行は改行を空白にした上で長さ 120 文字でトランケートし末尾に `…` を付ける。

### `--json` Output

機械処理用の出力。テキスト系フィールドは**トランケートしない**（生のまま出す）。配列は ワンショットと同じ順序（`ended_at` 降順）で `--limit` 適用後の結果。

```json
[
  {
    "session_id": "531497e8-...",
    "cwd": "/Users/tomotaka/.ghq/github.com/kiicorp/varanus-osrm",
    "git_branch": "master",
    "started_at": "2026-03-12T06:12:48.919Z",
    "ended_at": "2026-03-12T15:12:00.000Z",
    "turns": 245,
    "matches": 25,
    "first_user_text": "linuxのamd64用の.soを作ってるよね。macos用のバイナリ作れるかな？",
    "last_user_text": "テスト通ったね、コミットしといて",
    "file_path": "/Users/tomotaka/.claude/projects/.../531497e8-....jsonl"
  }
]
```

## Output: TUI (`-i`)

ratatui ベースの 2 ペインレイアウト。

```
┌─ cckit session search ──────────────────────────────────┐
│ query> osrm jni                                          │
├───────────────── results ──┬──────── preview ───────────┤
│ > 2026-03-12 varanus-osrm  │ == first 3 turns ==         │
│   2026-02-28 vrp-hub       │ [user] linuxのamd64用の...  │
│   2026-02-20 vrp-sfn       │ [asst] はい、darwinターゲ.. │
│                            │ [user] じゃあ cross...      │
│                            │                             │
│                            │ == last 3 turns ==          │
│                            │ [user] テスト通った？       │
│                            │ [asst] はい、すべて pass    │
│                            │ [user] コミットしといて     │
├────────────────────────────┴─────────────────────────────┤
│ enter: print cwd  o: open in Finder  q: quit             │
└──────────────────────────────────────────────────────────┘
```

### Layout

- **上部**: `query> <terms>` を表示（読み取り専用、編集不可）。
- **左ペイン**: ヒット一覧。各行は `YYYY-MM-DD <basename of cwd>`。選択中はカーソル表示。
- **右ペイン**: 選択中セッションの「**最初 3 ターン**」と「**最後 3 ターン**」を表示。ターンは user / assistant の両方を含める。各ターンは `[user] ...` / `[asst] ...` の 1 行に折り畳む（長さは幅で折り返し可）。first+last が重複するほど短いセッションでは重複分を merge して 1 回だけ出す。
- **下部ステータスバー**: キーバインドの案内。

### Key Bindings

- `↓` / `j` / `↑` / `k` — カーソル移動
- `PageDown` / `PageUp` — ページ単位移動
- `g` / `G` — 先頭／末尾
- `Enter` — カーソル行のセッションの `cwd` を stdout に 1 行出力し、TUI を終了（exit 0）
- `o` — macOS で `open <cwd>` を実行（失敗時はステータスバーにエラー表示、終了はしない）
- `q` / `Esc` / `Ctrl-C` — キャンセル（exit 1、stdout には何も出さない）

### Usage with shell

```bash
cd "$(cckit session search -i osrm jni)"
```

## Module Structure

```
src/history/
├── mod.rs        # pub API: run_search(opts), pub な型の re-export
├── loader.rs     # walkdir + jsonl ストリームパース
├── search.rs     # AND マッチャ
├── format.rs     # ワンショット / JSON 出力フォーマット
└── tui.rs        # ratatui ベースの TUI
```

- `src/monitor/` には一切触らない（ライブセッション管理と分離する）。
- `src/cli.rs` の `Session` サブコマンドに `Search { terms, interactive, limit, json }` バリアントを追加し、ハンドラは `history::run_search()` に委譲する。
- `src/main.rs` / `src/lib.rs` に `pub mod history;` を追加する。

## Dependencies

- 新規: `walkdir = "2"`
- 既存利用: `serde`, `serde_json`, `chrono`, `clap`, `ratatui`, `crossterm`, `dirs`

## Error Handling

- `~/.claude/projects` が存在しない → 0 件扱い、`No sessions found` を stdout に出して exit 0。
- 壊れた JSONL 行 → その行だけスキップ、処理は続行。デバッグ用に `RUST_LOG=debug` 時のみ warn ログ。
- 個々のファイル I/O エラー → そのファイルだけスキップ、統計に「skipped N files」を表示（ワンショット末尾）。
- クエリが空（`cckit session search` のみ）→ clap で required 違反にさせる。

## Testing

- `src/history/loader.rs` ユニットテスト
  - fixture jsonl `tests/fixtures/history/` に以下 4 種を置く:
    1. user content が文字列のもの
    2. user content が `text` ブロックを含むリストのもの
    3. user content が `tool_result` のみのリストのもの（ユーザー発話として**拾われない**ことを検証）
    4. assistant content が `text` + `tool_use` 混在のもの（`text` のみ抽出されることを検証）
  - 壊れた行を含む jsonl で panic しないこと。
- `src/history/search.rs` ユニットテスト
  - in-memory で `SessionRecord` を組み立て、AND マッチ／ case-insensitive ／ 部分一致／ マッチ 0 の各ケースを検証。
- `src/history/format.rs` ユニットテスト
  - 与えた `Hit` からワンショット出力が期待通りの複数行文字列になること（`assert_eq!`）。
  - トランケート（120 文字）と改行置換の挙動。
- TUI はユニットテスト対象外（既存 `src/monitor/tui.rs` とポリシー統一）。
- CI: 既存の `cargo test` / `cargo clippy -- -D warnings` / `cargo fmt --check` で通ること。

## Out of Scope Notes

本 spec では `cckit session search` のみ実装対象とする。将来拡張として Codex セッション対応、フィルタオプション、インデックスキャッシュなどが考えられるが、今回は扱わない。
