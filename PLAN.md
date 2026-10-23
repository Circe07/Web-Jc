# PLAN.md — `orq`: orquestador de compilación Rust para agentes IA

> Documento de entrada para Claude Code. Léelo entero antes de empezar.
> Eres el **agente líder**: coordinas subagentes, integras su trabajo y eres el único que toca los contratos compartidos.

---

## 0. Qué construimos (resumen)

`orq` es una herramienta en Rust que se sitúa entre un agente de IA y Cargo. Recibe la petición "he cambiado esto, ¿está bien?" y devuelve, lo más rápido posible y con pocos tokens, si el cambio compila y pasa sus tests.

Piezas:
1. **Vigilante**: detecta ficheros cambiados (`notify` + `git diff`).
2. **Analizador de impacto**: mapea ficheros → crates afectados + dependientes inversos (`cargo_metadata`).
3. **Escalera de verificación**: fmt → check → clippy → tests afectados (nextest). Se detiene en el primer fallo.
4. **Caché de resultados**: hash del estado relevante → resultado; si ya se verificó, no compila.
5. **Compresor para LLM**: deduplica diagnósticos, prioriza el primer error real y respeta un presupuesto de tokens.

Interfaces: CLI `orq` y servidor MCP. Daemon opcional para mantener estado caliente.

**No-objetivos**: reescribir rustc, sustituir rust-analyzer, optimizar builds de release.

---

## 1. Reglas para todos los agentes

- Cada subagente trabaja en **su propio worktree** (`isolation: worktree`) y **solo dentro de su crate asignado**.
- **Nadie salvo el líder modifica `crates/orq-core`.** Si un subagente necesita un cambio de contrato, lo pide en su informe final; no lo hace.
- Antes de terminar, cada subagente debe ejecutar y dejar en verde:
  - `cargo fmt --all -- --check`
  - `cargo clippy -p <su-crate> --all-targets -- -D warnings`
  - `cargo test -p <su-crate>`
- Sin `unwrap()`/`expect()` fuera de tests. Errores con `thiserror` en librerías y `anyhow` solo en binarios.
- Sin dependencias nuevas fuera de la lista de la sección 3 sin justificarlo en el informe.
- Commits con Conventional Commits (`feat(impact): ...`).
- Informe final de cada subagente (máx. 15 líneas): qué hizo, ficheros tocados, tests añadidos, decisiones y cualquier cambio de contrato que necesite.
- Compilar en desarrollo con lo mínimo: usar `cargo check -p` mientras se itera; `cargo test -p` al final.

---

## 2. Estructura del workspace

```
orq/
├── Cargo.toml                 # workspace, edition 2021, resolver 2
├── rust-toolchain.toml        # stable
├── .cargo/config.toml         # perfil dev rápido (ver 7.3)
├── crates/
│   ├── orq-core/              # tipos y traits compartidos (CONTRATOS)
│   ├── orq-impact/            # grafo del workspace + crates afectados
│   ├── orq-runner/            # invoca cargo, parsea JSON, escalera
│   ├── orq-compress/          # compresión de diagnósticos + presupuesto
│   ├── orq-cache/             # clave de caché + almacenamiento
│   ├── orq-daemon/            # watcher, estado caliente, IPC
│   ├── orq-mcp/               # servidor MCP
│   └── orq-cli/               # binario `orq`
├── fixtures/                  # workspaces de prueba (ver 6)
├── benches/                   # scripts de medición
└── docs/
```

---

## 3. Dependencias permitidas

| Uso | Crate |
| --- | --- |
| Metadatos de Cargo | `cargo_metadata` |
| Serialización | `serde`, `serde_json` |
| Async y procesos | `tokio` (features: rt-multi-thread, process, macros, net, fs, sync, time) |
| Vigilar ficheros | `notify`, `notify-debouncer-full` |
| CLI | `clap` (derive) |
| Hash | `blake3` |
| Almacenamiento caché | `redb` |
| MCP | `rmcp` (última versión estable; servidor stdio) |
| Errores | `thiserror`, `anyhow` |
| Logs | `tracing`, `tracing-subscriber` |
| Git (diff) | invocar `git` como proceso; no usar `git2` |
| Tests | `insta` (snapshots), `tempfile`, `assert_cmd`, `predicates` |

Herramientas externas que `orq` invoca (no se enlazan): `cargo`, `cargo-nextest`, `sccache`, `mold`/`wild`, componente Cranelift.

---

