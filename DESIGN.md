# 設計書

## 1. 概要

このプロジェクトでは Conway's Game of Life を GPUI 上に実装します。

設計では、盤面が大きくなっても次の性質を保つことを重視します。

- メモリ使用量が盤面の論理サイズに比例しない
- シミュレーションの計算量が盤面全体の面積に比例しない
- 描画の計算量が World 全体に比例しない
- UI ツリーの大きさがセル数に比例しない
- 更新されていない表示領域を再描画しない
- ズーム倍率に応じて描画量を制御する

そのため、World、Simulation、Viewport、Rendering の責務を明確に分けます。

## 2. 設計方針

優先順位は次の通りです。

1. 処理対象そのものを減らす
2. dirty の範囲を明示的に管理する
3. UI と simulation の更新頻度を分離する
4. データ構造を描画都合だけで歪めない
5. hot path が明確になってから低レベル最適化する

active・visible・dirty の各集合を明示し、計算と描画の対象を初めから絞ります。

## 3. 盤面データの表現

### 3.1 疎なチャンクマップ

World はチャンク単位の疎なマップで表します。

```rust
pub type ChunkCoord = (i32, i32);

pub struct World {
    chunks: HashMap<ChunkCoord, Chunk>,
}
```

空の chunk は保存しません。

World の論理座標範囲は事前に確保しません。

### 3.2 チャンクサイズ

チャンクの初期サイズは 64 x 64 セルとします。

```rust
pub struct Chunk {
    rows: [u64; 64],
}
```

理由:

- 1 行を 1 machine word で扱える
- メモリ上の配置が単純になる
- XOR / AND / OR による差分判定が容易
- 将来のビット並列な世代計算に対応しやすい
- TileView の描画単位として適度な粒度になる
- チャンク境界をビットシフトで扱いやすい

64 が絶対的な最適値というわけではありません。

シミュレーション、差分管理、描画キャッシュの単位を揃えやすい点も利点です。

### 3.3 座標

World 上のセル座標には符号付き整数を使います。

```rust
pub struct CellCoord {
    pub x: i64,
    pub y: i64,
}
```

チャンク座標も符号付き整数で表します。

負数の除算では Rust の `/` と `%` をそのまま使うと期待する floor division と異なるため、`div_euclid` / `rem_euclid` を使用します。

```rust
let chunk_x = x.div_euclid(64);
let local_x = x.rem_euclid(64);
```

これにより `x = -1` も正しく

```text
chunk = -1
local = 63
```

となります。

## 4. チャンク API

Chunk の public API では、ビット表現の詳細を隠します。

```rust
impl Chunk {
    pub fn get(&self, x: u8, y: u8) -> bool;
    pub fn set(&mut self, x: u8, y: u8, alive: bool);
    pub fn toggle(&mut self, x: u8, y: u8);
    pub fn is_empty(&self) -> bool;
    pub fn population(&self) -> u32;
}
```

最適化したシミュレーションコードから必要に応じて行データへアクセスできるようにします。

```rust
pub(crate) fn rows(&self) -> &[u64; 64];
```

## 5. シミュレーション構成

### 5.1 盤面全体を走査しない

World の bounding rectangle 全体を走査してはいけません。

Life は live cells の周囲でしか変化しないため、現在存在する chunk とその周囲だけが候補です。

```text
active chunk
 ├─ self
 ├─ north
 ├─ south
 ├─ east
 ├─ west
 └─ diagonals
```

候補集合は `HashSet<ChunkCoord>` で構築できます。

```rust
for coord in world.active_chunks() {
    for neighbor in coord.moore_neighborhood() {
        candidates.insert(neighbor);
    }
}
```

### 5.2 世代更新の出力

simulation は新しい World state だけでなく差分も返します。

```rust
pub struct GenerationDelta {
    pub changed_chunks: Vec<ChunkCoord>,
    pub born_chunks: Vec<ChunkCoord>,
    pub removed_chunks: Vec<ChunkCoord>,
}
```

最低限必要なのは `changed_chunks` です。

