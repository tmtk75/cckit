# `cckit skill promote` — プロジェクトスキルをuser scopeに昇格

## Context

プロジェクトごとに同じスキルをコピーして使っていたが、実は汎用的だったのでuser scope (`~/.claude/skills/`) に統合したい、というユースケース。既存の `skill copy` はプロジェクト→プロジェクトのコピーのみで、user scopeへの昇格・重複検出・クリーンアップ機能がない。

## 対象ファイル

- `src/cli.rs` — コマンド定義、実行ロジック追加

## 設計

### コマンドインターフェース

```bash
cckit skill promote [--name <name>] [--filter <pattern>] [--force] [--dry-run]
```

| フラグ | 説明 |
|--------|------|
| `--name` | スキル名を直接指定（fzfスキップ） |
| `--filter` | 名前パターンでフィルタ |
| `--force` | 確認プロンプトをスキップ |
| `--dry-run` | 実際のコピー/削除を行わず表示のみ |

### 処理フロー

```
1. 全プロジェクトからスキルを収集（globalは除外）
2. スキル名でグルーピング → 複数プロジェクトにあるものを上位表示
3. fzf / numbered listで選択
4. 同名スキルが複数プロジェクトにある場合:
   a. diff確認（SKILL.mdとディレクトリ構成の比較）
   b. 差分があれば、どのプロジェクト版を採用するか選択
   c. 差分がなければそのまま進行
5. npx/agentsインストール検出:
   a. symlink先が ~/.agents/ を指している → npxインストール
   b. ~/.agents/.skill-lock.json にエントリがある → npxインストール
   c. 検出した場合、アンインストールコマンドを表示
6. バックアップ作成:
   a. CWDに .claude/skill-promote-backup/<timestamp>/<project-hash>/<skill-dir>/ を作成
   b. 各プロジェクトから削除する前にコピー
7. ~/.claude/skills/ にコピー
8. 各プロジェクトのコピーを削除（確認付き、--forceでスキップ）
```

### 差分確認の実装

`diff -rq` をBashで実行して差分検出。差分がある場合:

```
Skill 'frontend-design' has differences across projects:

  [1] ~/proj-a/.claude/skills/frontend-design  (modified: 2026-03-10)
  [2] ~/proj-b/.claude/skills/frontend-design  (modified: 2026-03-15)
  [3] ~/.claude/skills/frontend-design (global) (modified: 2026-03-01)

Files differ:
  SKILL.md: [1] vs [2] differ
  data/config.json: only in [2]

Which version to promote? [1/2/3]:
```

同名スキルがglobalに既にある場合もdiff対象に含める。

### npx検出ロジック

```rust
fn is_npx_installed_skill(skill_dir: &Path) -> Option<NpxSkillInfo> {
    // 1. symlink先が ~/.agents/ 配下か
    if let Ok(target) = fs::read_link(skill_dir) {
        if target.to_string_lossy().contains(".agents/") {
            // ~/.agents/.skill-lock.json から情報取得
            return Some(...)
        }
    }
    // 2. .skill-lock.json にdir_nameのエントリがあるか
    None
}
```

検出時の出力:
```
Note: 'frontend-design' was installed via npx.
  To uninstall: claude /uninstall-skill frontend-design
```

### バックアップ

```
<cwd>/.claude/skill-promote-backup/
  20260316T1430/
    proj-a--frontend-design/
      SKILL.md
      ...
    proj-b--frontend-design/
      SKILL.md
      ...
```

- タイムスタンプ(YYYYMMDDTHHMMSS)でディレクトリ作成
- `<project-slug>--<skill-dir-name>` でサブディレクトリ
- 復元は手動 `cp -r` で可能（CLIに restore サブコマンドは不要 — YAGNI）

### 既存コードの再利用

| 関数 | 場所 | 用途 |
|------|------|------|
| `collect_all_skills()` | cli.rs:452 | スキル収集（globalを除外するよう修正して使う） |
| `scan_skills_with_paths()` | cli.rs:416 | パス付きスキルスキャン |
| `select_skill_fzf()` / `select_skill()` | cli.rs:496,621 | インタラクティブ選択 |
| `copy_dir_recursive()` | cli.rs:636 | ディレクトリコピー |
| `shorten_path()` | cli.rs | パス表示の短縮 |
| `parse_frontmatter()` | cli.rs | SKILL.md解析 |

### SkillCommands enum拡張

```rust
enum SkillCommands {
    Copy { ... },  // 既存
    /// Promote a project skill to user scope (~/.claude/skills/)
    Promote {
        #[arg(short, long)]
        filter: Option<String>,
        #[arg(short, long)]
        name: Option<String>,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        dry_run: bool,
    },
}
```

## 検証方法

```bash
# ビルド
cargo build

# テスト — dry-runで動作確認
cargo run -- skill promote --dry-run

# テスト — 特定スキルを指定
cargo run -- skill promote --name frontend-design --dry-run

# 既存テスト
cargo test
```
