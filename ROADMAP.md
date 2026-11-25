# astlens — Roadmap bàn giao

> **Cập nhật lần cuối:** 2026-11-25 | **Phiên bản hiện tại:** `v0.8.0` (đã release)
>
> Giai đoạn 1 (`v0.8.0`) đã hoàn thành và phát hành lên GitHub + crates.io.
> Các giai đoạn dưới đây là kế hoạch phát triển tiếp theo theo thứ tự ưu tiên.

---

## ✅ Giai đoạn 1 — Mở rộng hệ sinh thái Agent & đồng nhất MCP (`v0.8.0`) — ĐÃ XONG

### Đã thực hiện
- `astlens install/uninstall/doctor` hỗ trợ `--client antigravity` (auto-detect `--client all`), sinh plugin thuần Rust không phụ thuộc Python/bash/jq (`84b8238`, `9e448c0`, `613f6c2`).
- `astlens guard --client antigravity` — native Rust PreToolUse guard thay thế `agy-guard.py` (`9e448c0`, `c221548`).
- `fix(guard)`: xử lý Windows verbatim prefix `\\?\`, chuẩn hoá drive letter, hỗ trợ symlink root macOS (logical vs canonical path) (`c221548`).
- `read_symbol` nhận `names: string[]` (tối đa 20 symbols) để đọc nhiều định nghĩa trong một lần gọi MCP (`361e366`).
- `search_symbols`, `changed_symbols`, `git_status` thêm tham số `dir` tuỳ chọn để scope về subdirectory (`5146135`).
- Thiết kế lại trang GitHub Pages với tool inspector, typography JetBrains Mono, hướng dẫn Antigravity.

---

## 🔧 Giai đoạn 2 — Hiệu năng Monorepo lớn (`v0.8.x`) — ƯU TIÊN CAO

### Bối cảnh

Trên repo Linux kernel (`66,075` file): `search_symbols` ấm tốn **178.8 ms** vì phải `walk + stat` lại toàn bộ mỗi lần gọi; `codebase_map` lặp lại tốn nguyên **3,461 ms** như lần đầu. Xem [`BENCHMARKS.md`](./BENCHMARKS.md).

### Hạng mục 1 — Duyệt cây file 1 lượt khi vượt `max_files`

**File:** [`src/git/mod.rs`](./src/git/mod.rs#L18-L31)

**Vấn đề:** `list_files` hiện quét song song, nếu kết quả vượt `limit` thì hủy và quét lại tuần tự từ đầu — tốn 2 lượt duyệt.

**Hướng sửa:**
```rust
// Thay vì: quét song song → fail → quét tuần tự lại
// Làm: một lượt duyệt BFS, giữ `limit` đường dẫn nông nhất (ưu tiên depth nhỏ)
// dùng BinaryHeap hoặc reservoir sampling theo depth
fn walk_capped(root: &Path, limit: usize) -> Vec<PathBuf> {
    // 1 lượt, giữ nông nhất
}
```

**Tiêu chí xong:** `list_files` trên kernel repo chỉ duyệt 1 lần, kết quả deterministic (cùng thứ tự với lần trước), `cargo test --locked` pass.

---

### Hạng mục 2 — Cache danh sách file + phát hiện thay đổi

**Files:** [`src/index/mod.rs`](./src/index/mod.rs), [`src/mcp/tools.rs`](./src/mcp/tools.rs#L65-L84)

**Vấn đề:** Mỗi lần gọi `search_symbols`, `codebase_map`, `find_references` đều `walk + stat` lại toàn bộ workspace.

**Hướng sửa (2 lựa chọn, chọn 1):**

**Lựa chọn A — Watcher `notify`** (phức tạp hơn, chính xác hơn):
```toml
[dependencies]
notify = { version = "7", default-features = false, features = ["macos_kqueue"] }
```
```rust
// Workspace giữ Arc<Mutex<FileList>> được watcher cập nhật khi có event
pub struct Workspace {
    file_cache: Arc<Mutex<CachedFileList>>,
    _watcher: Option<notify::RecommendedWatcher>,
    // ...
}
```

**Lựa chọn B — `mtime` của `.git/index`** (đơn giản hơn, đủ dùng):
```rust
// Kiểm tra mtime của `.git/index` trước mỗi lần walk
// Nếu không đổi → trả cache, đổi → walk lại + cập nhật cache
fn git_index_mtime(root: &Path) -> Option<SystemTime> {
    fs::metadata(root.join(".git/index")).ok()?.modified().ok()
}
```

**Tiêu chí xong:** `search_symbols` ấm trên kernel repo < 5 ms (từ 178.8 ms), benchmark `benches/report.rs` ghi nhận cải thiện, test pass.

---

### Hạng mục 3 — Cache đồ thị & vector PageRank cho `codebase_map`

**File:** [`src/index/map.rs`](./src/index/map.rs)

**Vấn đề:** Mỗi lần gọi `codebase_map` đều dựng lại toàn bộ mention graph và chạy lại PageRank iterations (trên Linux kernel: 3,461 ms/lần).

**Hướng sửa:**
```rust
// Thêm vào Workspace:
pub struct MapCache {
    generation: u64,          // tăng khi file list đổi
    focus_path: Option<PathBuf>,
    rank_vec: Vec<(PathBuf, f64)>,  // kết quả đã sort
}