`born_chunks` と `removed_chunks` は viewport cache 管理や診断に利用できます。

### 5.3 二重バッファによる更新

同一 generation 内で current state を書き換えながら計算しません。

```text
current
  ↓
compute
  ↓
next
```

とします。

候補 chunk の計算終了後に World を入れ替えます。

### 5.4 初期アルゴリズム

最初の実装では correctness と読みやすさを優先できます。

候補セルについて neighbor count を求めても構いません。

ただし API と storage は将来の bit-parallel implementation に置き換えやすい形を維持します。

### 5.5 ビット並列化

将来的には複数セルの neighbor count を machine word 単位で並列化します。

64-bit row representation はこれを可能にします。

ただし Life の neighbor count は単純な OR/AND だけでは済まないため、bit-sliced arithmetic など実装複雑度が上がります。

そのためプロファイル前に導入しません。

## 6. 差分管理

### 6.1 シミュレーション上の差分

世代更新によって内容が変化した chunk を simulation が返します。

```text
current_chunk != next_chunk
    ↓
changed chunk
```

64 rows の比較で済みます。

### 6.2 描画上の差分

simulation 上変化したことと、次の UI frame で描画が必要なことは同じではありません。

render frame の間に複数 generation が進む可能性があります。

例:

```text
frame N
A

generation N+1
B

generation N+2
A

frame N+1
A
```

この場合、画面上の状態は変化していません。

したがって visible TileView は最後に描画した状態を保持できます。

```rust
struct PresentedChunk {
    rows: [u64; 64],
}
```

render 前に

```text
presented != current
```

の場合だけ dirty とします。

これにより simulation delta の単純な union よりも redraw を抑えられます。

### 6.3 差分判定の流れ

dirty 判定は以下の順番で絞ります。

```text
changed by simulation?
        ↓ yes
currently visible?
        ↓ yes
different from presented?
        ↓ yes
notify TileView
```

## 7. GPUI の描画モデル

### 7.1 セルごとに要素を作らない

以下のような構造にはしません。

```text
Board
├── Cell
├── Cell
├── Cell
├── ...
└── Cell x 1,000,000
```

GPUI element tree の構築、layout、paint bookkeeping のコストが cell 数に比例してしまうためです。

### 7.2 TileView

viewport 内の chunk ごとに TileView を持ちます。

```rust
struct TileView {
    coord: ChunkCoord,
}
```

TileView は 64 x 64 個の child element を返すのではなく、custom Element で直接 paint します。

```text
TileView
   ↓
one custom Element
   ↓
paint live cells
```

### 7.3 チャンク単位で View を分ける理由

盤面全体を単一 View にすると、一部だけ変化しても盤面 element 全体が paint 対象になります。

逆に cell ごとに View を分割すると管理 overhead が大きすぎます。

chunk 単位はその中間です。

```text
too coarse:
    one board View

balanced:
    one View per visible chunk

too fine:
    one View per cell
```

この粒度によって、GPUI の View 単位の invalidation / paint reuse を利用します。

## 8. ビューポートの仮想化

Viewport は現在見えている world 範囲だけを UI に出します。

### 8.1 カメラ

```rust
pub struct Camera {
    pub origin_x: f64,
    pub origin_y: f64,
    pub cell_size: f32,
}
```

`origin_x`, `origin_y` は viewport 上の基準となる world coordinate です。

### 8.2 表示範囲

viewport size が `(width, height)` の場合、

```text
screen bounds
     ↓
inverse camera transform
world cell bounds
     ↓
chunk conversion
visible chunk bounds
```

を計算します。

少量の overscan を設けても構いません。

例:

```text
visible chunks + 1 chunk margin
```

これにより pan 時の View 生成破棄を少し減らせます。

### 8.3 TileView のライフサイクル

TileView は以下の集合だけ保持します。

```text
visible chunks
+
small overscan
```

viewport から大きく離れた TileView は破棄します。

World data は UI lifecycle と独立して保持されます。

## 9. パンとズーム

### 9.1 パン

pan は World を変更しません。

Camera の origin だけを変更します。

pan によって visible chunk set が変わった場合、

