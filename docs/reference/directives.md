# Merge directives reference

Fragments apply after `image.yaml`. Local `by-<axis>/<value>.yaml` fragments apply in **matrix axis-declaration order**; the axis declared later has later precedence. This is not alphabetical and not directory-name order.

## Rules

| Data shape | Default merge |
| --- | --- |
| Maps | Deep-merge. |
| Lists | Append. |
| Scalars | Same value is OK; different values conflict unless the later value uses `$set`. |

A later `$set` wins over an earlier `$set`. A plain reassignment after a `$set` still conflicts.

## Directives

| Directive | Valid position | Value | Purpose |
| --- | --- | --- | --- |
| `$set` | Any field value, commonly scalars or whole tailor values | any YAML value | Explicitly override an earlier value. |
| `$replace` | List field value | list | Replace the inherited list wholesale. |
| `$remove` | List field value | list | Remove matching items from the inherited list. |
| `$include` | Mapping value or list item | path string | Splice a shared YAML file at that position. |

## `$set`

```yaml
config:
  os:
    hostname:
      $set: gizmo-edge
```

For a whole base override:

```yaml
base:
  $set:
    oci:
      uri: "registry.example/gizmo/base:edge"
      platform: "linux/${arch}"
```

## `$replace`

```yaml
outputs:
  $replace:
    - format: raw
```

## `$remove`

```yaml
config:
  os:
    packages:
      install:
        $remove:
          - base-extra
```

## `$include`

```yaml
config:
  storage:
    $include: layouts/storage/pro.yaml
```

The include path is relative to the image directory in user-facing examples. The included file is substituted as the value at that position. If `$include` is a list item and the included file is a list, the elements are spliced into the surrounding list.

`$include` must be the sole key in its mapping. To tweak included content, layer a later fragment with normal merge rules or directives.
