# Plan: `cckit skill copy` サブコマンドの追加

## Context

cckit に skill のコピー機能を追加する。他のプロジェクトや `~/.claude` (global) にある skill を、現在のプロジェクトの `.claude/skills/` にコピーできるようにする。skill の一覧を fzf で fuzzy 検索して選択する UX を実現する。

## 対象ファイル

- `src/cli.rs` — メイン実装（enum追加、関数追加、`run()` にディスパッチ追加）

## 実装ステップ

### 1. `SkillCommands` enum と `Commands::Skill` を追加

`Commands` enum に `Session` と同じパターンで `Skill` variant を追加:

```rust
/// Manage skills across projects
Skill {
    #[command(subcommand)]
    command: SkillCommands,
},
```

`SkillCommands` enum を定義:

```rust
#[derive(Subcommand)]
enum SkillCommands {
    /// Copy a skill from another project to the current project
    Copy {
        #[arg(short, long, help = "Filter skills by name pattern")]
        filter: Option<String>,

        #[arg(long, help = "Copy from a specific project path")]
        from: Option<String>,

        #[arg(short, long, help = "Skill name (skip interactive selection)")]
        name: Option<String>,

        #[arg(long, help = "Overwrite existing skill without confirmation")]
        force: bool,
    },
}
```

### 2. `SkillSource` 構造体を追加

```rust
struct SkillSource {
    project_display: String,        // 表示用のプロジェクトパス (短縮済み)
    skill_dir: std::path::PathBuf,  // skill ディレクトリの絶対パス
    dir_name: String,               // ディレクトリ名 (コピー先のフォルダ名)
    info: SkillInfo,                // 既存の SkillInfo (name, description)
}
```

### 3. `scan_skills_with_paths()` を追加

既存の `scan_skills()` と同じロジックだがパス情報も返す版。`scan_skills()` 自体は変更せず、新しい関数として追加（既存コードへの影響を避ける）。

### 4. `collect_all_skills(from: Option<&str>)` を実装

- `from` 指定時: その1プロジェクトのみスキャン
- `from` なし: `~/.claude` (global) + `~/.claude.json` の全プロジェクトをスキャン
- plugin 内の skill は対象外
- 現在のプロジェクト（cwd）の skill は除外
- skill 名でソート

### 5. fzf 連携のインタラクティブ選択を実装

`select_skill_with_fzf()`:

1. `which fzf` で fzf の存在確認
2. fzf がある場合:
   - `"{skill_name}\t[{project_display}]\t{description}"` 形式の行をパイプで fzf に渡す
   - `--header`, `--delimiter='\t'` を設定
   - fzf の出力からskill名を取得してマッチ
3. fzf がない場合:
   - 番号付きリストを表示 → stdin で番号入力のフォールバック

### 6. `copy_dir_recursive()` を実装

`src` から `dst` にディレクトリを再帰コピー。`fs::create_dir_all` + `fs::copy` を使用。コピーしたファイル数を返す。

### 7. `skill_copy_command()` メイン関数を実装

処理フロー:
1. `collect_all_skills()` で全 skill を収集
2. `--filter` があればフィルタリング
3. `--name` 指定なら直接選択、なければ fzf/番号入力で選択
4. コピー先 `.claude/skills/{dir_name}` の衝突チェック
5. `--force` なければ確認プロンプト
6. `copy_dir_recursive()` でコピー実行
7. 結果を表示

### 8. `run()` にディスパッチを追加

```rust
Some(Commands::Skill { command }) => match command {
    SkillCommands::Copy { filter, from, name, force } => {
        skill_copy_command(filter, from, name, force);
    }
},
```

## UX イメージ

```
$ cckit skill copy
# → fzf が起動、全プロジェクトの skill を fuzzy 検索可能
# 選択後:
Copying skill 'authoring-claude-md' from ~/.claude (global) ...
Done! Copied 3 files to .claude/skills/authoring-claude-md

$ cckit skill copy --filter terraform
# → terraform を含む skill のみ fzf に表示

$ cckit skill copy --name managing-terraform-safely
# → 直接コピー（fzf をスキップ）
```

## 検証方法

1. `cargo build` でビルド確認
2. `cckit skill copy` で fzf が起動し、skill一覧が表示されること
3. skill を選択してコピーされることを確認
4. 同名 skill の再コピーで上書き確認プロンプトが出ること
5. `--force` で確認なしにコピーされること
6. `--filter` でフィルタリングされること
7. `--name` で直接コピーされること
8. fzf がない環境で番号入力にフォールバックすること