## 4. Contratos (`orq-core`) — los escribe el líder en la Oleada 0

```rust
// crates/orq-core/src/lib.rs (esqueleto orientativo)

pub struct WorkspaceRoot(pub std::path::PathBuf);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct CrateId { pub name: String, pub manifest_path: PathBuf }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeSet { pub files: Vec<PathBuf> }   // rutas relativas al workspace

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactSet {
    pub changed: Vec<CrateId>,     // crates que contienen ficheros cambiados
    pub dependents: Vec<CrateId>,  // dependientes inversos transitivos
    pub full_rebuild: bool,        // true si cambió Cargo.toml raíz, Cargo.lock, build.rs, toolchain...
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Step { Fmt, Check, Clippy, Test }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Severity { Error, Warning, Note, Help }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: Option<String>,        // p.ej. "E0308"
    pub message: String,
    pub file: Option<PathBuf>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub snippet: Option<String>,     // líneas de código relevantes
    pub suggestion: Option<String>,  // sugerencia de rustc/clippy si existe
    pub crate_id: Option<CrateId>,
    pub step: Step,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestFailure { pub name: String, pub crate_id: CrateId, pub output: String }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Outcome { Pass, Fail { failed_step: Step } }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepReport { pub step: Step, pub duration_ms: u64, pub passed: bool }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyResult {
    pub outcome: Outcome,
    pub steps: Vec<StepReport>,
    pub diagnostics: Vec<Diagnostic>,
    pub test_failures: Vec<TestFailure>,
    pub impact: ImpactSet,
    pub cache_hit: bool,
    pub total_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyRequest {
    pub root: PathBuf,
    pub changes: Option<ChangeSet>,  // None = detectar con git diff
    pub up_to: Step,                 // último peldaño a ejecutar (por defecto Test)
    pub token_budget: usize,         // por defecto 1500
    pub use_cache: bool,             // por defecto true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactReport {           // lo que ve el agente
    pub text: String,                // texto compacto listo para el LLM
    pub json: serde_json::Value,     // versión estructurada equivalente
    pub estimated_tokens: usize,
    pub truncated: bool,
}

// Traits que implementan los crates
pub trait ImpactAnalyzer { fn analyze(&self, root: &Path, changes: &ChangeSet) -> Result<ImpactSet, CoreError>; }
pub trait Compressor { fn compress(&self, result: &VerifyResult, budget: usize) -> CompactReport; }
pub trait ResultCache {
    fn key(&self, root: &Path, impact: &ImpactSet, up_to: Step) -> Result<CacheKey, CoreError>;
    fn get(&self, key: &CacheKey) -> Result<Option<VerifyResult>, CoreError>;
    fn put(&self, key: &CacheKey, result: &VerifyResult) -> Result<(), CoreError>;
}
```

También en `orq-core`: `Config` (leída de `orq.toml` en la raíz del proyecto, todo opcional con valores por defecto), `CoreError` y una función `estimate_tokens(&str) -> usize` (heurística: `ceil(chars / 4)`).

---

## 5. Plan de oleadas

Cada oleada se lanza **en paralelo** dentro de sí misma. No se empieza una oleada hasta que la anterior está integrada en `main` y CI está en verde.

### Oleada 0 — Base (líder, secuencial)

**T0.1 Scaffold del workspace**
- Crear la estructura de la sección 2, `Cargo.toml` del workspace con `[workspace.dependencies]` y todos los crates vacíos compilando.
- `rust-toolchain.toml` (stable), `.gitignore`, licencia MIT/Apache-2.0 dual, `README.md` mínimo.
- Aceptación: `cargo check --workspace` en verde.

**T0.2 Contratos en `orq-core`**
- Implementar todo lo de la sección 4 con `serde` y tests de ida y vuelta JSON.
- Aceptación: `cargo test -p orq-core` en verde; los tipos están documentados con `///`.

**T0.3 Fixtures**
- Ver sección 6. Aceptación: cada fixture compila en limpio con `cargo check` (salvo las que deben fallar a propósito).

**T0.4 CI**
- GitHub Actions: fmt, clippy `-D warnings`, test en Linux y macOS. Caché con `Swatinem/rust-cache`. Instalar `cargo-nextest`.
- Aceptación: pipeline verde en un PR de prueba.

### Oleada 1 — Núcleo (4 subagentes en paralelo)

