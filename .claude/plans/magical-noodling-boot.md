# Plan: `cckit permissions --audit` 危険パターン検出機能

## Context

`cckit permissions` で全プロジェクトの allow/deny を一覧できるようになったが、数が多く、危険なパターンを目視で見つけるのが大変。`--audit` フラグで危険な allow のみをフィルター＋警告理由付きで表示する。

## 変更ファイル

- `src/cli.rs` のみ

## 実装

### 1. 危険パターン定義（定数配列）

```rust
struct RiskyPattern {
    pattern: &'static str,   // allow エントリの prefix match
    reason: &'static str,
}

const RISKY_PATTERNS: &[RiskyPattern] = &[
    // 任意コード実行
    RiskyPattern { pattern: "Bash(python:", reason: "arbitrary code execution via python" },
    RiskyPattern { pattern: "Bash(python3:", reason: "arbitrary code execution via python3" },
    RiskyPattern { pattern: "Bash(node:", reason: "arbitrary code execution via node" },
    RiskyPattern { pattern: "Bash(source:", reason: "arbitrary script sourcing" },
    // ファイル破壊
    RiskyPattern { pattern: "Bash(rm:", reason: "file deletion" },
    // Git 破壊操作
    RiskyPattern { pattern: "Bash(git push:", reason: "can force push and destroy remote history" },
    RiskyPattern { pattern: "Bash(git reset:", reason: "can discard uncommitted changes with --hard" },
    RiskyPattern { pattern: "Bash(git checkout:", reason: "can discard working tree changes" },
    // 広すぎるワイルドカード
    RiskyPattern { pattern: "Bash(gh:*)", reason: "allows ALL gh commands including destructive ones" },
    RiskyPattern { pattern: "Bash(terraform:*)", reason: "allows ALL terraform commands including apply/destroy" },
    RiskyPattern { pattern: "Bash(pnpm:*)", reason: "allows ALL pnpm commands including pnpm exec" },
    RiskyPattern { pattern: "Bash(cat:", reason: "can bypass Read deny rules to read sensitive files" },
    // インフラ操作
    RiskyPattern { pattern: "Bash(aws ", reason: "AWS CLI access (check scope)" },
    RiskyPattern { pattern: "Bash(AWS_PROFILE=", reason: "AWS CLI access with profile (check scope)" },
    // macOS 特殊
    RiskyPattern { pattern: "Bash(osascript", reason: "AppleScript can perform arbitrary macOS actions" },
    // Slack 送信 (read系は除外)
    RiskyPattern { pattern: "slack_send_message", reason: "can send Slack messages" },
];
```

### 2. `Permissions` サブコマンドに `--audit` フラグ追加

```rust
Permissions {
    #[arg(short, long)]
    filter: Option<String>,

    #[arg(long, help = "Show only risky allow patterns with warnings")]
    audit: bool,
},
```

### 3. `print_permissions` に audit モード追加

- `audit: true` のとき、allow エントリを `RISKY_PATTERNS` と prefix マッチ
- マッチしたエントリだけ表示、横に理由を黄色で表示
- deny は表示しない（audit は allow の危険性チェックなので）
- `-f` と `--audit` は併用可能（audit 結果をさらに絞る）

### 4. 出力イメージ

```
cckit permissions --audit

~/.ghq/github.com/kiicorp/vrp-hub/.claude/settings.local.json:
    Bash(rm:*)                    -- file deletion
    Bash(python:*)                -- arbitrary code execution via python
    Bash(git reset:*)             -- can discard uncommitted changes with --hard
    Bash(git push:*)              -- can force push and destroy remote history
    Bash(git checkout:*)          -- can discard working tree changes
    Bash(source:*)                -- arbitrary script sourcing

~/.ghq/github.com/kiicorp/tank-workspace/.claude/settings.local.json:
    Bash(gh:*)                    -- allows ALL gh commands including destructive ones
    Bash(cat:*)                   -- can bypass Read deny rules to read sensitive files
```

## 検証

```bash
cargo build
./target/debug/cckit permissions --audit
./target/debug/cckit permissions --audit -f 'rm'
cargo test
```
