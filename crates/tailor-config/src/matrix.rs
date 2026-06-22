use crate::{
    error::ConfigError,
    schema::{CellSelector, Matrix},
    types::OutputFormat,
};

const INCLUDE: &str = "include";
const EXCLUDE: &str = "exclude";

/// One expanded matrix cell: ordered `(axis, value)` pairs in matrix-declared order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxisTuple {
    pub values: Vec<(String, String)>,
}

impl AxisTuple {
    /// The axis-coordinate portion of a cell slug: values joined by `_`, in matrix order.
    pub fn coordinate(&self) -> String {
        self.values
            .iter()
            .map(|(_, value)| value.as_str())
            .collect::<Vec<_>>()
            .join("_")
    }

    /// The value pinned to `axis`, if this tuple includes it.
    pub fn get(&self, axis: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(name, _)| name == axis)
            .map(|(_, value)| value.as_str())
    }
}

/// The full output basename for a cell: `<image>_<axis values, in order>_<format>`
/// (`meta/docs/design.md` §10).
pub fn cell_slug(image_name: &str, tuple: &AxisTuple, format: OutputFormat) -> String {
    let coordinate = tuple.coordinate();
    format!("{image_name}_{coordinate}_{format}")
}

/// Expand a matrix into the cartesian product of its axes (in declared order), minus `exclude`,
/// plus `include`. Validates that axes are non-empty and that selectors reference declared
/// axes/values.
pub fn expand(matrix: &Matrix) -> Result<Vec<AxisTuple>, ConfigError> {
    for (axis, values) in &matrix.axes {
        if values.is_empty() {
            return Err(ConfigError::EmptyAxis { axis: axis.clone() });
        }
    }
    validate_selectors(matrix)?;

    let mut cells = cartesian(matrix);
    cells.retain(|cell| !matrix.exclude.iter().any(|sel| selector_matches(sel, cell)));
    for selector in &matrix.include {
        let tuple = selector_tuple(matrix, selector)?;
        if !cells.contains(&tuple) {
            cells.push(tuple);
        }
    }
    Ok(cells)
}

fn cartesian(matrix: &Matrix) -> Vec<AxisTuple> {
    let mut product: Vec<Vec<(String, String)>> = vec![Vec::new()];
    for (axis, values) in &matrix.axes {
        let mut next = Vec::with_capacity(product.len() * values.len());
        for partial in &product {
            for value in values {
                let mut row = partial.clone();
                row.push((axis.clone(), value.clone()));
                next.push(row);
            }
        }
        product = next;
    }
    product
        .into_iter()
        .map(|values| AxisTuple { values })
        .collect()
}

fn selector_matches(selector: &CellSelector, cell: &AxisTuple) -> bool {
    selector
        .iter()
        .all(|(axis, value)| cell.get(axis) == Some(value.as_str()))
}

/// Build a full, declaration-ordered tuple from an `include` selector (must pin every axis).
fn selector_tuple(matrix: &Matrix, selector: &CellSelector) -> Result<AxisTuple, ConfigError> {
    let mut values = Vec::with_capacity(matrix.axes.len());
    for axis in matrix.axes.keys() {
        let value = selector
            .get(axis)
            .ok_or_else(|| ConfigError::IncludeIncomplete { axis: axis.clone() })?;
        values.push((axis.clone(), value.clone()));
    }
    Ok(AxisTuple { values })
}

fn validate_selectors(matrix: &Matrix) -> Result<(), ConfigError> {
    let check = |selector: &CellSelector, label: &'static str| -> Result<(), ConfigError> {
        for (axis, value) in selector {
            let declared =
                matrix
                    .axes
                    .get(axis)
                    .ok_or_else(|| ConfigError::SelectorUnknownAxis {
                        selector: label,
                        axis: axis.clone(),
                    })?;
            if !declared.contains(value) {
                return Err(ConfigError::SelectorUnknownValue {
                    selector: label,
                    axis: axis.clone(),
                    value: value.clone(),
                });
            }
        }
        Ok(())
    };
    for selector in &matrix.include {
        check(selector, INCLUDE)?;
    }
    for selector in &matrix.exclude {
        check(selector, EXCLUDE)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use indoc::indoc;

    fn matrix(yaml: &str) -> Matrix {
        serde_yaml_ng::from_str(yaml).unwrap()
    }

    #[test]
    fn cartesian_product_preserves_declared_order() {
        let m = matrix(indoc! {"
            variant: [grub, root-verity]
            arch: [amd64, arm64]
        "});
        let cells = expand(&m).unwrap();
        let coords: Vec<_> = cells.iter().map(AxisTuple::coordinate).collect();
        assert_eq!(
            coords,
            [
                "grub_amd64",
                "grub_arm64",
                "root-verity_amd64",
                "root-verity_arm64"
            ]
        );
    }

    #[test]
    fn slug_is_image_axes_format() {
        let m = matrix(indoc! {"
            variant: [grub]
            arch: [amd64]
            release: ['4.0']
            phase: [base]
        "});
        let cells = expand(&m).unwrap();
        assert_eq!(
            cell_slug("trident-vm-testimage", &cells[0], OutputFormat::Cosi),
            "trident-vm-testimage_grub_amd64_4.0_base_cosi"
        );
    }

    #[test]
    fn exclude_drops_matching_cells() {
        let m = matrix(indoc! {"
            variant: [grub, usr-verity]
            release: ['3.0', '4.0']
            exclude:
              - variant: usr-verity
                release: '3.0'
        "});
        let cells = expand(&m).unwrap();
        assert_eq!(cells.len(), 3);
        assert!(
            !cells
                .iter()
                .any(|c| c.get("variant") == Some("usr-verity") && c.get("release") == Some("3.0"))
        );
    }

    #[test]
    fn include_readds_a_full_declared_cell() {
        let m = matrix(indoc! {"
            variant: [grub, usr-verity]
            release: ['3.0', '4.0']
            exclude:
              - variant: usr-verity
                release: '3.0'
            include:
              - variant: usr-verity
                release: '3.0'
        "});
        assert_eq!(expand(&m).unwrap().len(), 4);
    }

    #[test]
    fn empty_axis_is_an_error() {
        let err = expand(&matrix("variant: []")).unwrap_err();
        assert!(matches!(err, ConfigError::EmptyAxis { .. }), "got {err:?}");
    }

    #[test]
    fn selector_with_undeclared_value_is_an_error() {
        let m = matrix(indoc! {"
            variant: [grub]
            exclude:
              - variant: nope
        "});
        let err = expand(&m).unwrap_err();
        assert!(
            matches!(err, ConfigError::SelectorUnknownValue { .. }),
            "got {err:?}"
        );
    }
}
