# tailor configuration schema

[`tailor.schema.json`](./tailor.schema.json) is a JSON Schema (Draft 2020-12) for the **full tailor
configuration surface**. It deliberately does **not** model anything under an image's `config:` key —
that is Image Customizer configuration, which tailor treats as an opaque YAML tree (or a path string)
and passes through untouched.

> **Reading the config?** See [`reference/`](./reference/) for **human-readable** field-by-field docs
> (tables, defaults, examples) that mirror this schema — every field and enum is checked to appear
> there. Use `tailor.schema.json` itself for machine validation / editor IntelliSense.

## Three document kinds → three entry points

The file defines one schema per document kind under `$defs`; point your editor at the right one:

| File | `$schema` pointer | What it is |
| ---- | ----------------- | ---------- |
| `tailor.yaml` | `tailor.schema.json#/$defs/ToolConfig` | workspace root: `toolchains`, `runtime`, `defaults`, image catalogue |
| `image.yaml` | `tailor.schema.json#/$defs/ImageDefinition` | one image (the base document): `name`, `matrix`, `outputs`, `base`, `config`, … |
| `by-*/<value>.yaml` | `tailor.schema.json#/$defs/Fragment` | a conditional delta (partial image def + `match` + directives) |

```yaml
# yaml-language-server: $schema=../../schemas/tailor.schema.json#/$defs/ImageDefinition
name: my-image
...
```

(Pointed at the bare file, the root `oneOf` matches `ToolConfig` **or** `ImageDefinition`
automatically; fragments need the explicit `#/$defs/Fragment` pointer because they have no `name`.)

## Verified

- Valid Draft 2020-12 schema (`jsonschema.check_schema`).
- **All 14 example files validate** (5 `image.yaml`, 8 fragments, 1 `tailor.yaml`); the opaque
  `layouts/storage/*`, `rendered/*`, and `repos/*` files are correctly out of scope.
- **10 negative cases are rejected** (missing `name`/`toolchains`, typo'd keys, both `base` and
  `baseByArch`, two base kinds at once, bad output format, unsupported arch, inline toolchain missing
  `version`, a matrix in a fragment).

## Decisions this schema bakes in

These reconcile `design.md` (older) with `image-definitions.md` and the later design decisions
(newer wins):

1. **`features:` collision.** `design.md` used `features: {operation, injectFiles}`;
   `image-definitions.md` used `features: [flag, …]`. The schema keeps **`features` as the flag list**
   and lifts **`operation`** and **`injectFiles`** to top-level fields.
2. **`targets:` → `images:`.** The repo catalogue is `images` (object `{members?, exclude?, inline?}`),
   and is **optional** — omit it to auto-discover `*/image.yaml` at depth 1. This supersedes
   `design.md`'s `targets[]` + `import:` glob.
3. **`config:` is opaque** — typed as `object | string` (inline IC config, or a path to it), never
   validated against IC's schema here.
4. **Output cell-slug + reserved `_`.** Output artifacts are named by the full cell slug
   (`<image>_<every axis value>_<format>`); `_` is the reserved separator, so image names and axis
   values are restricted to `[A-Za-z0-9.-]+`. See [`reference/types.md`](./reference/types.md#output-naming-cell-slug).

These are folded into `image-definitions.md` and `design.md` (consolidation pass) so the prose, the
schema, the [reference](./reference/), and the [examples](../examples/) all agree.
