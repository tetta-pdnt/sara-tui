# SARA-TUI

[SARA](https://github.com/cledouarec/sara) の要件・設計文書を端末で検索し、本文や関連文書をたどれる Rust 製の TUI。

## 起動する

Homebrew では、SARA CLI と Leaf もまとめてインストールできる。

```sh
brew install tetta-pdnt/tap/sara-cli tetta-pdnt/tap/sara-tui
```

`sara-tui` で起動する。`--project` に `sara.toml` があるディレクトリを指定できる。省略時は作業ディレクトリを使う。

```sh
sara-tui --project /path/to/project
```

編集には `$EDITOR` を使い、空か未設定の場合は `vim` を使う。

### Cargo からインストールする

このリポジトリのルートで Rust 1.88 以降を使ってインストールする。`sara` 0.12.3 と `leaf` を `PATH` に用意する。 [leaf のインストール手順](https://github.com/RivoLink/leaf#installation)

```sh
cargo install --locked --path .
```

Cargo のインストール先の `bin` を `PATH` に追加すると、`sara-tui` で起動できる。

ソースから直接起動する場合は、リポジトリのルートで次のコマンドを実行する。

```sh
cargo run --locked -- --project /path/to/project
```

Nix で Rust と Cargo を用意して実行する場合は、次のコマンドを使う。

```sh
nix --extra-experimental-features 'nix-command flakes' shell nixpkgs#cargo nixpkgs#rustc \
  --command cargo run --locked -- --project /path/to/project
```

## 操作する

画面には濃紺の背景と薄紫のアクセントを使う。`Items` は Item をタイプごとにまとめ、`sara schema` の定義順に表示する。各グループの右端には Item の件数が出る。`Body` は `leaf --inline` で Markdown を描画し、属性のある Item では末尾の `Fields` に属性を表示する。端末の幅が狭い場合は `Items` の下に `Body` が表示される。

| キー | 操作 |
| --- | --- |
| `?` | 画面中央に `Key Bindings` を開く。`?` または `Esc` で閉じる |
| `/`、`Enter` | `/` で画面の最下行に `Search` を開き、`Enter` で名前、ID、本文などを検索する。空のまま確定すると全件に戻る |
| `1` / `2` / `3` | `Items`、`Body`、`Related Items` にそれぞれ移る。`Tab` / `Shift+Tab` でも切り替えられる |
| `↑` / `↓` または `k` / `j` | 選択を移動する。文書ツリーでは選択した文書をその場で表示し、本文ではスクロールする |
| `gg` / `G` | `Items` の選択、または `Body` のスクロール位置を先頭 / 末尾に移す |
| `←` / `→` または `h` / `l` | 文書タイプのグループを閉じる・開く。開いているグループから文書へ、文書からグループへ移動できる |
| `Enter` / `o` / `Space` | サイドバーでグループを開閉する。文書を選択している場合は本文へ移る |
| `→` / `l` | サイドバーで文書を選択しているときに本文へ移る |
| `←` / `h` | 本文からサイドバーへ移る |
| 左クリック | 本文中の Web リンクを既定のブラウザで開く。Sara 文書や見出しへのリンクは本文ペインでたどる。関係図では下線付きの ID から対応する Item を開く |
| `Enter` / `o` | 関連文書の一覧で選んだリンクをたどる |
| `[` / `]` | 2 枠の `Body`、`Upstream`、`Downstream` タブを前後に切り替える。関係図は矢印キーか `hjkl` で移動する |
| `o` | 本文にフォーカスがあるとき、元の Markdown を `$EDITOR` で開く。エディターを終了すると保存内容を読み直す |
| `b` / `Esc` | `b` でリンクをたどる前の文書に戻る。`Esc` で検索入力を取り消す。検索確定後に `Esc` を押すと絞り込みを解除して文書ツリーに戻る |
| `[` / `]` または `PageUp` / `PageDown` | 文書ツリーの選択を画面単位で移す |
| `v` / `r` | `v` でモデルを検証し、`r` で検索結果を読み直す |
| `q` | 関係図では `Body` タブに戻る。通常画面では終了する |
