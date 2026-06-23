# Fragments & directives

A **fragment** is a conditional delta merged onto an image's [base document](./image-yaml.md). It is
selected by **where it lives** — `by-release/4.0.yaml` applies when `release == 4.0`,
`by-variant/grub.yaml` when `variant == grub`, `by-feature/<name>.yaml` when that feature is enabled —
and/or by an inline [`match:`](#match) (ANDed with the path predicate).

Schema: [`#/$defs/Fragment`](../tailor.schema.json).

## What a fragment may contain

Any subset of the [image fields](./image-yaml.md#top-level-fields) **except `name` and `matrix`**
(those belong only to the base document), **plus** a [`match:`](#match). Every value may also be a
[directive](#directives). The common case is just a small `config:` delta:

```yaml
# by-release/4.0.yaml
params:
  grubEfiPkg: "grub2-efi-${efiArch}"
rpmSources:
  - repos/azurelinux-4.0-beta.repo
base:
  $set:                              # override the inherited by-arch base
    oci:
      uri: "mcr.microsoft.com/azurelinux/4.0/image/minimal-os:latest"
      platform: "linux/${arch}"
config:
  os:
    packages:
      install: [dnf5, iptables-nft, vim-enhanced]
```

## match

For conditions the directory path can't express. It sits at the **top level** (a tailor field, not IC
config) and is ANDed with the path predicate.

| Form | Example | Meaning |
| ---- | ------- | ------- |
| equality | `match: {release: "4.0"}` → block style below | axis equals a value |
| set membership | `variant: [root-verity, usr-verity]` | axis in a set (OR) |
| `all` | `all: [ … ]` | every sub-match (AND) |
| `any` | `any: [ … ]` | some sub-match (OR) |
| `not` | `not: {release: "4.0"}` | negation |
| `feature` | `feature: pcrlock-static-files` | a feature flag is enabled |

```yaml
# by-feature/pcrlock-static-files.yaml — feature predicate from the path, AND the release below
match:
  release: "3.0"
config:
  os:
    packages:
      install: [trident-static-pcrlock-files]
```

## Merge semantics (summary)

| Node | Rule |
| ---- | ---- |
| maps | deep-merge |
| lists (additive: packages, services, scripts, …) | **append** by default, then set-dedup |
| keyed lists (`storage.disks[].partitions[]` by `id`, `filesystems[]` by `deviceId`, `verity[]` by `id`) | merge by key |
| scalars | conflicting differing values error unless `$set` |
| order | base document first, then `by-*` by normalized path |

Full semantics live in [`image-definitions.md`](../../image-definitions.md) §7.

## Directives

Directives are the small `$`-prefixed tailor vocabulary (the `$` marks the only keys that aren't
literal IC config — IC has no `$`-fields, so they never collide). They appear as a field's value (or,
for `$include`, anywhere).

| Directive | Shape | Purpose |
| --------- | ----- | ------- |
| `$include` | `{ $include: <repo-root-relative path> }` | Splice the parsed file content here (mapping, list, or scalar). Sole key. |
| `$set` | `{ $set: <value> }` | Explicit override of an inherited scalar/object (resolves a would-be conflict). |
| `$replace` | `{ $replace: [ … ] }` | Replace an inherited list wholesale (instead of append). |
| `$remove` | `{ $remove: [ … ] }` | Drop the listed items from an inherited list. |
| `$rename` | `{ $rename: { from, to } }` | Rename a keyed-list element and fix dependent references. |
| `$select` | `{ $select: { <axis>: { <value>: <result>, default: <result> } } }` | **Optional** co-location sugar — pick a value by an axis (the primary mechanism is a per-axis *file*). |

### `$include` — splice a shared file

```yaml
config:
  storage:
    $include: layouts/storage/root-verity.yaml   # the file holds the bare storage subtree
```

`$include` covers arrays too: as a value it becomes the file's content; as a **list element**
(`- {$include: shared/files.yaml}`) a list-valued file is spliced into the surrounding list.

### `$replace` — emit instead of append

```yaml
# image.yaml sets outputs: [{format: cosi}]; lists append, so to swap (not add) vhd-fixed:
outputs:
  $replace:
    - format: vhd-fixed
```

### `$set` — intentional override

```yaml
base:
  $set:
    oci:
      uri: "mcr.microsoft.com/azurelinux/4.0/image/minimal-os:latest"
      platform: "linux/${arch}"
```

### `$remove` / `$rename`

```yaml
config:
  os:
    packages:
      install:
        $remove: [telnet]
  storage:
    filesystems:
      $rename:
        from: data
        to: srv
```

See [`examples/trident-vm-testimage`](../../examples/trident-vm-testimage) for `$include`/`$set`/
`$replace`/`params` used together on a real, verified image.