**T1.1 `orq-impact`** (subagente A)
- Leer el grafo con `cargo_metadata` (incluyendo dependencias `path` dentro del workspace).
- Mapear cada fichero cambiado al crate cuyo directorio lo contiene (el más profundo si hay anidados).
- Calcular dependientes inversos transitivos.
- `full_rebuild = true` si cambia: `Cargo.toml` raíz, `Cargo.lock`, `rust-toolchain*`, `.cargo/config*`, cualquier `build.rs`.
- Detectar cambios con `git diff --name-only HEAD` + ficheros sin seguimiento (`git ls-files --others --exclude-standard`) cuando `changes` es `None`.
- Ficheros fuera de cualquier crate (p.ej. `docs/`) se ignoran.
- Tests: sobre `fixtures/multi` → cambiar `leaf` solo afecta a `leaf`; cambiar `base` afecta a `base` + todos sus dependientes; cambiar `Cargo.lock` → `full_rebuild`.
- Aceptación: tests en verde y análisis en menos de 300 ms en `fixtures/multi` (con metadata cacheada en memoria tras la primera llamada).

**T1.2 `orq-runner`** (subagente B)
- Ejecutar cada peldaño con `tokio::process`, solo sobre los crates del `ImpactSet` (`-p <crate>` repetido; o `--workspace` si `full_rebuild`):
  - Fmt: `cargo fmt -p … -- --check` (parsear ficheros sin formato como diagnósticos `Warning` con paso `Fmt`).
  - Check: `cargo check -p … --all-targets --message-format=json-diagnostic-rendered-ansi` → quitar ANSI; usar `json` puro si es más simple.
  - Clippy: `cargo clippy -p … --all-targets --message-format=json`.
  - Test: `cargo nextest run -p … --no-fail-fast` si nextest está instalado; si no, `cargo test -p …`. Extraer tests fallidos.
- Parsear `compiler-message` de Cargo en `Diagnostic` (código, span principal, snippet de `rendered` o del fichero, sugerencias de `children` con `suggested_replacement`).
- Escalera: ejecutar peldaños en orden hasta `up_to`; parar en el primero con errores (los warnings no paran).
- Medir duración de cada peldaño.
- Respetar el `CARGO_TARGET_DIR` del entorno; no limpiar nunca `target/`.
- Tests: con `fixtures/broken-type` produce un `E0308` con fichero y línea correctos; con `fixtures/failing-test` el peldaño Test falla con el nombre del test; con `fixtures/multi` limpio todo pasa.
- Aceptación: tests en verde; snapshots `insta` del `VerifyResult` (normalizando duraciones y rutas absolutas).

**T1.3 `orq-compress`** (subagente C)
- Implementar `Compressor`:
  - Eliminar duplicados (mismo código + fichero + línea).
  - Ordenar: errores antes que warnings; dentro de errores, el primero en orden de compilación primero.
  - Heurística de cascada: si hay un error de resolución (`E0432`, `E0433`, `E0412`, `E0425`), degradar los errores posteriores del mismo fichero a "posiblemente derivados".
  - Formato de texto (ejemplo de salida esperada):
    ```
    FAIL check (2 errores, 3 warnings ocultos) · crates: api · 1.8s
    [1] E0308 tipos incompatibles — crates/api/src/handler.rs:42:17
        42 |     let id: u64 = req.id;
           |                   ^^^^^^ se esperaba u64, encontrado String
        sugerencia: req.id.parse::<u64>()?
    [2] E0599 ... (posiblemente derivado de [1])
    ```
  - Si se supera el presupuesto: recortar snippets, luego quitar warnings, luego mostrar solo los N primeros errores con un resumen "+K errores más".
  - `Pass` debe ser una sola línea: `PASS check+clippy+test · crates: api, cli · 3.2s (cache: no)`.
- Tests: snapshots con resultados sintéticos; propiedad "nunca supera el presupuesto" probada con presupuestos de 200, 500 y 1500.
- Aceptación: tests en verde; texto en español o inglés según `Config.lang` (por defecto inglés, que gasta menos tokens).

**T1.4 `orq-cache`** (subagente D)
- Clave = `blake3` de:
  - contenido de todos los ficheros versionados de los crates en `changed ∪ dependents` (y de todo el workspace si `full_rebuild`);
  - `Cargo.lock`, `Cargo.toml` raíz, `rust-toolchain*`, `.cargo/config*`;
  - salida de `rustc -Vv`;
  - variables de entorno: `RUSTFLAGS`, `RUSTDOCFLAGS`, `CARGO_*` relevantes;
  - `up_to` y la versión de `orq`.