// Logic:
// 1. Nếu generation == current_gen && focus_path == request.focus → trả rank_vec
// 2. Nếu chỉ max_tokens đổi → slice rank_vec, không tính lại
// 3. Nếu generation đổi → tính lại, cập nhật cache
```

**Tiêu chí xong:** Lần gọi `codebase_map` lặp lại (cùng repo, cùng focus) < 10 ms, đổi `max_tokens` không tính lại graph, test pass.

---

## 🛠 Giai đoạn 3 — Công cụ AST thế hệ mới (`v0.9.0`)

### Hạng mục 1 — Tool mới `symbol_call_graph`

**Files:** [`src/index/mod.rs`](./src/index/mod.rs#L142-L147), [`src/ast/parser.rs`](./src/ast/parser.rs#L683-L709), [`src/mcp/tools.rs`](./src/mcp/tools.rs)

**Mô tả:** Trả danh sách hàm **gọi tới** (callers) và **được gọi bởi** (callees) một symbol, 1–2 tầng, trong ~200 token.

```
// Tool input:
{
  "symbol": "evaluate_antigravity",
  "file": "src/guard.rs",
  "depth": 1   // 1 = callers/callees trực tiếp, 2 = thêm 1 tầng nữa
}

// Tool output:
callers (2):
  src/guard.rs:68 evaluate()
  tests/guard_test.rs:45 test_antigravity_deny()

callees (5):
  src/guard.rs:294 pick_workspace_root()
  src/guard.rs:308 record_root()
  src/guard.rs:160 check_antigravity_tool()
  src/guard.rs:131 antigravity_root_file()
  src/mcp/tools.rs:1392 file_uri_to_path()
```

**Triển khai:**
1. Tận dụng `enclosing_symbol` (đã có) và `parsed.idents` để tìm callees.
2. `find_references` ngược để tìm callers.
3. Giới hạn 50 callers + 50 callees ở depth=1 để kiểm soát token.

---

### Hạng mục 2 — Tool mới `rename_symbol` đa file, kiểm tra syntax nguyên tử

**Files:** [`src/mcp/tools.rs`](./src/mcp/tools.rs), [`src/index/mod.rs`](./src/index/mod.rs)

**Mô tả:** Đổi tên một symbol xuyên suốt toàn bộ workspace với kiểm tra tree-sitter syntax trước khi ghi.

```
// Tool input:
{
  "file": "src/guard.rs",
  "symbol": "evaluate_antigravity",
  "new_name": "evaluate_antigravity_hook",
  "dry_run": true   // kiểm tra trước, không ghi
}

// Tool output (dry_run):
would rename in 4 files:
  src/guard.rs:119 (definition)
  src/guard.rs:76  (call in evaluate())
  tests/jsonrpc.rs:245 (call in test)
  src/lib.rs:12 (re-export)
