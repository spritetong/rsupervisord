# Code Organization Rules

Rules distilled from the rsupervisord refactor series (`2d555c0` … `1d157ba`).
Each rule maps to one or more git commits and includes a minimal example.

---

## 1. Centralize constants in `src/consts.rs`

**Commits:** `877e79b`, `0b61d63`, `3df57f7`, `ad61750`, `5abf0ce`

- All magic numbers, timeouts, and config defaults live in `consts.rs`.
- Runtime timeouts use `Duration` constants (`AWAIT_ACTION`, `STOP_GRACE_EXTRA`, …).
- Wire/clap values that must stay integers keep their `_SECS` suffix (`DEFAULT_ACTION_TIMEOUT_SECS`).
- Windows SCM timings are named constants (`SCM_STOP_WAIT_HINT`, `SERVICE_STOP_TIMEOUT`), never bare literals.

```rust
// consts.rs
pub const AWAIT_ACTION: Duration = Duration::from_secs(30);
pub_const!(DEFAULT_STOP_WAIT: Duration = Duration::from_secs(10));
pub_const!(DEFAULT_PRIORITY: u32 = 50);
```

---

## 2. `pub_const!` macro: const + lowercase serde default in one shot

**Commits:** `3df57f7`, `ad61750`

- Use `pub_const!(NAME: Ty = value)` instead of a bare `const` + a hand-written `default_name()`.
- The macro emits `pub const NAME` and `pub const fn name()` (paste lowercases the name).
- Comments above `pub_const!` invocations must be `//`, not `///` (doc-comments attach to the macro call site incorrectly).

```rust
// consts.rs
macro_rules! pub_const {
    ($name:ident: $ty:ty = $value:expr) => {
        paste::paste! {
            pub const $name: $ty = $value;
            pub const fn [<$name:lower>]() -> $ty { $value }
        }
    };
}

// Usage in a config struct
#[serde(default = "default_priority")]
#[default(DEFAULT_PRIORITY)]
pub priority: u32,
```

- Drop per-type generic helpers (`u64_value`, `u32_value`, …) once `pub_const!` covers them; keep only `bool_value::<true>` for bools (cannot go through `pub_const!` cleanly for serde string paths).

---

## 3. `default_fn!` for non-const types

**Commits:** `877e79b`, `ad61750`

- Types that cannot be `const fn` defaults (`String`, `Vec<T>`) use `default_fn!` in `consts.rs` only.

```rust
default_fn!(default_log_level: String = DEFAULT_LOG_LEVEL.into());
default_fn!(default_exit_codes: Vec<i32> = vec![0]);
```

---

## 4. Serde boundary conversions in `src/serde_util.rs`

**Commits:** `2d555c0`, `f20767d`, `54c7dac`, `a612fd8`, `872eeab`

- Parse/convert **only at the serde boundary**; runtime structs hold native Rust types (`Duration`, `usize`, `Option<u32>`).
- Wire types never change (`u64` seconds, `"50MB"` strings, octal chmod strings).
- Naming convention for free converters: `string_to_*` / `*_to_string`.
- Serde `with = "..."` modules: `duration_secs`, `option_duration_secs`, `byte_size`, `option_byte_size`, `chmod`, `option_chmod`.
- Wrapper structs (`ByteSize`, `ChmodMode`) are forbidden; prefer `Option<T>` + a `with` module.
- Round-trip tests live next to the helpers in `serde_util::tests`.

```rust
// Free converters
pub fn string_to_bool(s: &str) -> Result<bool, ProgramError> { … }
pub fn string_to_bytes(s: &str) -> Result<usize, ProgramError> { … }
pub fn bytes_to_string(bytes: usize) -> String { … }
pub fn string_to_chmod(s: &str) -> Result<u32, ProgramError> { … }

// with = modules
pub mod duration_secs { … }        // u64 seconds ↔ Duration
pub mod option_byte_size { … }     // "50MB" ↔ Option<usize>
pub mod option_chmod { … }         // "0700" ↔ Option<u32>

// Config field
#[serde(default, with = "crate::serde_util::option_byte_size")]
pub max_bytes: Option<usize>,
```

- Keep domain-specific parsers (`parse_autorestart`, `parse_environment`, `parse_log_path`, …) in their original modules; only cross-cutting scalar converters move to `serde_util`.

---

## 5. Inheritance via macros, not `.or()` chains

**Commits:** `f20767d`, `ad61750`

- Replace manual `field.or(self.program_defaults.field)` chains with `inherit!` / `inherit_ref!` / `inherit_clone!`.
- Fallback to a `consts` default after inheritance.

```rust
let priority = inherit!(raw, self.program_defaults, priority)
    .unwrap_or(DEFAULT_PRIORITY);
```

---

## 6. `smart-default` for config structs

**Commits:** `947b4ed`

- Derive `SmartDefault` instead of writing `impl Default`.
- Non-type-default fields get `#[default(...)]` aligned with the serde default value.
- Constructors simplify to `..Default::default()`.

```rust
#[derive(Debug, Clone, SmartDefault, Serialize, Deserialize)]
pub struct ProgramConfig {
    #[serde(default = "default_priority")]
    #[default(DEFAULT_PRIORITY)]
    pub priority: u32,
}
```

---

## 7. Prune unused derives (strum and friends)

**Commit:** `1d157ba`

- Enumerations carry only the strum/serde derives that are actually called.
- Audit each derive: `Display`, `EnumString`, `AsRefStr`, `IntoStaticStr`, `EnumIter`.
- Restore a derive if a downstream macro needs it (e.g. tracing `%sig` requires `Display`).