- 新しく見える TileView を作る
- 見えなくなった TileView を破棄する
- 引き続き見えている TileView は再利用する

という形にします。

### 9.2 ズーム

zoom の中心は mouse cursor の world position を維持するのが自然です。

```text
before zoom:
cursor → world position P

change scale

after zoom:
camera origin を補正し、
cursor → world position P
```

となるよう調整します。

## 10. 詳細度の制御

cell size が pixel より小さくなると、個別セルを paint する意味が薄れます。

LOD を導入します。

### Level 0: Cells

```text
cell_size >= 2px
```

各 live cell を rectangle として描画します。

### Level 1: Aggregated cells

```text
0.5px <= cell_size < 2px
```

2x2 や 4x4 の cell block を集約し、population に応じて描画します。

### Level 2: Chunk density

```text
cell_size < 0.5px
```

chunk population を使って tile 単位または小ブロック単位で描画します。

このとき個別セル座標の paint は行いません。

### Cached population

LOD 用に population を Chunk 内にキャッシュすることもできます。

```rust
pub struct Chunk {
    rows: [u64; 64],
    population: u16,
}
```

ただし `set` 頻度が高い場合は maintenance cost もあるため、実測して決めます。

## 11. シミュレーション頻度と描画頻度

simulation tick と UI frame を1対1に結び付けません。

```text
simulation:
    120 gen/sec

render:
    up to display refresh rate
```

simulation は World state を進めます。

UI は次の frame で最新 state だけを表示します。

これにより simulation が高速な場合も frame queue を溜めません。

重要なのは

```text
render every generation
```

ではなく

```text
render latest available generation
```

です。

## 12. 操作

### Cell editing

mouse click を screen → world → cell coordinate に変換します。

```text
mouse px
  ↓ camera inverse transform
world coordinate
  ↓ floor
cell coordinate
```

その後 cell coordinate を chunk/local coordinate に変換します。

### Drag editing

高速 drag では pointer event 間に cell が飛ぶため、前回 cell と今回 cell の間を line rasterization して補間することを検討します。

### Pan gesture

編集 drag と pan drag は明確に操作を分けます。

例:

```text
left drag:
    draw

middle drag:
    pan

wheel:
    zoom
```

または modifier key を利用します。

## 13. スレッド構成

初期版ではシミュレーションを UI スレッド上で実行します。

盤面が大きくなると、シミュレーションが UI フレームを阻害する可能性があります。

将来的には simulation worker を分離します。

```text
UI thread
   │ commands
   ▼
Simulation worker
   │ snapshots / deltas
   ▼
UI thread
```

World 全体を generation ごとに clone する設計は避けます。

候補:

- immutable chunk sharing
- changed chunk だけ transfer
- double-buffered world ownership
- Arc ベース snapshot

どれを採用するかは実測後に決定します。

## 14. 並行処理の境界

シミュレーション中の可変な World を UI が直接読む構成にはしません。

理想的には

```text
simulation owns mutable world

UI reads stable snapshot
```

です。

ただし初期実装では複雑化を避け、single-threaded に保ちます。

threading は性能上必要になってから導入します。

## 15. 空チャンクの削除

next generation が空になった chunk は保存しません。

```rust
if next_chunk.is_empty() {
    next_world.remove(coord);
}
```

これを忘れると、一度 Life が通過した領域が永久に HashMap に残り、長時間実行時に world が膨張します。

## 16. HashMap の検討事項

初期実装では標準の `HashMap` を使います。

非常に多数の chunk を扱うようになった場合、

- hash cost
- allocation
- cache locality

が問題になる可能性があります。

その場合は、次の方式を比較します。

- faster hasher
- slab / arena
- spatial hash
- sorted chunk vector
- BTreeMap
- region-level secondary index

適した方式はデータ分布によって異なるため、計測結果をもとに選びます。

## 17. 再描画の範囲

このプロジェクトで言う「再描画を小さくする」は二種類あります。

### Application-level invalidation

変更された TileView だけを dirty にすること。

これはアプリケーション設計で制御します。