syntax check: ok (all 4 files parse after rename)
```

**Triển khai:**
1. `find_references` với `include_definitions: true` lấy tất cả vị trí.
2. Với mỗi file: áp dụng thay thế trong memory → parse với tree-sitter → nếu syntax error → abort toàn bộ.
3. Nếu tất cả OK → ghi atomic tất cả file.
4. `dry_run: true` chỉ chạy bước 1-2, không ghi.

---

### Hạng mục 3 — Mở rộng phát hiện thư viện ngoài

**File:** [`src/libraries.rs`](./src/libraries.rs#L19-L76)

**Vấn đề:** Hiện thiếu nhiều thư mục phổ biến:

| Ecosystem | Thiếu |
|---|---|
| Python | `$VIRTUAL_ENV`, `$CONDA_PREFIX`, `~/.cache/uv/virtualenvs` |
| JS/TS | `~/.pnpm-store`, `~/.bun/install/cache` |
| Java/Kotlin | `~/.gradle/caches`, `~/.m2/repository` |
| C# | `~/.nuget/packages` |
| Go | `$GOMODCACHE` (nếu khác `$GOPATH/pkg/mod`) |

**Triển khai:** Thêm vào `fn candidates()` trong `src/libraries.rs`, giữ nguyên logic dedup + is_dir check.

---

## 🌐 Giai đoạn 4 — Ngôn ngữ mới & Tái cấu trúc Hotspot (`v1.0.0`)

### Hạng mục 1 — Thêm ngôn ngữ mới

**File:** [`src/ast/parser.rs`](./src/ast/parser.rs#L35-L86)

| Ngôn ngữ | Extension | Crate tree-sitter | Độ phức tạp |
|---|---|---|---|
| Kotlin | `.kt`, `.kts` | `tree-sitter-kotlin` | Trung bình |
| Swift | `.swift` | `tree-sitter-swift` | Trung bình |
| Zig | `.zig` | `tree-sitter-zig` | Thấp |
| PHP | `.php`, `.phtml` | `tree-sitter-php` | Cao (PHP namespace) |
| Lua | `.lua` | `tree-sitter-lua` | Thấp |

Mỗi ngôn ngữ: thêm `Lang` variant, extension mapping, symbol query `.scm`, cập nhật `scripts/astlens-guard.sh` và `scripts/test-guard.sh`.

---

### Hạng mục 2 — Tách module hotspot lớn

**Vấn đề:** 3 file quá lớn, khó maintain và review:

| File | Dòng hiện tại | Hướng tách |
|---|---|---|
| [`src/mcp/tools.rs`](./src/mcp/tools.rs) | 4,023 | `workspace.rs` (walk/index), `edits.rs` (replace_*/insert_*), `server.rs` (MCP dispatch) |
| [`src/ast/parser.rs`](./src/ast/parser.rs) | 2,320 | `src/ast/queries/` — mỗi ngôn ngữ một file `.rs` chứa query string |
| [`src/index/map.rs`](./src/index/map.rs) | 2,841 | `graph.rs` (xây mention graph), `pagerank.rs` (tính rank), `render.rs` (format output) |

**Yêu cầu:** Tách phải giữ nguyên public API, không thay đổi hành vi, `cargo test --locked` pass sau mỗi lần tách.

---

## 📋 Thứ tự ưu tiên khuyến nghị

```
Giai đoạn 2 Hạng mục 2 (file cache + mtime)        ← impact lớn nhất, ít rủi ro
Giai đoạn 2 Hạng mục 1 (1-lượt walk)               ← đơn giản, cần trước cache
Giai đoạn 2 Hạng mục 3 (PageRank cache)            ← sau khi có file cache
Giai đoạn 3 Hạng mục 1 (symbol_call_graph)         ← tính năng mới cao giá trị
Giai đoạn 3 Hạng mục 2 (rename_symbol)             ← phức tạp, cần test kỹ
Giai đoạn 3 Hạng mục 3 (thêm libraries)            ← nhỏ, làm bất kỳ lúc nào
Giai đoạn 4 Hạng mục 1 (ngôn ngữ mới)              ← theo demand người dùng
Giai đoạn 4 Hạng mục 2 (tách module)               ← làm cuối, không thay đổi behavior
```

---

## 🔗 Liên kết tham khảo

- [BENCHMARKS.md](./BENCHMARKS.md) — dữ liệu hiệu năng baseline
- [CHANGELOG.md](./CHANGELOG.md) — lịch sử thay đổi
- [src/guard.rs](./src/guard.rs) — native Rust guard (đã refactor ở v0.8.0)
- [src/mcp/tools.rs](./src/mcp/tools.rs) — MCP server tools (hotspot cần tách)
- [src/index/map.rs](./src/index/map.rs) — PageRank codebase map
- [.github/workflows/release.yml](./.github/workflows/release.yml) — quy trình release

---

*Tài liệu này được tạo lúc release `v0.8.0` (2026-11-25) để bàn giao kế hoạch phát triển tiếp theo.*
