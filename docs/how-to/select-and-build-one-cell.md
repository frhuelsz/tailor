# Select and build one cell

Use `-s/--select` for axis slices and `--cell` for exact slugs.

## Select by axis values

```bash
tailor build gizmo -s arch=amd64,variant=full --dry-run
```

Unset axes still expand. For example, if `channel` has two values, the command above selects two cells.

`--select` is repeatable:

```bash
tailor build gizmo \
  --select arch=amd64 \
  --select variant=full \
  --dry-run
```

## Select by exact slug

List slugs:

```bash
tailor matrix gizmo --format slugs
```

Build one slug:

```bash
tailor build --cell gizmo_full_amd64_edge_cosi
```

## Restrict architecture

`--arch` is a build-only convenience:

```bash
tailor build gizmo --arch amd64 --dry-run
```

Related commands that also accept selectors include `validate`, `render`, `matrix`, `slugs`, `explain`, and `clean`.
