# 設計書

## 1. 概要

このプロジェクトでは Conway's Game of Life を GPUI 上に実装します。

盤面が大きくなっても、メモリ使用量や計算量が論理上の盤面全体に比例して増えない構成を目指します。UI ツリーの大きさはセル数に比例させず、表示されていても変化のない領域は再描画しません。ズーム倍率に応じて描画量も制御します。

World、Simulation、Viewport、Renderingの責務を分けます。

## 2. 設計方針

計算と描画の対象を絞り、変更範囲を明示して管理します。UI とシミュレーションの更新頻度を分け、描画だけを理由に盤面データの構造を複雑にしません。低レベルの最適化は、性能上のボトルネックを計測してから行います。

## 3. 盤面データの表現

### 3.1 疎なチャンクマップ

World はチャンク単位の疎なマップで表します。

```rust
pub type ChunkCoord = (i64, i64);

pub struct World {
    chunks: HashMap<ChunkCoord, Chunk>,
}
```

空のチャンクは保存しません。

World は固定サイズの座標領域を確保せず、必要なチャンクだけを保持します。

### 3.2 チャンクサイズ

チャンクの初期サイズは 64 x 64 セルとします。

```rust
pub struct Chunk {
    rows: [u64; 64],
}
```

各行を 64 ビット整数で表すと、メモリ配置が単純になり、XOR、AND、OR で差分を判定できます。この表現は将来のビット並列計算に利用でき、チャンク単位の描画や境界処理にも適しています。64 セルが常に最適とは限りません。初期設計では、シミュレーション、差分管理、描画キャッシュの単位を揃えられる点を重視します。

### 3.3 座標

World 上のセル座標とチャンク座標には符号付き 64 ビット整数を使います。World は疎なデータ構造ですが、座標の表現範囲は `i64` の範囲に限られます。

```rust
pub struct CellCoord {
    pub x: i64,
    pub y: i64,
}
```

セル座標をチャンク座標へ変換するときは `div_euclid(64)` を使います。したがって有効なチャンク座標は各軸で `i64::MIN.div_euclid(64)` から `i64::MAX.div_euclid(64)` までです。チャンクの隣接座標を計算するときは checked arithmetic を使い、この範囲を越える候補を作りません。セルの近傍座標も checked arithmetic で計算し、`i64` の範囲外は盤面外として扱います。

負の座標を正しいチャンクへ割り当てるため、Rust の `/` と `%` ではなく `div_euclid` と `rem_euclid` を使います。通常の除算は 0 方向へ丸めますが、この処理では床関数に相当する除算が必要です。

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

World の外接矩形全体は走査しません。

ライフゲームでは、生存セルの周囲だけが次の世代で変化します。そのため、生存セルを含むチャンクと隣接チャンクを計算候補にします。

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

Simulation は新しい World の状態と、更新されたチャンクの差分を返します。

```rust
pub struct GenerationDelta {
    pub changed_chunks: Vec<ChunkCoord>,
    pub born_chunks: Vec<ChunkCoord>,
    pub removed_chunks: Vec<ChunkCoord>,
}
```

最低限必要なのは `changed_chunks` です。

`born_chunks` と `removed_chunks` は Viewport のキャッシュ管理や診断に利用できます。

### 5.3 二重バッファによる更新

同じ世代の計算中に現在の状態を書き換えません。

```text
current
  ↓
compute
  ↓
next
```

とします。

候補チャンクの計算が終わったら、次世代の World に切り替えます。

### 5.4 初期アルゴリズム

最初の実装では、正しさを検証しやすい単純な方法を採用します。

候補セルごとに近傍の生存セル数を数える方法を使えます。

将来ビット並列の実装に置き換えられるよう、API とデータ構造の境界を保ちます。

### 5.5 ビット並列化

将来は複数セルの近傍数を一つの 64 ビット整数上で並列に計算します。

64 ビット単位の行表現は、この最適化の基礎になります。

ただし、近傍数の計算には単純な OR や AND 以外の演算も必要です。ビットスライス演算などを使うため、実装は複雑になります。

そのためプロファイル前に導入しません。

## 6. 差分管理

### 6.1 シミュレーション上の差分

Simulation は、世代更新で内容が変わったチャンクを返します。

```text
current_chunk != next_chunk
    ↓
changed chunk
```

