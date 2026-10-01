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
