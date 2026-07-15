# `tailor matrix --ado` — Azure DevOps matrix output

> **Status:** Implemented · _last reviewed 2026-06-29_
>
> The ADO matrix object and `--ado` wrapper are implemented in `crates/tailor-core/src/ado.rs`, `crates/tailor/src/cli.rs`, and `crates/tailor/src/run.rs`, with CLI coverage in `crates/tailor/tests/cli.rs`. The title's old proposed label was stale.

Azure DevOps expands one parallel job per matrix **leg** from a JSON object, and that object can be
produced *at runtime* by a setup job and consumed downstream with `strategy: matrix: $[ … ]` ([matrix
strategy](https://learn.microsoft.com/en-us/azure/devops/pipelines/yaml-schema/jobs-job-strategy)). To
let the pipeline drive **one job per tailor cell** — instead of today's hand-written per-image
stages — `tailor matrix` gains an **`--ado <VAR_NAME>`** flag that prints the selected cells as an ADO
matrix wrapped in a `task.setvariable` logging command, so a setup job can publish it as an output
variable for later stages to expand.

---

## 1. Background — how ADO consumes a runtime matrix

Two Azure Pipelines facts drive the format:

1. **Publishing a variable from a script** uses a logging command on **stdout**
   ([logging commands](https://learn.microsoft.com/en-us/azure/devops/pipelines/scripts/logging-commands)):

   ```
   ##vso[task.setvariable variable=<NAME>;isOutput=true]<value>
   ```

   `isOutput=true` makes it a cross-job **output** variable, referenced downstream as
   `dependencies.<job>.outputs['<step>.<NAME>']`. The agent only parses logging commands from a step's
   own stdout, so the line must be printed verbatim, alone.

2. **A runtime matrix** is a JSON object whose keys are leg names and whose values are the variables
   each expanded job receives:

   ```json
   { "<leg>": { "<VAR>": "<value>", … }, … }
   ```

   Downstream (same stage):

   ```yaml
   strategy:
     matrix: $[ dependencies.Setup.outputs['emit.BUILD_MATRIX'] ]   # ADO parses the JSON string
   ```

   (Cross-stage uses `stageDependencies.<stage>.Setup.outputs['emit.BUILD_MATRIX']`.) ADO runs one job
   per `<leg>`; inside it each `<VAR>` is `$(VAR)`. Note: ADO passes the output variable directly — there
   is **no** `fromJson` (that is GitHub Actions); the matrix value is `{ leg: { var: string } }` with
   **scalar string** values only.

`tailor matrix --ado <NAME>` emits **tailor's image matrix** in exactly that shape — one leg per cell —
wrapped in the `setvariable` line. tailor only knows about images; turning this into any *other* matrix
(e.g. a downstream test/scenario matrix) is the pipeline's job, not tailor's (§5).

---

## 2. The flags

```
tailor matrix [IMAGES]...  [--select AXIS=VALUE]...  [--cell SLUG]...
                           ( --ado <VAR_NAME>  |  --format ado )
```

- Composes with the existing cell selection (`crates/tailor/src/cli.rs` `MatrixArgs` / `SelectArgs`):
  image list + `--select` (slice) + `--cell` (exact). So you emit a **named matrix for a slice**.
- **`--ado <VAR_NAME>`** prints the matrix wrapped in the `setvariable` line (the pipeline form);
  `<VAR_NAME>` is the ADO variable to set (e.g. `BUILD_MATRIX`).
- **`--format ado`** prints the **bare** matrix object (no wrapper) — for debugging and non-ADO
  consumers (resolves Q3). `--ado <VAR>` is exactly `--format ado` plus the `setvariable` wrapper.

---

## 3. Output

Exactly one line to stdout — the inline (compact, single-line) JSON wrapped in the logging command
(`tailor matrix trident-mos --ado BUILD_MATRIX`):

```
##vso[task.setvariable variable=BUILD_MATRIX;isOutput=true]{"trident_mos_host_amd64_iso":{"image":"trident-mos","slug":"trident-mos_host_amd64_iso","format":"iso","baseImage":"baremetal","axis_runtime":"host","axis_arch":"amd64"}}
```

- **Inline JSON**, no pretty-printing — the agent reads one stdout line.
- **`isOutput=true` always** (resolves Q2): the matrix is consumed in a *later* stage, which requires an
  output variable. There is no toggle.
- Nothing else is printed on **stdout** — `--ado` suppresses the human table so the agent reads a clean
  single line. The ADO agent then *consumes* that `setvariable` line and hides it from the build log, so
  `--ado` also echoes a **leg roster to stderr** (`<legKey>  <slug>`, one per line) — which the log *does*
  show — so an operator can still see which legs were published and map each ADO leg key to its cell slug.

---

## 4. The ADO matrix object

tailor's image matrix maps directly: each selected cell becomes one leg.

- **Leg key** = an **ADO-safe** key derived from the slug. ADO matrix leg names allow only
  `[A-Za-z0-9_]` and must start with a letter, so the slug's `-`/`.` are replaced with `_` (a short hash
  suffix breaks any sanitisation collisions). The original slug is kept verbatim as the `slug` variable.
- **Variables** — **all scalar strings** (ADO matrix values are `{ leg: { var: string } }`, no nesting):
  - **Reserved fields:** `image`, `slug`, `format`, and `baseImage` (when the base is a catalogue slot,
    [`2026-06-29-base-image-catalogue.md`](./2026-06-29-base-image-catalogue.md) §6.2). These are what a build job keys off.
  - **Axes:** each cell axis as `axis_<name>` (e.g. `axis_runtime`, `axis_arch`) — kept *just in case*,
    prefixed so they can never collide with a reserved field, flat because ADO rejects nested values.

```jsonc
// tailor matrix trident-mos --format ado  →  (pretty-printed here for readability)
{
  "trident_mos_host_amd64_iso": {
    "image": "trident-mos",
    "slug": "trident-mos_host_amd64_iso",
    "format": "iso",
    "baseImage": "baremetal",
    "axis_runtime": "host",
    "axis_arch": "amd64"
  }
}
```

Downstream the build job uses `$(image)` + `$(slug)` (the slug carries the axes; `axis_*` are extras):

```yaml
- job: Build
  dependsOn: Setup
  strategy:
    matrix: $[ dependencies.Setup.outputs['emit.BUILD_MATRIX'] ]
  steps:
    - bash: tailor build "$(image)" --cell "$(slug)"
```

This is enough to replace the per-image build stages with one matrixed job.

---

## 5. Out of scope — turning the image matrix into *another* matrix

tailor knows about **images**, so `--ado` emits the **image matrix** (§4) and nothing else. A pipeline
that needs a *different* matrix downstream — say an E2E **test** matrix keyed by scenario, with its own
variable names and injected constants, e.g.

```json
{ "base_vm-host": { "SCENARIO": "base_vm-host", "HARDWARE": "vm", "RUNTIME": "host", "TEST_RING": "ci" } }
```

— assembles that **itself**. Scenario-shaped leg keys, renamed variables, and constants like
`TEST_RING` are *test* concepts, not image concepts; baking them into tailor would conflate the two.
tailor contributes the image matrix; the pipeline maps or joins it to whatever job shape it needs. So
there is **no** projection / rename / constant configuration in tailor — that was the conflation in an
earlier draft.

---

## 6. Composition with `--select` → one variable per slice

`--ado <VAR>` composes with `--select`, so a setup job can publish **several** image-matrix variables,
one per slice it wants to build as a group:

```bash
tailor matrix --select arch=amd64 --ado BUILD_MATRIX_AMD64
tailor matrix --select arch=arm64 --ado BUILD_MATRIX_ARM64
```

Each call emits one `setvariable` line for that slice. The *what to enumerate* (cells + `--select`) and
the *where it lands* (`<VAR_NAME>`) are both on the command line; the *shape* is always the image
matrix (§4).

---

## 7. Stringification & constraints

- **Scalar strings only.** ADO matrix values are `{ leg: { var: string } }`; every variable is a flat
  string (no nested objects — hence `axis_<name>`, not a nested `axes`). For the full nested view, use
  `--format json`.
- **Leg keys** are sanitised to `[A-Za-z0-9_]`, starting with a letter; the raw slug stays as the `slug`
  variable so `tailor build --cell "$(slug)"` still works.
- **`<VAR_NAME>` is validated** against `[A-Za-z_][A-Za-z0-9_]*` (so `outputs['emit.<NAME>']` references
  stay clean).
- **Empty matrix** (resolves Q4). A selection that matches no cells **fails non-zero** with a clear
  message — ADO must expand `matrix:` before a job `condition` can skip it, so an empty matrix would
  break planning. Failing the setup step early is the safe behaviour. (`--format ado` of an empty
  selection prints `{}` and exits 0, for inspection.)

---

## 8. Architecture / layering

- `MatrixArgs` gains `--ado <VAR_NAME>`, and `MatrixFormat` gains an `Ado` variant (the bare object)
  alongside `Json` / `Slugs` (`crates/tailor/src/cli.rs`). `--ado <VAR>` is `--format ado` plus the
  `setvariable` wrapper.
- The cells → ADO-object mapping lives in `tailor-config`/`tailor-core` next to the existing matrix
  rendering. It is a fixed mapping (slug → image fields, §4) — **no projection config**, so no new
  schema beyond the format/flag.
- `crates/tailor/src/run.rs` prints the single `setvariable` line (or the bare object) to stdout, and
  fails non-zero on an empty `--ado` selection. No new ports; this is pure formatting over the
  already-resolved cells.

---

## 9. Resolved decisions & remaining questions

**Resolved**

1. **Scope (Q1)** → tailor emits **only its image matrix** (slug → image fields, §4). There are **no
   named projections** and no rename/constant config; turning the image matrix into a different
   downstream matrix (e.g. a test/scenario matrix) is the pipeline's job, not tailor's (§5).
2. **`isOutput` (Q2)** → **always `true`** (cross-stage); no toggle (§3).
3. **Wrapper-less output (Q3)** → **`--format ado`** prints the bare object; `--ado <VAR>` wraps it
   (§2).
4. **Empty matrix (Q4)** → `--ado` **fails non-zero** with a clear message (an empty `matrix:` breaks
   ADO planning before a `condition` can skip it); `--format ado` prints `{}` for inspection (§7).
5. **Reserved-name collisions (Q5)** → leg values are **all scalar strings**: reserved fields (`image`,
   `slug`, `format`, `baseImage`) plus axes as `axis_<name>` (§4) — collision-free, and ADO-valid (no
   nesting). Axes kept *just in case*; the slug already encodes them.
6. **ADO leg key (duck review)** → slugs contain `-`/`.`, which ADO leg names forbid, so the key is
   sanitised to `[A-Za-z0-9_]` and the raw slug is kept as the `slug` variable (§4, §7).
7. **`fromJson` (duck review)** → that is GitHub Actions, not ADO; consumers pass the output variable
   directly (`matrix: $[ dependencies.Setup.outputs['emit.<VAR>'] ]`, cross-stage `stageDependencies`)
   (§1, §4).

**Remaining** — none open; ready to consolidate toward implementation.