64 行を比較すれば差分を判定できます。

### 6.2 描画上の差分

シミュレーションで変化したチャンクでも、次の UI フレームで再描画が必要とは限りません。

描画の合間に複数世代が進む場合があります。

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

この場合、画面上の状態は前回の描画から変わっていません。

表示中の TileView は、最後に描画したチャンクの状態を保持します。

```rust
struct PresentedChunk {
    rows: [u64; 64],
}
```

再描画の前に

```text
presented != current
```

の場合にだけ、そのチャンクを再描画します。

この比較により、世代更新で変化したチャンクを単純にまとめる方法より再描画を減らせます。

### 6.3 差分判定の流れ

差分判定では、次の順に対象を絞ります。

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

GPUI の要素ツリーの構築、レイアウト、描画管理にかかるコストがセル数に比例するためです。

### 7.2 TileView

ビューポート内のチャンクごとに TileView を設けます。

```rust
struct TileView {
    coord: ChunkCoord,
}
```

TileView はセルごとの子要素を作らず、GPUI Canvas の paint callback で生存セルを直接描きます。

GPUI は公式 crates.io の最新安定版を使います。2026-10-02 時点では `gpui = "=0.2.2"` とし、Cargo.lock をコミットして依存解決を固定します。ルート View はフレームごとに render されるため、チャンク表示には安定した Entity として保持する TileView を使い、表示範囲と overscan に含まれるタイルだけを保持します。パンで位置だけが変わった場合はルート View が配置を更新し、セル内容が変わった TileView だけを notify します。TileView は子要素としてセルを作らず、Canvas の paint callback で生存セル、4 x 4 ブロック、またはチャンク密度を描きます。GPUI の Entity、View、Element の役割は[公式 README](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md)と[Context の説明](https://github.com/zed-industries/zed/blob/main/crates/gpui/docs/contexts.md)に従います。

```text
TileView
   ↓
one custom Element
   ↓
paint live cells
```

### 7.3 チャンク単位で View を分ける理由

盤面全体を単一 View にすると、一部だけ変化しても盤面 element 全体が paint 対象になります。

セルごとに View を分けると、管理コストが大きくなります。

チャンク単位なら、盤面全体とセル単位の中間の粒度になります。

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

Viewport は現在表示している World の範囲だけを UI に含めます。

### 8.1 カメラ

```rust
pub struct Camera {
    origin_cell: CellCoord,
    offset_x: f64,
    offset_y: f64,
    cell_size: f64,
}
```

`origin_cell` は画面左上に対応するセル、`offset_x` と `offset_y` はセル内の位置（0 以上 1 未満）です。セル座標を浮動小数点数に変換しないため、`i64` の大きな座標でも隣接セルの精度を保てます。`cell_size` は画面上のセル幅をピクセルで表します。

### 8.2 表示範囲

ビューポートのサイズが `(width, height)` のとき、

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

画面上の領域は左上を含み、右端と下端を含まない範囲として扱います。セル範囲は包含端点で求め、`i64` の座標境界で切り詰めます。チャンク変換にはユークリッド除算を使い、overscan は有効なチャンク座標の範囲に収めます。`viewport::plan_visible_tiles` はこの範囲から表示計画を作ります。

表示範囲の周囲に少量の余白（overscan）を設ける方法もあります。

余白を 1 チャンク分設ける例です。

```text
visible chunks + 1 chunk margin
```

これにより、パン中に View を生成したり破棄したりする回数を減らせます。

### 8.3 表示タイル数の上限

画面に表示する TileView の総数には `viewport::MAX_RENDER_TILES = 4096` を設定し、overscan 分もこの上限に含めます。表示範囲に収まるチャンク数が上限を超える場合は、複数チャンクをまとめた表示タイルへ切り替えます。集約タイルは原点に揃えた 2 の累乗個のチャンクを単位とし、表示範囲と overscan を覆うタイル数が上限以下になる最小の集約幅を選びます。

この上限は描画単位だけに適用します。World の座標や保存済みチャンクを切り詰めず、集約表示中もセル編集とパンには元の座標を使います。

### 8.4 TileView のライフサイクル

TileView は以下の集合だけ保持します。

```text
visible chunks
+
small overscan
```

表示範囲から離れた TileView は破棄します。

World のデータは UI のライフサイクルとは独立して保持します。

## 9. パンとズーム

### 9.1 パン

パン操作では World を変更しません。

Camera の原点とセル内オフセットだけを更新します。画面上のドラッグ量をセル単位へ変換して原点を移動します。

パンによって表示中のチャンク集合が変わった場合、

- 新しく見える TileView を作る
- 見えなくなった TileView を破棄する
- 引き続き見えている TileView は再利用する

という形にします。

### 9.2 ズーム

ズーム時は、マウスカーソル下の World 座標を保ちます。

```text
before zoom:
cursor → world position P

change scale

after zoom:
camera origin を補正し、
cursor → world position P
```

`Camera::zoom_about` はズーム前のカーソル位置を World 座標で保持し、倍率変更後に原点とセル内オフセットを調整します。

## 10. 詳細度の制御

セルが 1 ピクセルより小さくなると、個別に描画しても状態を判別しにくくなります。

LOD を導入します。

### Level 0: Cells

```text
cell_size >= 2px
```

生存セルをそれぞれ矩形として描画します。

### Level 1: 集約表示

```text
0.5px <= cell_size < 2px
```

2 x 2 や 4 x 4 のセルをまとめ、生存数に応じて描画します。

### Level 2: チャンク密度

```text
cell_size < 0.5px
```

チャンク内の生存数を使い、タイルまたは小ブロック単位で描画します。`MAX_RENDER_TILES` を超える場合は、複数チャンクの密度をまとめた表示タイルを使います。集約タイルの密度は、範囲内の生存セル数を対象セル数（集約チャンク数 x 4096）で割った割合とします。座標範囲外のセルは死んだセルとして数えます。

このとき個別セル座標の paint は行いません。

### 生存セル数のキャッシュ

LOD 用にチャンク内の生存セル数をキャッシュする方法もあります。

```rust
pub struct Chunk {
    rows: [u64; 64],
    population: u16,
}
```

ただし、`set` を頻繁に呼ぶと更新コストも増えます。実測してから導入を決めます。

## 11. シミュレーション頻度と描画頻度

シミュレーションの更新と UI の描画を 1 対 1 で結び付けません。

```text
simulation:
    120 gen/sec

render:
    up to display refresh rate
```

SimulationがWorldの状態を更新します。

シミュレーションが描画より速く進んでも、未処理のフレームはたまりません。UIは次のフレームで利用できる最新の世代を表示します。

## 12. 操作

### セルの編集

マウスクリックの画面座標を World 座標に変換し、さらにセル座標へ変換します。

```text
mouse px
  ↓ camera inverse transform
world coordinate
  ↓ floor
cell coordinate
```

続けて、セル座標をチャンク内の座標に変換します。

### ドラッグ編集

ドラッグが速いと、ポインターイベントの間に複数のセルを通り過ぎることがあります。前回と今回のセルを結ぶ線をラスタライズし、その間のセルも同じ生死状態に設定します。ドラッグ開始時に決めた状態を保つため、イベントの間隔や重複によってセルが反転しません。

### パン操作

セル編集とパンのドラッグ操作は、別の入力として扱います。左ボタンはセル編集、中ボタンはパンです。ホイールはカーソル位置の World 座標を保ってズームし、セル表示倍率は 0.0625〜128 ピクセルに制限します。

初期版では、操作を次のように割り当てます。シミュレーション速度は 1〜120 世代/秒の範囲で設定します。

```text
left click / drag:
    draw

middle drag:
    pan

wheel:
    zoom around cursor
```

Start / Pause、Step、Clear、Randomize は画面上の操作部品から実行します。

## 13. スレッド構成

初期版では UI スレッド上でシミュレーションを実行します。

盤面が大きくなると、シミュレーションが UI の応答を遅らせる場合があります。

必要になった段階で、シミュレーションをワーカースレッドへ分離します。

```text
UI thread
   │ commands
   ▼
Simulation worker
   │ snapshots / deltas
   ▼
UI thread
```

World 全体を世代ごとに複製しない設計にします。

候補となる方式は次の通りです。

- immutable チャンク sharing
- 変更されたチャンクだけを転送
- double-buffered World ownership
- Arc ベース snapshot

転送方式は実測後に決定します。ワーカーへ分離する場合も、世代ごとの描画要求をキューに積まず、UI は次のフレームで最新の一貫した状態を読みます。保留中の通知はまとめ、UI は表示中のタイルを前回の描画状態と比較して必要なものだけ更新します。World 全体を世代ごとに複製せず、この最新状態を安全に公開する方法はワーカー分離時に選びます。

## 14. 並行処理の境界

シミュレーションを別スレッドへ移す場合、Simulation が可変な World を所有し、UI は安定したスナップショットを読みます。

```text
simulation owns mutable world

UI reads stable snapshot
```

初期実装ではシミュレーションをUIスレッド上で実行します。別スレッドへ移す段階で、この境界を適用します。

## 15. 空チャンクの削除

次の世代で空になったチャンクは保存しません。

```rust
if next_chunk.is_empty() {
    next_world.remove(coord);
}
```

空のチャンクを削除しないと、生存セルがなくなった領域も`HashMap`に残ります。長時間実行時にWorldのメモリ使用量が増えるため、空チャンクを削除します。

## 16. HashMap の検討事項

初期実装では標準の `HashMap` を使います。

非常に多くのチャンクを扱う場合、

- hash cost
- allocation
- cache locality

が性能上の問題になる可能性があります。

その場合は、次の方式を比較します。

- faster hasher
- slab / arena
- spatial hash
- sorted チャンク vector
- BTreeMap
- region-level secondary index

方式はデータ分布によって向き不向きがあるため、計測結果をもとに選びます。

## 17. 再描画の範囲

再描画には、アプリケーション内の要素更新と、ウィンドウフレームを画面へ提示する処理があります。

### アプリケーション内の更新

アプリケーションは変更された TileView だけを更新します。同じウィンドウフレームを生成する場合でも、変更されていない TileView の描画処理は繰り返しません。

### GPU とウィンドウへの画面提示

画面の一部だけを GPU や OS のコンポジターに提示できるかどうかは、GPUI と描画バックエンドの責務です。

## 18. 性能指標

性能を判断するため、次の値を計測します。

### シミュレーション

1 秒あたりの世代数、生存セルを含むチャンク数、世代ごとの候補チャンク数と変更チャンク数、生存セルの総数、世代ごとの処理時間を計測します。

### 描画

表示中のチャンク数と変更されたチャンク数、フレームごとの TileView の描画回数と生存セル数、描画時間、表示の遅延回数を計測します。

### メモリ

確保済みチャンク数、TileView 数、World の推定メモリ使用量を計測します。

これらの値をデバッグ表示に出すと、最適化の判断に役立ちます。

## 19. 受け入れ条件

### 機能

ライフゲームのルールと負の座標を正しく扱い、チャンク境界をまたいで世代を更新できることを確認します。パンとズーム、セル編集、Start / Pause / Step も操作できるようにします。

### 性能

World の外接矩形が広がっても空領域にメモリを割り当てません。ビューポート外に TileView を作らず、変化していない TileView も更新しません。シミュレーションでは生存セルの周辺だけを走査し、ズームアウト時の描画量を制御します。

表示範囲とoverscanを含む TileView の数は、ズーム倍率にかかわらず `MAX_RENDER_TILES` 以下に保ちます。集約表示中もセル座標を切り詰めず、個別セルの編集位置を正しく求めます。

## 20. テスト方針

### チャンクの操作

```text
set/get
toggle
empty
population
edge coordinates
```

### 座標

負の座標を重点的に確認します。

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

パターンはすべてチャンク境界の近くに配置します。

例:

```text
x = 62..66
y = 62..66
```

チャンクの角をまたぐ場合も確認します。

### 表示範囲の計算

表示範囲の計算を UI から分離し、

```text
viewport bounds
→ expected visible chunks
```

をユニットテストします。

## 21. 最適化の段階

最適化は次の順に進めます。まず疎なチャンク構造と候補チャンクの追跡を実装し、表示範囲の仮想化と変更タイルの更新に進みます。その後、シミュレーションと描画を分離し、詳細度制御を導入します。計測を行ったうえで、必要ならビット並列化します。

この順序なら、低レベルの最適化に進む前に、不要な計算や描画を省けます。

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

これらの条件を守ることで、システム全体の計算量が盤面の論理サイズに比例して増えることを防ぎます。

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

この構成では、計算対象を生存セルの周辺に、表示対象をビューポート内に限定します。さらに、前回の描画から変化したチャンクだけを更新し、全体の処理量を抑えます。
