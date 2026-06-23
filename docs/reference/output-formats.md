# Output formats

`outputs:` is a list of output specs. tailor creates one artifact per selected cell × output.

```yaml
outputs:
  - format: cosi
```

| Format | Artifact extension/name |
| --- | --- |
| `cosi` | `.cosi` |
| `vhd` | `.vhd` |
| `vhd-fixed` | `.vhd` |
| `vhdx` | `.vhdx` |
| `qcow2` | `.qcow2` |
| `raw` | `.raw` |
| `iso` | `.iso` |
| `pxe-dir` | directory named as the cell slug |
| `pxe-tar` | `.tar.gz` |
| `baremetal-image` | `.raw` |

Optional output fields:

| Field | Meaning |
| --- | --- |
| `cosiCompressionLevel` | COSI compression level. tailor passes IC `--cosi-compression-level` when set. |
| `name` | Optional `${...}` template for the output basename. |

Use `$replace` in a fragment when you want to swap inherited outputs instead of appending another output.