### GPU / window presentation

OS compositor や GPU backend に対して画面の小矩形だけを物理的に present できるかは GPUI/backend の責務です。

本プロジェクトでは主に前者を最適化対象とします。

つまり、

```text
同じ window frame を生成するとしても、
clean TileView の paint work を再実行しない
```

ことを狙います。

## 18. 性能指標

「速いか」だけではなく、以下を計測します。

### Simulation

- generations/sec
- active chunks
- candidate chunks / generation
- changed chunks / generation
- live population
- simulation time / generation

### Rendering

- visible chunks
- dirty visible chunks
- TileView paint count / frame
- live cells painted / frame
- render frame time
- dropped frames

### Memory

- allocated chunks
- TileViews
- world memory estimate

この数値を debug overlay に出せるようにすると最適化判断が容易になります。

## 19. 受け入れ条件

### 機能

- Life rule が正しい
- negative coordinates が正しい
- chunk border を跨いで Life が進む
- pan / zoom できる
- cell edit できる
- Start / Pause / Step が動く

### 性能

- world の bounding rectangle が広がっても、空領域のためにメモリを確保しない
- viewport 外の chunk のために TileView を生成しない
- unchanged TileView に notify しない
- simulation が active region 周辺以外を走査しない
- zoom out 時に描画セル数が無制限に増えない

## 20. テスト方針

### チャンク

```text
set/get
toggle
empty
population
edge coordinates
```

### 座標

特に負座標を重点的に確認します。

```text
-1   → chunk -1, local 63
-64  → chunk -1, local 0
-65  → chunk -2, local 63
0    → chunk 0, local 0
63   → chunk 0, local 63
64   → chunk 1, local 0
```

### ライフゲームのルール

- block
- blinker
- beacon
- glider

### 境界

すべて chunk border 付近に配置します。

例:

```text
x = 62..66
y = 62..66
```

corner crossing も確認します。

### 描画

可能な範囲で UI 非依存部分を切り出して、

```text
viewport bounds
→ expected visible chunks
```

を unit test します。

## 21. 最適化の段階

最適化は以下の順番で行います。

### Stage 1

正しい sparse chunk implementation。

### Stage 2

active/candidate chunk tracking。

### Stage 3

visible TileView virtualization。

### Stage 4

dirty TileView invalidation。

### Stage 5

simulation/render decoupling。

### Stage 6

LOD。

### Stage 7

profiling。

### Stage 8

必要なら bit-parallel Life。

この順序にする理由は、後半の micro optimization より前半の「処理しない」最適化の方が通常は効果が大きいためです。

## 22. 守るべき不変条件

実装中は以下を崩さないようにします。

```text
World:
    empty chunks do not exist

Simulation:
    current generation is immutable during calculation

Viewport:
    only visible + overscan tiles have UI entities

Rendering:
    cell count does not determine element count

Dirty tracking:
    unchanged visible tiles are not notified

Coordinates:
    negative positions use Euclidean division
```

これらを守れば、World の規模を大きくしてもシステム全体の計算量が盤面の論理サイズへ直接引きずられにくくなります。

## 23. 全体構成

```text
                    ┌────────────────────┐
                    │ Simulation         │
                    │                    │
                    │ sparse World       │
                    │ HashMap<Coord,...> │
                    └─────────┬──────────┘
                              │
                       GenerationDelta
                              │
                              ▼
                    ┌────────────────────┐
                    │ Viewport           │
                    │                    │
                    │ Camera             │
                    │ visible chunks     │
                    └─────────┬──────────┘
                              │
             changed ∩ visible ∩ not-presented
                              │
                              ▼
                  ┌────────────────────────┐
                  │ visible TileViews      │
                  │                        │
                  │ one View / chunk       │
                  │ custom Element paint   │
                  └────────────────────────┘
```

この構成では、盤面全体を毎回描画するのではなく、必要な計算と描画だけを行います。

そもそも

```text
計算する必要のない場所を計算しない
表示する必要のない場所を View にしない
変化していない場所を paint しない
```

ことで、全体の処理量を抑えます。