- Almacenar `VerifyResult` en `redb` dentro de `target/orq/cache.redb` (o `$XDG_CACHE_HOME/orq/<hash-del-root>` si `target` no es escribible).
- Expiración: máximo 5 000 entradas, LRU simple.
- Tests: mismo estado → misma clave; cambiar un byte en un fichero afectado → clave distinta; cambiar un fichero de un crate no afectado → misma clave; cambiar `RUSTFLAGS` → clave distinta.
- Aceptación: tests en verde; cálculo de clave < 100 ms en `fixtures/multi`.

### Oleada 2 — Integración Fase 1 + Daemon (2 subagentes en paralelo, luego el líder integra)

**T2.1 `orq-cli` — comando `verify`** (subagente E)
- `orq verify [--files a.rs,b.rs] [--up-to check|clippy|test] [--budget N] [--no-cache] [--json]`.
- Flujo: impact → cache.get → runner → cache.put → compress → stdout. Código de salida 0 si pasa, 1 si falla, 2 si error interno.
- Si hay daemon activo (socket existe), delegar en él; si no, ejecutar en proceso.
- `orq explain <CODIGO>`: ejecutar `rustc --explain` y devolver un resumen recortado al presupuesto.
- Tests de integración con `assert_cmd` sobre las fixtures.
- Aceptación: en `fixtures/multi`, modificar solo `leaf` y ejecutar `orq verify` no compila `base` ni `app` (verificable en la salida de cargo con `-v` o en el `ImpactSet`); la segunda ejecución sin cambios devuelve `cache: hit` en < 200 ms.

**T2.2 `orq-daemon`** (subagente F)
- Proceso `orq daemon start|stop|status`.
- Vigila el workspace con `notify-debouncer-full` (debounce 300 ms), ignora `target/` y `.git/`.
- Mantiene en memoria el `cargo_metadata` y lo invalida si cambia algún `Cargo.toml`.
- **Precalentamiento**: tras cada cambio detectado, lanza en segundo plano `cargo check` sobre el impacto; si llega una petición `verify` mientras corre, espera a ese resultado en lugar de lanzar otro.
- IPC por socket Unix (`$XDG_RUNTIME_DIR/orq-<hash>.sock`) y named pipe en Windows; protocolo JSON por líneas: `VerifyRequest` → `VerifyResult`.
- Se apaga solo tras 30 minutos sin peticiones.
- Tests: arrancar el daemon sobre una fixture temporal, modificar un fichero, pedir `verify` y comprobar que responde; `stop` limpia el socket.
- Aceptación: tests en verde en Linux y macOS; Windows puede quedar como `todo!` documentado en la Oleada 2.

**T2.3 Integración (líder)**
- Fusionar T2.1 y T2.2, prueba end-to-end y publicar `v0.1.0` (sin MCP).

### Oleada 3 — MCP, setup y métricas (3 subagentes en paralelo)

**T3.1 `orq-mcp`** (subagente G)
- `orq mcp` arranca un servidor MCP por stdio con `rmcp`.
- Herramientas:
  - `verify` — args: `files?: string[]`, `up_to?: "check"|"clippy"|"test"`, `budget?: number`. Devuelve `CompactReport.text` como contenido de texto y `json` como contenido estructurado.
  - `run_affected_tests` — atajo de `verify` con `up_to = test`.
  - `explain_error` — args: `code: string`.
  - `impact` — devuelve el `ImpactSet` para los ficheros dados (útil para que el agente planifique).
- Descripciones de herramientas pensadas para el modelo: cuándo usar cada una y que sustituyen a `cargo build/test`.
- Documentar en `docs/claude-code.md` cómo registrarlo: `claude mcp add orq -- orq mcp`.
- Aceptación: test de integración que habla JSON-RPC por stdio con el servidor y llama a `verify` sobre una fixture.

**T3.2 `orq setup`** (subagente H, dentro de `orq-cli`)
- Detecta qué hay instalado: nightly + componente Cranelift, `mold`/`wild`, `sccache`, `cargo-nextest`.
- Propone (y con `--apply`, escribe) un bloque en `.cargo/config.toml` del proyecto **solo para el perfil dev**, haciendo copia de seguridad del fichero previo.
- Nunca instala nada sin `--apply`; muestra los comandos de instalación que faltan.
- Aceptación: `orq setup` en una fixture sin config imprime el diff propuesto; `--apply` lo escribe y `cargo check` sigue funcionando.

