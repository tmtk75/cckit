# Marketplace Subcommand Design

## Summary

`cckit marketplace` サブコマンドグループを新設し、独自マーケットプレイス（例: vrp-hub）のプラグイン一覧表示と構造検証を提供する。

## CLI Interface

```
cckit marketplace summary <path>   # マーケットプレイス内の全プラグイン詳細一覧
cckit marketplace doctor <path>    # プラグイン構造・整合性の検証
```

`<path>` はマーケットプレイスのルートディレクトリ（`plugins/` を含むディレクトリ）。

## Marketplace Directory Structure

```
<marketplace-root>/
  plugins/
    <plugin-name>/
      .claude-plugin/
        plugin.json          # required: { name, version, description }
      skills/
        <skill-name>/
          SKILL.md           # frontmatter: name, description
      hooks/
        hooks.json           # { hooks: {...} }
      mcp-servers/
        config.json          # { mcpServers: { <name>: { type, url|command } } }
```

## Module Structure

- `src/cli.rs` — `Marketplace` サブコマンドグループ定義 + ディスパッチ
- `src/marketplace.rs` — スキャン・検証ロジック

## `summary` Command

全プラグインの詳細一覧を表示する。

### Output Format

```
vrp-hub (3 plugins)

  vrp-dev v1.0.0 — Development support: code review, testing
    Skills:
      sample — "サンプルスキル"
    Hooks: (none)
    MCP Servers: (none)

  vrp-ops v1.0.0 — Operations: tenant management, deployment, monitoring
    Skills:
      prepare-tenant-test-data — "テナントテストデータ準備"
    Hooks: (none)
    MCP Servers:
      context7 (http)
      notion (http)

  vrp-data v1.0.0 — Data processing and analysis
    Skills: (none)
    Hooks: (none)
    MCP Servers: (none)
```

### Data Flow

1. `<path>/plugins/` を読み取り、各サブディレクトリをプラグイン候補とする
2. 各プラグインの `.claude-plugin/plugin.json` を読んで name/version/description 取得
3. `skills/*/SKILL.md` をスキャンし、frontmatter から name/description を取得
4. `hooks/hooks.json` を読んでhook数をカウント
5. `mcp-servers/config.json` を読んでサーバー名とタイプを取得
6. 整形して出力

## `doctor` Command

プラグインの構造と整合性を検証する。

### Validation Items

| # | Check | Severity | Description |
|---|-------|----------|-------------|
| 1 | plugin.json existence | error | `.claude-plugin/plugin.json` が存在するか |
| 2 | plugin.json syntax | error | 有効なJSONか |
| 3 | plugin.json required fields | error | `name`, `version`, `description` が存在するか |
| 4 | plugin.json name consistency | warning | `name` フィールドとディレクトリ名が一致するか |
| 5 | SKILL.md existence | warning | `skills/<name>/` にSKILL.mdが存在するか |
| 6 | SKILL.md frontmatter | warning | frontmatterに `name`, `description` があるか |
| 7 | hooks.json syntax | error | 有効なJSONか |
| 8 | mcp-servers config.json syntax | error | 有効なJSONか |
| 9 | mcp-servers required fields | warning | 各サーバーに `type` と `url` or `command` があるか |
| 10 | empty plugin | warning | skills, hooks, mcp-servers すべてが空のプラグイン |

### Output Format

既存の `cckit doctor` と同じスタイル:

```
cckit Marketplace Doctor: /path/to/vrp-hub

Checking vrp-dev/plugin.json ... ok
Checking vrp-dev/plugin.json fields ... ok
Checking vrp-dev/plugin.json name consistency ... ok
Checking vrp-dev/skills/sample/SKILL.md ... ok
Checking vrp-dev/skills/sample/SKILL.md frontmatter ... ok
Checking vrp-dev/hooks/hooks.json ... ok
...

Issues:
  ✗ vrp-foo/.claude-plugin/plugin.json not found

Warnings:
  ! vrp-data has no skills, hooks, or MCP servers
  ! vrp-dev/hooks/hooks.json has empty hooks

✓ 2 of 3 plugins passed all checks
```

## Reuse from Existing Code

- `parse_frontmatter()` — SKILL.md のfrontmatter解析（cli.rs に既存）
- colored crate — 出力のカラーリング

新規のスキャンロジックは `src/marketplace.rs` に独立して実装する。既存の `scan_plugins()` はinstalled_plugins.jsonベースで異なるため再利用しない。

## Exit Code

- `doctor`: issues（error）がある場合 exit 1、warnings のみなら exit 0
- `summary`: 常に exit 0
