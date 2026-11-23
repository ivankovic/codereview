//! Which files are pictures, by their name. Both front ends ask: the page to show one rather
//! than "binary file", the terminal to draw one when it can.

/// The media type of an image `path` names, by its extension; `None` for anything else.
pub fn image_type(path: &str) -> Option<&'static str> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_are_known_by_their_names() {
        assert_eq!(image_type("docs/shot.PNG"), Some("image/png"));
        assert_eq!(image_type("logo.svg"), Some("image/svg+xml"));
        assert_eq!(image_type("src/main.rs"), None);
        assert_eq!(image_type("Makefile"), None);
    }
}

/// The files that configure a tool rather than hold data, though they are JSON.
const NOT_DATA: &[&str] = &[
    "package.json",
    "package-lock.json",
    "tsconfig.json",
    "jsconfig.json",
    "composer.json",
    "deno.json",
    "manifest.json",
    "renovate.json",
];

/// Whether `path` is a dataset, by its name: CSV, TSV, JSON in its forms, or Parquet, except
/// the JSON that configures a tool. What the Data focus lists.
pub fn is_dataset(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let lower = name.to_ascii_lowercase();
    let Some((_, ext)) = lower.rsplit_once('.') else {
        return false;
    };
    matches!(
        ext,
        "csv" | "tsv" | "json" | "jsonl" | "ndjson" | "geojson" | "parquet"
    ) && !NOT_DATA.contains(&lower.as_str())
        && !lower.starts_with("tsconfig.")
        && !path.starts_with(".vscode/")
}

#[cfg(test)]
mod dataset_tests {
    use super::is_dataset;

    #[test]
    fn a_dataset_is_data_not_a_tool_s_configuration() {
        for path in ["tests/data/kinds/kinds.csv", "a/b.json", "x.jsonl", "t.TSV"] {
            assert!(is_dataset(path), "{path}");
        }
        for path in [
            "package.json",
            "web/tsconfig.base.json",
            ".vscode/settings.json",
            "a.rs",
        ] {
            assert!(!is_dataset(path), "{path}");
        }
    }
}