**T3.3 Métricas y benchmarks** (subagente I)
- `orq stats`: lee un log JSONL en `target/orq/stats.jsonl` (una línea por `verify`: duración, cache_hit, tokens, crates) y muestra mediana, p90, tasa de caché y tokens medios.
- `benches/compare.sh`: en un proyecto dado, aplica N cambios sintéticos y compara `cargo build && cargo test` frente a `orq verify`, midiendo tiempo y tamaño de salida.
- Aceptación: el script produce una tabla markdown con ambos modos sobre `fixtures/multi` y sobre un proyecto real clonado (p.ej. un crate mediano de código abierto).

**T3.4 Cierre (líder)**
- README completo (instalación, uso con Claude Code, configuración `orq.toml`, benchmarks), `CHANGELOG.md`, publicar `v0.2.0`.

### Oleada 4 — Rendimiento, seguridad, proyectos reales y distribución (5 subagentes en paralelo)

Base preparada por el líder (`5e6cb9b`):
- `BuildOptions`: features y target, en `orq.toml` `[build]` y en cada petición, e incluidas ya en la clave de caché.
- `Config.autostart_daemon`.
- Directorio privado del socket (`ipc::prepare_socket_dir`).

**T4.1 Seguridad del daemon** (`orq-daemon`, `server.rs`): socket `0600` y comprobación del UID del peer; se rechazan las conexiones de otros usuarios.

**T4.2 Rendimiento del runner** (`orq-runner`):
- Un solo `cargo clippy` cubre los peldaños Check y Clippy; sus diagnósticos se reparten entre los dos.
- `fmt` corre en paralelo con la compilación.
- `build.cargo_args()` se pasa a check, clippy y test.

**T4.3 Rendimiento de la caché** (`orq-cache`):
- `rustc -Vv` se guarda en disco con una huella de la toolchain.
- Índice de hashes por (tamaño, mtime) para no releer los ficheros sin cambios.

**T4.4 Features en CLI/MCP y arranque automático del daemon** (`orq-cli` `verify`, `orq-mcp`, `orq-daemon` `client.rs`):
- Flags `--features/--all-features/--no-default-features/--target` y sus argumentos MCP equivalentes.
- `orq mcp` arranca el daemon si `autostart_daemon` está activo.

**T4.5 Distribución** (`.github/workflows/release.yml`, metadatos de los `Cargo.toml`, `packaging/`):
- Binarios por tag `v*` para Linux y macOS.
- Paquetes publicables en crates.io (`orq-*`; el nombre `orq` está ocupado).
- Fórmula de Homebrew.

Fuera de esta oleada: selección fina de tests, medición de la calidad de la compresión con tareas reales, named pipes en Windows.

### Oleada 5 — Velocidad en frío, integración con Claude Code y robustez (5 subagentes en paralelo)

Base preparada por el líder (`4aabdb2`):
- `ImpactSet.changed_files` y `BuildOptions.targets` (`auto` | `all`), ya incluidos en la clave de caché.
- Stubs de `Runner::prebuild_tests` y `orq_runner::fix`.
- Subcomandos `orq fix`, `orq hook` y `orq doctor`, cada uno en su fichero.

**T5.1 Runner** (`orq-runner`, salvo `fix.rs`):
- `targets = auto`: compila lib, bins y tests siempre; benches y examples solo si cambió alguno de sus ficheros.
- Ejecuta primero los tests que fallaron la última vez.
- Implementa `prebuild_tests`.

**T5.2 Daemon** (`orq-daemon`, `server.rs`):
- El precalentamiento usa el mismo peldaño que una verificación normal (clippy, no check), para que los artefactos sirvan.
- Después precompila los tests con `prebuild_tests`.

**T5.3 `orq fix` / `apply_fix`** (`orq-runner/src/fix.rs`, `orq-cli/src/fix.rs`, herramienta MCP): aplica las sugerencias `MachineApplicable` de rustc y clippy a los crates afectados y vuelve a verificar.

**T5.4 Hook y plugin de Claude Code** (`orq-cli/src/hook.rs`, `integrations/claude-code/`, `docs/claude-code.md`):
- `orq hook` para `PostToolUse` sobre Edit/Write/MultiEdit: verifica y devuelve el fallo al agente.
- Plugin con el hook y el servidor MCP.

