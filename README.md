# GPUI Conway's Game of Life

GPUI 向けの、大規模な盤面を扱う Conway's Game of Life です。

盤面を固定サイズのチャンクに分割し、セル数が増えても処理量と表示負荷を抑えられる構成を目指します。

- 大きな座標空間を扱う
- 生存セルのない領域にはメモリを割り当てない
- 世代更新で盤面全体を走査しない
- 画面外の領域を描画しない
- 変化していない領域の再描画を抑える
- ズームアウト時の描画量を制御する

詳細な設計判断は [DESIGN.md](./DESIGN.md) を参照してください。

## 目標

このプロジェクトの主な目標は次の通りです。

1. 大規模で疎な盤面を効率よく保持する
2. シミュレーションと UI を分離する
3. GPUI の描画キャッシュを活用し、更新範囲を抑える
4. パンとズームに対応した仮想ビューポートを提供する
5. シミュレーションと描画の更新頻度を分離する
6. 将来のビット並列な世代計算に対応できる構造にする

## 初期段階の対象外

初期段階では、次の機能を対象に含めません。

- Hashlife
- GPU compute による Life 計算
- 無限精度座標
- 完全な partial present / damage region 制御
- RLE など外部 Life フォーマットの完全対応
- 巨大パターンの永続化

まずは設計を単純に保ち、広い座標空間や多数の生存セルを扱っても UI が破綻しにくい構造を作ります。

## 構成

全体は次の責務に分けます。

```text
App
├── Toolbar
├── SimulationController
├── World
└── BoardViewport
    └── visible TileViews
```

### World

盤面状態を保持します。

```rust
type ChunkCoord = (i32, i32);

struct Chunk {
    rows: [u64; 64],
}

struct World {
    chunks: HashMap<ChunkCoord, Chunk>,
}
```

1 チャンクは 64 x 64 セルです。

各行を `u64` で保持するため、1 chunk の raw cell data は 512 bytes です。

空の chunk は保持しません。

そのため、座標空間そのものが大きくても、生存セルの存在する領域が疎であればメモリ使用量は小さく保てます。

## 描画

盤面全体を巨大な Element tree にしません。

画面内に見えているチャンクだけを `TileView` として生成します。

```rust
struct TileView {
    coord: ChunkCoord,
}
```

各 `TileView` は 4096 個の UI 要素を作らず、カスタム `Element` の `paint` 処理で生存セルを直接描画します。

```text
Viewport
  ↓
visible cell bounds
  ↓
visible chunk bounds
  ↓
TileView x N
```

典型的な画面では数十個程度の TileView だけが存在する状態を目標にします。

## 差分描画

世代更新時には、盤面全体ではなく変更されたチャンクを特定します。

```rust
struct GenerationDelta {
    changed_chunks: Vec<ChunkCoord>,
}
```

画面外のチャンクは UI の更新対象にしません。

画面内のチャンクも、前回描画した状態と現在の状態が同じなら再描画しません。

```text
changed by simulation
        AND
visible in viewport
        AND
current != presented
        ↓
notify TileView
```

これにより、シミュレーションが高速に進んでいる間に一時的に変化したものの、次の render 時点では元に戻っている tile の不要な再描画も避けられます。

## シミュレーション

次世代計算では、生存セルを含むチャンクとその周囲だけを候補にします。

```text
active chunks
    ↓
self + 8 neighbors
    ↓
candidate chunks
    ↓
next generation
```

空になった chunk は World から削除します。

最初の実装では、読みやすく正しさを検証しやすいアルゴリズムを使います。

`[u64; 64]` の表現を活かし、必要に応じてビット並列に更新できる構造を保ちます。

## 操作

初期 UI では次を提供します。

- Start / Pause
- Step
- Clear
- Randomize
- シミュレーション速度の調整
- マウスクリックやドラッグによるセル編集
- ドラッグによるパン
- ホイールやジェスチャーによるズーム

## カメラ

ビューポートは盤面座標と画面座標を分けて扱います。

```rust
struct Camera {
    origin_x: f64,
    origin_y: f64,
    cell_size: f32,
}
```

描画時にはビューポートの境界を盤面上のセル範囲に逆変換し、さらにチャンク範囲へ変換します。

```text
screen rect
   ↓ inverse camera transform
cell rect
   ↓ divide by 64
chunk rect
```

## 詳細度の制御

大きくズームアウトした状態で個々のセルを描画し続けると、セルが pixel より小さくなり無意味です。

そのため、セルの大きさに応じて描画を集約する詳細度制御（LOD）を導入します。

例:

```text
cell_size >= 2 px
    normal cell rendering

0.5 px <= cell_size < 2 px
    aggregated blocks

cell_size < 0.5 px
    chunk density rendering
```

LOD の詳細は DESIGN.md を参照してください。

## ディレクトリ構成案

```text
src/
├── main.rs
├── app.rs
├── world.rs
├── chunk.rs
├── simulation.rs
├── viewport.rs
├── tile.rs
└── camera.rs
```

責務の目安:

```text
main.rs
    GPUI application startup

app.rs
    top-level application state

world.rs
    sparse world storage

chunk.rs
    64 x 64 bitset representation

simulation.rs
    generation update

viewport.rs
    visible chunk selection and UI coordination

tile.rs
    custom GPUI drawing

camera.rs
    coordinate transforms
```

## 実装順序

実装は以下の順番を推奨します。

1. `Chunk`
2. `World`
3. Life rule の unit test
4. chunk boundary を跨ぐ Life rule
5. `Camera`
6. static board rendering
7. click による cell edit
8. pan / zoom
9. Step
10. Start / Pause
11. dirty chunk tracking
12. TileView 単位の redraw
13. simulation/render 分離
14. LOD
15. bit-parallel optimization

最初から最適化しすぎず、ただし後から全体構造を壊さず最適化できる境界を先に作る方針です。

## テスト

特に chunk 境界のテストを重視します。

最低限、次をテストします。

- still life
- oscillator
- glider
- chunk edge 上の oscillator
- chunk corner を跨ぐ glider
- chunk が空になった場合の削除
- negative coordinates
- viewport → cell coordinate conversion
- cell → chunk/local coordinate conversion

simulation logic は GPUI から独立させ、通常の Rust unit test だけで検証できるようにします。

## 性能設計

最も重要なのは、1セル当たりの処理を極端に速くすることよりも、そもそも処理対象を減らすことです。

```text
全 world を走査しない
全 cell を Element にしない
画面外を描かない
変化していない tile を描き直さない
pixel 未満の cell を個別描画しない
```

この方針を守った上で、必要になった箇所だけ bit operation やキャッシュ最適化を行います。