```rust
// Before: Display, EnumString, AsRefStr, IntoStaticStr, EnumIter all derived
// After: only what compiles and is used
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, strum::EnumString)]
pub enum AutoRestartPolicy { … }

#[derive(…, strum::EnumString, strum::Display)] // Display kept for tracing
pub enum StopSignal { … }
```

---

## 8. Free functions → type methods when they operate on one type

**Commit:** `1d157ba`

- A helper that only takes `ProgramState` (or similar) becomes an inherent method.
- Prefer the method name over a free-function name that duplicates the type prefix.

```rust
// Before
fn state_to_supervisor_name(state: ProgramState) -> &'static str { … }

// After
impl ProgramState {
    pub fn supervisor_name(self) -> &'static str { self.into() }
}
// Caller: old_state.supervisor_name()
```

---

## 9. Drop unnecessary `async` on non-awaiting functions

**Commits:** `1d157ba`, `877e79b`

- A function with no `.await` is sync — remove `async` and update call sites.
- **Exception:** axum handlers (`Handler` trait) and middleware must remain `async` even without `.await`; the framework requires a future.
- When converting, also fix tests that `.await` the result.

```rust
// Sync (no framework constraint)
pub fn handle_version() -> Result<i32> { … }

// Must stay async (axum Handler)
pub async fn static_handler(uri: Uri) -> Response { … }
```

---

## 10. Collections: prefer simple lookups over heavy structures

**Commit:** `ad61750`

- A small static set (`KNOWN_EVENT_TYPES`) becomes a sorted `Vec` + `binary_search`, not a `HashSet`, when the set is tiny and order/readability matters.

```rust
const KNOWN_EVENT_TYPES: &[&str] = &["PROCESS_STATE", " … "]; // sorted
fn is_valid_event_type(t: &str) -> bool {
    KNOWN_EVENT_TYPES.binary_search(&t).is_ok()
}
```

---

## 11. Glob-import shared modules at call sites

**Commits:** `ad61750`, `3df57f7`

- After centralizing, use `use crate::consts::*;` and `use crate::serde_util::{…};` (or `*`) so call sites stay short and renames propagate.

```rust
use crate::consts::*;
use crate::serde_util::{duration_secs, option_byte_size};
```

---

## 12. Path rules: never resolve symbolic links

**Commit:** `d8a958b`, follow-up `abs_path` unification

- Paths are **never** `canonicalize`d / `realpath`ed / symlink-resolved. Forbidden: `fs::canonicalize`, `PlatformBackend::real_path` (removed).
- Free functions only (not on `PlatformBackend`):
  - **`norm_path` / `lexical_norm_path`**: pure lexical (no filesystem, no CWD). Use in config parse/transform only.
  - **`abs_path`**: `std::path::absolute` + `norm_path`. For relative paths may read CWD; never resolves symlinks. Use for config-file path production and all other absolute-path needs (spawn/watch/include identity).
- Config file paths are standardized with `abs_path` once produced (`resolve_config_path`, `from_file`).

```rust
// transform.rs — during config parse (lexical only, zero I/O)
let abs = norm_path(&base.join(&relative));

// paths / watch / resolve_executable / include visited
let p = abs_path(&candidate);
```

---

## 13. Security / platform hardening patterns

**Commits:** `7d525e6`, `c1452df`, `0df76a5`, `bf81ba0`, `eab10b0`

- Fail closed on peer-credential errors (Windows AF_UNIX `SIO_AF_UNIX_GETPEERPID`).
- Percent-decode auth tokens extracted from URL query strings.
- Close bind→chmod races with a temporary umask (`BIND_UMASK`).
- Default Windows `uds_chmod` to `0o770` (admin + Administrators) when unelevated access is disallowed; Unix stays `0o700`.
- Prefer server-side privilege checks over client-side validation.

---

## 14. Naming and naming-only refactors

**Commits:** `5abf0ce`, `3df57f7`, `0b61d63`, `4e9b042`

- Prefer domain names over generic ones for platform timings (`SCM_STOP_WAIT_HINT` > `STOP_WAIT`).
- When a unit is encoded in the type (`Duration`), drop the `_SECS` suffix from the constant name.
- Unify divergent CLI defaults onto a single shared const (e.g. all `-t` use `DEFAULT_ACTION_TIMEOUT_SECS`).

---

## 15. Verification checklist (run before commit)

1. `cargo check --all-targets`
2. `cargo clippy --all-targets -- -D warnings`
3. `cargo fmt`
4. `cargo test`

Known flaky: `tests/event_tests.rs::test_sse_all_logs_stream_endpoint`.

---

## Quick commit → rule map

| Commit | Rule(s) |
|--------|---------|
| `877e79b` | 1, 3, 11 |
| `0b61d63` | 1, 14 |
| `2d555c0` | 4 |
| `f20767d` | 4, 5 |
| `54c7dac` | 4 |
| `a612fd8` | 4 |
| `947b4ed` | 6 |
| `5abf0ce` | 1, 14 |
| `3df57f7` | 1, 2, 14 |
| `872eeab` | 4, 11 |
| `ad61750` | 1, 2, 5, 10, 11 |
| `1d157ba` | 7, 8, 9 |
| `d8a958b` | 12 |
| `7d525e6`, `c1452df`, `0df76a5`, `bf81ba0`, `eab10b0` | 13 |