**T5.5 Robustez** (`.github/workflows/realworld.yml`, `benches/realworld.sh`, `orq-cli/src/doctor.rs`, `orq-cli/src/stats.rs`):
- CI nocturno sobre proyectos reales.
- `orq doctor`.
- Estimación de ahorro en `orq stats`.

Fuera de esta oleada: medir la calidad de la compresión con tareas reales (Oleada 6), named pipes en Windows.

---

## 6. Fixtures (`fixtures/`)

| Nombre | Contenido | Uso |
| --- | --- | --- |
| `multi` | workspace: `base` ← `mid` ← `app`, y `leaf` independiente; tests simples en cada uno | impacto, caché, CLI |
| `single` | un solo crate con módulos | caso sin ganancia de impacto |
| `broken-type` | crate con un `E0308` deliberado | runner, compresor |
| `cascade` | un `use` inexistente que provoca 10+ errores derivados | heurística de cascada |
| `failing-test` | compila pero un test falla | peldaño Test |
| `clippy-warn` | compila con warnings de clippy | warnings no paran la escalera |
| `build-rs` | crate con `build.rs` | `full_rebuild` |

Las fixtures que deben fallar llevan un `README` de una línea explicando el fallo esperado. Ninguna fixture forma parte del workspace principal (excluirlas en `[workspace] exclude`).

---

## 7. Detalles técnicos transversales

### 7.1 Formato de `orq.toml`
```toml
budget = 1500          # tokens por respuesta
up_to = "test"         # peldaño máximo por defecto
lang = "en"            # "en" | "es"
nextest = "auto"       # "auto" | "always" | "never"
ignore = ["docs/**"]
```

### 7.2 Concurrencia
- Un solo `cargo` a la vez por workspace (Cargo ya bloquea `target/`, pero evitamos colas largas): mutex en el daemon; en modo sin daemon, lock de fichero en `target/orq/lock`.

### 7.3 Perfil dev del propio repo `orq` (para que los agentes compilen rápido)
```toml
# .cargo/config.toml
[profile.dev]
debug = "line-tables-only"

[profile.dev.package."*"]
opt-level = 1
```
Si el líder detecta `mold` en Linux, añadir el linker; no exigirlo en CI.

### 7.4 Logging
- `tracing` con `ORQ_LOG=debug`. Nunca escribir logs en stdout en modo MCP (stdout es el canal JSON-RPC): todo a stderr o a fichero.

---

## 8. Orquestación — instrucciones para el agente líder

1. Lee este plan entero. Crea la lista de tareas con todas las T de la sección 5 y sus dependencias.
2. Ejecuta la **Oleada 0** tú mismo, en orden. No lances subagentes hasta que `orq-core` esté en `main` y CI en verde.
3. Para cada oleada siguiente, lanza **un subagente por tarea en paralelo**, cada uno en su worktree, con este encargo:
   - el texto completo de su tarea (T-x.y);
   - las secciones 1, 3, 4 y 7 de este plan;
   - la lista de fixtures que necesita;
   - la instrucción de terminar con el informe de la sección 1.
4. Cuando vuelvan los informes: revisa el diff de cada uno, ejecuta `cargo clippy --workspace --all-targets -- -D warnings` y `cargo test --workspace`, resuelve conflictos y fusiona.
5. Si un subagente pide un cambio de contrato, decide tú, cámbialo en `orq-core` y avisa a los subagentes de la oleada siguiente.
6. Si una tarea falla dos veces, no la relances igual: divídela en subtareas más pequeñas o hazla tú.
7. Al cerrar cada oleada, añade un resumen breve en `docs/progress.md` (qué se fusionó, qué queda, métricas si las hay).
8. Para verificar tu propio trabajo en este repo, en cuanto exista `orq verify`, úsalo en lugar de `cargo build` (dogfooding).

---

## 9. Definición de terminado (v0.2.0)

- [ ] `orq verify` solo compila los crates afectados y devuelve una salida dentro del presupuesto.
- [ ] Una verificación repetida sin cambios responde de caché en < 200 ms.
- [ ] El daemon mantiene estado caliente y precompila tras cada cambio.
- [ ] `orq mcp` funciona registrado en Claude Code y el agente lo usa en lugar de `cargo build`.
- [ ] `orq setup` propone y aplica el perfil rápido.
- [ ] `orq stats` y `benches/compare.sh` muestran tiempo y tokens ahorrados frente a la línea base.
- [ ] CI verde en Linux y macOS; README y CHANGELOG completos.
